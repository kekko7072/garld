#!/usr/bin/env python3
"""Generate garld's application icons from code.

Draws the icon at 4x and box-downsamples for antialiasing, then writes PNGs, a
macOS .icns (via iconutil when available) and a Windows .ico. Pure standard
library, so the repository carries no pre-rendered binary art and the icon can
be tweaked by editing the numbers below.

    python3 packaging/icon/make_icons.py [outdir]
"""

from __future__ import annotations

import os
import shutil
import struct
import subprocess
import sys
import zlib

# --- artwork, in fractions of the canvas -------------------------------------

BACKGROUND = (0x16, 0x19, 0x1E)
BORDER = (0x3A, 0x3D, 0x44)
BAR_DIM = (0x2F, 0x7D, 0x4F)
BAR_MID = (0x3E, 0xA5, 0x66)
BAR_LIT = (0x53, 0xD1, 0x83)

PLATE_INSET = 0.045
PLATE_RADIUS = 0.225
BORDER_WIDTH = 0.012

# (centre x, height, colour) — a rising bar chart: a host under increasing load.
BARS = (
    (0.295, 0.26, BAR_DIM),
    (0.500, 0.40, BAR_MID),
    (0.705, 0.54, BAR_LIT),
)
BAR_WIDTH = 0.125
BAR_BASE = 0.775
BAR_RADIUS = 0.062

SUPERSAMPLE = 4
MACOS_SIZES = (16, 32, 128, 256, 512)
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)


def rounded_rect(x, y, left, top, right, bottom, radius):
    """Signed coverage test for a rounded rectangle: True when (x, y) is inside."""
    if x < left or x > right or y < top or y > bottom:
        return False
    cx = min(max(x, left + radius), right - radius)
    cy = min(max(y, top + radius), bottom - radius)
    dx, dy = x - cx, y - cy
    return dx * dx + dy * dy <= radius * radius


def blend(under, over, alpha):
    return tuple(round(u + (o - u) * alpha) for u, o in zip(under, over))


def render(size):
    """Returns RGBA bytes for one square icon of `size` pixels."""
    hi = size * SUPERSAMPLE
    # Render the plate and bars at high resolution as coverage masks.
    plate = bytearray(hi * hi)
    border = bytearray(hi * hi)
    bars = [bytearray(hi * hi) for _ in BARS]

    inner = PLATE_INSET + BORDER_WIDTH
    for py in range(hi):
        y = (py + 0.5) / hi
        row = py * hi
        for px in range(hi):
            x = (px + 0.5) / hi
            if rounded_rect(x, y, PLATE_INSET, PLATE_INSET,
                            1 - PLATE_INSET, 1 - PLATE_INSET, PLATE_RADIUS):
                border[row + px] = 1
                if rounded_rect(x, y, inner, inner, 1 - inner, 1 - inner,
                                PLATE_RADIUS - BORDER_WIDTH):
                    plate[row + px] = 1

    for index, (centre, height, _colour) in enumerate(BARS):
        left = centre - BAR_WIDTH / 2
        right = centre + BAR_WIDTH / 2
        top = BAR_BASE - height
        mask = bars[index]
        for py in range(hi):
            y = (py + 0.5) / hi
            if y < top - 0.02 or y > BAR_BASE + 0.02:
                continue
            row = py * hi
            for px in range(hi):
                x = (px + 0.5) / hi
                if x < left - 0.02 or x > right + 0.02:
                    continue
                if rounded_rect(x, y, left, top, right, BAR_BASE, BAR_RADIUS):
                    mask[row + px] = 1

    # Box-downsample every mask to the target size, then composite.
    out = bytearray(size * size * 4)
    area = SUPERSAMPLE * SUPERSAMPLE
    for y in range(size):
        for x in range(size):
            def coverage(mask):
                total = 0
                for sy in range(SUPERSAMPLE):
                    base = (y * SUPERSAMPLE + sy) * hi + x * SUPERSAMPLE
                    total += sum(mask[base:base + SUPERSAMPLE])
                return total / area

            border_a = coverage(border)
            if border_a == 0:
                continue

            plate_a = coverage(plate)
            colour = blend(BORDER, BACKGROUND, plate_a)
            for index, (_c, _h, bar_colour) in enumerate(BARS):
                bar_a = coverage(bars[index])
                if bar_a:
                    colour = blend(colour, bar_colour, bar_a)

            offset = (y * size + x) * 4
            out[offset:offset + 3] = bytes(colour)
            out[offset + 3] = round(border_a * 255)
    return bytes(out)


def write_png(path, size, rgba):
    def chunk(tag, payload):
        return (struct.pack(">I", len(payload)) + tag + payload
                + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF))

    stride = size * 4
    raw = b"".join(b"\x00" + rgba[y * stride:(y + 1) * stride] for y in range(size))
    with open(path, "wb") as handle:
        handle.write(b"\x89PNG\r\n\x1a\n")
        handle.write(chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)))
        handle.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        handle.write(chunk(b"IEND", b""))


def write_ico(path, pngs):
    """Writes a PNG-compressed .ico, which Windows has accepted since Vista."""
    with open(path, "wb") as handle:
        handle.write(struct.pack("<HHH", 0, 1, len(pngs)))
        offset = 6 + 16 * len(pngs)
        for size, data in pngs:
            handle.write(struct.pack(
                "<BBBBHHII",
                0 if size >= 256 else size,
                0 if size >= 256 else size,
                0, 0, 1, 32, len(data), offset,
            ))
            offset += len(data)
        for _size, data in pngs:
            handle.write(data)


def main():
    outdir = sys.argv[1] if len(sys.argv) > 1 else "packaging/icon/out"
    os.makedirs(outdir, exist_ok=True)

    cache = {}

    def rendered(size):
        if size not in cache:
            print(f"  rendering {size}x{size}", flush=True)
            cache[size] = render(size)
        return cache[size]

    # Linux and general-purpose PNGs.
    for size in (32, 64, 128, 256, 512):
        write_png(os.path.join(outdir, f"garld-{size}.png"), size, rendered(size))

    # Windows .ico
    ico_entries = []
    for size in ICO_SIZES:
        temp = os.path.join(outdir, f"_ico-{size}.png")
        write_png(temp, size, rendered(size))
        with open(temp, "rb") as handle:
            ico_entries.append((size, handle.read()))
        os.remove(temp)
    write_ico(os.path.join(outdir, "garld.ico"), ico_entries)

    # macOS .icns via iconutil, which needs a .iconset directory.
    iconset = os.path.join(outdir, "garld.iconset")
    os.makedirs(iconset, exist_ok=True)
    for size in MACOS_SIZES:
        write_png(os.path.join(iconset, f"icon_{size}x{size}.png"), size, rendered(size))
        double = size * 2
        write_png(os.path.join(iconset, f"icon_{size}x{size}@2x.png"), double, rendered(double))

    if shutil.which("iconutil"):
        icns = os.path.join(outdir, "garld.icns")
        subprocess.run(["iconutil", "-c", "icns", iconset, "-o", icns], check=True)
        print(f"wrote {icns}")
    else:
        print("iconutil not found (not macOS) — .iconset written, .icns skipped")

    print(f"icons written to {outdir}")


if __name__ == "__main__":
    main()
