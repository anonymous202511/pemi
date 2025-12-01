package main

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"flag"
	"log"
	"math/big"
	"net"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/quic-go/quic-go"
)

const MAX_DATAGRAM_SIZE = 1350

func main() {
	bindAddr := flag.String("p", "127.0.0.1:8080", "bind IP and port")
	flag.Parse()
	disableGSO()

	udpAddr, err := net.ResolveUDPAddr("udp", *bindAddr)
	if err != nil {
		log.Fatalf("Failed to resolve UDP address: %v", err)
	}

	conn, err := net.ListenUDP("udp", udpAddr)
	if err != nil {
		log.Fatalf("Listen UDP error: %v", err)
	}

	tlsConf, err := generateTLSConfig()
	if err != nil {
		log.Fatalf("TLS config error: %v", err)
	}

	listener, err := quic.Listen(conn, tlsConf, &quic.Config{})
	if err != nil {
		log.Fatalf("QUIC listen error: %v", err)
	}

	log.Printf("Server running on %s", *bindAddr)

	for {
		conn, err := listener.Accept(context.Background())
		if err != nil {
			log.Fatal(err)
		}
		handleConnection(conn)
	}
}

func handleConnection(conn *quic.Conn) {
	defer conn.CloseWithError(0, "")

	stream, err := conn.AcceptStream(context.Background())
	if err != nil {
		log.Println("Accept stream error:", err)
		return
	}

	buf := make([]byte, 4096)
	n, err := stream.Read(buf)
	if err != nil {
		log.Println("Read error:", err)
		return
	}

	request := strings.TrimSpace(string(buf[:n]))
	if strings.HasPrefix(request, "GETN") {
		numStr := strings.TrimSpace(strings.TrimPrefix(request, "GETN"))
		numBytes, err := strconv.Atoi(numStr)
		if err != nil || numBytes <= 0 {
			stream.CancelWrite(42)
			return
		}

		packetBuf := make([]byte, numBytes)

		start := time.Now()
		if err := writeFull(stream, packetBuf); err != nil {
			log.Println("Write error:", err)
			return
		}
		if err := stream.Close(); err != nil {
			log.Println("Stream close error:", err)
			return
		}
		elapsed := time.Since(start).Seconds()

		mb := float64(numBytes) / 1_000_000.0
		mbps := mb * 8.0 / elapsed
		KB := float64(numBytes) / 1024.0

		log.Printf("Send %.2f KB in %.3f s, goodput: %.2f Mbps\n", KB, elapsed, mbps)
		return
	}
}

func generateTLSConfig() (*tls.Config, error) {
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		return nil, err
	}

	tmpl := &x509.Certificate{
		SerialNumber: big.NewInt(1),
		NotBefore:    time.Now(),
		NotAfter:     time.Now().Add(24 * time.Hour),
		KeyUsage:     x509.KeyUsageKeyEncipherment | x509.KeyUsageDigitalSignature,
		ExtKeyUsage:  []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		Subject:      pkix.Name{CommonName: "localhost"},
	}

	certDER, err := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &key.PublicKey, key)
	if err != nil {
		return nil, err
	}

	cert := tls.Certificate{
		Certificate: [][]byte{certDER},
		PrivateKey:  key,
	}

	return &tls.Config{
		Certificates: []tls.Certificate{cert},
		NextProtos:   []string{"http/0.9"},
	}, nil
}

// disable GSO; in Mininet’s virtual links, GSO behaves unexpectedly and
// results in oversized UDP packets being transmitted without MTU-based segmentation.
// It should instead produce multiple MTU-sized UDP packets before transmission.
func disableGSO() {
	if err := os.Setenv("QUIC_GO_DISABLE_GSO", "true"); err != nil {
		log.Fatalf("failed to disable GSO: %v", err)
	}
}

func writeFull(stream *quic.Stream, data []byte) error {
	remaining := data
	for len(remaining) > 0 {
		n, err := stream.Write(remaining)
		if n > 0 {
			remaining = remaining[n:]
		}
		if err != nil {
			return err
		}
	}
	return nil
}
