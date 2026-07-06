#!/usr/bin/env python3
"""Generate Windows .ico and macOS .icns icons from assets/icons/pi.svg.

The original SVG is left untouched; this script wraps it with a black
background and a green foreground colour before rendering the bitmaps.
"""

import re
from pathlib import Path

import cairosvg

ROOT = Path(__file__).resolve().parent.parent
SVG_PATH = ROOT / "assets" / "icons" / "pi.svg"
ICO_PATH = ROOT / "scripts" / "installer" / "app.ico"
ICNS_PATH = ROOT / "scripts" / "installer" / "app.icns"
TRAY_PATH = ROOT / "assets" / "icons" / "tray_icon.png"
TRAY_SIZE = 64

ICO_SIZES = [16, 32, 48, 64, 128, 256]
ICNS_SIZES = [16, 32, 64, 128, 256, 512, 1024]

ICON_GREEN = "#7fff6e"
ICON_BACKGROUND = "#000000"
TRAY_FOREGROUND = "#000000"

# Apple ICNS type codes for PNG-encoded images.
ICNS_TYPES = {
    16: b"icp4",
    32: b"icp5",
    64: b"icp6",
    128: b"ic07",
    256: b"ic08",
    512: b"ic09",
    1024: b"ic10",
}


def apply_icon_theme(svg: bytes) -> bytes:
    """Wrap the source icon so it renders as green on a black background.

    The original SVG file is not modified; this transformation happens on
    the bytes read into memory before rendering.
    """
    text = svg.decode("utf-8")
    match = re.search(r"<svg[^>]*>(.*)</svg>", text, re.DOTALL | re.IGNORECASE)
    inner = match.group(1) if match else text
    themed = (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 800 800">'
        f'<rect width="800" height="800" fill="{ICON_BACKGROUND}"/>'
        f'<g color="{ICON_GREEN}">{inner}</g>'
        "</svg>"
    )
    return themed.encode("utf-8")


def apply_tray_theme(svg: bytes) -> bytes:
    """Render the source icon as a monochrome black shape on a transparent background.

    macOS uses this as a template image so it adapts to the menu-bar theme.
    """
    text = svg.decode("utf-8")
    match = re.search(r"<svg[^>]*>(.*)</svg>", text, re.DOTALL | re.IGNORECASE)
    inner = match.group(1) if match else text
    themed = (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 800 800">'
        f'<g color="{TRAY_FOREGROUND}">{inner}</g>'
        "</svg>"
    )
    return themed.encode("utf-8")


def render_png(svg: bytes, size: int) -> bytes:
    return cairosvg.svg2png(bytestring=svg, output_width=size, output_height=size)


def write_ico(svg: bytes, sizes: list[int], out: Path) -> None:
    # Render each size as a PNG and assemble an ICO with PNG entries.
    pngs = [render_png(svg, size) for size in sizes]
    count = len(sizes)

    # ICO header: Reserved (2), Type (2), Count (2)
    header = (0).to_bytes(2, "little") + (1).to_bytes(2, "little") + count.to_bytes(2, "little")

    # ICONDIRENTRY is 16 bytes each. Data starts after header + entries.
    entry_size = 16
    data_offset = len(header) + count * entry_size
    entries = bytearray()
    data = bytearray()

    for size, png in zip(sizes, pngs):
        width = size if size < 256 else 0
        height = width
        entry = bytes([
            width, height,  # bWidth, bHeight
            0,              # bColorCount
            0,              # bReserved
        ])
        entry += (1).to_bytes(2, "little")       # wPlanes
        entry += (32).to_bytes(2, "little")      # wBitCount
        entry += len(png).to_bytes(4, "little")  # dwBytesInRes
        entry += data_offset.to_bytes(4, "little")  # dwImageOffset
        entries += entry
        data += png
        data_offset += len(png)

    out.write_bytes(header + entries + data)
    print(f"wrote {out}")


def write_icns(svg: bytes, sizes: list[int], out: Path) -> None:
    chunks = bytearray()
    for size in sizes:
        png = render_png(svg, size)
        type_code = ICNS_TYPES[size]
        chunk_len = 8 + len(png)
        chunks += type_code
        chunks += chunk_len.to_bytes(4, "big")
        chunks += png

    # ICNS file header: magic + total file length.
    file_len = 8 + len(chunks)
    out.write_bytes(b"icns" + file_len.to_bytes(4, "big") + chunks)
    print(f"wrote {out}")


def main() -> None:
    svg = SVG_PATH.read_bytes()
    themed = apply_icon_theme(svg)
    write_ico(themed, ICO_SIZES, ICO_PATH)
    write_icns(themed, ICNS_SIZES, ICNS_PATH)

    tray_svg = apply_tray_theme(svg)
    TRAY_PATH.write_bytes(render_png(tray_svg, TRAY_SIZE))
    print(f"wrote {TRAY_PATH}")


if __name__ == "__main__":
    main()
