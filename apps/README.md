# Applications Used to Evaluate PEMI

We evaluate PEMI using two applications, both implemented on top of quiche.

### 1. HTTP

We evaluate how PEMI improves QUIC goodput during file transfers of various sizes. Our setup uses nginx as the HTTP server and curl as the client, with both supporting HTTP/3 via quiche.
To enable an apple-to-apple comparison with TCP under transparent optimization, we modify quiche’s implementation of spurious congestion detection. The related discussion can be found at: https://github.com/cloudflare/quiche/issues/1411 and https://dl.acm.org/doi/10.1145/3618257.3624811.
The `http/` directory provides scripts for compiling these quiche-based versions of nginx and curl.

### 2. RTC Frames

We implement dummy media servers and clients using quiche to transmit frames at 30 fps and 3000 kbps. The server generates and sends one frame every 33 ms.