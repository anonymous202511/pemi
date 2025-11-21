import sys
import time


def mac(digit):
    assert 0 <= digit < 10
    return f"00:00:00:00:00:0{int(digit)}"


def ip(digit):
    assert 0 <= digit < 10
    return f"10.0.{int(digit)}.10/24"


def pemilog(val):
    """
    Print to stderr with prefix [PEMI]
    """
    print(f"[PEMI] {val}", file=sys.stderr)


def popen(host, cmd):
    """
    Run a command on a Mininet host and print its output.
    """
    p = host.popen(cmd.split(" "))
    exitcode = p.wait()
    for line in p.stderr:
        sys.stderr.buffer.write(line)
    for line in p.stdout:
        sys.stdout.buffer.write(line)
    sys.stderr.buffer.flush()
    sys.stdout.buffer.flush()
    if exitcode != 0:
        print(f"{host}({cmd}) = {exitcode}")
        sys.stderr.buffer.write(b"\n")
        sys.stderr.buffer.flush()
        exit(1)


def check_cap_start(log_file, tool="tcpdump", timeout=10):
    """
    Check whether the packet capture has started by monitoring the log file.
    """
    start_time = time.time()
    start_sign = {
        "tcpdump": "listening on",
        "tshark": "Capturing on",
    }
    while True:
        if time.time() - start_time > timeout:
            raise TimeoutError(
                f"{tool} did not start capturing within {timeout} seconds."
            )
        try:
            with open(log_file, "r") as f:
                if start_sign[tool] in f.read():
                    break
        except FileNotFoundError:
            pass  # file not created yet
        time.sleep(0.1)


def estimate_timeout(n, quic, loss):
    """
    Timeout is linear in the data size, larger if
    the client uses HTTP/3 instead of HTTP/1.1, larger when there is more loss,
    and has a floor of 15 seconds. Otherwise defaults to 5 minutes. Timeout is
    measured in seconds.
    """
    try:
        if "k" in n:
            kb = int(n[:-1])
        elif "M" in n:
            kb = int(n[:-1]) * 1000
        scale = 0.01
        if quic:
            scale *= 4
        if float(loss) > 1:
            scale *= float(loss) / 1.5
        return max(int(scale * kb), 15)
    except:
        return 300


def get_max_queue_size_bytes(rtt_ms, bw_mbitps):
    bdp = rtt_ms * bw_mbitps * 1000000.0 / 1000.0 / 8.0
    bdp = max(
        5000, bdp
    )  # 5KB min size. too small setup will cause fail when set the network
    return bdp
