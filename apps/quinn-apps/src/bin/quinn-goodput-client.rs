use std::{
    fs,
    io::{self, Write},
    net::{SocketAddr, ToSocketAddrs},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use clap::Parser;
use quinn::crypto::rustls::QuicClientConfig;
use rustls::pki_types::CertificateDer;
use tracing::{error, info};
use url::Url;

use quinn_apps::ALPN_QUIC_HTTP;

mod common;
use common::ClientStats;

/// HTTP/0.9 over QUIC client
#[derive(Parser, Debug)]
#[clap(name = "client")]
struct Opt {
    /// Perform NSS-compatible TLS key logging to the file specified in `SSLKEYLOGFILE`.
    #[clap(long = "keylog")]
    keylog: bool,

    url: Url,

    /// Override hostname used for certificate verification
    #[clap(long = "host")]
    host: Option<String>,

    /// Custom certificate authority to trust, in DER format
    #[clap(long = "ca")]
    ca: Option<PathBuf>,

    /// Simulate NAT rebinding after connecting
    #[clap(long = "rebind")]
    rebind: bool,

    /// Address to bind on
    #[clap(long = "bind", default_value = "[::]:0")]
    bind: SocketAddr,

    /// Optional: request N KB data (GetN mode)
    #[clap(long = "request-kb")]
    request_kb: Option<usize>,
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
    let mut stats = ClientStats::new();
    let url = options.url;
    let url_host = strip_ipv6_brackets(url.host_str().unwrap());
    let remote = (url_host, url.port().unwrap_or(4433))
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("couldn't resolve to an address"))?;

    let mut roots = rustls::RootCertStore::empty();
    if let Some(ca_path) = options.ca {
        roots.add(CertificateDer::from(fs::read(ca_path)?))?;
    } else {
        let dirs = directories_next::ProjectDirs::from("org", "quinn", "quinn-examples").unwrap();
        match fs::read(dirs.data_local_dir().join("cert.der")) {
            Ok(cert) => {
                roots.add(CertificateDer::from(cert))?;
            }
            Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
                info!("local server certificate not found");
            }
            Err(e) => {
                error!("failed to open local server certificate: {}", e);
            }
        }
    }
    let mut client_crypto = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    client_crypto.alpn_protocols = ALPN_QUIC_HTTP.iter().map(|&x| x.into()).collect();
    if options.keylog {
        client_crypto.key_log = Arc::new(rustls::KeyLogFile::new());
    }

    let mut client_config =
        quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(client_crypto)?));

    // disable GSO; in Mininet’s virtual links, GSO behaves unexpectedly and
    // results in oversized UDP packets being transmitted without MTU-based segmentation.
    // It should instead produce multiple MTU-sized UDP packets before transmission.
    let mut transport_config = quinn::TransportConfig::default();
    transport_config.enable_segmentation_offload(false);
    client_config.transport_config(Arc::new(transport_config));
    let mut endpoint = quinn::Endpoint::client(options.bind)?;
    endpoint.set_default_client_config(client_config);

    // Build request: support GetN mode via --request-kb
    let request = if let Some(kb) = options.request_kb {
        // Request request_kb * 1024 bytes
        let payload_bytes = kb * 1024;
        format!("GetN {}\r\n", payload_bytes)
    } else {
        format!("GET {}\r\n", url.path())
    };
    // Start to send the HTTP request.
    stats.request_start();
    let start = Instant::now();
    let rebind = options.rebind;
    let host = options.host.as_deref().unwrap_or(url_host);

    info!("connecting to {host} at {remote}");
    let conn = endpoint
        .connect(remote, host)?
        .await
        .map_err(|e| anyhow!("failed to connect: {}", e))?;
    info!("connected at {:?}", start.elapsed());
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow!("failed to open stream: {}", e))?;
    if rebind {
        let socket = std::net::UdpSocket::bind("[::]:0").unwrap();
        let addr = socket.local_addr().unwrap();
        eprintln!("rebinding to {addr}");
        endpoint.rebind(socket).expect("rebind failed");
    }

    send.write_all(request.as_bytes())
        .await
        .map_err(|e| anyhow!("failed to send request: {}", e))?;
    send.finish().unwrap();
    let response_start = Instant::now();
    info!("request sent at {:?}", response_start - start);
    let resp = recv
        .read_to_end(usize::MAX)
        .await
        .map_err(|e| anyhow!("failed to read response: {}", e))?;
    let duration = response_start.elapsed();
    eprintln!(
        "response received in {:?} - {} KiB/s",
        duration,
        resp.len() as f32 / (duration_secs(&duration) * 1024.0)
    );
    stats.bytes_recv(resp.len());
    if options.request_kb.is_none() {
        io::stdout().write_all(&resp).unwrap();
        io::stdout().flush().unwrap();
    } else {
        eprintln!(
            "Got {} bytes (GetN mode, requested {} KB) - not dumping to stdout",
            resp.len(),
            options.request_kb.unwrap()
        );
    }
    stats.print_stats();
    conn.close(0u32.into(), b"done");

    tokio::select! {
        _ = conn.closed() => {
            info!("connection closed (gracefully)");
        }
        _ = endpoint.wait_idle() => {
            info!("connection closed (drained)");
        }
    }

    Ok(())
}

fn strip_ipv6_brackets(host: &str) -> &str {
    // An ipv6 url looks like eg https://[::1]:4433/Cargo.toml, wherein the host [::1] is the
    // ipv6 address ::1 wrapped in brackets, per RFC 2732. This strips those.
    if host.starts_with('[') && host.ends_with(']') {
        &host[1..host.len() - 1]
    } else {
        host
    }
}

fn duration_secs(x: &Duration) -> f32 {
    x.as_secs() as f32 + x.subsec_nanos() as f32 * 1e-9
}
