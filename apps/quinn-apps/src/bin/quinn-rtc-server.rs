use std::{ascii, fs, io, io::Write, net::SocketAddr, path::PathBuf, str, sync::Arc};

use std::time;

use quinn_apps::ALPN_QUIC_HTTP;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use quinn::crypto::rustls::QuicServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, pem::PemObject};
use tracing::{error, info, info_span};
use tracing_futures::Instrument as _;

mod common;

use common::{PeerTime, Stats};

// 30fps. 1 frame every 33ms.
const FRAME_INTERVAL: time::Duration = time::Duration::from_millis(33);

struct MediaClient {
    conn: quinn::Connection,
    request_frames: u64,
    frame_count: u64,

    last_frame_time: time::Instant,
    stats: Stats,

    // for log
    server_time: PeerTime,

    frame_size: usize, // set by command line argument
}

impl MediaClient {
    fn new(conn: quinn::Connection, server_time: PeerTime, frame_size: usize) -> Self {
        Self {
            conn,
            request_frames: 0,
            frame_count: 0,
            last_frame_time: time::Instant::now(),
            stats: Stats::new(),
            server_time,
            frame_size,
        }
    }

    fn set_request_frames(&mut self, request_frames: u64) {
        self.request_frames = request_frames;
    }

    // each frame is sent on a new stream
    async fn send_next_frame(&mut self) -> Result<()> {
        if self.all_frames_sent() {
            return Ok(());
        }
        let body = vec![0; self.frame_size];
        self.frame_count += 1;

        // Open a new unidirectional stream for this frame and send the payload.
        let mut send = self
            .conn
            .open_uni()
            .await
            .context("failed to open unidirectional stream for frame")?;

        // print the time when the frame is sent to the stream
        println!(
            "frame {}, sent time: {}",
            send.id().index() + 1,
            self.server_time.elapsed().as_secs_f64()
        );
        self.last_frame_time = time::Instant::now();

        send.write_all(&body)
            .await
            .context("failed to write frame to stream")?;

        // finish the stream so receiver sees EOF for this frame
        send.finish()
            .map_err(|e| anyhow!("failed to finish stream: {}", e))?;

        self.stats.bytes_sent(body.len());
        Ok(())
    }

    fn all_frames_sent(&self) -> bool {
        assert!(self.frame_count <= self.request_frames);
        self.frame_count == self.request_frames
    }

    fn finish_request(&self) {
        // Stream's response is fully writted into QUIC lib.
        self.stats.print_stats();
    }
}

#[derive(Parser, Debug)]
#[clap(name = "server")]
struct Opt {
    /// file to log TLS keys to for debugging
    #[clap(long = "keylog")]
    keylog: bool,
    /// TLS private key in PEM format
    #[clap(short = 'k', long = "key", requires = "cert")]
    key: Option<PathBuf>,
    /// TLS certificate in PEM format
    #[clap(short = 'c', long = "cert", requires = "key")]
    cert: Option<PathBuf>,
    /// Enable stateless retries
    #[clap(long = "stateless-retry")]
    stateless_retry: bool,
    /// Address to listen on
    #[clap(long = "listen", default_value = "[::1]:4433")]
    listen: SocketAddr,
    /// Client address to block
    #[clap(long = "block")]
    block: Option<SocketAddr>,
    /// Maximum number of concurrent connections to allow
    #[clap(long = "connection-limit")]
    connection_limit: Option<usize>,
    /// Frame size
    #[clap(short, long, default_value = "12500")]
    frame_size: usize,
}

fn main() {
    env_logger::builder()
        .format(|buf, record| writeln!(buf, "[{}] {}", record.level(), record.args()))
        .init();

    let opt = Opt::parse();
    let code = {
        if let Err(e) = run(opt) {
            eprintln!("ERROR: {e}");
            1
        } else {
            0
        }
    };
    ::std::process::exit(code);
}

#[tokio::main]
async fn run(options: Opt) -> Result<()> {
    let (certs, key) = if let (Some(key_path), Some(cert_path)) = (&options.key, &options.cert) {
        let key = if key_path.extension().is_some_and(|x| x == "der") {
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                fs::read(key_path).context("failed to read private key file")?,
            ))
        } else {
            PrivateKeyDer::from_pem_file(key_path)
                .context("failed to read PEM from private key file")?
        };

        let cert_chain = if cert_path.extension().is_some_and(|x| x == "der") {
            vec![CertificateDer::from(
                fs::read(cert_path).context("failed to read certificate chain file")?,
            )]
        } else {
            CertificateDer::pem_file_iter(cert_path)
                .context("failed to read PEM from certificate chain file")?
                .collect::<Result<_, _>>()
                .context("invalid PEM-encoded certificate")?
        };

        (cert_chain, key)
    } else {
        let dirs = directories_next::ProjectDirs::from("org", "quinn", "quinn-examples").unwrap();
        let path = dirs.data_local_dir();
        let cert_path = path.join("cert.der");
        let key_path = path.join("key.der");
        let (cert, key) = match fs::read(&cert_path).and_then(|x| Ok((x, fs::read(&key_path)?))) {
            Ok((cert, key)) => (
                CertificateDer::from(cert),
                PrivateKeyDer::try_from(key).map_err(anyhow::Error::msg)?,
            ),
            Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
                info!("generating self-signed certificate");
                let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
                let key = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
                let cert = cert.cert.into();
                fs::create_dir_all(path).context("failed to create certificate directory")?;
                fs::write(&cert_path, &cert).context("failed to write certificate")?;
                fs::write(&key_path, key.secret_pkcs8_der())
                    .context("failed to write private key")?;
                (cert, key.into())
            }
            Err(e) => {
                bail!("failed to read certificate: {}", e);
            }
        };

        (vec![cert], key)
    };

    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    server_crypto.alpn_protocols = ALPN_QUIC_HTTP.iter().map(|&x| x.into()).collect();
    if options.keylog {
        server_crypto.key_log = Arc::new(rustls::KeyLogFile::new());
    }

    let mut server_config =
        quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server_crypto)?));
    let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
    transport_config.max_concurrent_uni_streams(0_u8.into());

    // disable GSO; in Mininet’s virtual links, GSO behaves unexpectedly and
    // results in oversized UDP packets being transmitted without MTU-based segmentation.
    // It should instead produce multiple MTU-sized UDP packets before transmission.
    transport_config.enable_segmentation_offload(false);

    let endpoint = quinn::Endpoint::server(server_config, options.listen)?;
    eprintln!("Listening on {}", endpoint.local_addr()?);

    while let Some(conn) = endpoint.accept().await {
        if options
            .connection_limit
            .is_some_and(|n| endpoint.open_connections() >= n)
        {
            info!("refusing due to open connection limit");
            conn.refuse();
        } else if Some(conn.remote_address()) == options.block {
            info!("refusing blocked client IP address");
            conn.refuse();
        } else if options.stateless_retry && !conn.remote_address_validated() {
            info!("requiring connection to validate its address");
            conn.retry().unwrap();
        } else {
            info!("accepting connection");
            let fut = handle_connection(conn, options.frame_size);
            tokio::spawn(async move {
                if let Err(e) = fut.await {
                    error!("connection failed: {reason}", reason = e.to_string())
                }
            });
        }
    }

    Ok(())
}

async fn handle_connection(conn: quinn::Incoming, frame_size: usize) -> Result<()> {
    let connection = conn.await?;
    let span: tracing::Span = info_span!(
        "connection",
        remote = %connection.remote_address(),
        protocol = %connection
            .handshake_data()
            .unwrap()
            .downcast::<quinn::crypto::rustls::HandshakeData>().unwrap()
            .protocol
            .map_or_else(|| "<none>".into(), |x| String::from_utf8_lossy(&x).into_owned())
    );
    async {
        info!("established");
        let request_frame_num: usize;
        // Wait the rtc request
        let mut client = MediaClient::new(connection.clone(), PeerTime::new(&0.0), frame_size);
        loop {
            let stream = connection.accept_bi().await;
            let mut stream = match stream {
                Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                    info!("connection closed");
                    return Ok(());
                }
                Err(e) => {
                    return Err(e);
                }
                Ok(s) => s,
            };

            request_frame_num = match handle_request(stream.1).await {
                Ok(frame_num) => {
                    info!("request succeeded");
                    frame_num
                }
                Err(e) => {
                    error!("request failed: {}", e);
                    continue;
                }
            };
            println!("RTC Server GetN request: {} frames", request_frame_num);
            // Gracefully terminate the stream
            stream.0.finish().unwrap();
            break;
        }

        client.stats.request_recved();
        client.set_request_frames(request_frame_num as u64);

        // Send frames periodically at FRAME_INTERVAL
        let mut ticker = tokio::time::interval(FRAME_INTERVAL);

        while !client.all_frames_sent() {
            ticker.tick().await;
            if let Err(e) = client.send_next_frame().await {
                error!("failed to send frame: {}", e);
                break;
            }
        }

        client.finish_request();
        info!("gracefully close the connection");
        connection.closed().await; // wait close from client
        connection.close(0u32.into(), b"done"); // close connection gracefully at the server side
        Ok(())
    }
    .instrument(span)
    .await?;
    Ok(())
}

async fn handle_request(mut recv: quinn::RecvStream) -> Result<usize> {
    let req = recv
        .read_to_end(64 * 1024)
        .await
        .map_err(|e| anyhow!("failed reading request: {}", e))?;
    let mut escaped = String::new();
    for &x in &req[..] {
        let part = ascii::escape_default(x).collect::<Vec<_>>();
        escaped.push_str(str::from_utf8(&part).unwrap());
    }
    info!(content = %escaped);
    // Execute the request
    match process_get(&req) {
        Ok(frame_num) => {
            return Ok(frame_num);
        }
        Err(e) => {
            error!("failed to process request: {}", e);
            return Err(e);
        }
    }
}

fn process_get(x: &[u8]) -> Result<usize> {
    // Parse "GetN <N>\r\n" requests and return the
    // requested number of frames N to be sent later by the RTC sender.
    if x.len() >= 5 && &x[..5] == b"GetN " {
        // Expect format: "GetN <number>\r\n"
        if x.len() < 7 || &x[x.len() - 2..] != b"\r\n" {
            bail!("missing \r\n for GetN");
        }
        let body = &x[5..x.len() - 2];
        let s = str::from_utf8(body).context("GetN payload malformed UTF-8")?;
        let request_frame_num: usize = s.trim().parse().context("invalid GetN number")?;
        Ok(request_frame_num)
    } else {
        // unknown request
        bail!("unknown request");
    }
}
