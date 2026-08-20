"""Print one pixel from a binary (P6) PPM as "R G B"."""
import sys


def main():
    path, x, y = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    with open(path, "rb") as f:
        data = f.read()
    # Header is three whitespace-separated tokens after the magic: width,
    # height, maxval. Parsed by scanning rather than splitting the whole file,
    # which would copy megabytes of pixel data to read 15 bytes.
    parts = []
    i = 2
    while len(parts) < 3:
        while i < len(data) and data[i:i + 1].isspace():
            i += 1
        j = i
        while j < len(data) and not data[j:j + 1].isspace():
            j += 1
        parts.append(int(data[i:j]))
        i = j
    i += 1  # the single whitespace byte after maxval
    w = parts[0]
    off = i + (y * w + x) * 3
    print(data[off], data[off + 1], data[off + 2])


main()
