"""Renders the app and tray icons with no dependencies: python3 scripts/icons.py"""
import math
import struct
import sys
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "app" / "src-tauri" / "icons"
INK = (14, 17, 22)
LIME = (198, 244, 50)


def coverage(distance):
    return max(0.0, min(1.0, 0.5 - distance))


def rounded(x, y, half, radius):
    dx, dy = abs(x) - (half - radius), abs(y) - (half - radius)
    return math.hypot(max(dx, 0), max(dy, 0)) + min(max(dx, dy), 0) - radius


def pixel(x, y, size, tray):
    scale = size / 64
    cx, cy = (x + 0.5 - size / 2) / scale, (y + 0.5 - size / 2) / scale
    r = math.hypot(cx, cy)
    ring = coverage((abs(r - 15) - 3.2) * scale)
    hub = coverage((r - 4.2) * scale)
    angle = math.atan2(cy, cx)
    spoke = min(abs(math.sin(angle - k * math.pi / 3)) * r for k in range(3))
    spokes = coverage((spoke - 1.5) * scale) * (1.0 if 4 < r < 15 else 0.0)
    dial = max(ring, hub, spokes)
    if tray:
        return (255, 255, 255, round(255 * dial))
    body = coverage(rounded(cx, cy, 29, 14) * scale)
    color = tuple(round(INK[i] + (LIME[i] - INK[i]) * dial) for i in range(3))
    return (*color, round(255 * body))


def png(size, tray=False):
    rows = b"".join(
        b"\x00" + bytes(c for x in range(size) for c in pixel(x, y, size, tray)) for y in range(size)
    )
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))
    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b"")


def ico(image):
    entry = struct.pack("<BBBBHHII", 0, 0, 0, 0, 1, 32, len(image), 22)
    return struct.pack("<HHH", 0, 1, 1) + entry + image


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for name, size in (("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("icon.png", 512)):
        (OUT / name).write_bytes(png(size))
    (OUT / "tray.png").write_bytes(png(64, tray=True))
    (OUT / "icon.ico").write_bytes(ico(png(256)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
