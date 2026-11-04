import glob
import json
import socket
import struct
import sys
import time


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


dest = sys.argv[1]
# The first socket that ANSWERS — see nix/kanshou-get.py.
# A kanshou socket outlives a process that dies hard, and
# picking a dead one reads as "the compositor is not
# answering" rather than "that file has no owner".
sock = None
for cand in sorted(
    glob.glob("/run/user/*/kanshou/omoya-*.sock")
):
    try:
        probe = socket.socket(
            socket.AF_UNIX, socket.SOCK_STREAM
        )
        probe.settimeout(2)
        probe.connect(cand)
        probe.close()
        sock = cand
        break
    except OSError:
        continue
if sock is None:
    print("no LIVE omoya kanshou socket found")
    sys.exit(1)
print("socket:", sock)
# ★ PARSE THE ENVELOPE, DO NOT STRING-MATCH IT.
#
# This probe read `str(result).startswith("ok:")`, which
# was right until `capture_result` became a JSON envelope
# (`{"request_id": N, "outcome": "ok:..."}`, drm.rs:1842).
# From that commit the prefix never matched and this gate
# — the ONLY pixel-level check omoya has — was red, while
# reading like a genuine capture failure.
#
# And matching a prefix was never sufficient anyway:
# `capture_result` OUTLIVES the client that asked, and
# introspect.rs's own note says to compare `request_id`
# against the id the request returned. Without that, a
# probe can read a PREVIOUS capture's success and pass on
# a seat that captured nothing.
req = q(sock, ["capture"], [dest])
print("request:", req)
want = req["Ok"]["request_id"]
for _ in range(50):
    time.sleep(0.2)
    r = q(sock, ["capture_result"], [])
    if not r.get("Ok"):
        continue
    env = json.loads(r["Ok"])
    print("result:", env)
    got = env.get("request_id")
    if got != want:
        # A stale result from an earlier request: keep
        # waiting rather than judging on someone else's.
        print(f"stale request_id {got} != {want}, waiting")
        continue
    outcome = str(env.get("outcome"))
    sys.exit(0 if outcome.startswith("ok:") else 1)
print("capture never completed")
sys.exit(1)
