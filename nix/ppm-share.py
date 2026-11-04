"""Print the share of a binary (P6) PPM's pixels that are exactly one of some colours.

Usage: ppm-share <file> <r> <g> <b> [<r> <g> <b> ...]
"""
import sys


def main():
    path, numbers = sys.argv[1], [int(v) for v in sys.argv[2:]]
    want = {bytes(numbers[i:i + 3]) for i in range(0, len(numbers), 3)}
    with open(path, "rb") as f:
        data = f.read()
    parts = []
    i = 2
    while len(parts) < 3:
        while data[i:i + 1].isspace():
            i += 1
        j = i
        while not data[j:j + 1].isspace():
            j += 1
        parts.append(int(data[i:j]))
        i = j
    body = data[i + 1:]
    total = parts[0] * parts[1]
    if not want:
        pixels = [body[at:at + 3] for at in range(0, total * 3, 3)]
        top = sorted(set(pixels), key=pixels.count, reverse=True)[:6]
        print(" ".join(f"{p.hex()}:{pixels.count(p)}" for p in top))
        return
    same = sum(
        1 for at in range(0, total * 3, 3) if body[at:at + 3] in want
    )
    print(f"{same / total:.4f}")


main()
