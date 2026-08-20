"""Read one leaf from omoya's kanshou introspection socket.

Usage: kanshou-get <leaf> [<leaf> ...]  ->  prints one value per line.

Separate from `kanshou-capture` because capture is a REQUEST-then-poll dance
with its own retry loop, while this is a single query. Folding them together
would give the simple read a retry budget it does not need and hide which of
the two failed.
"""
import glob
import json
import socket
import struct
import sys


def q(sock, path, args):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(10)
    s.connect(sock)
    r = json.dumps({"path": path, "args": args}).encode()
    s.sendall(struct.pack(">I", len(r)) + r)
    n = struct.unpack(">I", s.recv(4))[0]
    b = b""
    while len(b) < n:
        b += s.recv(n - len(b))
    s.close()
    return json.loads(b)


def main():
    socks = glob.glob("/run/user/*/kanshou/omoya-*.sock")
    if not socks:
        print("no omoya kanshou socket found", file=sys.stderr)
        return 1
    sock = socks[0]
    for leaf in sys.argv[1:]:
        r = q(sock, [leaf], [])
        # `kotae`: an answer says WHICH of four things happened. A missing
        # leaf must not print as a number, or a typo in the test reads as a
        # measurement of zero.
        if "Ok" not in r:
            print(f"leaf {leaf!r} did not answer: {r}", file=sys.stderr)
            return 1
        print(r["Ok"])
    return 0


sys.exit(main())
