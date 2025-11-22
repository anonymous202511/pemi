def time_from_str(ts):
    """
    input the time ends with units, like "ms", "s", "us", "ns"
    output time in ms
    """
    if ts.endswith("ms"):
        return float(ts[:-2])
    elif ts.endswith("us") or ts.endswith("µs"):
        return float(ts[:-2]) / 1000
    elif ts.endswith("ns"):
        return float(ts[:-2]) / 1000000
    elif ts.endswith("s"):
        return float(ts[:-1]) * 1000
    else:
        raise ValueError(f"unknown time unit: {ts}")
