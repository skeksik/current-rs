"""Synthetic test media for the D2 Windows video spike.

Writes, into the output folder given as the first argument:
  sdr-1080p60.mkv, sdr-2160p60.mkv     HEVC Main 8-bit, BT.709
  hdr10-1080p60.mkv, hdr10-2160p60.mkv HEVC Main10, BT.2020 + PQ, HDR10 static metadata
Each file is 70 s at 60 fps with two audio tracks (stereo AAC 440 Hz tone, 5.1 AC-3 660 Hz beeps),
an ASS-styled track (demo.ass) and a PGS track written by this script (FFmpeg has no PGS encoder).

The HDR chart is PQ-coded directly: a 0-10000 nit ramp, grey patches at known nits, colour patches,
an SDR test pattern mapped to 203 nit white and a moving ~1000 nit box.

Needs ffmpeg with libx265 on PATH (a local tool only; nothing it produces is shipped).
Usage: python -I make_media.py <out-dir> [--only NAME]
"""

import os
import struct
import subprocess
import sys
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
FONT = "C\\:/Windows/Fonts/segoeui.ttf"
FONT_BOLD = "C\\:/Windows/Fonts/segoeuib.ttf"
DURATION = 70
FPS = 60


# ---------------------------------------------------------------- PQ (SMPTE ST 2084)
def pq(nits):
    m1, m2 = 2610 / 16384, 2523 / 4096 * 128
    c1, c2, c3 = 3424 / 4096, 2413 / 4096 * 32, 2392 / 4096 * 32
    y = (nits / 10000) ** m1
    return ((c1 + c2 * y) / (1 + c3 * y)) ** m2


GREY_PATCHES = [80, 100, 203, 400, 600, 1000, 2000, 4000]
COLOUR_PATCHES = [  # (label, (r, g, b) channel nits)
    ("R 203", (203, 0, 0)), ("G 203", (0, 203, 0)), ("B 203", (0, 0, 203)),
    ("R 1000", (1000, 0, 0)), ("G 1000", (0, 1000, 0)), ("B 1000", (0, 0, 1000)),
    ("W 203", (203, 203, 203)), ("W 1000", (1000, 1000, 1000)),
]


def write_chart_png(path, w=3840, h=2160):
    """16-bit RGB PNG whose code values are PQ (no transfer conversion is applied later)."""
    def row(pixels):
        out = bytearray(b"\x00")
        for r, g, b in pixels:
            out += struct.pack(">HHH", r, g, b)
        return bytes(out)

    def code(nits):
        return round(pq(nits) * 65535) if nits > 0 else 0

    black = row([(0, 0, 0)] * w)
    ramp = row([(v, v, v) for v in (round(x / (w - 1) * 65535) for x in range(w))])
    pw, gap = w // len(GREY_PATCHES), 24

    def patch_row(values):
        px = []
        for i in range(w):
            j, off = divmod(i, pw)
            px.append((0, 0, 0) if off < gap or j >= len(values) else values[j])
        return row(px)

    grey = patch_row([(code(n),) * 3 for n in GREY_PATCHES])
    colour = patch_row([tuple(code(c) for c in rgb) for _, rgb in COLOUR_PATCHES])
    rows = []
    for y in range(h):
        if y < 240:
            rows.append(ramp)
        elif 264 <= y < 744:
            rows.append(grey)
        elif 768 <= y < 1056:
            rows.append(colour)
        else:
            rows.append(black)
    raw = zlib.compress(b"".join(rows), 1)

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 16, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", raw))
        f.write(chunk(b"IEND", b""))


# ---------------------------------------------------------------- PGS (.sup) writer
PGS_EVENTS = [  # (start s, end s, text, fill rgb, top?)
    (2.0, 6.0, "PGS 1 - bottom centre, white", (255, 255, 255), False),
    (8.0, 12.0, "PGS 2 - yellow", (255, 215, 0), False),
    (14.0, 18.0, "PGS 3 - top of the frame", (255, 255, 255), True),
    (20.0, 24.0, "PGS 4 - after a gap", (255, 255, 255), False),
    (26.0, 30.0, "PGS 5 - yellow again", (255, 215, 0), False),
    (32.0, 44.0, "PGS 6 - long event (12 s)", (255, 255, 255), False),
    (46.0, 56.0, "PGS 7 - another long event", (255, 255, 255), False),
    (58.0, 69.0, "PGS 8 - last event", (255, 255, 255), False),
]


def render_mask(text, size, border, w=1800, h=160):
    """Grey 8-bit coverage mask of `text` rendered by ffmpeg's drawtext."""
    vf = (f"drawtext=fontfile='{FONT_BOLD}':text='{text}':fontsize={size}:fontcolor=white"
          f":borderw={border}:bordercolor=white:x=(w-text_w)/2:y=(h-text_h)/2")
    cmd = ["ffmpeg", "-v", "error", "-f", "lavfi", "-i", f"color=c=black:s={w}x{h}:d=1",
           "-vf", vf, "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "-"]
    data = subprocess.run(cmd, check=True, capture_output=True).stdout
    assert len(data) == w * h, (len(data), w * h)
    return data, w, h


def rgb_to_ycrcb(r, g, b):  # BT.709 limited range
    r, g, b = r / 255, g / 255, b / 255
    y = 0.2126 * r + 0.7152 * g + 0.0722 * b
    cb, cr = (b - y) / 1.8556, (r - y) / 1.5748
    return round(16 + 219 * y), round(128 + 224 * cr), round(128 + 224 * cb)


def rle_line(line):
    out, i, n = bytearray(), 0, len(line)
    while i < n:
        c, run = line[i], 1
        while i + run < n and line[i + run] == c and run < 16383:
            run += 1
        if c != 0 and run < 3:
            out += bytes([c]) * run
        elif c == 0:
            out += bytes([0, run]) if run < 64 else bytes([0, 0x40 | (run >> 8), run & 0xFF])
        else:
            out += bytes([0, 0x80 | run, c]) if run < 64 else bytes([0, 0xC0 | (run >> 8), run & 0xFF, c])
        i += run
    return bytes(out) + b"\x00\x00"


def segment(kind, pts, data):
    return b"PG" + struct.pack(">IIBH", round(pts * 90000), 0, kind, len(data)) + data


def write_sup(path, cw=1920, ch=1080):
    # An empty epoch at 0 s: ffmpeg shifts each input to start at 0, which would move every event early.
    out = bytearray(segment(0x16, 0.0, struct.pack(">HHBHBBBB", cw, ch, 0x10, 0, 0x80, 0, 0, 0)))
    out += segment(0x17, 0.0, b"\x00") + segment(0x80, 0.0, b"")
    comp = 1
    for start, end, text, fill, top in PGS_EVENTS:
        outer, w, h = render_mask(text, 60, 4)
        inner, _, _ = render_mask(text, 60, 0)
        xs = [i % w for i in range(w * h) if outer[i]]
        ys = [i // w for i in range(w * h) if outer[i]]
        x0, x1, y0, y1 = min(xs), max(xs) + 1, min(ys), max(ys) + 1
        ow, oh = x1 - x0, y1 - y0
        palette, index, rows = [(0, 128, 128, 0)], {}, []
        for y in range(y0, y1):
            line = bytearray()
            for x in range(x0, x1):
                a, f = outer[y * w + x], inner[y * w + x]
                aq = round(a / 17)
                if aq == 0:
                    line.append(0)
                    continue
                rq = round(min(f / a, 1.0) * 7)
                key = (aq, rq)
                if key not in index:
                    r = rq / 7
                    index[key] = len(palette)
                    palette.append((*rgb_to_ycrcb(*(round(c * r) for c in fill)), aq * 17))
                line.append(index[key])
            rows.append(rle_line(line))
        assert len(palette) <= 255
        rle = b"".join(rows)
        assert len(rle) + 11 < 65535, "object too large for one ODS segment"
        px, py = (cw - ow) // 2, (80 if top else ch - 70 - oh)
        window = struct.pack(">BBHHHH", 1, 0, px, py, ow, oh)
        pcs = struct.pack(">HHBHBBBB", cw, ch, 0x10, comp, 0x80, 0, 0, 1) + struct.pack(">HBBHH", 0, 0, 0, px, py)
        pds = struct.pack(">BB", 0, 0) + b"".join(struct.pack(">BBBBB", i, *e) for i, e in enumerate(palette))
        ods = struct.pack(">HBB", 0, 0, 0xC0) + (len(rle) + 4).to_bytes(3, "big") + struct.pack(">HH", ow, oh) + rle
        out += segment(0x16, start, pcs) + segment(0x17, start, window) + segment(0x14, start, pds)
        out += segment(0x15, start, ods) + segment(0x80, start, b"")
        comp += 1
        clear = struct.pack(">HHBHBBBB", cw, ch, 0x10, comp, 0x00, 0, 0, 0)
        out += segment(0x16, end, clear) + segment(0x17, end, window) + segment(0x80, end, b"")
        comp += 1
    with open(path, "wb") as f:
        f.write(out)


# ---------------------------------------------------------------- video
def labels_hdr():
    pw = 3840 // len(GREY_PATCHES)
    parts = []
    for i, n in enumerate(GREY_PATCHES):
        parts.append(f"drawtext=fontfile='{FONT_BOLD}':text='{n} nit':fontsize=56:fontcolor=black"
                     f":x={i * pw + 60}:y=660")
    for i, (label, _) in enumerate(COLOUR_PATCHES):
        parts.append(f"drawtext=fontfile='{FONT_BOLD}':text='{label}':fontsize=48:fontcolor=black@0.8"
                     f":x={i * pw + 60}:y=980")
    return parts


def timecode(colour):
    return (f"drawtext=fontfile='{FONT}':text='%{{pts\\:hms}}  frame %{{n}}':fontsize=72:fontcolor={colour}"
            f":x=1980:y=1900")


def build(out_dir, name, hdr, height, chart, ass, sup):
    path = os.path.join(out_dir, f"{name}.mkv")
    box_x = "1920+1440*(1-abs(1-mod(t\\,4)/2))"
    to_bt2020 = "scale=out_color_matrix=bt2020:out_range=tv,format=yuv420p10le"
    if hdr:
        video_inputs = ["-loop", "1", "-framerate", str(FPS), "-t", str(DURATION), "-i", chart,
                        "-f", "lavfi", "-i", f"testsrc2=s=1920x1080:r={FPS}:d={DURATION}",
                        "-f", "lavfi", "-i", f"color=c=0xC0C0C0:s=240x240:r={FPS}:d={DURATION}"]
        sdr_white = pq(203)
        graph = [
            f"[0:v]{to_bt2020}[bg]",
            f"[1:v]noise=alls=6:allf=t,format=rgb24,lutrgb=r=val*{sdr_white:.4f}:g=val*{sdr_white:.4f}"
            f":b=val*{sdr_white:.4f},{to_bt2020}[ts]",
            f"[2:v]{to_bt2020}[box]",
            "[bg][ts]overlay=x=0:y=1080:format=yuv420p10[a]",
            f"[a][box]overlay=x='{box_x}':y=1500:eval=frame:format=yuv420p10[b]",
            "[b]" + ",".join(labels_hdr() + [
                timecode("0x949494"),
                f"drawtext=fontfile='{FONT_BOLD}':text='HDR10 PQ BT.2020 - box about 1000 nit, text 203 nit'"
                ":fontsize=56:fontcolor=0x949494:x=1980:y=1120"])
            + (",scale=1920:1080:flags=bicubic" if height == 1080 else "") + ",format=yuv420p10le"
            ",setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc:range=tv[v]",
        ]
        x265 = ("hdr10=1:hdr10-opt=1:repeat-headers=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc"
                ":master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)"
                ":max-cll=1000,400")
        tags = ["-color_primaries", "bt2020", "-color_trc", "smpte2084", "-colorspace", "bt2020nc",
                "-color_range", "tv"]
    else:
        video_inputs = ["-f", "lavfi", "-i", f"testsrc2=s=3840x2160:r={FPS}:d={DURATION}",
                        "-f", "lavfi", "-i", f"color=c=white:s=240x240:r={FPS}:d={DURATION}"]
        graph = [
            "[0:v]noise=alls=6:allf=t,format=yuv420p[bg]",
            f"[bg][1:v]overlay=x='{box_x}':y=1500:eval=frame[b]",
            "[b]" + ",".join([timecode("white"),
                              f"drawtext=fontfile='{FONT_BOLD}':text='SDR BT.709 8-bit':fontsize=72"
                              ":fontcolor=white:box=1:boxcolor=black@0.6:x=60:y=60"])
            + (",scale=1920:1080:flags=bicubic" if height == 1080 else "")
            + ",scale=out_color_matrix=bt709:out_range=tv,format=yuv420p"
            ",setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv[v]",
        ]
        x265 = "colorprim=bt709:transfer=bt709:colormatrix=bt709"
        tags = ["-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709", "-color_range", "tv"]
    n_video = 3 if hdr else 2
    rate = ["-b:v", "40M", "-maxrate", "60M", "-bufsize", "80M"] if height == 2160 else \
        ["-b:v", "15M", "-maxrate", "25M", "-bufsize", "40M"]
    cmd = ["ffmpeg", "-y", "-hide_banner", "-v", "warning", "-stats", *video_inputs,
           "-f", "lavfi", "-i", f"sine=f=440:r=48000:d={DURATION}",
           "-f", "lavfi", "-i", f"aevalsrc='0.25*sin(2*PI*660*t)*lt(mod(t,1),0.5)':c=5.1:s=48000:d={DURATION}",
           "-i", ass, "-i", sup,
           "-filter_complex", ";".join(graph),
           "-map", "[v]", "-map", f"{n_video}:a", "-map", f"{n_video + 1}:a",
           "-map", f"{n_video + 2}:s", "-map", f"{n_video + 3}:s",
           "-c:v", "libx265", "-preset", "superfast", *rate,
           "-x265-params", f"{x265}:keyint=120:min-keyint=120:log-level=warning", *tags,
           "-c:a:0", "aac", "-b:a:0", "192k", "-ac:a:0", "2",
           "-c:a:1", "ac3", "-b:a:1", "448k",
           "-c:s", "copy",
           "-metadata", f"title=D2 {name}",
           "-metadata:s:a:0", "language=eng", "-metadata:s:a:0", "title=Tone 440 Hz stereo AAC",
           "-metadata:s:a:1", "language=fra", "-metadata:s:a:1", "title=Beeps 660 Hz 5.1 AC-3",
           "-metadata:s:s:0", "language=eng", "-metadata:s:s:0", "title=Styled (ASS)",
           "-metadata:s:s:1", "language=eng", "-metadata:s:s:1", "title=Bitmap (PGS)",
           "-disposition:a:0", "default", "-disposition:a:1", "0",
           "-disposition:s:0", "0", "-disposition:s:1", "0",
           "-t", str(DURATION), path]
    print(">>", name, flush=True)
    subprocess.run(cmd, check=True)


def main():
    out_dir = sys.argv[1]
    only = sys.argv[sys.argv.index("--only") + 1] if "--only" in sys.argv else None
    os.makedirs(out_dir, exist_ok=True)
    chart = os.path.join(out_dir, "hdr-chart-pq16.png")
    sup = os.path.join(out_dir, "demo.sup")
    ass = os.path.join(HERE, "demo.ass")
    if not os.path.exists(chart):
        write_chart_png(chart)
    write_sup(sup)
    for name, hdr, height in [("sdr-1080p60", False, 1080), ("hdr10-1080p60", True, 1080),
                              ("sdr-2160p60", False, 2160), ("hdr10-2160p60", True, 2160)]:
        if only is None or only == name:
            build(out_dir, name, hdr, height, chart, ass, sup)


if __name__ == "__main__":
    main()
