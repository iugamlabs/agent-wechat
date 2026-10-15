#!/usr/bin/env python3
"""
Convert WeChat proprietary media formats to standard formats.

Modes:
  wxgf2img  — WXGF (HEVC in custom container) → JPEG or GIF
  silk2mp3  — SILK_V3 voice audio → MP3

Usage:
  media-convert wxgf2img < input.wxgf > output.jpg
  media-convert silk2mp3 < input.silk > output.mp3

Reads binary from stdin, writes converted binary to stdout.
Format hint printed to stderr as "FORMAT:<ext>".
"""

import sys
import os
import struct
import subprocess
import tempfile
import time

MIN_RATIO = 0.6


MAX_JPEG_TRAILING_BYTES = 4096
MAX_XOR_CANDIDATES = 32
XOR_RECOVERY_SECONDS = 3.0


def normalize_jpeg(data):
    """Find the structural EOI, skipping length-delimited metadata and scan stuffing.

    Never search for the last FF D9: APP payloads and trailing bytes can contain it.
    Pixel completeness is checked separately by the strict runtime decoder.
    """
    if not data.startswith(b'\xff\xd8'):
        raise ValueError('Invalid JPEG header')
    pos, in_scan, saw_frame, saw_scan = 2, False, False, False
    while pos < len(data):
        if in_scan:
            pos = data.find(b'\xff', pos)
            if pos < 0:
                break
        elif data[pos] != 0xff:
            raise ValueError('Invalid JPEG marker')
        while pos < len(data) and data[pos] == 0xff:
            pos += 1
        if pos >= len(data):
            break
        marker = data[pos]
        pos += 1
        if in_scan and (marker == 0 or 0xd0 <= marker <= 0xd7):
            continue
        was_in_scan = in_scan
        in_scan = False
        if marker == 0xd9:
            if not saw_frame or not saw_scan:
                raise ValueError('JPEG has no image scan')
            if len(data) - pos > MAX_JPEG_TRAILING_BYTES:
                raise ValueError('JPEG trailing data exceeds limit')
            return data[:pos]
        # Only length-delimited JPEG markers are accepted outside entropy data.
        if not (0xc0 <= marker <= 0xcf or marker in (0xda, 0xdb, 0xdc, 0xdd, 0xde, 0xdf)
                or 0xe0 <= marker <= 0xef or marker == 0xfe):
            raise ValueError('Invalid JPEG segment')
        if pos + 2 > len(data):
            break
        length = struct.unpack('>H', data[pos:pos + 2])[0]
        if length < 2 or pos + length > len(data):
            raise ValueError('Incomplete JPEG segment')
        if marker in (0xc0, 0xc1, 0xc2, 0xc3, 0xc5, 0xc6, 0xc7,
                      0xc9, 0xca, 0xcb, 0xcd, 0xce, 0xcf):
            saw_frame = True
        if marker == 0xda:
            if not saw_frame:
                raise ValueError('JPEG scan before frame')
            saw_scan, in_scan = True, True
        elif marker == 0xdc and was_in_scan:
            in_scan = True  # DNL can occur inside an entropy-coded scan.
        pos += length
    raise ValueError('Incomplete JPEG')


def normalized_image(data):
    if data.startswith(b'\x89PNG\r\n\x1a\n'):
        complete = data.endswith(b'\x00\x00\x00\x00IEND\xaeB`\x82')
        fmt = 'png'
    elif data.startswith(b'\xff\xd8'):
        return normalize_jpeg(data), 'jpeg'
    elif data.startswith((b'GIF87a', b'GIF89a')):
        complete, fmt = data.endswith(b';'), 'gif'
    elif data[:4] == b'RIFF' and data[8:12] == b'WEBP':
        complete = len(data) >= 12 and struct.unpack('<I', data[4:8])[0] + 8 == len(data)
        fmt = 'webp'
    else:
        complete, fmt = False, 'unknown'
    if not complete:
        raise ValueError('Incomplete image')
    return data, fmt


def decode_image(data, timeout=10):
    result = subprocess.run(
        ['ffmpeg', '-hide_banner', '-loglevel', 'error', '-xerror',
         '-err_detect', 'explode', '-i', 'pipe:0', '-f', 'null', '-'],
        input=data, capture_output=True, timeout=timeout,
    )
    # -xerror makes decode failures fatal; recoverable metadata warnings are allowed.
    if result.returncode:
        raise ValueError('Image validation failed')


def validate_image(data):
    normalized, _ = normalized_image(data)
    decode_image(normalized)
    return b'ok', 'validation'


def normalize_image(data):
    normalized, fmt = normalized_image(data)
    decode_image(normalized)
    return normalized, fmt


def jpeg_xor_candidates(data, xor_size):
    """Candidate keys from EOI pairs within a bounded XOR-encrypted suffix."""
    if not 2 <= xor_size <= len(data):
        raise ValueError('Invalid XOR suffix size')
    start = max(len(data) - xor_size, len(data) - MAX_JPEG_TRAILING_BYTES - 2)
    candidates = []
    for pos in range(len(data) - 2, start - 1, -1):
        key = data[pos] ^ 0xff
        if data[pos + 1] ^ key == 0xd9 and key not in candidates:
            candidates.append(key)
            if len(candidates) > MAX_XOR_CANDIDATES:
                raise ValueError('Too many JPEG XOR candidates')
    return candidates


def recover_jpeg_xor(payload):
    """Input: LE u32 XOR size, LE u32 budget ms, AES-decoded image with XOR=0.

    Return one byte only. No keys, image bytes or cache paths are logged or saved.
    The legacy last-pair key gets priority only after structure and full decoding.
    """
    if len(payload) < 8:
        raise ValueError('Invalid XOR recovery input')
    xor_size, budget_ms = struct.unpack('<II', payload[:8])
    data = payload[8:]
    deadline = time.monotonic() + min(budget_ms / 1000, XOR_RECOVERY_SECONDS)
    if not 2 <= xor_size <= len(data):
        raise ValueError('Invalid XOR suffix size')

    def validates(key):
        candidate = data[:-xor_size] + bytes(b ^ key for b in data[-xor_size:])
        normalized = normalize_jpeg(candidate)
        if len(normalized) - 2 < len(data) - xor_size:
            raise ValueError('JPEG EOI does not establish the XOR key')
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise subprocess.TimeoutExpired('JPEG XOR recovery', 0)
        decode_image(normalized, timeout=remaining)
        return len(normalized)

    fast_key = data[-2] ^ 0xff
    if data[-1] ^ fast_key == 0xd9:
        try:
            if validates(fast_key) == len(data):
                return bytes([fast_key]), 'xor'
        except ValueError:
            pass
    candidates = jpeg_xor_candidates(data, xor_size)
    valid = []
    for key in candidates:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise ValueError('JPEG XOR recovery timed out')
        try:
            validates(key)
        except ValueError:
            continue
        valid.append(key)
        if len(valid) > 1:
            raise ValueError('Ambiguous JPEG XOR candidates')
    if len(valid) != 1:
        raise ValueError('No validated JPEG XOR candidate')
    return bytes(valid), 'xor'

# ============================================
# WXGF → Image
# ============================================

def find_data_partitions(data):
    """Find HEVC NALU partitions in WXGF data.

    Port of chatlog's wxgf.go findDataPartition().
    Scans for HEVC start codes (0x00000001 or 0x000001) with 4-byte BE
    length prefix before each start code.
    """
    if len(data) < 15 or data[0:4] != b"wxgf":
        raise ValueError("Invalid WXGF data")

    header_len = data[4]
    if header_len >= len(data):
        raise ValueError("Invalid WXGF header length")

    patterns = [b"\x00\x00\x00\x01", b"\x00\x00\x01"]

    for pattern in patterns:
        partitions = []
        offset = 0

        while header_len + offset < len(data):
            idx = data.find(pattern, header_len + offset)
            if idx == -1:
                break

            if idx < 4:
                offset = idx - header_len + 1
                continue

            length = (data[idx - 4] << 24 | data[idx - 3] << 16 |
                      data[idx - 2] << 8 | data[idx - 1])

            if length <= 0 or idx + length > len(data):
                offset = idx - header_len + 1
                continue

            ratio = length / len(data)
            partitions.append({
                "offset": idx,
                "size": length,
                "ratio": ratio,
            })
            offset = idx - header_len + length

        if partitions:
            max_idx = max(range(len(partitions)), key=lambda i: partitions[i]["ratio"])
            max_ratio = partitions[max_idx]["ratio"]
            return partitions, max_idx, max_ratio

    raise ValueError("No HEVC partitions found in WXGF data")


def like_anime(partitions, max_ratio):
    """Detect if WXGF contains animation (alternating anime/mask frames)."""
    return len(partitions) > 1 and max_ratio < MIN_RATIO


def convert_wxgf_static(hevc_data):
    """Convert a single HEVC frame to JPEG via ffmpeg."""
    proc = subprocess.run(
        ["ffmpeg", "-hide_banner", "-loglevel", "error",
         "-i", "-",
         "-vframes", "1",
         "-c:v", "mjpeg",
         "-q:v", "4",
         "-f", "image2",
         "-"],
        input=hevc_data,
        capture_output=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"ffmpeg failed: {proc.stderr.decode()}")
    if not proc.stdout:
        raise RuntimeError("ffmpeg produced no output")
    return proc.stdout, "jpeg"


def convert_wxgf_animated(data, partitions):
    """Convert alternating anime/mask HEVC frames to GIF via ffmpeg."""
    anime_frames = []
    mask_frames = []
    for i, p in enumerate(partitions):
        chunk = data[p["offset"]:p["offset"] + p["size"]]
        if i % 2 == 0:
            mask_frames.append(chunk)
        else:
            anime_frames.append(chunk)

    # Write to temp files (ffmpeg needs seekable input for multi-input)
    with tempfile.NamedTemporaryFile(suffix=".hevc", delete=False) as af:
        for frame in anime_frames:
            af.write(frame)
        anime_path = af.name

    with tempfile.NamedTemporaryFile(suffix=".hevc", delete=False) as mf:
        for frame in mask_frames:
            mf.write(frame)
        mask_path = mf.name

    try:
        proc = subprocess.run(
            ["ffmpeg", "-hide_banner", "-loglevel", "error",
             "-i", anime_path,
             "-i", mask_path,
             "-filter_complex",
             "[0:v][1:v]alphamerge,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse",
             "-f", "gif",
             "-"],
            capture_output=True,
        )
        if proc.returncode != 0:
            raise RuntimeError(f"ffmpeg animated failed: {proc.stderr.decode()}")
        if not proc.stdout:
            raise RuntimeError("ffmpeg animated produced no output")
        return proc.stdout, "gif"
    finally:
        os.unlink(anime_path)
        os.unlink(mask_path)


def wxgf2img(data):
    """Convert WXGF data to JPEG or GIF."""
    partitions, max_idx, max_ratio = find_data_partitions(data)

    if like_anime(partitions, max_ratio):
        return convert_wxgf_animated(data, partitions)
    else:
        p = partitions[max_idx]
        hevc_data = data[p["offset"]:p["offset"] + p["size"]]
        return convert_wxgf_static(hevc_data)


# ============================================
# SILK → MP3
# ============================================

def silk2mp3(data):
    """Convert SILK_V3 voice data to MP3.

    WeChat SILK variant: prepends 0x02 before standard #!SILK_V3 header.
    Uses pysilk (silk-python) for SILK→PCM, then ffmpeg for PCM→MP3.
    """
    import io
    import pysilk

    # Strip WeChat's 0x02 prefix if present (non-standard SILK header)
    if data and data[0:1] == b"\x02":
        data = data[1:]

    silk_io = io.BytesIO(data)
    pcm_io = io.BytesIO()

    # Decode SILK → PCM (24kHz mono s16le)
    pysilk.decode(silk_io, pcm_io, 24000)
    pcm_data = pcm_io.getvalue()

    proc = subprocess.run(
        ["ffmpeg", "-hide_banner", "-loglevel", "error",
         "-f", "s16le",
         "-ar", "24000",
         "-ac", "1",
         "-i", "-",
         "-c:a", "libmp3lame",
         "-q:a", "2",
         "-f", "mp3",
         "-"],
        input=pcm_data,
        capture_output=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"ffmpeg failed: {proc.stderr.decode()}")
    if not proc.stdout:
        raise RuntimeError("ffmpeg produced no output")
    return proc.stdout, "mp3"


# ============================================
# Main
# ============================================

def main():
    if len(sys.argv) < 2:
        print("Usage: media-convert <wxgf2img|silk2mp3|validate-image|normalize-image|recover-jpeg-xor>", file=sys.stderr)
        sys.exit(1)

    mode = sys.argv[1]
    data = sys.stdin.buffer.read()

    if not data:
        print("Error: no input data", file=sys.stderr)
        sys.exit(1)

    try:
        if mode == "validate-image":
            result, fmt = validate_image(data)
        elif mode == "normalize-image":
            result, fmt = normalize_image(data)
        elif mode == "recover-jpeg-xor":
            result, fmt = recover_jpeg_xor(data)
        elif mode == "wxgf2img":
            result, fmt = wxgf2img(data)
        elif mode == "silk2mp3":
            result, fmt = silk2mp3(data)
        else:
            print(f"Unknown mode: {mode}", file=sys.stderr)
            sys.exit(1)

        print(f"FORMAT:{fmt}", file=sys.stderr)
        sys.stdout.buffer.write(result)

    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
