#!/usr/bin/env python3
"""Write the synthetic L1 regression clip as raw yuv420p10le frames on stdout.

Six shots of different lengths, none a multiple of 24 (a detector that cuts on a fixed period
cannot pass), limited-range 10-bit codes. The samples are the same for PQ and HLG;
the encoder sets the transfer tag. Everything is integer arithmetic, so the output is identical
on every platform and Python version.

    python3 make_corpus.py | ffmpeg -f rawvideo -pix_fmt yuv420p10le -video_size 320x180 \\
        -framerate 24 -i - -c:v ffv1 ... clip.mkv

Shots (frames):
  0  mid grey with grain (uniform noise of +-12 codes, new pattern per frame)   61
  1  saturated red block on a dim neutral background                            29
  2  raised black with one small specular block                                 31
  3  horizontal luma ramp over the whole limited range                          43
  4  the ramp picture fading in from 30% to full level                          41
  5  dim scene with a one-frame flash in the middle                             37

`--scenes` prints the first frame of every shot instead: the scene cuts by construction.
"""

import argparse
import sys
from array import array

SHOT_FRAMES = (61, 29, 31, 43, 41, 37)
SHOT_STARTS = tuple(sum(SHOT_FRAMES[:shot]) for shot in range(len(SHOT_FRAMES)))
FRAME_COUNT = sum(SHOT_FRAMES)
FLASH_FRAME = 18
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
    shot = max(s for s, start in enumerate(SHOT_STARTS) if start <= index)
    pos = index - SHOT_STARTS[shot]
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
        # Mirrored ramp, so the first frame also differs from the last frame of shot 3.
        gain = 300 + (1000 - 300) * pos // (SHOT_FRAMES[4] - 1)
        ramp = array(
            "H",
            (64 + (940 - 64) * (width - 1 - x) // (width - 1) * gain // 1000 for x in range(width)),
        )
        y = array("H")
        for _ in range(height):
            y.extend(ramp)
    else:
        y = flat(width * height, 850 if pos == FLASH_FRAME else 250)
    return y, cb, cr


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--width", type=int, default=320)
    parser.add_argument("--height", type=int, default=180)
    parser.add_argument("--scenes", action="store_true", help="print the scene start frames")
    args = parser.parse_args()
    if args.scenes:
        print("\n".join(str(start) for start in SHOT_STARTS))
        return
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
