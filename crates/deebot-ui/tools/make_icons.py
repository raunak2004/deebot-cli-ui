#!/usr/bin/env python3
"""Draws the Deebot app icon and writes every size the Android and desktop apps need.

Run from crates/deebot-ui:  python3 tools/make_icons.py
No dependencies: it renders the shapes itself (3x3 supersampling) and writes the PNG files.
"""
import math
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
RES = os.path.join(ROOT, "android-res")
ASSETS = os.path.join(ROOT, "assets")

NAVY_TOP = (27, 36, 51)
NAVY_BOTTOM = (13, 18, 24)
TEAL_LIGHT = (53, 208, 186)
TEAL_DARK = (31, 158, 142)
TEAL_FACE = (22, 105, 95)
EDGE = (127, 240, 223)
INK = (13, 18, 24)


def mix(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))


def over(base, color, alpha):
    """Composite `color` at `alpha` over `base` (RGB + alpha tuples in 0..255 / 0..1)."""
    br, bg, bb, ba = base
    out_a = alpha + ba * (1 - alpha)
    if out_a <= 0:
        return (0, 0, 0, 0)
    return tuple((color[i] * alpha + base[i] * ba * (1 - alpha)) / out_a for i in range(3)) + (out_a,)


def rounded_rect_alpha(x, y, radius):
    """Coverage of the unit square with rounded corners at (x, y)."""
    if not (0 <= x <= 1 and 0 <= y <= 1):
        return 0.0
    cx = min(max(x, radius), 1 - radius)
    cy = min(max(y, radius), 1 - radius)
    return 1.0 if math.hypot(x - cx, y - cy) <= radius else 0.0


def robot_layers(x, y, scale, background):
    """Colour at a point from the robot drawing. `scale` shrinks it around the centre."""
    # Move into the robot's own space, where it is centred at (0.5, 0.5) with a 0.30 body radius
    rx = 0.5 + (x - 0.5) / scale
    ry = 0.5 + (y - 0.5) / scale
    pixel = background

    # A trail of dots behind the robot hints that it is out cleaning
    for i, (dx, dy, r) in enumerate([(0.17, 0.84, 0.020), (0.235, 0.775, 0.026), (0.30, 0.71, 0.032)]):
        if math.hypot(rx - dx, ry - dy) <= r:
            pixel = over(pixel, TEAL_LIGHT, 0.30 + 0.20 * i)

    cx, cy = 0.55, 0.50
    d = math.hypot(rx - cx, ry - cy)
    # Soft shadow under the body
    shadow = math.hypot(rx - cx, ry - (cy + 0.025))
    if shadow <= 0.325:
        pixel = over(pixel, (0, 0, 0), 0.30 * (1 - shadow / 0.325))
    if d <= 0.30:
        # Body, lighter towards the top left
        t = min(1.0, max(0.0, ((rx - cx) + (ry - cy)) / 0.6 + 0.5))
        pixel = over(pixel, mix(TEAL_LIGHT, TEAL_DARK, t), 1.0)
        if d >= 0.285:
            pixel = over(pixel, EDGE, 0.55)
    if d <= 0.205:
        pixel = over(pixel, TEAL_FACE, 1.0)
    if d <= 0.200 and d >= 0.185:
        pixel = over(pixel, EDGE, 0.20)
    # The sensor bump on top
    bump = math.hypot(rx - (cx - 0.005), ry - (cy - 0.035))
    if bump <= 0.062:
        pixel = over(pixel, INK, 1.0)
        if math.hypot(rx - (cx - 0.025), ry - (cy - 0.055)) <= 0.016:
            pixel = over(pixel, EDGE, 0.85)
    # A small notch at the front edge shows which way it faces
    notch = math.hypot(rx - (cx + 0.245), ry - (cy - 0.07))
    if notch <= 0.026:
        pixel = over(pixel, INK, 0.9)
    return pixel


def render(size, mode):
    """mode: 'legacy' (rounded tile), 'foreground' (robot only), 'background' (solid tile)."""
    samples = 3
    rows = []
    for py in range(size):
        row = bytearray()
        for px in range(size):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(samples):
                for sx in range(samples):
                    x = (px + (sx + 0.5) / samples) / size
                    y = (py + (sy + 0.5) / samples) / size
                    if mode == "foreground":
                        # Android keeps the middle 66% of an adaptive icon safe from cropping
                        c = robot_layers(x, y, 0.62, (0, 0, 0, 0.0))
                    else:
                        tile = rounded_rect_alpha(x, y, 0.22) if mode == "legacy" else 1.0
                        shade = mix(NAVY_TOP, NAVY_BOTTOM, y)
                        base = shade + (tile,)
                        c = robot_layers(x, y, 1.0, base) if mode == "legacy" else base
                    # Accumulate premultiplied so soft edges average correctly
                    acc[0] += c[0] * c[3]
                    acc[1] += c[1] * c[3]
                    acc[2] += c[2] * c[3]
                    acc[3] += c[3]
            n = samples * samples
            a = acc[3] / n
            if a > 0:
                row += bytes([round(acc[0] / acc[3]), round(acc[1] / acc[3]), round(acc[2] / acc[3]), round(a * 255)])
            else:
                row += bytes([0, 0, 0, 0])
        rows.append(bytes(row))
    return rows


def write_png(path, size, rows):
    os.makedirs(os.path.dirname(path), exist_ok=True)

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + r for r in rows)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)
    print(f"wrote {os.path.relpath(path, ROOT)} ({size}x{size})")


def main():
    # Old-style launcher icons, one per screen density
    for density, size in [("mdpi", 48), ("hdpi", 72), ("xhdpi", 96), ("xxhdpi", 144), ("xxxhdpi", 192)]:
        write_png(os.path.join(RES, f"mipmap-{density}", "ic_launcher.png"), size, render(size, "legacy"))
    # Adaptive icon (Android 8+): the system masks it to the phone's icon shape
    write_png(os.path.join(RES, "mipmap-xxxhdpi", "ic_launcher_foreground.png"), 432, render(432, "foreground"))
    write_png(os.path.join(RES, "mipmap-xxxhdpi", "ic_launcher_background.png"), 108, render(108, "background"))
    xml = os.path.join(RES, "mipmap-anydpi-v26", "ic_launcher.xml")
    os.makedirs(os.path.dirname(xml), exist_ok=True)
    with open(xml, "w") as f:
        f.write(
            '<?xml version="1.0" encoding="utf-8"?>\n'
            '<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">\n'
            '    <background android:drawable="@mipmap/ic_launcher_background"/>\n'
            '    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>\n'
            "</adaptive-icon>\n"
        )
    print(f"wrote {os.path.relpath(xml, ROOT)}")
    # The desktop window icon
    write_png(os.path.join(ASSETS, "icon.png"), 256, render(256, "legacy"))


if __name__ == "__main__":
    main()
