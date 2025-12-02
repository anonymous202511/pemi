# PEMI: Transparent Performance Enhancements for QUIC

## New Update!

We have evaluated PEMI with **quinn** and **quic-go**. The tested new applications have been added to the `apps/quinn-apps` and `apps/quicgo-apps` directories, along with updated test scripts such as `mininet/run.py`. Below are some result figures; full results can be found in [other_stacks_report/other_stacks_report.md](other_stacks_report/other_stacks_report.md).

For each stack, we tested two applications: a simple goodput test application, and a dummy RTC application that sends frames.

The following two figures were generated under the same settings:  
`loss1 = 1%, RTT1 = 2 ms, BW1 = 100 Mbps, loss2 = 0%, RTT2 = 50 ms, BW2 = 10 Mbps`.

- Goodput test with the quinn-based/quic-go-based data transfer application  

  <img src="other_stacks_report/other_stacks_goodput_loss1.png" alt="other_stacks_goodput" width="250"/>

- RTC test with the quinn-based/quic-go-based dummy RTC application  

  <img src="other_stacks_report/other_stacks_frame_delay.png" alt="other_stacks_rtc" width="250"/>




## Key Insight

PEMI runs on middleboxes and infers QUIC losses to provide fast retransmissions. This is normally impossible because QUIC encrypts both packet numbers and ACK frames. PEMI’s key insight is that many network traffic exhibits locality: packets naturally form flowlets. By leveraging this locality, PEMI can narrow down the set of sent packets that a returning packet most likely corresponds to, and then detect losses.

Such locality is very common in real traffic. Below are timing plots of server-sent packets from the largest QUIC flow in two example cases:

- Accessing a webpage on https://sourceforge.net
    ![web_locality](sourceforge_traffic.png)

- Watching the live media demo on https://moq.dev/ (a short slice)
    ![moq_demo](moq_media_traffic.png)

You can see that the transmitted packets form a series of small packet bursts (flowlets in PEMI), with clear gaps (milliseconds to hundreds of milliseconds in the above two cases) between them.
When these gaps are large enough, a middlebox can more easily match the receiver’s returning packets to a small region of sent packets (most likely the packets that triggered those returns), enabling further inference.

## Repository Overview

Our artifact includes the following directories:

#### pemi/
- Prototype implementation of PEMI.

#### apps/
- Example applications based on quiche, quinn, and quic-go.

#### mininet/
- Mininet-based testbed to evaluate PEMI under various network conditions.
- Mahimahi(CellReplay and LeoReplayer) integrated for realistic network emulation.

#### tools/
Utilities to get:
- Traces from the perspective of middle-boxes
- Ground truth of packet loss and timestamps of packets reached the each hop(sender, middle-boxes, receiver)

## Dependencies

We test our code on Ubuntu 22.04.

### Building PEMI

The prototype implementation of PEMI is in Rust. To build it, you need to install Rust and Cargo first. You can follow the instructions at https://rust-lang.org/tools/install.

CMake is also required. It could be installed via:
```bash
sudo apt update
sudo apt install cmake -y
```


After installing Rust and Cargo, you can build PEMI by running the following command:
```bash
cargo build --release
```

### Building applications

The quic-go-based applications(`apps/quicgo-apps`) need Go (we test with version 1.25.4).

Use make to build both PEMI and the applications:
```bash
make release
```

### Other Dependencies
The following dependencies need to be installed, required for different components:

```bash
sudo apt-get install -y mininet python3-pip  # mininet
sudo apt-get install -y autoconf libtool libpsl-dev libnghttp2-dev # curl
sudo apt-get install -y cmake libpcre3 libpcre3-dev zlib1g zlib1g-dev libssl-dev  # nginx
sudo apt-get install -y libnfnetlink-dev  # pepsal
```

To enable TCP traffic enhancement via `--pep` when running `mininet/run.py`, you need to install `pepsal`. See: https://github.com/CNES/pepsal.git.
The quiche-based nginx and curl installation scripts are in `apps/http/`.

If your system has another version of curl, it may interfere with the operation of the quiche-based one. 
This could be resolved by modifying the `PATH` environment variable, or simply uninstalling the other curl version.

The test scripts and tools also need some Python3 dependencies. You can install them via:
```bash
pip3 install -r requirements.txt
```

## Running Tests

After installing dependencies, building PEMI and the used components, you can run the Mininet-based testbed to evaluate PEMI.

If the dependencies are installed in a virtual environment created by `venv` or `conda`, you need to use the corresponding `python3` — e.g., the one located in the specific bin directory.

The HTTP file transfer and goodput tests will output the results to the stdout.
For the tests of transferring RTC frames, you could calculate the frame-level statistics by using `mininet/rtc_frame_stats.py`, its main function shows an example.

Here are some example commands:
```bash
# http + mininet(requires nginx and curl built with quiche)
sudo -E python3 mininet/run.py --loss1 1 http -n 5000k --proto tcp -t 2
sudo -E python3 mininet/run.py --loss1 1 --pep http -n 5000k --proto tcp -t 2 # requires pepsal
sudo -E python3 mininet/run.py --loss1 1 http -n 5000k --proto quic -t 2
sudo -E python3 mininet/run.py --loss1 1 --pemi http -n 5000k --proto quic -t 2

# goodput(simple data transferring) + mininet
sudo -E python3 mininet/run.py --loss1 1 quinn_goodput --size 1000
sudo -E python3 mininet/run.py --loss1 1 --pemi quinn_goodput --size 1000

# rtc + mininet
sudo -E python3 mininet/run.py --loss1 1 quiche_rtc --video-long 20
sudo -E python3 mininet/run.py --loss1 1 --pemi quiche_rtc --video-long 20

# rtc + mininet + cellular network emulation
sudo -E python3 mininet/run.py --delay2 50 --loss1ge 0.08 8 100 0 --loss-seed 1 --mm-config mininet/mahimahi/cell_tmobile_driving.toml --pemi quiche_rtc --video-long 30
sudo -E python3 mininet/run.py --delay2 50 --loss1ge 0.08 8 100 0 --loss-seed 1 --mm-config mininet/mahimahi/cell_tmobile_driving.toml --pemi quiche_rtc --video-long 30

# http + mininet + cellular network emulation (note that the GE-model in TC is per-packet driven, goodput affects the state duration, whereas good–bad transitions in real cellular scenarios are typically time-correlated).
# Compared with the random-loss setting, the GE-model loss events are less frequent (e.g., several consecutive seconds without any loss). As a result, the goodput improvement may be smaller.
sudo -E python3 mininet/run.py --delay2 50 --loss1ge 0.08 8 100 0 --loss-seed 1 --mm-config mininet/mahimahi/cell_tmobile_driving.toml http -n 20000k --proto quic -t 1
sudo -E python3 mininet/run.py --delay2 50 --loss1ge 0.08 8 100 0 --loss-seed 1 --mm-config mininet/mahimahi/cell_tmobile_driving.toml --pemi http -n 20000k --proto quic -t 1
```