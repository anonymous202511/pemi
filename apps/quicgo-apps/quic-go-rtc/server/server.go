package main

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"flag"
	"fmt"
	"io"
	"log"
	"math/big"
	"os"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/quic-go/quic-go"
)

const (
	FRAME_INTERVAL = 33 * time.Millisecond // 30fps
)

func main() {
	addr := flag.String("p", "127.0.0.1:8080", "server port")
	frameSize := flag.Int("f", 12500, "size of each frame in bytes")
	t := flag.Float64("t", 0.0, "Start time of the test (unix seconds)")
	flag.Parse()
	disableGSO()

	// compute start time baseline: use provided unix seconds (with fraction)
	var baseline time.Time
	sec := int64(*t)
	nsec := int64((*t - float64(sec)) * 1e9)
	baseline = time.Unix(sec, nsec)

	tlsConf := generateTLSConfig()
	quicConfig := &quic.Config{
		MaxIncomingStreams:    3000,
		MaxIncomingUniStreams: 3000,
	}

	listener, err := quic.ListenAddr(*addr, tlsConf, quicConfig)
	if err != nil {
		log.Fatal(err)
	}

	log.Printf("Server running on %s, frame size: %d bytes", *addr, *frameSize)

	for {
		session, err := listener.Accept(context.Background())
		if err != nil {
			log.Println("Accept session error:", err)
			continue
		}
		go handleSession(session, *frameSize, baseline)
	}
}

func handleSession(session *quic.Conn, frameSize int, startTime time.Time) {
	defer session.CloseWithError(0, "")

	buf := make([]byte, 4096)

	stream, err := session.AcceptStream(context.Background())
	if err != nil {
		log.Println("Accept stream error:", err)
		return
	}

	n, err := stream.Read(buf)
	if err != nil && err != io.EOF {
		log.Println("Read request error:", err)
		return
	}

	req := strings.TrimSpace(string(buf[:n]))
	if !strings.HasPrefix(req, "GETN") {
		log.Println("Unknown request:", req)
		return
	}

	numFrames, err := strconv.Atoi(strings.TrimSpace(strings.TrimPrefix(req, "GETN")))
	if err != nil {
		log.Println("Invalid GETN request number:", err)
		return
	}

	log.Printf("RTC Server GetN request: %d frames, each is %d B", numFrames, frameSize)

	var wg sync.WaitGroup
	var totalBytes int64

	// record actual request start time for elapsed/goodput
	requestStart := time.Now()

	for i := 0; i < numFrames; i++ {
		frame := make([]byte, frameSize)
		wg.Add(1)
		idx := i + 1
		go func(idx int, f []byte) {
			defer wg.Done()

			fs, err := session.OpenUniStreamSync(context.Background())
			if err != nil {
				if qerr, ok := err.(*quic.ApplicationError); ok && qerr.ErrorCode == 0 {
					return
				}
				log.Println("OpenStreamSync error:", err)
				return
			}

			fmt.Printf("frame %d, sent time: %.6f\n", idx, time.Since(startTime).Seconds())

			// write loop to handle partial writes
			remaining := f
			for len(remaining) > 0 {
				n, err := fs.Write(remaining)
				if n > 0 {
					atomic.AddInt64(&totalBytes, int64(n))
					remaining = remaining[n:]
				}
				if err != nil {
					// if stream write returns EOF or other error, log and stop trying for this stream
					if err == io.EOF {
						break
					}
					log.Println("Stream write error:", err)
					break
				}
			}

			fs.Close()
		}(idx, frame)

		time.Sleep(FRAME_INTERVAL)
	}

	wg.Wait()

	elapsed := time.Since(requestStart).Seconds()
	total := atomic.LoadInt64(&totalBytes)
	goodput := 0.0
	if elapsed > 0 {
		goodput = float64(total) * 8.0 / 1e6 / elapsed // Mbps
	}
	log.Printf("Sent %s in %.3f seconds, goodput: %.2f Mbps", printBytes(int(total)), elapsed, goodput)
}

// printBytes formats bytes into human-readable string similar to Rust impl
func printBytes(b int) string {
	units := []string{"B", "KB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"}
	size := float64(b)
	unit := 0
	for size >= 1024.0 && unit < len(units)-1 {
		size /= 1024.0
		unit++
	}
	return fmt.Sprintf("%.2f %s", size, units[unit])
}

func generateTLSConfig() *tls.Config {
	key, _ := rsa.GenerateKey(rand.Reader, 2048)
	template := &x509.Certificate{
		SerialNumber: big.NewInt(1),
		NotBefore:    time.Now(),
		NotAfter:     time.Now().Add(time.Hour),
		KeyUsage:     x509.KeyUsageKeyEncipherment | x509.KeyUsageDigitalSignature,
		ExtKeyUsage:  []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		Subject:      pkix.Name{CommonName: "localhost"},
	}
	certDER, _ := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	cert := tls.Certificate{
		Certificate: [][]byte{certDER},
		PrivateKey:  key,
	}
	return &tls.Config{
		Certificates: []tls.Certificate{cert},
		NextProtos:   []string{"http/0.9"},
	}
}

// disable GSO; in Mininet’s virtual links, GSO behaves unexpectedly and
// results in oversized UDP packets being transmitted without MTU-based segmentation.
// It should instead produce multiple MTU-sized UDP packets before transmission.
func disableGSO() {
	if err := os.Setenv("QUIC_GO_DISABLE_GSO", "true"); err != nil {
		log.Fatalf("failed to disable GSO: %v", err)
	}
}
