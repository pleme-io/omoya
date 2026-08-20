"""Count distinct colours in a binary (P6) PPM, sampling on a grid."""
import sys


def main():
    path = sys.argv[1]
    with open(path, "rb") as f:
        data = f.read()
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
    i += 1
    w, h = parts[0], parts[1]
    seen = set()
    for y in range(0, h, 4):
        row = i + y * w * 3
        for x in range(0, w, 4):
            o = row + x * 3
            seen.add(data[o:o + 3])
    print(len(seen))


main()
