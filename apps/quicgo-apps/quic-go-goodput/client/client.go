package main

import (
	"context"
	"crypto/tls"
	"flag"
	"fmt"
	"io"
	"log"
	"os"
	"time"

	"github.com/quic-go/quic-go"
)

const MAX_DATAGRAM_SIZE = 1350

type ClientStats struct {
	bytesRecv     int
	intervalRecv  int
	startTime     time.Time
	lastPrintTime time.Time
}

func NewClientStats() *ClientStats {
	now := time.Now()
	return &ClientStats{
		bytesRecv:     0,
		intervalRecv:  0,
		lastPrintTime: now,
		startTime:     now,
	}
}

func (s *ClientStats) Add(n int) {
	s.bytesRecv += n
	s.intervalRecv += n

	elapsedSec := time.Since(s.startTime).Seconds()
	if elapsedSec-s.lastPrintTime.Sub(s.startTime).Seconds() >= 1.0 {
		start := int(elapsedSec) - 1
		end := int(elapsedSec)
		fmt.Printf("%d-%d sec   %.2f MB   %.2f Mbits/sec\n",
			start,
			end,
			float64(s.intervalRecv)/1_000_000.0,
			float64(s.intervalRecv)/1_000_000.0*8.0)
		s.intervalRecv = 0
		s.lastPrintTime = time.Now()
	}
}

func (s *ClientStats) PrintFinal() {
	elapsed := time.Since(s.startTime).Seconds()

	if s.intervalRecv > 0 {
		startSec := elapsed - (elapsed - s.lastPrintTime.Sub(s.startTime).Seconds())
		fmt.Printf("%d-%.3f sec   %.2f MB   %.2f Mbits/sec\n",
			int(startSec),
			elapsed,
			float64(s.intervalRecv)/1_000_000.0,
			float64(s.intervalRecv)/1_000_000.0*8.0/(elapsed-startSec))
	}

	fmt.Printf("Recv %.2f KB bytes in %.3f s, goodput: %.2f Mbps\n",
		float64(s.bytesRecv)/1024.0,
		elapsed,
		float64(s.bytesRecv)/1_000_000.0*8.0/elapsed)
}

func main() {
	serverAddr := flag.String("p", "127.0.0.1:8080", "server IP and port")
	requestKB := flag.Int("n", 1, "request_kb")
	flag.Parse()
	disableGSO()

	tlsConf := &tls.Config{
		InsecureSkipVerify: true,
		NextProtos:         []string{"http/0.9"},
	}

	session, err := quic.DialAddr(context.Background(), *serverAddr, tlsConf, nil)
	if err != nil {
		log.Fatal("Dial error:", err)
	}
	defer session.CloseWithError(0, "")

	stream, err := session.OpenStreamSync(context.Background())
	if err != nil {
		log.Fatal("Open stream error:", err)
	}

	// send a GETN request
	payload_bytes := 1024 * (*requestKB)
	cmd := "GETN " + fmt.Sprintf("%d", payload_bytes) + "\r\n"
	_, err = stream.Write([]byte(cmd))
	if err != nil {
		log.Fatal("Write GETN error:", err)
	}

	stats := NewClientStats()
	buf := make([]byte, 65536)

	for {
		n, err := stream.Read(buf)
		if n > 0 {
			stats.Add(n)
		}
		if err != nil {
			if err != io.EOF {
				// ignore ApplicationError 0x0 (normal close signal)
				if qe, ok := err.(*quic.ApplicationError); ok && qe.ErrorCode == 0 {
					break
				}
				log.Println("Read error:", err)
			}
			break
		}
	}

	stats.PrintFinal()
}

// disable GSO; in Mininet’s virtual links, GSO behaves unexpectedly and
// results in oversized UDP packets being transmitted without MTU-based segmentation.
// It should instead produce multiple MTU-sized UDP packets before transmission.
func disableGSO() {
	if err := os.Setenv("QUIC_GO_DISABLE_GSO", "true"); err != nil {
		log.Fatalf("failed to disable GSO: %v", err)
	}
}
