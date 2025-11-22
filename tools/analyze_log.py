"""
Analyzing log files.
Output csv files.
Timestamps:
1. All server and client timestamps are starting from the first packet of h1.
2. PEMI timestamps are starting from the time when r1 recved the first packet from h1.
"""

import sys
from common import *
import argparse


# %%
class packet:
    def __init__(self, line):
        self.direction = line[0]
        self.time_from_str(line[1])
        self.id = line[2]

    def time_from_str(self, ts):
        self.time = time_from_str(ts)

    # replyed packets of this sent packet
    def set_replyed(self, recv):
        assert self.direction == "send"
        self.replyed = recv

    def get_replyed(self):
        return self.replyed


class PEMI_packet(packet):
    def __init__(self, line, sender):
        self.sender = sender  # server or client
        self.direction = "pemi"
        self.time_from_str(line[3])
        self.id = line[4]


def process_duplicated_pkt(pkt_id):
    # if begin with "c" and end with "000": may be a client handshake packet
    # if begin with "f0000": may be a server handshake packet
    if (pkt_id.startswith("c") and pkt_id.endswith("000")) or pkt_id.startswith(
        "f0000"
    ):
        print(f"warn: duplicated packet id: {pkt_id}\033[0m", file=sys.stderr)
    else:
        # generally, when PEMI is disabled, the packet should not be duplicated
        raise ValueError("duplicated packet id " + pkt_id)


# analyze send/recv packets of one peer
class peer_packets:
    def __init__(self, log_file, analyze_reply=False):
        self.recv = {}
        self.send = {}
        burst_recv = []
        with open(log_file, "r") as f:
            for line in f:
                line = line.strip().split()
                if line[0] == "[INFO]":
                    line = line[1:]  # remove [INFO] prefix
                if len(line) < 3:
                    continue
                if line[0] == "send":
                    pkt = packet(line)
                    if pkt.id in self.send:
                        process_duplicated_pkt(pkt.id)
                    if analyze_reply:
                        pkt.set_replyed(burst_recv)
                    self.send[pkt.id] = pkt
                    burst_recv = []
                elif line[0] == "recv":
                    pkt = packet(line)
                    if pkt.id in self.recv:
                        if not PEMI:
                            process_duplicated_pkt(pkt.id)
                    else:
                        self.recv[pkt.id] = pkt
                    burst_recv.append(line[2])

    def recv_time(self, id):
        if id in self.recv:
            return self.recv[id].time
        return -1


# analyze packets seen by middlebox r1
class pemi_packets:
    def __init__(self, log_file, server_pkts, client_pkts):
        self.processed = {}
        with open(log_file, "r") as f:
            for line in f:
                if line.startswith("[INFO] process pkt"):
                    if "process pkt(server)" in line:
                        sender = "server"
                    elif "process pkt(client)" in line:
                        sender = "client"
                    else:
                        raise ValueError("unknown PEMI log line: ", line)
                    line = line.strip().split()
                    pkt = PEMI_packet(line, sender)
                    if pkt.id in self.processed:
                        process_duplicated_pkt(pkt.id)
                    self.processed[pkt.id] = pkt
                    # check the packet seen by r1 is sent by server or client
                    if pkt.sender == "server":
                        if pkt.id not in server_pkts.send:
                            raise ValueError(
                                f"PEMI processed unknown packet id from server: {pkt.id}"
                            )
                    elif pkt.sender == "client":
                        if pkt.id not in client_pkts.send:
                            raise ValueError(
                                f"PEMI processed unknown packet id from client: {pkt.id}"
                            )

    def process_time(self, id):
        if id in self.processed:
            return self.processed[id].time
        return -1


# since we use h1 1st packet.time as start time, so sync the server and client time
# r1 times aren't synced
def get_time(timestamp):
    if timestamp == -1:
        return -1
    if timestamp < start_time:
        print(f"timestamp: {timestamp}, start_time: {start_time}")
        raise ValueError("timestamp < start_time")
    return round(timestamp - start_time, 1)  # ms, keep 1 decimal places


if __name__ == "__main__":
    parser = argparse.ArgumentParser(prog="Analyze Log")
    parser.add_argument(
        "--log-dir",
        default=".",
        help="Directory containing log files (default: current directory)",
    )
    parser.add_argument(
        "--pemi",
        action="store_true",
        default=False,
        help="If set, skip packet duplicate checks",
    )
    args = parser.parse_args()
    C1_LOG = f"{args.log_dir}/c1.log"
    S1_LOG = f"{args.log_dir}/s1.log"
    R1_LOG = f"{args.log_dir}/r1.log"
    # %% parse log files
    PEMI = args.pemi
    h1_packets = peer_packets(C1_LOG, analyze_reply=True)
    h2_packets = peer_packets(S1_LOG)
    r1_packets = pemi_packets(R1_LOG, h2_packets, h1_packets)

    # %%
    # use h1 1st packet.time as start time
    first_send_pkt_id = next(iter(h1_packets.send))
    start_time = h1_packets.send[first_send_pkt_id].time
    print(f"start time: {start_time}")

    summary = open(f"{args.log_dir}/summary_log.csv", "w")
    summary.write("direction,num,send_time,mid_time,recv_time,id,replyed\n")
    h1_idx = 0
    h2_idx = 0
    keys_h1_send = list(h1_packets.send.keys())
    keys_h2_send = list(h2_packets.send.keys())
    while h1_idx < len(keys_h1_send) or h2_idx < len(keys_h2_send):
        h1_time = (
            h1_packets.send[keys_h1_send[h1_idx]].time
            if h1_idx < len(keys_h1_send)
            else float("inf")
        )
        h2_time = (
            h2_packets.send[keys_h2_send[h2_idx]].time
            if h2_idx < len(keys_h2_send)
            else float("inf")
        )
        num = h1_idx + h2_idx + 1
        if h1_time < h2_time:
            h1_pkt = h1_packets.send[keys_h1_send[h1_idx]]
            direction = "h1->"
            send_time = get_time(h1_time)
            id = h1_pkt.id
            recv_time = get_time(h2_packets.recv_time(id))
            replyed = h1_pkt.get_replyed()
            h1_idx += 1
        else:
            h2_pkt = h2_packets.send[keys_h2_send[h2_idx]]
            direction = "<-h2"
            send_time = get_time(h2_time)
            id = h2_pkt.id
            recv_time = get_time(h1_packets.recv_time(id))
            replyed = ""  # haven't analyzed h2's replyed packets
            h2_idx += 1
        r1_time = r1_packets.process_time(id)
        summary.write(
            f"{direction},{num},{send_time},{r1_time},{recv_time},{id},{replyed}\n"
        )
