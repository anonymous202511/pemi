package main

import (
	"context"
	"crypto/tls"
	"flag"
	"fmt"
	"io"
	"log"
	"os"
	"sync"
	"sync/atomic"
	"time"

	"github.com/quic-go/quic-go"
)

func main() {
	serverAddr := flag.String("p", "127.0.0.1:8080", "server IP:port")
	requestFrames := flag.Int("f", 300, "number of frames to request")
	t := flag.Float64("t", 0.0, "Start time of the test (unix seconds)")
	flag.Parse()
	disableGSO()

	var baseline time.Time
	sec := int64(*t)
	nsec := int64((*t - float64(sec)) * 1e9)
	baseline = time.Unix(sec, nsec)

	tlsConf := &tls.Config{
		InsecureSkipVerify: true,
		NextProtos:         []string{"http/0.9"},
	}

	session, err := quic.DialAddr(context.Background(), *serverAddr, tlsConf, nil)
	if err != nil {
		log.Fatal("Dial error:", err)
	}
	defer session.CloseWithError(0, "")

	log.Printf("GetN request: %d frames ( %d seconds)", *requestFrames, int(*requestFrames/30))

	stream, err := session.OpenStreamSync(context.Background())
	if err != nil {
		log.Fatal("Open stream error:", err)
	}
	cmd := fmt.Sprintf("GETN %d\r\n", *requestFrames)
	if _, err := stream.Write([]byte(cmd)); err != nil {
		log.Fatal("Write GETN error:", err)
	}

	totalBytes := 0
	var totalBytesMutex sync.Mutex
	var wg sync.WaitGroup
	var frameCounter int64

	// record the actual request start time (for elapsed/goodput)
	requestStart := time.Now()

	// receive each server-initiated uni stream
	wg.Add(*requestFrames)
	for i := 0; i < *requestFrames; i++ {
		go func() {
			defer wg.Done()

			s, err := session.AcceptUniStream(context.Background())
			if err != nil {
				if qerr, ok := err.(*quic.ApplicationError); ok && qerr.ErrorCode == 0 {
					// normal close signal, ignore
					return
				} else {
					log.Println("AcceptUniStream error:", err)
					return
				}
			}

			buf := make([]byte, 12500)
			for {
				n, err := s.Read(buf)
				if n > 0 {
					totalBytesMutex.Lock()
					totalBytes += n
					totalBytesMutex.Unlock()
				}
				if err != nil {
					if err != io.EOF {
						log.Println("Read stream error:", err)
					}
					break
				}
			}
			id := int(atomic.AddInt64(&frameCounter, 1))
			fmt.Printf("frame %d, fin time: %.6f\n", id, time.Since(baseline).Seconds())
		}()
	}

	// wait for all frames to be received
	wg.Wait()

	elapsed := time.Since(requestStart).Seconds()
	mb := float64(totalBytes) / 1000.0 / 1000.0
	mbps := mb * 8.0 / elapsed

	log.Printf("Recv %s bytes in %.3f s, goodput: %.2f Mbps", printBytes(totalBytes), elapsed, mbps)
}

// disable GSO; in Mininet’s virtual links, GSO behaves unexpectedly and
// results in oversized UDP packets being transmitted without MTU-based segmentation.
// It should instead produce multiple MTU-sized UDP packets before transmission.
func disableGSO() {
	if err := os.Setenv("QUIC_GO_DISABLE_GSO", "true"); err != nil {
		log.Fatalf("failed to disable GSO: %v", err)
	}
}
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
