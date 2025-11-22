"""
r1.log -> trace as json
"""

# %%

from common import *
import json
import argparse

SERVER_IP = "10.0.2.10"
CLIENT_IP = "10.0.1.10"


def get_pkt_traces(log_file):
    """
    Extracts packets observed by the middlebox (mid) in both directions from r1.log: timestamp, id, and size.

    :param log_file: Path to the log file
    :return: Lists of packets for the client and server sides
    """
    client_pkts = []
    server_pkts = []
    with open(log_file, "r") as f:
        for line in f:
            if line.startswith("[INFO] process pkt"):
                line = line.strip().split()
                time = time_from_str(line[3])  # ms
                id = line[4]
                if len(line) > 5:
                    size = int(line[5][:-1])  # remove 'B'. unit: bytes
                else:
                    # fake size
                    print(f"Warning: no size in {line}, using fake size -1")
                    size = -1
                if line[2] == "pkt(server)":
                    server_pkts.append((time, id, size))
                elif line[2] == "pkt(client)":
                    client_pkts.append((time, id, size))
                else:
                    raise ValueError(f"Unknown pkt type: {line[2]} in {line}")
    return client_pkts, server_pkts


# main
if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Parse r1.log to get a trace.")
    parser.add_argument(
        "--log_file",
        type=str,
        help="Path to the log file (e.g., ./r1.log)",
    )
    args = parser.parse_args()

    log_file = args.log_file

    client_pkts, server_pkts = get_pkt_traces(log_file)
    print(f"Client packets: {len(client_pkts)}")
    print(f"Server packets: {len(server_pkts)}")
    # save to json
    json_file = log_file.replace(".log", ".json")
    with open(json_file, "w") as f:
        json.dump(
            {
                "client": client_pkts,
                "server": server_pkts,
            },
            f,
            indent=4,
        )
        print(f"Saved trace: {json_file}")
