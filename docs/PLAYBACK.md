# Playback tests on the owner's devices

Written 2026-10-17 (ROADMAP WS6 (2)) from how the P8 test (2026-10-11 to 2026-10-13) and the WS6
and P10 viewings (2026-10-17) were run, and what they did not record.

A playback test answers one question that open-source decoders cannot answer, such as "does the
TV apply the composer in the RPU?" (P8) or "does the TV read codes as `code / 1023` or
`code / 1024`?" (WS6). It is the gate for P8, for the WS6 normalization question and for the P10 `ms_weight` step.
Claude prepares the files and the guide. The owner views them and reports. Claude logs the
result.

## Devices and paths

- Display: LG OLED C9. It is the only Dolby Vision display available (owner, 2026-10-17).
- Paths used so far: Plex on the Nvidia Shield TV Pro (HDMI into the C9; P8, WS6, P10) and the
  Plex app on the C9 (P8). Jellyfin also scans the hand-off folder. Picture modes used: Dolby
  Vision Cinema Home (WS6 canaries, P10), Dolby Vision Cinema (WS6.4 ladder; fixed for the
  WS6.9 probe, owner 2026-10-27).
- Meter: the owner's X-Rite i1Display Pro, read from this PC (see "Metered tests").
- Record these for every test: player app and version, device software version, TV firmware
  version (C9: Settings > All Settings > General > About This TV), TV picture mode, HDMI path
  (input number, AVR or soundbar in between, eARC) and room light. P8, WS6 and P10 all missed the
  firmware, so Claude asks for these fields first, before the per-file questions. A result
  without them is logged with the missing fields as "not recorded".
- Play every file of a test through the same path. A second path is a separate result.

## Building the files

- **One question per test, decided before viewing.** The guide states what each outcome means
  before the owner views anything. A pre-registered table maps each observation to a decision.
- **Matched material.** All files in a group share one HLG or PQ base layer, byte for byte
  (`dovi_tool remove` plus `cmp`). Only the RPU differs. The decoded frames are checked against
  the source: `ffmpeg -f framemd5` on every frame for synthetic patterns. A synthetic pattern
  that depends on exact codes is encoded losslessly (x265 `lossless=1`). The level is set with
  `hevc_metadata=level=5.1`, because x265 writes level 8.5 for lossless streams, and a hardware
  decoder may refuse that. Such a stream played without problems through Plex on the Shield
  (WS6, 2026-10-17).
- **Produced like mkvdovi.** Files come from `mkvdovi --verify --keep-source --no-resume` on a
  copy. A diagnostic RPU (canary) is written from that RPU with a recorded tool. It is injected
  with `dovi_tool inject-rpu` and muxed with `mkvmerge`. Checks: every frame of the RPU
  extracted from the muxed file carries the intended composer, the extracted RPU equals the
  written one byte for byte, and a field diff against the control shows only the intended
  fields changed.
- **A canary first.** A test that depends on the device applying the RPU includes a file whose
  RPU makes the picture obviously wrong if it is applied, for example the green canary of P8.
- **Do not assume the device's neutral is the model's.** The device's arithmetic is not the
  model's. In WS6 a steep chroma composer (coefficients 3.73 on u, u², u³, constant about −9.3)
  came out the same uniform green on every patch, where the model predicted a green-to-magenta
  row. The slope ladder (WS6.4, 2026-10-21) showed why: the C9 follows the curve's shape at every
  slope up to 20.46, but the apparent neutral moves to higher BL chroma codes as the gain grows
  (about +2.5 codes at gain 2.5, +5 to +6.5 at gain 5, more than +12 at gain 10), which is
  consistent with the uniform green. A test whose verdict depends on where grey sits must
  measure or cancel that shift first (WS6.8/WS6.9), or span it with its patches. Scale the patch
  spacing with the steepness: the WS6.3 ladder uses rows 1, 2 and 4 codes apart, because at 4
  codes the steep rungs pushed even the middle patches past what the display can show (600
  nits, P3).
- **A row for every observation.** The interpretation table includes "all patches look the same"
  and "nothing looks as predicted", so a result outside the hypotheses is still logged as an
  outcome, not argued into one.
- **Control.** One file in the group is the normal mkvdovi output for the same base layer.
- **No plain HLG file as a reference through Plex.** In P8, Plex played a plain HLG file as SDR,
  so a plain HLG file only goes in when the player path is known to signal HLG.
- **Reference renders.** libplacebo with pinned settings: `apply_dolbyvision=1`,
  `upscaler=bilinear`, clip tone mapping and clip gamut mapping for numeric checks. The libplacebo
  version goes into `versions.txt`. With its default chroma upscaler, vf_libplacebo v7.374.0
  moved the WS6 canary by about half a code. Gate the numeric check on patches inside the
  display range. On saturated patches with a channel near black, v7.374.0 departs from the
  model by up to 53 codes (WS6.3, 2026-10-18), so check only their hue order. Renders are SDR
  approximations: they show hue, not the TV's brightness. In P8 the preset's grey tint was
  predicted lavender and seen yellow.

## Naming and hand-off

- Folder: `<test>-<YYYY-MM-DD>` in the owner's hand-off folder (`forplex`, scanned by the media
  servers). It holds the files, `VIEWING_GUIDE.txt` (from
  [`VIEWING_GUIDE.template.txt`](VIEWING_GUIDE.template.txt)) and
  `reference-renders/`.
- Copy with `command cp` and `cmp` every file after copying (the 9P mount).
- Add one line to `TESTS_PENDING.txt` in the hand-off folder when the files are delivered. Move
  it to DONE with a one-line result when the owner reports. Delete a finished test's folder only
  when the owner asks; never touch the owner's other files there.
- Work files, logs, `versions.txt` and a `run.sh` that rebuilds the set go to
  `~/mkvdovi-work/<test>-<date>/`, never into the repository.

## Blinding

- File names carry no answer: `<TEST>_1`, `<TEST>_2`, ... in an order that does not follow the
  hypothesis. The control may be named, because it is not compared for preference.
- The guide has the questions and the report form first. The key and the interpretation table
  come last, under "read after you have written down your answers". Reference renders sit in a
  subfolder the guide asks the owner not to open before answering.
- For preference tests (P10), the key stays in the work directory and is revealed only after
  the report.

## What the owner reports

- The section 0 fields of the guide (player, device software, TV firmware, picture mode, HDMI
  path, room light).
- For each file: the Dolby Vision badge or DV picture mode (yes or no), and the answer to the
  test's question, in the guide's words (for example "grey = patch 3").
- How sure the owner is, and anything unexpected, in the owner's own words.

When the owner reports in a session, Claude asks the questions with the question tool: the
setup fields first, then one question per file. Every file of a group gets the same answer
options, in the same order, and no option or description hints at the expected answer. An
answer outside the options goes in as typed. If the answers fit no row of the table, Claude asks
what the picture looks like (for example uniform or graded across the patches) before drawing a
conclusion. The interpretation and any blinding key are shown only after all answers are in.

## Metered tests

Written 2026-10-27 from the WS6.8 build (neutral-finder probe). A test whose answer is a
position (where grey sits, to a fraction of a code) is read with the owner's X-Rite i1Display
Pro rather than by eye.

- **Meter path.** ArgyllCMS 3.5.0 `spotread.exe` on the Windows side (installed 2026-10-27 next
  to DisplayCAL's 2.3.1, under `%APPDATA%\DisplayCAL\dl\Argyll_V3.5.0`), called from WSL with
  `-O` (one reading per call) and the WOLED spectral correction
  `LG OLED 6-Series (i1 Pro 2, ColorMunki Photo).ccss`. Use 3.0.0 or later: 2.3.1 lacks the i1d3
  Rev B low-light fix for OLED. 3.2.0 and later also accept piped stdin on Windows, which a
  persistent `spotread` session (faster than one call per reading) would need.
- **Time the meter first.** The guide's first step takes a few readings and reports the
  cadence. Holds are sized from it (at least 2 whole readings inside each hold, 1 s clear of
  its edges).
- **Not blinded.** The meter reads the files, so they are named by role. The canary is a file
  whose composer ignores the BL chroma: it must read the same at every code.
- **Segment by the timeline, not by luminance.** Steep composers move a patch's luminance far
  from its neighbours (WS6.8 design: at gain 10 a hold reached 4.8× the neutral hold). A
  bright MARK before any test content anchors the timeline; a second MARK and dim gaps check it.
- **Ratios, not absolute colour.** The analysis compares ln(R/G) and ln(B/G) against a gain-0
  reference played at the start, middle and end; ratios do not move with the C9's
  static-image dimming, and the reference is interpolated in time for white-point drift.
- **Shuffle and scramble.** Holds play in a fixed shuffled order inside each file and the files
  in a scrambled order, so drift does not line up with the code or the gain.
- **Self-test before hand-off.** The analysis runs on synthetic devices with known answers
  (each hypothesis, drift, curvature, noise, an output offset) and on libplacebo renders of the
  delivered files; every case must give its pre-set outcome, and none may give a false answer.

## How results are logged

- `docs/ROADMAP_LOG.md`: a dated entry with what was viewed, the build (mkvdovi version and
  commit), the section 0 fields, the owner's words per file quoted as reported, the outcome
  from the pre-registered table, what the test does not cover, and the local work directory.
- `ROADMAP.md`: the item's status, the Log row, and the gate it closes or leaves open.
- `TESTS_PENDING.txt` in the hand-off folder: the line moves to DONE.
- Claude's project memory: the test's note (files, result, what is still open).
- A missing section 0 field is written as "not recorded", never guessed.
