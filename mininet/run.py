"""
This file is used to run a test.
"""

from network import (
    PEMINetwork,  # PEMI test env in mininet
)

from common import *
import argparse
from mininet.log import setLogLevel
from mininet.cli import CLI
import time


def monitor_func(args, net):
    if args.iperf_r1 is not None:
        net.run_iperf(args.iperf_r1, host="r1")
    elif args.iperf is not None:
        net.run_iperf(args.iperf, host="h1")
    elif args.iperf_max is not None:
        net.run_iperf(args.iperf_max, host="h1", max=True)
    elif args.ping is not None:
        net.run_ping(num_pings=args.ping)
    else:
        # default: pings with default number of pings
        net.run_ping()


def quinn_goodput_func(args, net):
    net.start_quinn_goodput_server(args.log_level)
    net.start_quinn_goodput_client(args.log_level, args.size)


def quinn_rtc_func(args, net):
    net.start_quinn_rtc_server(args.log_level)
    net.start_quinn_rtc_client(args.log_level, args.video_long)


def quicgo_goodput_func(args, net):
    net.start_quicgo_goodput_server(args.log_level)
    net.start_quicgo_goodput_client(args.log_level, args.size)


def quicgo_rtc_func(args, net):
    net.start_quicgo_rtc_server(args.log_level)
    net.start_quicgo_rtc_client(args.log_level, args.video_long)


def quiche_rtc_func(args, net):
    start_time = time.time()
    net.start_quiche_rtc_server(args.log_level, start_time)
    net.start_quiche_rtc_client(args.log_level, start_time, args.video_long)


def http_func(args, net):
    net.start_webserver()
    # client
    if args.timeout:
        timeout = args.timeout
    else:
        timeout = estimate_timeout(
            args.n,
            args.proto == "quic",
            loss=max(args.loss1, args.loss2),
        )
    client_cmd = (
        f"{net.client_mm_prefix} python3 apps/http/http_client.py --addr {net.h2.IP()}:443 -n {args.n} "
        f"--trials {args.trials} "
        f"--stdout {args.stdout} --stderr {args.stderr} "
        f"--timeout {timeout} "
        f"{args.proto} "
        f"{net.client_mm_suffix}"
    )
    pemilog("Starting the client on h1...")
    net.h1.cmdPrint(client_cmd)


# basic setup
setLogLevel("info")
parser = argparse.ArgumentParser(
    prog="sudo -E python3 mininet/main.py",
    description="PEMI emulation using Mininet",
)
# Network Configurations
net_config = parser.add_argument_group("net_config")
# TC parameters, supported in mininet
net_config.add_argument(
    "--delay1",
    type=float,
    default=1,
    metavar="MS",
    help="1/2 RTT between h1 and r1 (default: 1)",
)
net_config.add_argument(
    "--delay2",
    type=float,
    default=25,
    metavar="MS",
    help="1/2 RTT between r1 and h2 (default: 25)",
)
net_config.add_argument(
    "--loss1ge",
    nargs=4,
    type=float,
    default=None,
    help="Four parameters for GE model. If set, use GE model for loss1. Example: --loss1ge 0.08 8 100 0; 4 paras: good->bad, bad->good, bad_loss, good_loss.",
)
net_config.add_argument(
    "--loss1",
    type=float,
    default=3.6,
    metavar="PERCENT",
    help="loss (in %%) between h1 and r1 (default: 3.6). Only used if --loss1ge is not set.",
)
net_config.add_argument(
    "--loss2",
    type=float,
    default=0,
    metavar="PERCENT",
    help="loss (in %%) between r1 and h2 (default: 0)",
)
net_config.add_argument(
    "--loss-seed",
    type=int,
    default=None,
    metavar="SEED",
    help="Optional random seed for netem loss (passed to `seed`); if set, netem commands will include 'seed <SEED>'",
)
net_config.add_argument(
    "--bw1",
    type=int,
    default=100,
    metavar="MBPS",
    help="link bandwidth (in Mbps) between h1 and r1 (default: 100)",
)
net_config.add_argument(
    "--bw2",
    type=int,
    default=10,
    metavar="MBPS",
    help="link bandwidth (in Mbps) between r1 and h2 (default: 10)",
)
net_config.add_argument(
    "--qdisc",
    default="grenville",
    help="queuing discipline [tbf|cake|codel|red|grenville|none]",
)
# Additional set: use Mahimahi(CellReplay/LeoReplayer) to control the first hop (h1<->r1) on client side. Enable by providing a TOML config file.
net_config.add_argument(
    "--mm-config",
    type=str,
    default=None,
    metavar="MM_CONFIG",
    help=(
        "Path to Mahimahi TOML config file. If provided, Mahimahi(CellReplay/LeoReplayer) is enabled."
    ),
)

# Other net configurations
net_config.add_argument(
    "--cap", action="store_true", default=False, help="capture packets"
)
net_config.add_argument(
    "--pep", action="store_true", default=False, help="start pepsal on r1"
)
net_config.add_argument(
    "--pemi", action="store_true", default=False, help="start PEMI on r1"
)
net_config.add_argument(
    "--pemi-proxy-only",
    action="store_true",
    default=False,
    help="start PEMI on r1 with proxy only",
)
# FLOWLET_INTERVAL_FACTOR for PEMI
net_config.add_argument(
    "--fl-inv-factor",
    type=float,
    default=2.0,
    help="FLOWLET_INTERVAL_FACTOR (default: 2.0)",
)
# FLOWLET_END_FACTOR for PEMI
net_config.add_argument(
    "--fl-end-factor",
    type=float,
    default=0.5,
    help="FLOWLET_END_FACTOR (default: 0.5)",
)
net_config.add_argument(
    "--log-level",
    type=str,
    default="error",
    help="log level of PEMI rust logger [error|warn|info|debug|trace|trace] (default: error)",  # most used: error(for experiment), info(for middlebox traces collection), debug(for debug)
)
############################################################################
# cli
subparsers = parser.add_subparsers(required=True)
cli = subparsers.add_parser("cli", help="start mininet CLI")
cli.set_defaults(func=lambda _: CLI(net.net))

############################################################################
# Network monitoring tests
monitor = subparsers.add_parser("monitor", help="network monitoring")
monitor.set_defaults(func=monitor_func)
monitor.add_argument(
    "--ping", type=int, help="Run this number of pings between each peer of hosts."
)
monitor.add_argument(
    "--iperf-r1",
    type=int,
    metavar="TIME_S",
    help="Run an iperf test(100M) for this length of time with a server on h2 "
    "and client on r1.",
)
monitor.add_argument(
    "--iperf",
    type=int,
    metavar="TIME_S",
    help="Run an iperf test(100M) for this length of time with a server on h2 "
    "and client on h1.",
)
monitor.add_argument(
    "--iperf-max",
    type=int,
    metavar="TIME_S",
    help="Run an iperf test to measure the maximum bandwidth between h1 and h2.",
)
############################################################################
# QUIC(quinn stack) goodput bench: quinn client - switch - quinn server
quinn_goodput = subparsers.add_parser("quinn_goodput", help="QUIC(quinn) goodput bench")
quinn_goodput.set_defaults(func=quinn_goodput_func)
quinn_goodput.add_argument(
    "--size",
    type=int,
    default=1000,
    metavar="KB",
    help="Size of the request in KB (default: 1000)",
)

############################################################################
# RTC(quinn): dummy media app based on quinn stack
quinn_rtc = subparsers.add_parser("quinn_rtc", help="RTC dummy app based on quinn")
quinn_rtc.set_defaults(func=quinn_rtc_func)
quinn_rtc.add_argument(
    "--video-long",
    type=int,
    default=10,
    help="Video time in seconds, default: 10",
)

############################################################################
# QUIC(quicgo stack) goodput bench: quicgo client - switch - quicgo server
quicgo_goodput = subparsers.add_parser(
    "quicgo_goodput", help="QUIC(quicgo) goodput bench"
)
quicgo_goodput.set_defaults(func=quicgo_goodput_func)
quicgo_goodput.add_argument(
    "--size",
    type=int,
    default=1000,
    metavar="KB",
    help="Size of the request in KB (default: 1000)",
)
############################################################################
# RTC(quicgo): dummy media app based on quicgo stack
quicgo_rtc = subparsers.add_parser("quicgo_rtc", help="RTC dummy app based on quicgo")
quicgo_rtc.set_defaults(func=quicgo_rtc_func)
quicgo_rtc.add_argument(
    "--video-long",
    type=int,
    default=10,
    help="Video time in seconds, default: 10",
)

############################################################################
# RTC: dummy media app based on quiche stack
quiche_rtc = subparsers.add_parser("quiche_rtc", help="RTC dummy app based on quiche")
quiche_rtc.set_defaults(func=quiche_rtc_func)
quiche_rtc.add_argument(
    "--video-long",
    type=int,
    default=10,
    help="Video time in seconds, default: 10",
)
############################################################################
# http: curl client - router - http server(with nginx)
# test a GET download request
http = subparsers.add_parser("http", help="HTTP emulation to test FCT")
http.set_defaults(func=http_func)
http.add_argument(
    "-n", default="1M", metavar="BYTES_STR", help="Number of bytes (default: 1M)"
)
http.add_argument(
    "--stdout",
    default="/tmp/pemistdout",
    metavar="FILENAME",
    help="File to write curl stdout (default: /tmp/pemistdout)",
)
http.add_argument(
    "--stderr",
    default="/tmp/pemistderr",
    metavar="FILENAME",
    help="File to write curl stderr (default: /tmp/pemistderr)",
)
http.add_argument(
    "--timeout",
    type=int,
    metavar="S",
    help="Timeout, in seconds. Default is estimated.",
)
http.add_argument(
    "--proto",
    type=str,
    default="tcp",
    help="Protocol to use [tcp|quic]. Default is tcp(http2).",
)
http.add_argument(
    "-t", "--trials", type=int, default=1, help="Number of trials (default: 1)."
)
############################################################################

if __name__ == "__main__":
    args = parser.parse_args()
    print("Args setup:", args)
    net = PEMINetwork(
        args.delay1,
        args.delay2,
        args.loss1ge,
        args.loss1,
        args.loss2,
        args.bw1,
        args.bw2,
        args.qdisc,
        args.mm_config,
        args.loss_seed,
    )
    pemilog(f"Link1 delay={args.delay1} loss={args.loss1} bw={args.bw1}")
    pemilog(f"Link2 delay={args.delay2} loss={args.loss2} bw={args.bw2}")
    pemilog(f"Mahimahi config: {args.mm_config}")

    # init arp
    net.init_arp()

    # start packet capture
    if args.cap:
        net.start_capture(args, net)

    # start the pemi
    if args.pep:
        net.start_tcp_pep()
    if args.pemi or args.pemi_proxy_only:
        net.start_pemi(
            args.log_level, args.fl_inv_factor, args.fl_end_factor, args.pemi_proxy_only
        )
    try:
        args.func(args, net)
    except Exception as e:
        pemilog(f"[Error] error occurred: {e}")
        net.stop()
        raise e
    net.stop()
