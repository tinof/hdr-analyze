#!/usr/bin/env python3
"""Write the synthetic L1 regression clip as raw yuv420p10le frames on stdout.

Six shots of 24 frames, limited-range 10-bit codes. The samples are the same for PQ and HLG;
the encoder sets the transfer tag. Everything is integer arithmetic, so the output is identical
on every platform and Python version.

    python3 make_corpus.py | ffmpeg -f rawvideo -pix_fmt yuv420p10le -video_size 320x180 \\
        -framerate 24 -i - -c:v ffv1 ... clip.mkv

Shots:
  0  mid grey with grain (uniform noise of +-12 codes, new pattern per frame)
  1  saturated red block on a dim neutral background
  2  raised black with one small specular block
  3  horizontal luma ramp over the whole limited range
  4  fade from dark to bright
  5  dim scene with a one-frame flash
"""

import argparse
import sys
from array import array

SHOT_FRAMES = 24
SHOTS = 6
FRAME_COUNT = SHOT_FRAMES * SHOTS
NEUTRAL = 512


class Lcg:
    def __init__(self, seed):
        self.state = seed

    def next(self):
        self.state = (self.state * 1664525 + 1013904223) & 0xFFFFFFFF
        return self.state >> 16


def flat(count, code):
    return array("H", [code]) * count


def frame_planes(index, width, height, rng):
    cw, ch = width // 2, height // 2
    shot, pos = divmod(index, SHOT_FRAMES)
    cb = flat(cw * ch, NEUTRAL)
    cr = flat(cw * ch, NEUTRAL)

    if shot == 0:
        y = array("H", (400 + rng.next() % 25 - 12 for _ in range(width * height)))
    elif shot == 1:
        y = flat(width * height, 200)
        for row in range(height // 4, 3 * height // 4):
            start = row * width + width // 4
            y[start : start + width // 2] = flat(width // 2, 500)
        for row in range(ch // 4, 3 * ch // 4):
            start = row * cw + cw // 4
            cb[start : start + cw // 2] = flat(cw // 2, 400)
            cr[start : start + cw // 2] = flat(cw // 2, 800)
    elif shot == 2:
        y = flat(width * height, 120)
        for row in range(height // 2, height // 2 + 8):
            start = row * width + width // 2
            y[start : start + 8] = flat(8, 800)
    elif shot == 3:
        ramp = array("H", (64 + (940 - 64) * x // (width - 1) for x in range(width)))
        y = array("H")
        for _ in range(height):
            y.extend(ramp)
    elif shot == 4:
        y = flat(width * height, 100 + (700 - 100) * pos // (SHOT_FRAMES - 1))
    else:
        y = flat(width * height, 850 if pos == 12 else 250)
    return y, cb, cr


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--width", type=int, default=320)
    parser.add_argument("--height", type=int, default=180)
    args = parser.parse_args()
    if args.width % 2 or args.height % 2 or args.width < 32 or args.height < 32:
        parser.error("width and height must be even and at least 32")

    rng = Lcg(0x4C31)
    out = sys.stdout.buffer
    for index in range(FRAME_COUNT):
        for plane in frame_planes(index, args.width, args.height, rng):
            if sys.byteorder == "big":
                plane.byteswap()
            out.write(plane.tobytes())


if __name__ == "__main__":
    main()
