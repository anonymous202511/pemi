"""
Process RTC frame logs to compute frame-level statistics (frame-level jitter, frame delay).
"""

import os


def save_frame_log(
    client_log="c1.log", server_log="s1.log", output_file="run.txt", append=True
):
    """
    save content of client_log, server_log to the result file
    if append is False, overwrite the output_file.
    """
    with open(client_log) as client_f, open(server_log) as server_f, open(
        output_file, "a" if append else "w"
    ) as result_f:
        for line in server_f:
            result_f.write(line)
        for line in client_f:
            result_f.write(line)


def parse_one_trail(lines):
    """
    Parse one trail's data from the log lines.
    """
    data = (
        {}
    )  # data: {end(server/client) : timestamps}; timestamps: {frame id: timestamp}
    data["server"] = {}
    data["client"] = {}
    begin_data = None
    video_long = None
    i = 0
    while i < len(lines):
        line = lines[i]
        i += 1
        line = line.strip()
        # found the data size, prepare to read data
        if "RTC Server GetN request:" in line:
            begin_data = "server"
            # example1: RTC Server GetN request: 300 frames
            # example2: RTC Server GetN request: 300 frames, each is 12500 B
            if "each is" in line:
                line = line.split("each is", 1)[0].rstrip(" ,")
            video_long = int(line.split(" ")[-2]) / 30
            # print(f"video_long: {video_long}")
            continue
        if begin_data is None:  # wait server data
            continue
        if not line.startswith("frame "):
            # Done reading data for this trail
            begin_data = "client"
        else:
            # Read a data point
            # example: frame 6, sent time: 420.885449
            line = line.split(" ")
            frame = int(line[1][:-1])
            time = float(line[-1])
            data["server"][frame] = time

        # after server data, read client data
        if begin_data == "client":
            begin = False  # turn True when find the begin of client data
            i = i - 1  # go back one line to re-process this line
            while i < len(lines):
                line = lines[i]
                i += 1
                line = line.strip()
                if "GetN request:" in line and "seconds" in line:
                    # GetN request: 300 frames( 10 seconds)
                    client_video_long = int(line.split(" ")[-2])
                    assert client_video_long == video_long
                    begin = True
                    continue
                if "Application error 0x0 (remote)" in line:
                    continue
                if not begin:
                    continue
                if not line.startswith("frame "):
                    # Done reading data for this trail
                    begin_data = None
                    video_long = None
                    return i, data
                else:
                    # Read a data point
                    # example: frame 1, fin time: 287.208894
                    line = line.split(" ")
                    frame = int(line[1][:-1])
                    time = float(line[-1])
                    data["client"][frame] = time
    return None, data  # no data found, end the parse


def parse_data(logfile="run.txt", trials=1, video_long=20):
    """
    Compute frame timestamps from the log file containing multiple trails.
    """
    data = []
    assert os.path.exists(logfile), f"file not found: {logfile}"

    with open(logfile) as f:
        lines = f.read().split("\n")
    while len(data) < trials:
        # print(f"len(lines): {len(lines)}")
        used_line, d = parse_one_trail(lines)
        if used_line is None:
            break

        lines = lines[used_line:]
        if d["server"] == {} or d["client"] == {}:
            continue
        # print(f"trail {len(data)}: {len(d['server'])} frames")
        if len(d["server"]) != video_long * 30 or len(d["client"]) != video_long * 30:
            print(f"warning: trail {len(data)}: {len(d['server'])} frames")
            continue
        data.append(d)
    return data


def compute_frame_stats(data, video_long):
    """
    Compute frame-level statistics.
    1. frame delay: client arrive time - server sent time.
    2. frame-level jitter: statistical variance of the frame interarrival time.

    reference: Enabling passive measurement of zoom performance in production networks (IMC ’22), and RFC 3550.
    """
    frame_delays = []
    jitter = []
    ideal_interval = 1.0 / 30.0  # 30 fps
    for d in data:
        last_fin = None
        interval_sample = None
        jitter_sample = 0.0
        for i in range(1, video_long * 30 + 1):
            frame_delays.append(d["client"][i] - d["server"][i])
            # to ms
            frame_delays[-1] *= 1000
            if last_fin is not None:
                # D(i,j) = (Rj - Ri) - (Sj - Si)
                # J(i) = J(i-1) + (|D(i-1,i)| - J(i-1))/16
                interval_sample = d["client"][i] - last_fin
                diff = abs(interval_sample - ideal_interval)
                jitter_sample = jitter_sample + (diff - jitter_sample) / 16.0
                jitter.append(jitter_sample * 1000)
            last_fin = d["client"][i]
    frame_delays.sort()
    jitter.sort()
    return frame_delays, jitter


def print_key_points(data, title):
    n = len(data)
    print(
        f"{title}: 99%: {data[int(n*0.99)]}, 95%: {data[int(n*0.95)]}, 90%: {data[int(n*0.9)]}, 50%: {data[int(n*0.5)]}"
    )


if __name__ == "__main__":
    # example: suppose a video_long=5 experiment has been run. Parse the log
    save_frame_log(append=False)  # overwrite the log file
    data = parse_data(logfile="run.txt", trials=1, video_long=5)
    frame_delays, jitter = compute_frame_stats(data, video_long=5)
    print_key_points(frame_delays, "Frame Delays")
    print_key_points(jitter, "Jitter")
