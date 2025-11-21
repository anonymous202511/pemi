import sys


def pemilog(val):
    """
    Print to stderr with prefix [PEMI]
    """
    print(f"[PEMI] {val}", file=sys.stderr)
