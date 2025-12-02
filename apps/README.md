# Applications Used to Evaluate PEMI

## quiche-based Applications

We evaluate PEMI using two applications, both implemented on top of quiche.

### 1. HTTP

We evaluate how PEMI improves QUIC goodput during file transfers of various sizes. Our setup uses nginx as the HTTP server and curl as the client, with both supporting HTTP/3 via quiche.
To enable an apple-to-apple comparison with TCP under transparent optimization, we modify quiche’s implementation of spurious congestion detection. The related discussion can be found at: https://github.com/cloudflare/quiche/issues/1411 and https://dl.acm.org/doi/10.1145/3618257.3624811.
The `http/` directory provides scripts for compiling these quiche-based versions of nginx and curl.

### 2. RTC Frames

We implement dummy media servers and clients using quiche to transmit frames at 30 fps and 3000 kbps. The server generates and sends one frame every 33 ms. Each frame is transmitted over a newly created stream.

Other transmission strategies are possible but not explored in this work, such as using a single stream for all frames, grouping multiple frames per stream (e.g., opening a new group for each key frame), or incorporating mechanisms that drop or skip older frames. These alternatives are outside the scope of this study.

## quinn-based Applications

### Requirements

Quinn does not expose its UDP packet sending/receiving to applications. 
To support log-based analysis (see `tools/README.md` for details), we modified quinn to add per-packet id and timestamp logging.
This implementation needs to be located under `apps/quinn-apps/deps`, and can be obtained via a Git command for fetching submodules (e.g., `git submodule update --init --recursive`).


### 1. File transfer

   Since we cannot test quinn using nginx and curl as we did with quiche, we implemented a simple quinn-based data-transfer application to measure goodput.

### 2. RTC frames

   Similar to the quiche evaluations, we implemented a dummy RTC application that sends frames.


## quic-go-based Applications

The application implementation of quic-go is similar to that of quinn, including file transfer and RTC frame transfer.

Similar to quinn, quic-go does not expose UDP packet sending/receiving. We currently have not modified the source code of quic-go to support log-based analysis.