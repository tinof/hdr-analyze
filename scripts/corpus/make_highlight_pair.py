#!/usr/bin/env python3
"""Write synthetic neutral highlights of a known code into a clean/grainy pair of raw streams.

Inputs and outputs are raw yuv420p10le frames (files or FIFOs) of the same size and frame count:
the clean and the grainy version of one cut, aligned frame by frame (ROADMAP WS8.1). The same
highlights go into both outputs:

  clean   the highlight is flat: Y code 820 on neutral chroma (PQ 0.86301, 12-bit 3534.0).
  grainy  the highlight carries the cut's own grain: 820 + (grainy - clean) at each highlight
          pixel, less the mean of (grainy - clean) over the highlight and 8 pixels around it
          (the two versions can come from different masters, which shifts their level).

The chroma samples of every 4:2:0 quad a highlight touches are set to neutral (512) in both
outputs, so a highlight pixel's R'G'B' is its luma. A grainy highlight value outside the
limited range [64, 940] stops the run: the generator never clips.

Segments (frame ranges inclusive; offsets from --x0/--y0, which must be even):
  none         0-29     no highlight
  2x2          30-59    2x2, on the 2x2 block grid
  2x2_odd_x    60-89    2x2, one column right: two half-filled blocks
  2x2_odd_y    90-119   2x2, one row down: two half-filled blocks
  2x2_odd_xy   120-149  2x2, one column right and one row down: four quarter-filled blocks
  8x8          150-174  8x8, on the grid
  8x8_odd_xy   175-189  8x8, one column right and one row down
  32x32        190-209  32x32, on the grid
  flash1       220      8x8 for one frame, one column right and one row down
  flash3       235-237  2x2 for three frames, one column right and one row down
  moving2x2    250-299  2x2, moving one pixel right per frame (alternating grid phase)

--truth writes, per highlight frame, the patch geometry and for each output the pixel peak and
the SMPTE ST 2094-10 reduced-set value (§6.1.2: 2x2 block means in linear light, max-RGB, back
to PQ) as 12-bit PQ codes, and the patch's luma range, computed from the written samples. The block grid is anchored at the
picture's top-left, which is the analyzer's grid under --no-crop. The target for an estimator is
the clean value; the grainy values are what the grain produced. --segments prints the segment
table in the shape a corpus manifest's "highlight" key holds.

Pure Python with integer sample arithmetic, so the outputs are identical on every platform.

    make_highlight_pair.py --clean c.yuv --grainy g.yuv --out-clean hc.yuv --out-grainy hg.yuv \\
        --truth truth.json
"""

import argparse
import hashlib
import json
import os
import sys
from array import array

Y_CODE = 820
NEUTRAL = 512
Y_MIN, Y_MAX = 64, 940
PAD = 8
SEGMENTS = {
    # name: (first, last, size, dx, dy, step); x = x0 + dx + step * (frame - first)
    "none": (0, 29, 0, 0, 0, 0),
    "2x2": (30, 59, 2, 0, 0, 0),
    "2x2_odd_x": (60, 89, 2, 1, 0, 0),
    "2x2_odd_y": (90, 119, 2, 0, 1, 0),
    "2x2_odd_xy": (120, 149, 2, 1, 1, 0),
    "8x8": (150, 174, 8, 0, 0, 0),
    "8x8_odd_xy": (175, 189, 8, 1, 1, 0),
    "32x32": (190, 209, 32, 0, 0, 0),
    "flash1": (220, 220, 8, 1, 1, 0),
    "flash3": (235, 237, 2, 1, 1, 0),
    "moving2x2": (250, 299, 2, 0, 0, 1),
}

M1 = 2610 / 16384
M2 = 2523 / 4096 * 128
C1 = 3424 / 4096
C2 = 2413 / 4096 * 32
C3 = 2392 / 4096 * 32


def eotf(pq):
    """ST 2084: normalized PQ to linear light / 10000 nits."""
    p = pq ** (1 / M2)
    return (max(p - C1, 0.0) / (C2 - C3 * p)) ** (1 / M1)


def inverse_eotf(linear):
    l = max(linear, 0.0) ** M1
    return ((C1 + C2 * l) / (1 + C3 * l)) ** M2


def pq_of(y_code):
    """Normalized PQ of a limited-range luma code on neutral chroma, clamped to [0, 1]."""
    return min(max((y_code - 64) / 876, 0.0), 1.0)


def expected_peak_pq12():
    return (Y_CODE - 64) / 876 * 4095


def manifest_segments():
    return {
        "expected_peak_pq12": expected_peak_pq12(),
        "segments": {name: [s[0], s[1]] for name, s in SEGMENTS.items()},
    }


def patch(index, x0, y0):
    """(segment, x, y, size) of the highlight on frame `index`, or None."""
    for name, (first, last, size, dx, dy, step) in SEGMENTS.items():
        if size and first <= index <= last:
            return name, x0 + dx + step * (index - first), y0 + dy, size
    return None


def read_frame(stream, frame_bytes):
    data = stream.read(frame_bytes)
    if not data:
        return None
    if len(data) != frame_bytes:
        raise SystemExit(f"error: a truncated frame in {stream.name}")
    samples = array("H")
    samples.frombytes(data)
    if sys.byteorder == "big":
        samples.byteswap()
    return samples


def write_frame(stream, samples, digest):
    if sys.byteorder == "big":
        samples = array("H", samples)
        samples.byteswap()
    data = samples.tobytes()
    digest.update(data)
    stream.write(data)


def measure(luma, width, x, y, size):
    """Pixel peak and ST 2094-10 reduced-set value of the blocks the patch touches (12-bit),
    and the spread of the patch's luma codes (0 on a flat highlight)."""
    codes = [luma[row * width + col] for row in range(y, y + size) for col in range(x, x + size)]
    best = 0.0
    for by in range(y // 2, (y + size - 1) // 2 + 1):
        for bx in range(x // 2, (x + size - 1) // 2 + 1):
            linear = [
                eotf(pq_of(luma[row * width + col]))
                for row in (2 * by, 2 * by + 1)
                for col in (2 * bx, 2 * bx + 1)
            ]
            best = max(best, inverse_eotf(sum(linear) / 4))
    return {
        "pixel_pq12": pq_of(max(codes)) * 4095,
        "st2094_pq12": best * 4095,
        "luma_range": max(codes) - min(codes),
    }


def paint(index, clean, grainy, width, height, found):
    """Write the highlight into both frames in place; return its truth record."""
    name, x, y, size = found
    left, top = max(x - PAD, 0), max(y - PAD, 0)
    right, bottom = min(x + size + PAD, width), min(y + size + PAD, height)
    total = count = 0
    for row in range(top, bottom):
        for col in range(left, right):
            total += grainy[row * width + col] - clean[row * width + col]
            count += 1
    local_mean = (2 * total + count) // (2 * count)  # rounded, the same on every platform

    for row in range(y, y + size):
        for col in range(x, x + size):
            i = row * width + col
            value = Y_CODE + grainy[i] - clean[i] - local_mean
            if not Y_MIN <= value <= Y_MAX:
                raise SystemExit(
                    f"error: frame {index}: the grainy highlight at ({col}, {row}) would be "
                    f"{value}, outside [{Y_MIN}, {Y_MAX}]; move it (--x0/--y0)"
                )
            grainy[i] = value
            clean[i] = Y_CODE

    luma_size = width * height
    chroma_width = width // 2
    chroma_size = luma_size // 4
    for plane in (clean, grainy):
        for cy in range(y // 2, (y + size - 1) // 2 + 1):
            for cx in range(x // 2, (x + size - 1) // 2 + 1):
                plane[luma_size + cy * chroma_width + cx] = NEUTRAL
                plane[luma_size + chroma_size + cy * chroma_width + cx] = NEUTRAL

    grainy_truth = measure(grainy, width, x, y, size)
    grainy_truth["local_mean"] = local_mean
    return {
        "frame": index,
        "segment": name,
        "x": x,
        "y": y,
        "size": size,
        "clean": measure(clean, width, x, y, size),
        "grainy": grainy_truth,
    }


def same_file(a, b):
    """True when the two paths name one file, through links too."""
    try:
        return os.path.samefile(a, b)
    except OSError:  # a path that does not exist yet
        return os.path.realpath(a) == os.path.realpath(b)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--clean", help="clean input stream")
    parser.add_argument("--grainy", help="grainy input stream, aligned with --clean")
    parser.add_argument("--out-clean", help="clean output stream")
    parser.add_argument("--out-grainy", help="grainy output stream")
    parser.add_argument("--truth", help="truth JSON to write")
    parser.add_argument("--width", type=int, default=3840)
    parser.add_argument("--height", type=int, default=2160)
    parser.add_argument("--x0", type=int, default=1000)
    parser.add_argument("--y0", type=int, default=600)
    parser.add_argument("--segments", action="store_true", help="print the segment table")
    args = parser.parse_args()
    if args.segments:
        print(json.dumps(manifest_segments()))
        return
    for option in ("clean", "grainy", "out_clean", "out_grainy", "truth"):
        if getattr(args, option) is None:
            parser.error(f"--{option.replace('_', '-')} is required")
    if args.width % 2 or args.height % 2 or args.x0 % 2 or args.y0 % 2:
        parser.error("width, height, x0 and y0 must be even")
    if args.x0 < 0 or args.y0 < 0:
        parser.error("x0 and y0 must not be negative")
    reach_x = max(s[3] + s[5] * (s[1] - s[0]) + s[2] for s in SEGMENTS.values())
    reach_y = max(s[4] + s[2] for s in SEGMENTS.values())
    if args.x0 + reach_x + PAD > args.width or args.y0 + reach_y + PAD > args.height:
        parser.error(f"the highlights need {reach_x}x{reach_y} pixels from (x0, y0) plus {PAD}")

    paths = {f"--{o.replace('_', '-')}": getattr(args, o) for o in
             ("clean", "grainy", "out_clean", "out_grainy", "truth")}
    names = list(paths)
    for i, output in enumerate(names[2:], 2):
        for other in names[:i]:
            if same_file(paths[output], paths[other]):
                parser.error(f"{output} names the same file as {other}")

    # The outputs may be FIFOs, so they cannot be staged: withdraw an old truth file before
    # they change, so a failed run never leaves truth that describes other outputs.
    if os.path.lexists(args.truth):
        os.remove(args.truth)

    frame_bytes = args.width * args.height * 3
    digests = {"clean": hashlib.sha256(), "grainy": hashlib.sha256()}
    frames = []
    index = 0
    with open(args.clean, "rb") as clean_in, open(args.grainy, "rb") as grainy_in, open(
        args.out_clean, "wb"
    ) as clean_out, open(args.out_grainy, "wb") as grainy_out:
        while True:
            clean = read_frame(clean_in, frame_bytes)
            grainy = read_frame(grainy_in, frame_bytes)
            if clean is None or grainy is None:
                if clean is not grainy:
                    raise SystemExit(f"error: the inputs differ in length (after {index} frames)")
                break
            found = patch(index, args.x0, args.y0)
            if found:
                frames.append(paint(index, clean, grainy, args.width, args.height, found))
            write_frame(clean_out, clean, digests["clean"])
            write_frame(grainy_out, grainy, digests["grainy"])
            index += 1

    needed = max(s[1] for s in SEGMENTS.values()) + 1
    if index < needed:
        raise SystemExit(f"error: the inputs hold {index} frames; the segments need {needed}")

    truth = {
        "generator": "scripts/corpus/make_highlight_pair.py",
        "version": 1,
        "width": args.width,
        "height": args.height,
        "frames_written": index,
        "y_code": Y_CODE,
        "grid_anchor": [0, 0],
        "grain": f"grainy - clean, less its mean over the highlight +-{PAD} px",
        "raw_sha256": {name: digest.hexdigest() for name, digest in digests.items()},
        **manifest_segments(),
        "frames": frames,
    }
    with open(args.truth, "w") as out:
        json.dump(truth, out, indent=1)
        out.write("\n")


if __name__ == "__main__":
    main()
