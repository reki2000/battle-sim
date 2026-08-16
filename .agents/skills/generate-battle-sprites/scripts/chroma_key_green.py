#!/usr/bin/env python3
"""Replace the exact battle-sim chroma key #00ff00 with transparent alpha."""

from __future__ import annotations

import argparse
from collections import deque
import struct
import zlib
from pathlib import Path


PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
KEY_RGB = (0, 255, 0)


def read_png(path: Path) -> tuple[int, int, bytearray]:
    data = path.read_bytes()
    if data[:8] != PNG_SIGNATURE:
        raise ValueError(f"{path}: not a PNG")
    position = 8
    idat = bytearray()
    width = height = color_type = None
    while position < len(data):
        length = struct.unpack(">I", data[position:position + 4])[0]
        chunk_type = data[position + 4:position + 8]
        payload = data[position + 8:position + 8 + length]
        position += 12 + length
        if chunk_type == b"IHDR":
            width, height, depth, color_type, compression, filtering, interlace = struct.unpack(
                ">IIBBBBB", payload
            )
            if depth != 8 or color_type not in (2, 6) or compression or filtering or interlace:
                raise ValueError(f"{path}: require non-interlaced 8-bit RGB/RGBA PNG")
        elif chunk_type == b"IDAT":
            idat.extend(payload)
        elif chunk_type == b"IEND":
            break
    if width is None or height is None or color_type is None:
        raise ValueError(f"{path}: missing IHDR")

    channels = 4 if color_type == 6 else 3
    stride = width * channels
    raw = zlib.decompress(idat)
    pixels = bytearray(width * height * 4)
    previous = bytearray(stride)
    position = 0
    for y in range(height):
        filter_type = raw[position]
        position += 1
        row = bytearray(raw[position:position + stride])
        position += stride
        for x in range(stride):
            left = row[x - channels] if x >= channels else 0
            up = previous[x]
            up_left = previous[x - channels] if x >= channels else 0
            if filter_type == 1:
                row[x] = (row[x] + left) & 255
            elif filter_type == 2:
                row[x] = (row[x] + up) & 255
            elif filter_type == 3:
                row[x] = (row[x] + ((left + up) // 2)) & 255
            elif filter_type == 4:
                estimate = left + up - up_left
                pa = abs(estimate - left)
                pb = abs(estimate - up)
                pc = abs(estimate - up_left)
                predictor = left if pa <= pb and pa <= pc else (up if pb <= pc else up_left)
                row[x] = (row[x] + predictor) & 255
            elif filter_type != 0:
                raise ValueError(f"{path}: unsupported PNG filter {filter_type}")
        for x in range(width):
            source = x * channels
            target = (y * width + x) * 4
            pixels[target:target + 3] = row[source:source + 3]
            pixels[target + 3] = row[source + 3] if channels == 4 else 255
        previous = row
    return width, height, pixels


def write_png(path: Path, width: int, height: int, pixels: bytearray) -> None:
    def chunk(kind: bytes, payload: bytes) -> bytes:
        crc = zlib.crc32(kind + payload) & 0xFFFFFFFF
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", crc)

    rows = b"".join(
        b"\x00" + pixels[y * width * 4:(y + 1) * width * 4]
        for y in range(height)
    )
    output = PNG_SIGNATURE
    output += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
    output += chunk(b"IDAT", zlib.compress(rows, 9))
    output += chunk(b"IEND", b"")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(output)


def chroma_key(input_path: Path, output_path: Path) -> int:
    width, height, pixels = read_png(input_path)
    # ImageGen can introduce small RGB deviations into an otherwise green field.
    # Normalize border-connected green-dominant pixels first, then remove any
    # remaining strongly green pixels. The subject contract forbids green on the
    # character, so this also catches background pockets enclosed by limbs or gear.
    def greenish(x: int, y: int) -> bool:
        offset = (y * width + x) * 4
        red, green, blue = pixels[offset:offset + 3]
        return green >= 120 and green >= red + 25 and green >= blue + 25

    background = bytearray(width * height)
    queue: deque[tuple[int, int]] = deque()
    for x in range(width):
        for y in (0, height - 1):
            if greenish(x, y) and not background[y * width + x]:
                background[y * width + x] = 1
                queue.append((x, y))
    for y in range(height):
        for x in (0, width - 1):
            if greenish(x, y) and not background[y * width + x]:
                background[y * width + x] = 1
                queue.append((x, y))
    while queue:
        x, y = queue.popleft()
        for next_x, next_y in ((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)):
            if (
                0 <= next_x < width
                and 0 <= next_y < height
                and not background[next_y * width + next_x]
                and greenish(next_x, next_y)
            ):
                background[next_y * width + next_x] = 1
                queue.append((next_x, next_y))

    normalized = 0
    for index, is_background in enumerate(background):
        if is_background:
            offset = index * 4
            pixels[offset:offset + 3] = bytes(KEY_RGB)
            normalized += 1

    for y in range(height):
        for x in range(width):
            if greenish(x, y):
                offset = (y * width + x) * 4
                pixels[offset:offset + 3] = bytes(KEY_RGB)
                normalized += 1

    replaced = 0
    for offset in range(0, len(pixels), 4):
        rgb = tuple(pixels[offset:offset + 3])
        if rgb == KEY_RGB:
            pixels[offset:offset + 4] = b"\x00\x00\x00\x00"
            replaced += 1
        elif pixels[offset + 3] == 0:
            pixels[offset:offset + 3] = b"\x00\x00\x00"
    if replaced == 0:
        raise ValueError(f"{input_path}: no exact #00ff00 pixels found; refusing a no-op key pass")
    write_png(output_path, width, height, pixels)
    return replaced + normalized


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    replaced = chroma_key(args.input, args.output)
    print({"input": str(args.input), "output": str(args.output), "key": "#00ff00", "replacedPixels": replaced})


if __name__ == "__main__":
    main()
