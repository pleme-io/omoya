"""Count pixels in a rectangle that differ from a reference colour.

Usage: ppm-region <file> <x> <y> <w> <h> <r> <g> <b>

★ A REGION, NOT A POINT, BECAUSE CLIENTS DECIDE THEIR OWN SIZE.
`ppm-probe` samples one pixel, which only answers a layout question if you
already know how big the window is. A client is free to ignore the size in an
xdg configure — weston's demos do, they are fixed-size — so a point sampled at
the centre of a half-screen lands on the background even when the layout is
perfectly correct, and the assertion reads as "the windows are stacked" when
what actually happened is "the window is small".

Counting non-background pixels inside a half answers the question that was
really being asked: is there ANY client content over there?
"""
import sys


def read_ppm(path):
    with open(path, "rb") as f:
        data = f.read()
    # P6 <w> <h> <maxval>\n<binary>. Fields are whitespace-separated and may
    # be split across lines, so tokenise rather than assuming one header line.
    fields = []
    i = 0
    while len(fields) < 4:
        while i < len(data) and data[i:i + 1].isspace():
            i += 1
        if data[i:i + 1] == b"#":
            while i < len(data) and data[i:i + 1] != b"\n":
                i += 1
            continue
        start = i
        while i < len(data) and not data[i:i + 1].isspace():
            i += 1
        fields.append(data[start:i])
    i += 1
    magic, w, h = fields[0], int(fields[1]), int(fields[2])
    if magic != b"P6":
        raise SystemExit(f"not a P6 ppm: {magic!r}")
    return w, h, data[i:]


def main():
    if len(sys.argv) != 9:
        raise SystemExit(__doc__)
    path = sys.argv[1]
    x, y, rw, rh = (int(v) for v in sys.argv[2:6])
    ref = tuple(int(v) for v in sys.argv[6:9])
    w, h, px = read_ppm(path)
    differing = 0
    total = 0
    for row in range(y, min(y + rh, h)):
        base = row * w * 3
        for col in range(x, min(x + rw, w)):
            o = base + col * 3
            total += 1
            if (px[o], px[o + 1], px[o + 2]) != ref:
                differing += 1
    # Both numbers, always: `differing` alone cannot distinguish "no content"
    # from "the rectangle was off the edge of the image", and those need
    # different fixes.
    print(f"{differing} {total}")
    return 0


sys.exit(main())
