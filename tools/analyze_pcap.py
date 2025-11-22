"""
Analyzing pcap files.
Output csv files. All timestamps are starting from the first packet of h1.
"""

# %%
from scapy.all import rdpcap
import os
import argparse

parser = argparse.ArgumentParser(description="Analyze pcap files")
parser.add_argument(
    "--pcap_dir", type=str, default="pcap", help="Directory of pcap files"
)
parser.add_argument(
    "--pemi",
    action="store_true",
    default=False,
    help="If set, skip packet count assertion checks",
)  # to skip packet count assertion checks
args = parser.parse_args()

pcap_dir = args.pcap_dir
os.system(f"sudo chmod 777 {pcap_dir}/*.pcap")

h1_packets = rdpcap(f"{pcap_dir}/h1-eth0.pcap")
h2_packets = rdpcap(f"{pcap_dir}/h2-eth0.pcap")
r1_packets = rdpcap(f"{pcap_dir}/r1-eth0.pcap")

# use h1 1st packet.time as start time
start_time = h1_packets[0].time
print(f"start time: {start_time}")

h1_IP = "10.0.1.10"
h2_IP = "10.0.2.10"
# result csv files
h1toh2_csv = open("h1-h2.csv", "w")
h2toh1_csv = open("h2-h1.csv", "w")
# %%

""" example:
h1-h2.csv
id,size,h1_time,r1_time,h2_time
1,10,10000,-1,-1
"""
h1toh2_csv.write("num,size,h1_time,r1_time,h2_time,id\n")
h2toh1_csv.write("num,size,h2_time,r1_time,h1_time,id\n")


# %%
class statistics:
    def __init__(self):
        self.sent = 0
        self.arrive_mid = 0
        self.arrive_dst = 0


def get_time(timestamp):
    return round((timestamp - start_time) * 1000, 1)  # ms, keep 1 decimal places


# use first 8 bytes + last 8 bytes of UDP payload as packet id
def get_pkt_id(pkt):
    id = pkt["UDP"].load[:8] + pkt["UDP"].load[-8:]
    # return as hex
    return id.hex()


def analyze_packets(
    src_packets, middlebox_packets, dst_packets, src_IP, dst_IP, csv_file
):
    stats = statistics()
    print(f"Analyzing {src_IP} -> {dst_IP}")
    # output csv
    for i, pkt in enumerate(src_packets):
        if pkt.haslayer("UDP"):
            # only consider src -> dst packets
            if pkt["IP"].src != src_IP or pkt["IP"].dst != dst_IP:
                continue
            stats.sent += 1
            # get fields
            id = i + 1
            size = len(pkt)
            src_time = get_time(pkt.time)
            mid_time = -1
            dst_time = -1
            # check next 2 packets to find lost handshaking packets
            if i + 2 < len(src_packets) and (
                src_packets[i + 1]["UDP"] == pkt["UDP"]
                or src_packets[i + 2]["UDP"] == pkt["UDP"]
            ):
                # duplicate packet, this packet is lost
                print(f"Packet {id} is lost")
                pass
            else:
                for mid_pkt in middlebox_packets:
                    if pkt["UDP"] == mid_pkt["UDP"]:
                        mid_time = get_time(mid_pkt.time)
                        stats.arrive_mid += 1
                        break
                for dst_pkt in dst_packets:
                    if pkt["UDP"] == dst_pkt["UDP"]:
                        dst_time = get_time(dst_pkt.time)
                        stats.arrive_dst += 1
                        break
            # write to csv
            csv_file.write(
                f"{id},{size},{src_time},{mid_time},{dst_time},{get_pkt_id(pkt)}\n"
            )
            # print(f"{id},{size},{src_time},{mid_time},{dst_time},{get_pkt_id(pkt)}")
        else:
            # raise error
            raise ValueError("Packet is not UDP")
    return stats


# %%

h1toh2_stats = analyze_packets(
    h1_packets, r1_packets, h2_packets, h1_IP, h2_IP, h1toh2_csv
)
h1toh2_csv.close()
# %%
h2toh1_stats = analyze_packets(
    h2_packets, r1_packets, h1_packets, h2_IP, h1_IP, h2toh1_csv
)
h2toh1_csv.close()
# %%
if args.pemi == False:
    # check stats, make sure the capturing and analyzing(at least the packet count relation) are correct
    assert h1toh2_stats.arrive_mid + h2toh1_stats.arrive_mid == len(
        r1_packets
    )  # count check of all packets arrive at middlebox
    assert h1toh2_stats.arrive_dst + h2toh1_stats.sent == len(
        h2_packets
    )  # count check of all packets on h2
    assert h1toh2_stats.sent + h2toh1_stats.arrive_dst == len(
        h1_packets
    )  # count check of all packets on h1
# %%
# generate summary file
# write earlier record into summary

import pandas as pd

h1_to_h2 = pd.read_csv("h1-h2.csv")
h2_to_h1 = pd.read_csv("h2-h1.csv")

# write to summary
# write by the send_time
summary = open("summary.csv", "w")
summary.write("direction,num,size,send_time,r1_time,recv_time,id\n")
h1_idx = 0
h2_idx = 0
while h1_idx < len(h1_to_h2) and h2_idx < len(h2_to_h1):
    h1_time = h1_to_h2.loc[h1_idx, "h1_time"]
    h2_time = h2_to_h1.loc[h2_idx, "h2_time"]
    if h1_time < h2_time:
        summary.write(
            f"h1->,{h1_to_h2.loc[h1_idx, 'num']},{h1_to_h2.loc[h1_idx, 'size']},{h1_to_h2.loc[h1_idx, 'h1_time']},{h1_to_h2.loc[h1_idx, 'r1_time']},{h1_to_h2.loc[h1_idx, 'h2_time']},{h1_to_h2.loc[h1_idx, 'id']}\n"
        )
        h1_idx += 1
    else:
        summary.write(
            f"<-h2,{h2_to_h1.loc[h2_idx, 'num']},{h2_to_h1.loc[h2_idx, 'size']},{h2_to_h1.loc[h2_idx, 'h2_time']},{h2_to_h1.loc[h2_idx, 'r1_time']},{h2_to_h1.loc[h2_idx, 'h1_time']},{h2_to_h1.loc[h2_idx, 'id']}\n"
        )
        h2_idx += 1
# %% write remaining records
while h1_idx < len(h1_to_h2):
    summary.write(
        f"h1->,{h1_to_h2.loc[h1_idx, 'num']},{h1_to_h2.loc[h1_idx, 'size']},{h1_to_h2.loc[h1_idx, 'h1_time']},{h1_to_h2.loc[h1_idx, 'r1_time']},{h1_to_h2.loc[h1_idx, 'h2_time']},{h1_to_h2.loc[h1_idx, 'id']}\n"
    )
    h1_idx += 1
while h2_idx < len(h2_to_h1):
    summary.write(
        f"<-h2,{h2_to_h1.loc[h2_idx, 'num']},{h2_to_h1.loc[h2_idx, 'size']},{h2_to_h1.loc[h2_idx, 'h2_time']},{h2_to_h1.loc[h2_idx, 'r1_time']},{h2_to_h1.loc[h2_idx, 'h1_time']},{h2_to_h1.loc[h2_idx, 'id']}\n"
    )
    h2_idx += 1
summary.close()

# %%
