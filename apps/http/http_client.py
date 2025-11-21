import tempfile
from common import pemilog
import os
import argparse


def print_and_run_cmd(cmd):
    pemilog(cmd)
    return os.system(cmd)


def run_client(args, base_command, http_flag):
    if args.getfile is not None:
        print(f"transfer file: {args.getfile}")
        fname = args.getfile
    else:
        f = tempfile.NamedTemporaryFile()
        print_and_run_cmd(f"head -c {args.n} /dev/urandom > {f.name}")
        fname = f.name
        print(f"Data Size: {args.n}")
    print(f"HTTP: {http_flag}")

    cmd = f"{base_command} -o /tmp/pemidownload https://{args.addr}{fname} "
    cmd += f"{http_flag} --insecure "
    cmd += f"--max-time {args.timeout} "
    cmd += f"2>>{args.stderr} "  # need stdout(contains result data)

    # Run the command
    pemilog(cmd)
    if args.trials is None:
        fmt = r"\n\n      time_connect:  %{time_connect}s\n   time_appconnect:  %{time_appconnect}s\ntime_starttransfer:  %{time_starttransfer}s\n                   ----------\n        time_total:  %{time_total}s\n\nexitcode: %{exitcode}\nresponse_code: %{response_code}\nsize_upload: %{size_upload}\nsize_download: %{size_download}\nerrormsg: %{errormsg}\n"
        cmd += f'-w "{fmt}" '
        os.system(f"eval '{cmd}'")
    else:
        fmt = r"%{time_connect}\\t%{time_appconnect}\\t%{time_starttransfer}\\t\\t%{time_total}\\t%{exitcode}\\t\\t%{response_code}\\t\\t%{size_upload}\\t\\t%{size_download}\\t%{errormsg}\\n"
        cmd += f'-w "{fmt}" '
        header = "time_connect\ttime_appconnect\ttime_starttransfer\ttime_total\texitcode\tresponse_code\tsize_upload\tsize_download\terrormsg"
        # print immediately
        print(header, flush=True)
        for _ in range(args.trials):
            os.system(f"eval '{cmd}'")


def run_tcp_client(args):
    cmd = "RUST_LOG=info curl "
    run_client(args, cmd, "--http2")


def run_quic_client(args):
    cmd = "RUST_LOG=info curl "
    run_client(args, cmd, "--http3-only")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(prog="Http Client")
    parser.add_argument("-n", default=None, help="Number of bytes to send e.g. 1M")
    parser.add_argument(
        "--addr",
        default="127.0.0.1:443",
        help="Server address (default: 127.0.0.1:443)",
    )
    parser.add_argument(
        "--timeout", type=int, default=15, help="Timeout, in seconds (default: 15s)."
    )
    parser.add_argument(
        "--stdout",
        default="/dev/null",
        metavar="FILE",
        help="File to write stdout to (default: /dev/null)",
    )
    parser.add_argument(
        "--stderr",
        default="/dev/null",
        metavar="FILE",
        help="File to write stderr to (default: /dev/null)",
    )
    parser.add_argument(
        "-t", "--trials", type=int, default=1, help="Number of trials (default: 1)."
    )
    parser.add_argument(
        "--getfile",
        default=None,  # is set, the args.n is ignored
        metavar="FILE",
        help="File to send (default: None, generate random data)",
    )

    subparsers = parser.add_subparsers(required=True)
    tcp = subparsers.add_parser("tcp")
    tcp.set_defaults(func=run_tcp_client)
    quic = subparsers.add_parser("quic")
    quic.set_defaults(func=run_quic_client)

    args = parser.parse_args()
    if args.n is None and args.getfile is None:
        parser.error("Either --n or --getfile must be specified.")
    args.func(args)
