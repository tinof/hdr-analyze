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
  Plex app on the C9 (P8). Jellyfin also scans the hand-off folder. Picture mode used: Dolby
  Vision Cinema Home (WS6, P10).
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
- **Stay inside the range the device follows.** The device's arithmetic is not the model's. In
  WS6 a steep chroma composer (coefficients 3.73 on u, u², u³, constant about −9.3) came out the
  same uniform green on every patch, where the model predicted a green-to-magenta row: the C9
  applied the RPU but did not follow the curve. Keep coefficients near the magnitude of the
  composers the device is known to follow (bt2100, the preset), or first test a ladder of
  increasing steepness to find where it stops following.
- **A row for every observation.** The interpretation table includes "all patches look the same"
  and "nothing looks as predicted", so a result outside the hypotheses is still logged as an
  outcome, not argued into one.
- **Control.** One file in the group is the normal mkvdovi output for the same base layer.
- **No plain HLG file as a reference through Plex.** In P8, Plex played a plain HLG file as SDR,
  so a plain HLG file only goes in when the player path is known to signal HLG.
- **Reference renders.** libplacebo with pinned settings: `apply_dolbyvision=1`,
  `upscaler=bilinear`, clip tone mapping and clip gamut mapping for numeric checks. The libplacebo
  version goes into `versions.txt`. With its default chroma upscaler, vf_libplacebo v7.374.0
  moved the WS6 canary by about half a code. Renders are SDR approximations: they show hue,
  not the TV's brightness. In P8 the preset's grey tint was predicted lavender and seen yellow.

## Naming and hand-off

- Folder: `<test>-<YYYY-MM-DD>` in the owner's hand-off folder (`forplex`, scanned by the media
  servers). It holds the files, `VIEWING_GUIDE.txt` (from the template in
  `~/mkvdovi-work/ws6-normalization-2026-10-17/VIEWING_GUIDE.template.txt`) and
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

## How results are logged

- `docs/ROADMAP_LOG.md`: a dated entry with what was viewed, the build (mkvdovi version and
  commit), the section 0 fields, the owner's words per file quoted as reported, the outcome
  from the pre-registered table, what the test does not cover, and the local work directory.
- `ROADMAP.md`: the item's status, the Log row, and the gate it closes or leaves open.
- `TESTS_PENDING.txt` in the hand-off folder: the line moves to DONE.
- Claude's project memory: the test's note (files, result, what is still open).
- A missing section 0 field is written as "not recorded", never guessed.
