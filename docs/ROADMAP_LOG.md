# Roadmap log

History for [`ROADMAP.md`](../ROADMAP.md): the full text of each progress-log step and the
measurement detail that used to sit inside the roadmap items. `ROADMAP.md` keeps the current
status, the open work and a one-line log; the detail is here, newest first.

## Progress log

### 2026-10-07: E13 step 3, mkvdovi module split

E13 step 3 is done. Four commits, each a pure move with no behavior change:

- Inline tests moved to `mkvdovi/src/metadata/tests.rs` and `mkvdovi/src/pipeline/tests.rs`.
- `metadata.rs` split into `metadata/{mod,format,probe,rpu_config,sidecar,static_metadata}.rs`;
  `pipeline.rs` into `pipeline/{mod,analyzer,dovi_steps,hdr10plus}.rs`. Re-exports keep every
  `metadata::X` and `pipeline::X` path valid.
- `pipeline/mod.rs` stays at 1037 lines, over the 800-line limit: `convert_file` alone is about
  900 lines, and step 4 splits it by phase.
- Gates on 281b927: fmt, clippy (also `--features cuda`), workspace tests (result lines and the 5
  `Skipping` lines identical to main@f104c78), the corpus classification test
  (`MKVDOVI_CORPUS_DIR`), the L1 regression gate without `--update`: all pass. All 8
  `scripts/rpu-baseline.sh compare --require-identical-l1` runs against
  `~/mkvdovi-work/rpu-baseline/refactor-e6d63ed/` pass (default d1, g3, h1, r1, r2; mdfix r4, r5;
  preset g3); unchanged main@f104c78 also passed all 8 before the move. Logs in
  `~/mkvdovi-work/rpu-baseline/e13-step3-281b927/proof/` and in the PR body.
- Still open: E13 steps 4-5.

### 2026-10-07: E13 step 2, refactor safety net

E13 step 2 is done. It adds the checks the pure-move splits of steps 3-5 are judged by, and
changes no production code: every Rust hunk is in a `#[cfg(test)]` module or under `tests/`.

- 34 new characterization tests: `mkvdovi/src/metadata.rs` (format detection, static L6, the whole
  `extra.json`), `mkvdovi/src/pipeline.rs` unit tests (source retention, output naming), and one
  e2e test in `mkvdovi/tests/hlg_profile84.rs`,
  `successful_hlg_conversion_deletes_the_source_and_the_temp_dir`.
- Mutation-checked during development. `metadata.rs`: 69 of 75 mutants caught; the survivors are
  equivalent or reachable only by the opt-in corpus test. `pipeline.rs`: the unit tests on
  `finish_success`/`should_keep_source` missed two mutants, both in the production deletion path.
  For non-DV inputs, source deletion and temp cleanup are an inline copy at the end of
  `convert_file`; `finish_success` is called only by the Profile 7 MEL fast path. The new e2e test
  covers that copy: removing its `fs::remove_file(input_file)` or `remove_dir_all(&temp_dir)` makes
  it fail.
- The opt-in test `corpus_cuts_are_classified_like_their_manifests` (`MKVDOVI_CORPUS_DIR`)
  classifies all 45 development cuts like their manifests.
- Final-RPU baselines captured at main@e6d63ed in
  `~/mkvdovi-work/rpu-baseline/refactor-e6d63ed/{default,mdfix,preset}`: HDR10, HDR10+, HLG with
  `bt2100` and `preset`, two MEL incl. an open-GOP start, `--mdfix` on MEL and Profile 8.
  `scripts/rpu-baseline.sh compare --require-identical-l1` of each baseline against itself exits 0.
  The capture of `--mdfix` runs now finds the `*.mdfix.DV.mkv` output.
- Tooling: the CUDA gate in `.claude/workflows/pre-pr-panel.js` also matches module directories
  (`pipeline/` like `pipeline.rs`); the Stop hook notes a touched `.rs` file over 800 code lines
  once per session and compiles a touched `.cu` kernel with `nvcc -ptx`; module-size rule in
  CLAUDE.md.
- Gates on f950ca8: fmt, clippy, test (no unexpected skips); all pass. No CUDA or L1 gate required
  (no analyzer path touched).
- Still open: E13 steps 3-5.

### 2026-10-06: P1, E1, CUDA as the main pipeline

mkvdovi now treats CUDA as the main analysis pipeline. An explicit `--analysis-quality balanced` or
`fast` with GPU analysis is kept but warned about: it saves no time (the run is NVDEC-bound), drops
the measured MaxCLL (`fast` also MaxFALL) and is not parity-checked. Under `accurate` (the `auto`
default with GPU analysis), measurements analyzed more coarsely are re-analyzed instead of reused
with a warning; other presets keep the warning. mkvdovi warns when a run that expected GPU analysis
comes back with sidecar `analysis.gpu: false`, and when `--hwaccel cuda` resolves with an analyzer
built without the `cuda` feature. CI lints the analyzer with `--features cuda`; cudarc loads the
driver and NVRTC at runtime, so no GPU or toolkit is needed.

- Evidence: unit tests `coarser_sampling`, `rejects_coarser_sidecar`, `analysis_quality_notice` and
  `gpu_analysis_missing` (`mkvdovi/src/pipeline.rs`); e2e test
  `accurate_reanalyzes_coarser_measurements` (`mkvdovi/tests/hlg_profile84.rs`).
- Gates on d459681: fmt, clippy, clippy-cuda, test (no unexpected skips), `scripts/cuda-parity.sh`,
  L1 regression; all pass, L1 references unchanged. No measurement changes on either backend.
- The CI `cuda` clippy step passed locally with cudarc's build script rerun without nvcc or CUDA
  environment variables, and in hosted CI on #31 (run 37530882715).
- Still open: P1 item 1 (CPU fallback default); E1 GPU runner and NVRTC compile-only spike; E2
  addition; E12.

### 2026-10-06: E11, `l1_diff` and open-GOP cuts

E11 is done. A cut that starts at a CRA has RASL pictures no decoder outputs; sidecar v5 records
them, and our frame `i` is stream frame `i + leading`. `tools/l1_diff` paired rows by position
and stopped on the count difference, so such cuts could only be compared in decoded coordinates.

- The sidecar is checked against the `.bin` first: v5 stream fields present,
  decoded + leading = `stream_frames`, every per-frame array of length n.
- Reference frame labels must count up by one. A reference that ends at the last stream picture
  is read in stream coordinates and its rows for the leading pictures are skipped. One labelled
  from 0 with a row per decoded frame (older exports) keeps decoded coordinates and refuses
  `--per-shot` and `--scenes`. Every other count difference is an error, with a hint when no v5
  sidecar was read.
- `--per-shot` and `--scenes` are re-based by the same offset; shots and cuts in the leading
  pictures are dropped and counted, and a cut beyond the reference's frames is an error.
- `--export-reference` labels rows with stream frames (unchanged without leading pictures).

Acceptance gate:

- Synthetic test `tools/l1_diff/tests/open_gop.rs`: a `.bin` of 10 decoded frames with a v5
  sidecar recording 2 leading pictures. A 12-row stream reference scores zero error with its first
  2 rows skipped; `--per-shot` and `--scenes` with a stream shot list match every cut; a
  stream-labelled export reads back with zero error. S+1 rows, a wrong `stream_frames`, a v5
  sidecar without its source fields and a cut beyond the stream are refused; an older sidecar with
  a stream reference gets a hint.
- Gates on 0e5f6da: `tools/l1_diff` fmt, clippy `-D warnings`, 22 unit + 7 integration tests
  pass. `l1-regression` passes in check mode (PQ and HLG synthetic clips, 5/5 scene cuts each);
  `tools/l1_diff/corpus` is unchanged, so export output without leading pictures is
  byte-identical.
- Real material, no new analysis: the three Blade Runner 2049 development cuts have v5 sidecars
  with 2 leading pictures (bitstream: CRA, then 2 RASL pictures, checked with ffmpeg
  `trace_headers`). With a stream-labelled export from this `l1_diff` and their shot lists in
  stream frames, scene cuts match 14/14, 5/5 and 7/7, the same counts as the original
  decoded-frame lists against our decoded scene starts.

Corpus convention (owner decision, 2026-10-06): truth shot lists count stream frames, like the
RPU. Four development lists that were on decoded frames (the three Blade Runner 2049 cuts and G4)
were moved to stream frames.

Still open, supporting evidence only: Joker (the cut with retail L1: 1448 stream frames, 2 RASL,
1446 decoded) and the Champions League cut (G4, 3 RASL) have only v4 runs. Scoring Joker's retail
L1 per frame needs a re-analysis with a v5 analyzer.

### 2026-10-06: P8, spec 4:2:0 decode in the analyzer

The analyzer now measures HLG max-RGB through the spec's 4:2:0 structure (ETSI GS CCM 001
§5.4.2.3.3): the chroma MMR runs once per chroma sample on luma down-sampled with `[1 2 1]` across
two rows, in `code / 1023` f32 arithmetic (`spec-float`), and the composed chroma is upsampled
bilinearly at the stream's chroma location (left, also when unspecified, or top-left; other
locations warn and use left). CUDA runs the chroma pass in a second kernel, `compose_chroma`. New
sidecar names `dovi84-v3` (preset) and `dovi84-bt2100-v1-spec420` (bt2100); the old names are
legacy and re-analyzed; the sidecar stays at version 5. mkvdovi requires the analyzer's `--help` to
name the selected composer's mapping, preset included.

**Gate** (`scripts/validate_hlg_chroma_siting.sh <analyzer +cuda from #29> --gate --cuts
~/mkvdovi-work/corpus/dev`): exit 0, "P8 gate ok", wall 2:34:33. A NaN hole in the script's checks,
found in review, was fixed afterwards in the same PR, with two more check fixes from the Codex
review; the saved outputs re-evaluated with the fixed checks give the same verdicts.

- Synthetic clips (256×256 patterns, left and top-left siting; a letterbox clip with 32-px bars),
  both composers, CPU and CUDA: analyzer against the tool's `spec-float` at its own siting, worst
  |max| 0.000 and |avg| 0.000 codes over 13 frames; per pixel 0 of 851,968 differ, and inside the
  detected crop of the letterbox clip (256×192 at 0,32) 0 of 638,976. CPU and CUDA identical
  (`.bin` bytes, sidecar frames/scenes/light_level, frame statistics). Anchor 2 (tool renderer
  against libplacebo bilinear) within its 4-code tolerance.
- Five HLG cuts, both composers, CPU and CUDA (`gpu: true` on every CUDA sidecar), worst |max| /
  |avg| per frame against `spec-float` at the cut's own siting:

| Cut | Siting | Frames | Worst \|max\| / \|avg\| | CPU = CUDA |
|-----|--------|--------|------------------|-----------|
| d11 Blue Lights S01E02 | left | 2976 | 0.000 / 0.000 | identical |
| g1 Wimbledon 2024 final | top-left | 3051 | 0.000 / 0.000 | identical |
| g2 Glastonbury 2025 | top-left | 1550 | 0.000 / 0.000 | identical |
| g3 The Green Planet | left | 1526 | 0.000 / 0.000 | identical |
| g4 UCL Bayern-PSG (open GOP) | left | 3033 decoded (stream_frames 3036, 3 leading skipped) | 0.000 / 0.000 | identical |

- `HDR_ANALYZER_CUDA_HOST_FRAMES=1` (CUDA analyzes frames downloaded from NVDEC; every CUDA run
  printed that it took this path), g2, the synthetic clips, both composers: "P8 gate ok", 0.000 codes
  worst |max| and |avg| over 1550 frames, per pixel 0 differ, CPU = CUDA identical.

Per-scene diagnostics, variant minus new analyzer, 12-bit codes, largest |difference| over scenes
(`spec-float-nearest` = spec structure with nearest-neighbour upsampling; `spec-fixed` =
`code / 1024`):

| Cut | Composer | `spec-float-nearest` L1 max / avg | `spec-fixed` L1 max / avg |
|-----|----------|------|------|
| d11 | bt2100 | 9.86 / 0.06 | 4.71 / 2.25 |
| d11 | preset | 9.57 / 0.18 | 5.37 / 1.77 |
| g1 | bt2100 | 13.45 / 0.05 | 4.94 / 3.14 |
| g1 | preset | 22.73 / 0.08 | 6.09 / 3.22 |
| g2 | bt2100 | 0.00 / 0.35 | 0.00 / 4.63 |
| g2 | preset | 0.00 / 0.36 | 0.00 / 3.36 |
| g3 | bt2100 | 11.87 / 0.60 | 5.02 / 1.02 |
| g3 | preset | 3.10 / 0.66 | 5.35 / 1.36 |
| g4 | bt2100 | 0.00 / 0.15 | 0.00 / 2.14 |
| g4 | preset | 0.00 / 0.14 | 0.00 / 2.44 |

g2 and g4 sit at the 3079 source-max clamp in every scene, so their L1 max cannot move.

**What it does to L1** (main 71fd03d against this branch; CUDA, `--no-crop --downscale 1
--disable-optimizer`; sidecar per-scene values, new minus old, 12-bit codes; scene boundaries
identical):

| Cut | Composer | Scenes | L1 max: max abs (mean) | avg max-RGB: max abs (mean) | avg luma | min | MaxCLL old → new |
|-----|----------|--------|------------------|------------------|----------|-----|-------------------|
| d11 | bt2100 | 32 | 9 (−0.38) | 0 | 0 | 0 | 905 → 903 nits |
| d11 | preset | 32 | 11 (−0.44) | 1 (−0.16) | 0 | 0 | 925 → 925 |
| g1 | bt2100 | 22 | 15 (−1.41) | 0 | 0 | 1 (−0.09) | 1001 → 1001 |
| g1 | preset | 22 | 19 (−2.18) | 1 (−0.14) | 0 | 1 (−0.14) | 1001 → 1001 |

Luma averages do not move (the luma path is unchanged). Scene L1 max moves by up to 19 codes,
mostly down.

**Other checks.**

- `scripts/cuda-parity.sh`: PASS.
- PQ unchanged: `.bin` and sidecar (timestamps removed) byte-identical between main (71fd03d) and
  this branch on 16 configurations (CPU/CUDA × crop/no-crop × downscale 1/2 × max-RGB/luma) of a
  134-frame 4K PQ clip.
- L1 regression reference: only `tools/l1_diff/corpus/hlg.reference.csv` shot 1 (frames 61–89)
  average 1360 → 1359 (#29).
- mkvdovi by hand: a pre-P8 analyzer is refused up front; a legacy-name sidecar is re-analyzed with
  the "pre-spec 4:2:0" message; `--verify` passes; the source is kept.
- CUDA throughput (RTX 4070, WSL2; `--hwaccel cuda --no-crop --disable-optimizer --transfer hlg`,
  frames ÷ wall time, best of 2 interleaved runs): −0.03% to −1.12% on g1 and d11, both composers,
  downscale 1 and 2 (table in [`CUDA_PIPELINE.md`](CUDA_PIPELINE.md)). NVDEC-bound. Below the 10%
  follow-up threshold; no follow-up item.

Where: [#29](https://github.com/tinof/hdr-analyze/pull/29); [`HLG_COMPOSER.md`](HLG_COMPOSER.md) §9

### 2026-10-06: P8, spec arithmetic measured; decode target `spec-float`

Step 0 of the P8 decode change measured the `bt2100` composer's section 6 criteria
([`HLG_COMPOSER.md`](HLG_COMPOSER.md)) through the spec composer's fixed-point arithmetic, on flat
fields (`fit_hlg_composer report --spec-fixed`). Two deciding criteria fail, so the planned
`spec-fixed` target was dropped by owner decision.

- `bt2100` under `spec-fixed`: neutral R′G′B′ spread 5.891 codes (float decode 0.005), neutral luma
  error 3.57 codes at code 902 (0.64), ΔE_ITP 1.05 (0.11); superwhite 0.03 and the source clamp
  pass. Neutral composed chroma at code 721 is (32751, 32750), 17 LSB below 32768. The preset
  barely moves (spread 172 → 173 codes); it is tinted under either arithmetic.
- Cause: the normalization, not the 16-bit output. ETSI GS CCM 001 §5.4.2.3.2–3 (pp. 18–20) feeds a
  10-bit code into the polynomial and the MMR as `s << 10` on a 2^20 scale, i.e. `code / 1024`;
  the composer was fitted at `code / 1023` (`tools/fit_hlg_composer/src/model.rs`). About 5.78 of
  the 5.89 codes of spread are the moved neutral point, 0.1 the quantization; the luma error is
  entirely the moved variable.
- Checked three ways: the spec text read against `SpecComposer` term by term; an independent
  Python re-derivation from the constants (5.891 and 3.57 exactly); libplacebo, which scales the
  pivots by `1 / ((1 << bl_bit_depth) − 1)` and evaluates on texture values, i.e. `code / 1023`
  with no correction (`libav_internal.h:967-968`). No open-source decoder runs the spec's fixed
  point. CCM 001 defines only BT.1886 and PQ base layers, so spec arithmetic for an HLG base layer
  is an extrapolation.
- Decision: the analyzer takes the spec's structure (MMR at chroma resolution on down-sampled luma,
  bilinear upsampling at the stream's chroma location) with the `code / 1023` f32 arithmetic the
  composer was fitted for and libplacebo uses (`spec-float`). The structure is unambiguous and
  carried up to 17.4 codes of L1 max per scene; the arithmetic moved scene averages by 0.6 to 3.3.
  On flat fields `spec-float` equals the current decode, so section 6 holds unchanged. Which
  convention devices use is a WS6 question; a `code / 1024` decision would be a refit (new
  composer, new RPU).

Where: this file; `fit_hlg_composer report --spec-fixed`

### 2026-10-05: P8, 4:2:0 chroma of the HLG decode

The P8 step "4:2:0 chroma siting of the analyzer's decode against a renderer on non-flat
patterns" is done; it opens a decode change. Planning found that the reference is the spec
composer, not a renderer: ETSI GS CCM 001 v1.1.1 §5.4.2.3.3 down-samples the BL luma to the chroma
positions and runs the MMR at chroma resolution, in integer arithmetic with 10-bit inputs as
`code / 1024`. The analyzer replicates each chroma sample over its quad and reshapes it with each
pixel's own luma (`code / 1023`, f32); libplacebo upsamples chroma first and reshapes per pixel
(`code / 1023`). Tables and method: [`HLG_COMPOSER.md`](HLG_COMPOSER.md) §9.

- New `fit_hlg_composer chroma-siting` (analyzer, spec in the analyzer's arithmetic, spec in its
  fixed point, renderer; nearest or bilinear for chroma location left and top-left) and
  `scripts/validate_hlg_chroma_siting.sh` (synthetic edges, lines, 1×1 to 3×3 highlights, colour
  ramp, flat controls; optional real cuts). Anchors on every run: the tool reproduces the
  analyzer's per-frame maximum exactly and average within 0.5 code; its renderer variant matches
  libplacebo (`upscaler=bilinear`) within 0.32 codes per pixel (`bt2100`) and 3.84 (preset),
  including every pixel around the test highlights.
- Acceptance limit, set in plan mode: per-scene |analyzer − spec| ≤ 4 codes for L1 max and avg on
  every scene of the five HLG cuts, both composers. Measured (`bt2100` on every frame, preset on
  every 4th): L1 max up to 18.3 (`bt2100`) and 18.2 (preset), on Wimbledon; average up to 4.9 and
  3.5, on Glastonbury. Exceeded, so P8 gets the open step "spec 4:2:0 decode in the analyzer".
- The structure (MMR at chroma resolution) accounts for up to 17.4 codes of L1 max and below
  1 code of average; the arithmetic (`code / 1024`) for most of the average (−4.0 codes on flat
  red through `bt2100`, on every pixel). The display's upsampler of the composed chroma is not
  specified: replicated instead of bilinear gives 6.5 codes on Wimbledon instead of 18.3.
- Two of the five cuts (Glastonbury, Champions League) reach the 3079 clamp in every scene, so
  the L1 max evidence comes from three cuts. 18 codes of L1 max lies below the smallest WS7 point
  (+75 codes).
- Hypothesis, not tested: the PQ path replicates chroma over the quad too (`frame.rs`, the non-HLG
  branch). Its matrix is linear, so only colour edges differ, but saturated small highlights may
  contribute to the clean-source over-read on the MEL cuts (WS1).

Where: this file; [`HLG_COMPOSER.md`](HLG_COMPOSER.md) §9

### 2026-10-05: WS8 item 5 done; findings for P9, WS1, E7

Two deliveries (2026-10-04 and 2026-10-05) filled the per-format gaps. All cuts are stream copies
of about 60 s, checked on arrival with MediaInfo, `dovi_tool` and `hdr10plus_tool`, and remuxed
with mkvmerge (the containers carried the statistics tags of the full source). Each new PQ title
also gave two holdout cuts; 29 cuts are now held out, unanalyzed.

| Format | Title | What it adds |
|---|---|---|
| HDR10+ | Deadloch S01E01 (Amazon WEB-DL) | Profile B, 1000-nit master, two cuts (day, night) |
| HDR10+ | The Shining (disc) | DV MEL + HDR10+ Profile B, 4000-nit master; retail L1 |
| HDR10+ | Alien (disc) | Profile A with MaxSCL 0 on every frame |
| HDR10+ | Alita: Battle Angel (disc) | DV MEL + HDR10+ Profile A on one disc: the same-master pair; retail L1 |
| HLG | Wimbledon 2024 (iPlayer) | live sport, 50p, HLG only as the alternative transfer |
| HLG | Glastonbury 2025 (iPlayer) | concert, stage strobes |
| HLG | The Green Planet (iPlayer) | nature, saturated highlights |
| HLG | Champions League (broadcast capture) | 50p, starts at an open-GOP CRA (3036 pictures, 3033 decoded) |
| HDR10 / MEL | Joker: Folie à Deux (disc) | MEL, 1000-nit master; starts at a CRA with 2 RASL pictures |
| HDR10 / MEL | Skyfall (disc) | MEL with per-shot L1 (32 and 11 distinct values), no MaxCLL |
| HDR10 | Exodus: Gods and Kings (disc) | 1100-nit master, no MaxCLL |
| HDR10 | Star Trek (2009, disc) | 1000-nit master, no MaxCLL/MaxFALL |
| DV 8.1 | Inside Out 2 (Disney+) | animation; RPU shot list kept, static L1 not used |
| HDR10 | The Revenant ("Open Matte" hybrid) | bright snow; injected RPU and HDR10+ not used |

Not usable as references: The Revenant's RPU states a 0.0001/1000-nit master against 0.005/4000 in
its own L6 and the HDR10 base, and marks one shot in 60 s. Inside Out 2's L1 max is 2467 to 2471 on
all 21 shots. The Shining's two formats are not proven to come from one master; Alita's are.

First scores with the default estimator (per-shot peak against retail L1, bias in 12-bit codes):
Alita +162.3 (day) and +147.6 (night), Skyfall +88.4 and +161.8, The Shining +25.6 (night) and
−106.3 (day). Alita is digital and largely computer-generated, so the over-read on clean MEL
sources is not a grain effect (WS1).

HDR10+ Profile A (Alien, Alita) carries MaxSCL 0 on every frame, with the percentile distribution
and the average present (P9).

Before sidecar v5 (#26) every analyzer cut on the Joker cut sat exactly 2 frames before the
authored one (10 of 10) and the L1 comparison stopped on 1446 against 1448 frames; it is to be
rescored with a v5 build (E7).

Scene detection against the RPU and HDR10+ shot lists: Deadloch 13/13 and 20/21, Alita 15/17 and
17/17, Skyfall 27/34 and 10/10, Inside Out 2 20/20, Alien 12/18, The Shining 2/7 (day). Shot lists
made by the acquisition agent (HLG, plain HDR10) are unchecked; two of them (the football cut, the
Red Sea cut) list 2 cuts where the analyzer finds 8 and 25.

Where: this file

### 2026-10-05: WS8, P9, P8, coverage of real material

The development tier was reviewed per input format. Results:
- HDR10: 8 cuts plus 9 held out, from two titles, all mastered at 0.005 / 4000 nits, with shot
  lists but no authored L1.
- HDR10+: one cut (11 scenes).
- HLG: one drama cut, with no shot list or reference.
- Synthetic and open-content pairs: 7 entries.
- The only real-content L1 reference comes from the Dolby Vision titles (Profile 7 FEL and MEL),
  whose base layers are HDR10.

P9's gate (scored on the development tier) cannot be met with one HDR10+ cut, so HDR10+ material
now comes before P9. HLG material goes with the P8 playback test. The acquisition order is WS8
item 5.

Where: this file

### 2026-10-04: E7, open-GOP cuts

A dev-tier HDR10 cut failed `--verify` with 1443 measured frames against 1445 in the video.

**Cause.** The cut starts at a CRA picture (NAL type 21), followed by a complete RASL_R (9) and a
RASL_N (8) picture. FFmpeg skips both ("Skipping invalid undecodable NALU"). `dovi_tool` 2.3.4
`inject-rpu` assigns `rpus[presentation_number]` (`rpu_injector.rs`). `hevc_parser` sorts each GOP
by POC, so the two RASL pictures come first, and missing entries repeat the last RPU.

**Measurement.**
- Took 60 frames of the cut and an RPU with a distinct L1 max per frame.
- Matched the RPU bytes of each access unit in decode order, and mapped access units to displayed
  frames through the decoder's packet positions.
- Result: CRA→RPU 2, RASL_R→0, RASL_N→1. Displayed frame *k* carried RPU *k* + 2, so every scene's
  L1 was shown two frames early.

**Fix.**
- The analyzer counts the RASL pictures of the first IRAP from the packets' NAL types. It requires
  `decoded + leading == pictures` (packets that carry a slice; an end-of-sequence packet is not
  one) and errors on any other loss.
- Sidecar v5 records both numbers.
- `mkvdovi` shifts the shots and makes the RPU one entry per picture. `--verify` compares in stream
  frames.
- A measured RPU that `inject-rpu` reports as mismatched is refused.

**Results.**
- The cut now passes `--verify`. It reports 2 leading pictures and 1443 measured frames, and its
  MKV statistics tags are stale: MediaInfo reports the full film's 235152 frames.
- A new integration test cuts a synthetic open-GOP clip with `mkvmerge --split`, which leaves 3
  RASL pictures. It checks the displayed-frame-to-L1 association without `dovi_tool`'s parser. A
  second test checks that a stream starting after its CRA is refused and that its source is kept.

Where: [#26](https://github.com/tinof/hdr-analyze/pull/26)

### 2026-10-04: P10, E10

P10 steps 1 and 2. Profile 8.1 `extra.json` passes `source_min_pq` / `source_max_pq` as the PQ codes
of the mastering minimum and peak (`dolby_vision::utils::nits_to_pq_12_bit`, the crate's conversion
for a Dolby CM XML). Checked on `dovi_tool` 2.3.4: with the keys, a 0.05 / 600-nit master is
written as 189 / 2851; without them the lookup writes 0 / 3079. Standard masters (min 0.0001 or
0.005 nits; peak 1000, 2000, 4000 or 10000 nits) and the defaults give the same values as the
lookup, which a unit test checks against the crate's `source_meta_from_l6`. Profile 8.4 keeps
62 / 3079. `--verify` parses a generated RPU in-process. It checks the source range on every frame,
and checks that every frame of every sidecar scene carries the measured L1 after the generator's
clamp (`ExtMetadataBlockLevel1::from_stats_cm_version` with CM v2.9). It reports per field how many
scenes were clamped and the largest change, and counts scenes whose L1 max lies above
`source_max_pq`. E10: the generator converts with ×4095 like the analyzer; the retail-RPU check
against the ×4096 of ETSI GS CCM 001 is still open.

Where: [#25](https://github.com/tinof/hdr-analyze/pull/25)

### 2026-10-03: FEL (replaces F1, F2, R1)

Profile 7 FEL compositor and its re-encode removed; FEL inputs are refused with a pointer to the
plan. Measured against the reconstruction in ETSI GS CCM 001 on real base-layer pixels: seven of
eight Profile 7 FEL test cuts use order-3 MMR chroma on every frame, and there the composed chroma
missed the specified prediction by a mean of 69 to 534 10-bit codes (mostly clipped to 0 or 1023),
while the specified prediction stays within 0.2 to 2.8 codes of the base layer. Pivots were used as
absolute values although they are delta-coded, the polynomial constant and the MMR constant and
cross terms were scaled wrongly, and the unit tests encoded the same convention. One cut with
identity mapping was unaffected.

Where: [`docs/FEL_PLAN.md`](FEL_PLAN.md)

### 2026-10-03: P8–P11, E8–E10, WS7, WS8; P0, P1, P7, WS1, WS2, WS4, E4

Review of all conversion paths against public specifications, the generator's code and output, and
the retail RPUs of the test cuts. Priorities reordered: defects that make delivered output wrong for
every file of an input type first, then evaluation in displayed-picture units, then measured
improvements, then research. New items: HLG composer tint, HDR10+ L1 average, generator clamps and
source range, histogram percentile reader, `--mdfix` and authored trims, display-mapping simulator,
test-material fixes. Statements that the measured L1 minimum is delivered were corrected.

Where: this roadmap; [`docs/CM_ANALYZE_PARITY.md`](CM_ANALYZE_PARITY.md)

### 2026-10-02: WS1, WS2

Robust peak estimator replaced by a rule on the shape of the histogram top (centre and width of the
top population, grain variance removed from the width, correction limited to 5.2 sigma; flat tops
and detached groups kept; aware of the 10-bit code grid). Same kernel statistics, CPU and CUDA
identical, default unchanged. Flat highlights and flashes far above the picture: 0 codes lost on 12
synthetic segments (old rule: 14 to 246). Three clean/grain pairs against the clean raw maximum:
bias +102.7 → +23.4, +41.0 → +17.0, +83.3 → +50.5; mean absolute error 108 → 60, 41 → 19, 85 → 71.
Grainy retail cut against embedded L1: +76.1 → +54.4. Not met: one to three pixel highlights near
the grain are lowered by up to 5.2 sigma, up to 27% of grainy frames read below the clean twin,
clean content moves 3 to 21 codes per frame, in-shot variation is 10% above `max`, three cuts with
embedded L1 move further below it. Shot aggregation unchanged. Stays opt-in.

Where: [#22](https://github.com/tinof/hdr-analyze/pull/22)

### 2026-10-02: WS1

`--peak-estimator robust` runs in the CUDA kernel with output identical to the CPU (kernel fills the
cross-quad difference histogram; CPU max-RGB mix and histogram binning moved to the kernel's f32
arithmetic). No algorithm change. A real-content run now takes the same time as the default
estimator, so estimator candidates can be scored in minutes. First scores of the unchanged
estimator: clean/grain pair bias +102.7 → −1.1 codes per frame, but the largest error grows (251 →
333) and shot-to-shot spikes get larger; one grainy retail cut +76.1 → +64.0 against its embedded
L1.

Where: [#21](https://github.com/tinof/hdr-analyze/pull/21)

### 2026-10-02: WS2, E1

Scene detection rebuilt: cuts chosen after analysis from a flash-tolerant score and the local
frame-to-frame level, strongest candidate first. Authored shot lists of ten real-content cuts: 142
of 168 cuts matched with 13 extra (before: 91 matched, 228 extra). The fixed threshold was saturated
by grain on about half of all frames; picture type played no role. Open: pictures that change
completely on every frame. The CI clip now has unequal shot lengths and a constructed scene
reference.

Where: [#20](https://github.com/tinof/hdr-analyze/pull/20)

### 2026-10-01: WS1, WS2

Measurement study for the grain-robust peak started: how isolated the raw peak is on the available
clips, spatial-support and temporal candidates, CUDA feasibility. Blocker for promotion: the two
grainy titles and their `cm_analyze` v2 output from [VALIDATION.md §7](VALIDATION.md) are not on the
development host.

Where: in progress

### 2026-10-01: WS2

Scene averages from unfiltered frame means (sidecar version 4). `mkvdovi` regenerates a stale RPU on
resume.

Where: [#18](https://github.com/tinof/hdr-analyze/pull/18)

### 2026-10-01: P0, WS6

Final RPU checked against the sidecar on real HDR10 and HLG clips; 0.5.1 final-RPU baselines
captured for five clips.

Where: [#17](https://github.com/tinof/hdr-analyze/pull/17)

### 2026-10-01: E1, WS0

`l1_diff` limits, L1 regression gate in CI, CPU/CUDA parity check, final-RPU baseline script. The
gate's first CI run exposed a lossy test-clip encode on newer FFmpeg; fixed.

Where: [#17](https://github.com/tinof/hdr-analyze/pull/17)

## Item detail as of 2026-10-03

The full roadmap rows before they were shortened on 2026-10-04, verbatim.

### Priority table

- **1** (P8, P7)
  - Work: HLG → Profile 8.4 (`--hlg-composer bt2100` and the input refusal landed 2026-10-03; bt2100
    made the default by owner decision before the playback test, which the owner runs with it):
    composer that keeps neutrals neutral, fitted to the BT.2100 / BT.2408 1000-nit HLG-to-PQ
    conversion. Refuse full-range or non-BT.2020 HLG instead of warning.
  - Evidence: The fixed preset decodes 75% grey to R′G′B′ 2384/2387/2439 in 12-bit PQ codes (blue 52
    codes high, ΔE_ITP 6.8) and reaches ΔE_ITP 10.3 near black. Its `ycc_to_rgb` and `rgb_to_lms`
    matrices are plain BT.2020, so nothing compensates. Nominal peak white decodes to about 1150
    nits and L1 clamps it to 1000. Every HLG output is affected on a display that applies the
    composer.
  - Gate: Default since 2026-10-03 by owner decision. Whether devices accept a mapping that is not
    the preset is unverified; the WS6 playback test decides whether it stays (`--hlg-composer
    preset` is the fallback).
- **2** (P9)
  - Work: HDR10+ → Profile 8.1: hybrid mode with the scene list and peak from HDR10+ and the
    average, minimum and crop from pixels.
  - Evidence: `dovi_tool` takes L1 from the first frame of each HDR10+ scene: average = PQ of a
    linear-light mean rounded to whole nits, minimum 0, no measured L5. On the HDR10+ test cut it
    reads 156 to 505 codes (median 264) above the analyzer's mean of PQ max-RGB in all 11 scenes.
    The cause is not isolated: PQ concavity and the producer's own measurement are not separated.
  - Gate: Opt-in; scored on the development tier before any default change.
- **3** (P10)
  - Work: Deliver what was measured: pass `source_min_pq` / `source_max_pq` explicitly, record
    measured against delivered values, decide whether to write L1 in-process to avoid the
    generator's clamp.
  - Evidence: `dovi_tool generate` writes an L1 minimum above 12 codes as 12 (135 of 435 test
    scenes), a maximum below 2081 as 2081 (24 of 435) and an average below 819 as 819. It maps a
    mastering peak other than 1000/2000/4000/10000 nits to 1000 and a mastering minimum other than ≤
    0.001 or exactly 0.005 nits to 0. Retail RPUs carry L1 minima above 12 codes (up to 251).
  - Gate: The source-range fix has no open question. Writing unclamped L1 needs WS7 or a playback
    test to show it is at least as good.
- **4** (E8, E9, P1, E10)
  - Work: Code defects with narrow reach: histogram percentile reader; `--pre-denoise` values;
    half-resolution CPU default; L1 scaling note.
  - Evidence: The percentile reader returns bin/255 on a histogram split into 64 + 192 bins, so a
    1000-nit value reads as about 305 nits; it reaches the RPU only with `--peak-source
    histogram99/999` or `--peak-domain luma` with a non-conservative profile. `--pre-denoise`
    advertises `nlmeans`, but any value other than `median3` does nothing. CPU-only hosts default to
    half-resolution `FAST_BILINEAR` analysis, which averages small highlights; the effect is
    unmeasured.
  - Gate: Reader and option fixes: unit tests. CPU default: measure first.
- **5** (P11)
  - Work: `--mdfix` replaces the whole RPU: decide between a targeted repair that keeps unaffected
    levels and full regeneration.
  - Evidence: Authored L2 and L4 are replaced by neutral L2 and no L4. Authored trims are far from
    neutral: up to 928 codes on slope and 1189 on power at the 100-nit target on one MEL title; L4
    is on every frame of all nine retail RPUs. Against a targeted repair: trims were authored
    against the authored L1 and shot list.
  - Gate: Open decision.
- **6** (WS7)
  - Work: Evaluation in displayed-picture units: a histogram-domain display-mapping simulator, then
    ColorVideoVDP on a few cuts.
  - Evidence: Recomputed with open curves for a 1000-nit shot on a 1000-nit panel: L1 max +75 / +140
    codes costs 14.6% / 24.4% at the peak with libplacebo's spline and 2.5% / 4.5% with the ITU-R
    BT.2390 EETF, and nothing when L1 max stays below the panel peak. Reading low clips highlights.
  - Gate: Simulator output reproduces these numbers. It does not model a Dolby display (limits under
    WS7).
- **7** (WS8)
  - Work: Test material: grain on the synthetic highlights and unaligned highlight positions, a
    grainy twin from the same master, transitions.
  - Evidence: The synthetic highlight pair writes its highlights after the grain and aligned to the
    chroma quad; the grainy twin comes from a different master (about ±50 codes); no test cut
    contains a dissolve or a fade.
  - Gate: Needed before 9 can be scored.
- **8** (WS2, E4)
  - Work: Scene detection: second signal for whole-picture motion; strong cuts closer than 12
    frames.
  - Evidence: Missed cuts dominate on two whole-picture-motion cuts: 7 detected against 32 authored
    shots, 10 against 36. Eight authored shots are shorter than 12 frames.
  - Gate: Precision and recall against authored shot lists, no loss on the other cuts.
- **9** (WS1, WS2, WS4)
  - Work: Research, default unchanged: grain-robust peak; per-frame L1 inside detected transitions;
    L4; automatic trims.
  - Evidence: No spatial statistic passes on the clean/grainy twins (WS1). All nine retail RPUs are
    shot-constant in L1 (WS2). Nothing can validate L4 or trims in the open (WS7).
  - Gate: Each stays opt-in or unbuilt until its gate in the tables below is met.
- **10** (FEL)
  - Work: Profile 7 FEL: **placeholder.** The compositor was removed on 2026-10-03 and FEL inputs
    are refused. A design that delivers FEL information without re-encoding is tracked in
    [`docs/FEL_PLAN.md`](FEL_PLAN.md).
  - Evidence: See the progress log.
  - Gate: Set in the plan.
- **Conditional** (R2, WS5)
  - Work: Profile 5 through an established encoder, only if matched playback tests justify it

### P0 (Core complete)

Measured per-scene L1 (minimum, max-RGB mean, maximum) reaches the RPU as explicit `dovi_tool
generate` shots and bypasses optimizer targets. A missing, invalid, or mismatched sidecar re-runs
analysis. The old `--madvr-file --use-custom-targets` generation, where L1 max follows optimizer
`target_pq` and L1 avg is a placeholder, is reachable only through `--legacy-madvr-l1`. Checked
2026-10-01 on the final RPU of one real HDR10 clip (33 scenes), three real HLG clips (96 scenes) and
the synthetic HDR10 clip: every scene's L1 equals the sidecar after the generator's limits (minimum
at most 12, maximum at least 2081, average at least 819). These limits are rules of `dovi_tool` and
the `dolby_vision` crate, not of ETSI GS CCM 001; what they do to the measured values is P10.

### P1 (Partial)

Source-honest generation is the default. Open: decide whether full-resolution every-frame analysis
becomes the CPU default. Today `auto` resolves to `accurate` only with CUDA analysis and to
`balanced` otherwise; measure the CPU cost first. `balanced` downsizes with `FAST_BILINEAR` at half
resolution, which averages small highlights, where CUDA samples full-resolution pixels with a
stride; the size of the effect on L1 max is unmeasured (2026-10-03) and should be measured before
any more elaborate peak estimator. `fast` also copies each measurement into two skipped frames, so
cuts land on a 3-frame grid and peaks on skipped frames are missed.

### P8 (Landed; default bt2100; playback test open)

HLG → Profile 8.4 composer. 2026-10-03: `--hlg-composer bt2100` implemented as designed in
[`docs/HLG_COMPOSER.md`](HLG_COMPOSER.md) (neutral spread 0.005 of a 12-bit code, nominal white 1000
nits, colour-patch ΔE_ITP 11.8 against 26.1 in libplacebo's render; HLG colour input contract
enforced). Default switched to `bt2100` the same day by owner decision; the WS6 playback test still
has to show that devices apply it (`--hlg-composer preset` is the fallback). Background before the
change: `mkvdovi` wrote the preset in every HLG output. 4:2:0 chroma siting measured 2026-10-05
against the spec composer: up to 18 codes of L1 max per scene, so the analyzer decode is to move to
the spec's (progress log, 2026-10-05). Background: the fixed,
phone-derived preset of the `dolby_vision` crate. Its chroma MMR tints neutrals; recomputed from the
preset coefficients through the 8.4 decode (12-bit PQ codes R′/G′/B′, ΔE_ITP against a neutral of
the same luminance): code 200 → 879/868/793, 10.3; code 502 → 1819/1813/1851, 4.5; code 721 (75%) →
2384/2387/2439, 6.8; code 940 → 3134/3143/3155, about 1150 nits luminance. The preset's `ycc_to_rgb`
and `rgb_to_lms` are plain BT.2020, so the tint is not compensated later in the chain. In luma the
preset is close to the reference (75% HLG decodes to 208 nits against 203 in BT.2408). Plan: fit the
same mapping syntax (8-piece luma polynomial, order-3 chroma MMR) to the public BT.2100 / BT.2408
1000-nit HLG-to-PQ conversion with neutrals preserved and nominal white at 1000 nits; weight the low
end, where the tint is largest; fit colour behaviour, not only the grey ramp. The emitted RPU and
the analyzer's decode must change together, with a new `luminance_mapping` value in the sidecar.
Opt-in first. Unverified: whether playback devices apply a composer that is not the preset, and how
visible the tint is on a TV (nothing was measured on a display); both need the WS6 playback test.

### P9 (Open)

HDR10+ → Profile 8.1 L1. `mkvdovi` never analyzes HDR10+ inputs; `dovi_tool generate
--hdr10plus-json` takes L1 from the first frame of each HDR10+ scene, with minimum 0, average = PQ
of the linear-light mean of max-RGB rounded to whole nits, and no measured crop. On the HDR10+ test
cut (11 scenes, no letterbox) this average is 156 to 505 codes (median 264) above the analyzer's
mean of PQ max-RGB. The rounding alone moves a 0.7-nit scene by 71 codes. How much of the gap is PQ
concavity and how much the producer's own measurement is not separated; the sidecar has no
linear-light mean to test it. Plan: opt-in hybrid mode (HDR10+ scene list and peak; average, minimum
and L5 from pixels), scored on the development tier before any default change; a pixel fallback for
missing or implausible HDR10+ statistics. The panel peak is still not passed as a trim target, and
suspicious scene peaks still only warn.

### P10 (Open)

Measured against delivered. `dovi_tool generate` (2.3.4, checked on its output) clamps each L1: a
minimum above 12 codes (0.00026 nits) becomes 12, a maximum below 2081 (100 nits) becomes 2081, an
average below 819 (2.43 nits) becomes 819; `mkvdovi` requests the CM v2.9 average floor, otherwise
it would be 1229. Of 435 test scenes, 135 have a measured minimum above 12 and 24 a maximum below
2081. Retail RPUs carry minima above 12 (up to 251 codes), so the minimum cap is the generator's,
not a rule every deliverable obeys. For Profile 8.1 the generator derives `source_min_pq` /
`source_max_pq` from L6 through a lookup: a mastering peak other than 1000/2000/4000/10000 nits
gives 3079 (1000 nits), a mastering minimum other than ≤ 0.001 or exactly 0.005 nits gives 0;
`mkvdovi` does not pass the two fields. Plan: (1) pass `source_min_pq` / `source_max_pq` explicitly
from the mastering metadata; (2) record measured and delivered L1 per scene and report the
difference in `--verify`; (3) decide whether to write L1 in-process (the `dolby_vision` crate is
already a dependency) so the measured minimum and sub-100-nit maxima reach the RPU. Step 3 needs
evidence from WS7 or playback; raising the average floor to 1229 is not planned. Also open:
generated L2 carries `ms_weight` 2048 where the retail RPUs carry 512 or 0; the effect is
unverified.

### P11 (Open; decision)

`--mdfix` on Profile 7 MEL and Profile 8 regenerates the whole RPU from a clean base layer, so
authored L2 and L4 are replaced by neutral L2 and no L4, authored L5 is reduced to one sampled value
and L6 comes from the container. Authored trims in the retail RPUs are far from neutral: at the
100-nit target slope 1120 to 1384 and power 859 to 1149 against the neutral 2048 on one MEL title,
typically 100 to 400 codes on the others; L4 is present on every frame of all nine. The open
decision: a targeted repair that replaces only the defective level and keeps the others (`dovi_tool`
can carry the display metadata over unchanged), against the finding that trims were authored for the
authored L1 and shot list (the analyzer's average read 24 to 169 codes lower on one cut and its
segmentation differed by up to 4.6× in shot count), so new L1 under old trims applies a correction
to a curve it was not made for. Until decided, `--mdfix` should state in its output which authored
levels it drops.

### WS1 (Partial)

Measurement core, per-scene minimum passed to the generator (which then clamps it, P10), and the
max-RGB-mean average domain have shipped. Open: a grain-robust peak. Raw max-RGB reads +92.6 / +74.4
codes hot against `cm_analyze` v2; the first opt-in robust estimator reached +80.4 / +66.4 and
missed its promotion gate ([VALIDATION.md §7](VALIDATION.md)). It also lowered flat highlights and
was replaced on 2026-10-02 by a rule on the shape of the histogram top ([TECHNICAL_REFERENCE.md
§2.4](TECHNICAL_REFERENCE.md)), which keeps flat highlights far above the picture and removes 39 to
77% of the per-frame bias of three clean/grain pairs (17 to 55% of the mean absolute error), but
lowers small highlights near the grain by up to 5.2 sigma, lowers clean content by 3 to 21 codes per
frame, raises in-shot variation by 10% and leaves a grainy retail cut at +54 against its embedded
L1; it has not been scored against `cm_analyze`. Three histogram-only designs (tail shape, sigma
estimate, shot aggregation) were tried on one development corpus, and none separated grain from
clean detail or an isolated grain pixel from a small specular. Spatial statistics were measured on
2026-10-03 on the clean/grainy twins (median and range of the error against the clean raw maximum,
12 frames): raw +112 (−58 to +216); 3x3 sliding minimum −22 (−338 to +90), losing up to 314 codes on
clean frames; `[1 3 3 1]/8` prefilter +22.5 (−198 to +117), losing up to 157; quad-aligned 2x2
minimum +23 (−155 to +148), losing up to 99. Compared in the same statistic, they remove 55 to 70%
of the grain excess. The 3x3 minimum and the prefilter read a 2x2 highlight more than 1000 codes
low; the 2x2 minimum keeps it only because the test highlights are aligned to the chroma quad (WS8).
On grain correlated over two pixels a 3x3 minimum still leaves about 3 sigma in simulation. None
passes, and the 3x3 window and the prefilter should not be built as estimators on these numbers.
Public prior art for spatial smoothing before L1 is WO2021247670A1; it does not validate any of
these. The 5.2-sigma reach of the current rule is set for white noise; the simulated max-RGB excess
at 8 megapixels is 5.8 sigma. Research track: next is better test material (WS8) and the
display-unit scale (WS7), which shows that an over-read costs nothing while L1 max stays below the
panel peak; do not enable the robust estimator by default. The new estimator must run on CPU and
CUDA with identical output (integer counts, fixed-point sums, order-independent reductions; see
[`docs/CUDA_PIPELINE.md`](CUDA_PIPELINE.md)), as the current `robust` estimator does since
2026-10-02 (`median3` is still CPU-only). Its frozen synthetic gate must include what temporal
persistence can wrongly remove: a small specular, a one-frame flash, a highlight on the first or
last frame of a shot, a 2–3 frame specular and a fade. HLG max-RGB shipped in v0.5.0. Also open:
true target-gamut peaks.

### WS2 (Partial)

Initial shot aggregation shipped: the shot maximum is the maximum of its frame peaks, so one
retained grain spike can set a whole shot. A pooled two-level shot rule and a 4-pixel support floor
were prototyped on 2026-10-02 and not adopted: the floor erases 1 to 3 pixel highlights and moves
clean shot peaks by 7 to 58 codes, and the pooled rule lowers highlights within about two sigma of
the shot maximum. Scene averages are unbiased since sidecar version 4 (2026-10-01): they are the
mean of unfiltered frame means; before, a forward-only EMA made a 24-frame fade read 893 codes
instead of 1569 ([VALIDATION.md §9](VALIDATION.md)). The `cm_analyze` v2 average comparison has not
been repeated for version 4. Scene boundaries (2026-10-02): the detector chooses cuts after analysis
from a flash-tolerant score against the local frame-to-frame level; 142 of 168 authored cuts matched
with 13 extra on ten real-content cuts. Checked 2026-10-03 against the nine retail RPUs: L1 changes
only at shot starts (0 changes elsewhere), so shot-constant L1 is kept. L4 is present on every
frame; its anchor correlates with the analyzer's per-frame average at r = 0.94 to 0.99 in eight RPUs
and 0.71 in one, equally well with the luma average, so it does not decide the average domain. In 9
of 18 test cuts the analyzer's scenes leave fewer frames more than 100 codes from their segment mean
than the authored shots do; missed cuts dominate only on two whole-picture-motion cuts (7 scenes
against 32 authored shots, 10 against 36). Open, in this order: (1) a second scene signal for those
pictures (E4) and strong cuts closer than `--min-scene-length` 12 (eight authored shots in three
cuts are shorter), by a confidence exception, not a lower global minimum; (2) research, opt-in:
per-frame L1 inside detected dissolves and fades through the generator's per-frame edits, which
needs a per-frame peak in the sidecar (a version bump) and transition test material (WS8), and can
itself cause pumping; (3) L4 emission, deferred until something can validate it (no open renderer
reads L4); (4) robust aggregation (investigated with WS1).

### WS6 (Open)

Final-RPU regression corpus covering grain, saturated highlights, raised blacks, fades, flashes,
rapid cuts, and changing aspect ratios. Checks run on the RPU extracted from the muxed file. Add a
Shield/TV playback procedure comparing matched material from the same master, recording player,
firmware, TV picture mode, and HDMI path. The procedure is the gate for P8 (does the device apply a
fitted 8.4 composer) and for the default changes of P9 and P10.

### WS7 (Open)

Evaluation in displayed-picture units. (1) A histogram-domain display-mapping simulator in a tool
crate outside the workspace: per-frame PQ histograms and an L1 stream go through libplacebo's
spline, the SMPTE ST 2094-10 curve and the ITU-R BT.2390 EETF for 100-, 600- and 1000-nit panels,
and candidate metadata is compared with reference metadata as change in displayed luminance, clipped
highlight share and frame-to-frame steps. It needs a per-frame histogram dump from the analyzer
(integer counts, identical on CPU and CUDA). (2) ColorVideoVDP on rendered output of a few cuts.
Recomputed 2026-10-03 for a 1000-nit shot on a 1000-nit panel (change at 10 / 100 / 200 nits /
peak): spline, L1 max +75 codes −3.5 / −5.9 / −7.8 / −14.6%, +140 codes −6.5 / −10.4 / −13.6 /
−24.4%; BT.2390 EETF −2.5% / −4.5% at the peak and nothing below 200 nits; a shot whose L1 max stays
below the panel peak is unchanged; L1 max 100 codes low on a 600-nit panel brightens mid-tones by 5
to 11% and clips everything above it. The average sets mid-tone placement in both open curves, so an
average error is of the same order as a peak error. Limits to state with every result: open curves
are not a Dolby display; libplacebo reads only L1 max and avg and the source range (and treats L1
max as luminance), so trims, L1 min, L4 and L11 cannot be evaluated in the open; libplacebo's tone
mapping changed on 2026-09-30, so the version must be pinned; the development host's libplacebo runs
on software Vulkan (about 6 fps at 4K) and ffmpeg's filter has no target-peak option, so a 600-nit
render needs another front end; released ColorVideoVDP (0.5.7) runs, but the cross-display metric
adaptation needs an unreleased version and its repository declares no licence.

### WS8 (Open)

Item 5, real material per format, was done on 2026-10-05 (progress log). Test material the
2026-10-03 review found wanting. (1) The synthetic highlight pair writes its
highlights after the grain, so they carry none and the clean and grainy twins are identical on them;
all 2x2 highlights sit on even coordinates and fill exactly one chroma quad. Add grain on the
highlight and odd offsets. (2) The grainy twin of the clean/grainy pair comes from a different
master and conversion (median level up to 71 codes higher, raw maximum below the clean one on one
frame), so "clean is truth" holds only to about ±50 codes. Build a twin from the same master. (3) No
test cut contains a dissolve or a fade. Add synthetic cross-dissolves and fades built from real
shots (blended in PQ code and in linear light) with their intervals as truth, and retail cuts with
authored transitions. (4) Keep a held-out split: synthetic references and open implementations for
development, licensed reference output for scoring only ([`docs/PROVENANCE.md`](PROVENANCE.md)).

### FEL (Placeholder)

Profile 7 FEL. The BL+EL compositor and its re-encode were removed on 2026-10-03 because the
composed picture did not match the reconstruction in ETSI GS CCM 001 (progress log). `mkvdovi`
refuses Profile 7 FEL inputs, keeps the source, and continues with the next file. The only FEL work
of interest is what can be delivered without re-encoding: the base layer stays bit-exact and FEL
information is carried, as far as that is possible, in the Profile 8.1 RPU. Design, open questions
and what was learned are in [`docs/FEL_PLAN.md`](FEL_PLAN.md). This row replaces F1 (compositing),
F2 (FEL-discard mode) and R1 (metadata fit). Profile 7 MEL conversion is unchanged: the enhancement
layer is dropped and the video is copied (see P11 for `--mdfix`).

### R2 (Conditional)

Profile 5. Genuine Profile 5 uses the IPT signal representation, so HDR10 BL → Profile 5 requires
decoding, color transformation, and re-encoding. Feasible through an established encoder (Resolve
Studio, Dolby Encoding Engine); an open-source Profile 5 encoder in this project is substantial
separate work; changing profile flags on untouched HEVC is not a target. Precondition: a matched
HDR10 / P8.1 / correctly encoded P5 playback comparison from the same master with the playback chain
recorded.

### E8 (Open)

Histogram percentile reader. The 256-bin luma histogram is split into 64 bins below PQ(100 nits) and
192 above, but `compute_histogram_percentile_pq` and `find_highlight_knee_nits`
(`hdr_analyzer_mvp/src/analysis/histogram.rs`) convert a bin index back with bin/255: the lower edge
of bin 64 (100 nits) reads as 5.2 nits and a 1000-nit value as about 305 nits. It reaches the
`.bin`, the sidecar and the RPU only with `--peak-source histogram99/histogram999`, or with
`--peak-domain luma` and a non-conservative optimizer profile, where `histogram99` is the default.
`mkvdovi` never selects either, and `--peak-estimator percentile` uses the uniform 4096-bin
histogram, so default output is unaffected. Fix: one shared bin-edge function for writer and
readers, with a round-trip test.
