"""Ask omoya's kanshou socket one path, and print the answer as JSON.

Usage: kanshou-call <segment> [<segment> ...]
"""
import glob
import json
import socket
import struct
import sys


def query(path, segments):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(10)
    s.connect(path)
    request = json.dumps({"path": segments, "args": []}).encode()
    s.sendall(struct.pack(">I", len(request)) + request)
    size = struct.unpack(">I", s.recv(4))[0]
    body = b""
    while len(body) < size:
        body += s.recv(size - len(body))
    s.close()
    return json.loads(body)


def main():
    for candidate in sorted(glob.glob("/run/user/*/kanshou/omoya-*.sock")):
        try:
            answer = query(candidate, sys.argv[1:])
        except OSError:
            continue
        print(json.dumps(answer.get("Ok")))
        return 0 if "Ok" in answer else 1
    print("no LIVE omoya kanshou socket")
    return 1


sys.exit(main())
