# Changelog

This document provides a historical record of completed milestones, feature implementations, and significant refactoring efforts for the `hdr-analyze` project.

---

## [Unreleased]

### Added

- **`fit_hlg_composer chroma-siting` and `scripts/validate_hlg_chroma_siting.sh`** (tooling, ROADMAP
  P8). They measure how the analyzer's 4:2:0 handling of the HLG Profile 8.4 decode differs from the
  spec composer (ETSI GS CCM 001 §5.4.2.3.3: MMR at chroma resolution on down-sampled luma, integer
  arithmetic) and from a renderer, on synthetic non-flat patterns and on real HLG cuts, with the
  tool anchored to the analyzer and to libplacebo on every run. Result: up to 18 codes of L1 max per
  scene on the HLG development cuts, so the analyzer decode changed (see Changed). No change
  to any shipped binary. Measurements: [`docs/HLG_COMPOSER.md`](docs/HLG_COMPOSER.md) §9.
- **Breaking: HLG output uses a composer fitted to BT.2100 by default: `--hlg-composer bt2100`**
  (`mkvdovi` and `hdr_analyzer_mvp`; `--hlg-composer preset` restores the previous output). The Profile 8.4 preset that `dovi_tool` writes
  decodes neutral greys with a blue tint (75% grey to R′G′B′ 2384/2387/2439 in 12-bit PQ codes) and
  nominal white to about 1150 nits. The new composer uses the same syntax, fitted to the BT.2100 /
  BT.2408 1000-nit HLG-to-PQ conversion with neutrals kept neutral: grey decodes to equal R′G′B′
  within 0.005 of a 12-bit code, black and sub-black to black, nominal white to 1000 nits, and libplacebo's render of the 52
  colour test patches is ΔE_ITP 11.8 from the reference on average, against 26.1 for the preset.
  The analyzer measures through the selected composer (CPU and CUDA, bit-identical), the sidecar
  names it (`dovi84-bt2100-v1`), and `mkvdovi` installs the same composer into the RPU that
  `dovi_tool generate` wrote. Whether playback devices apply a composer other than the preset is
  not yet confirmed by a playback test. HLG measurements and temp dirs from earlier versions carry
  the preset, so under the new default they are re-analyzed and regenerated. Design and measurements:
  [`docs/HLG_COMPOSER.md`](docs/HLG_COMPOSER.md).
  - New workspace library `dovi84_composer` (composer definitions, guarded RPU rewrite) and the
    fitter `tools/fit_hlg_composer` that generates its constants and reports both composers.
  - `scripts/validate_hlg_dv84.sh` and `validate_hlg_dv84_color.sh` take `--composer`, and the
    colour script reports ΔE_ITP against the BT.2100 reference.
- **`--verify` compares what the RPU delivers with what was measured** (`mkvdovi`). For a
  generated RPU, the RPU extracted from the muxed file is parsed in-process. Every frame must carry
  the expected source range (8.1: from the mastering display, not checked when the lookup is used;
  8.4: 62/3079). Every frame of every
  measured scene must carry the measured L1 after the documented `dovi_tool generate` limits
  (minimum at most 12 codes, maximum at least 2081, average at least 819 and below the maximum).
  Anything else fails. The limits it applied are reported: how many scenes per field, and the
  largest change in codes, with one line per scene under `--verbose`. Scenes whose L1 max lies above
  `source_max_pq` get an advisory. The RPU frame count used by the completeness checks now comes
  from that parse, not from the optional `dovi_tool info --summary` output. The Profile 7 MEL
  passthrough keeps the external checks only, because its RPU can carry levels the `dolby_vision`
  crate does not read.

### Fixed

- **`tools/l1_diff` lines references up with open-GOP cuts** (tooling, ROADMAP E11). It reads
  `source.leading_skipped_frames` from a v5 sidecar: a reference over the whole stream has its rows
  for the undecodable leading pictures skipped, and `--per-shot` and `--scenes` are re-based by the
  same offset. `--export-reference` labels rows with stream frames (unchanged without leading
  pictures). Stricter reference checks: frame labels must count up by one; a reference labelled in
  decoded frames refuses `--per-shot` and `--scenes` when the cut has leading pictures; a cut beyond
  the reference's frames is an error; any other count difference is still refused.

- **L1 lines up with the picture in sources cut at an open-GOP CRA picture** (`hdr_analyzer_mvp`,
  `mkvdovi`). A stream cut at a CRA picture (x265's default open GOP; `mkvmerge --split`,
  stream-copy cuts, some captures) keeps the RASL pictures that follow the CRA in decode order
  but come first in display order. They reference pictures from before the cut, so no decoder
  outputs them, but `dovi_tool inject-rpu` still gives RPU `n` to presentation picture `n`.
  Before this fix the measured L1 started on those RASL pictures. Every scene's L1 was shown as
  many frames early as there were RASL pictures, and the last frames repeated the final RPU.
  Measured on a real cut with two RASL pictures: displayed frame *k* carried the RPU for *k* + 2.
  - The analyzer counts the RASL pictures of the first IRAP picture from the packets' NAL types.
    It requires every picture of the stream to be either decoded or one of those. Any other loss,
    such as a stream that does not start at a random access picture, stops the analysis with an
    error instead of writing shifted measurements.
  - Sidecar version 5 records `source.stream_frames` and `source.leading_skipped_frames`.
  - `mkvdovi` moves every scene back by the leading count and lets the first scene cover the
    leading pictures, so the RPU has exactly one entry per picture.
  - `--verify` compares in the stream's frame numbers.
  - A sidecar older than version 5 is reused only when its frame count matches the input exactly.
  - When `dovi_tool inject-rpu` reports mismatched lengths for a measured RPU, the conversion
    stops instead of muxing it.
- **Profile 8.1 RPUs state the mastering display range** (`mkvdovi`). `extra.json` now passes
  `source_min_pq` / `source_max_pq`, converted from the mastering display luminance with the
  `dolby_vision` crate's PQ conversion (the one it uses for a Dolby CM XML). Before, `dovi_tool
  generate` derived them from a coarse L6 lookup. Masters that change:
  - a mastering peak other than 1000/2000/4000/10000 nits, which always got 3079 (1000 nits); for
    example 600 nits is now 2851 and 1100 nits 3121;
  - a mastering minimum other than 0.0001 or 0.005 nits; for example 0.001 nits was 7 and is now 26,
    and 0.05 nits was 0 and is now 189.

  The standard masters and the defaults used when the source states nothing (0.005 / 1000 nits)
  give the same values as before, so those RPUs are unchanged. Values that cannot describe a
  mastering display keep the lookup, with a warning: a peak outside 100–10000 nits (for example
  a mastering SEI written in the wrong units, which reads as 0.1 nit) or a minimum outside 0–1 nit.
  Profile 8.4 keeps the preset's 62/3079. A run interrupted under an earlier build regenerates its
  RPU on resume, because the configuration now carries the range.

### Removed

- **Breaking: Profile 7 FEL conversion is removed from `mkvdovi`.** The BL+EL compositor
  (`fel_composite.rs`) did not match the reconstruction in ETSI GS CCM 001 on real discs: on seven
  of eight Profile 7 FEL test cuts the composed chroma missed the specified prediction by a mean of
  69 to 534 10-bit codes. Removed with it:
  - the `composite-pipe` subcommand;
  - the flags `--fel-crf`, `--fel-preset`, `--fel-encoder`, `--fel-nvenc-preset` and `--encoder`;
  - the Modal remote-encode backend and all libx265, NVENC and VideoToolbox encoding. `mkvdovi` no
    longer encodes video: every remaining path copies the video stream bit-exactly;
  - the notes in `docs/experimental/`. What is still useful from them is in
    [`docs/FEL_PLAN.md`](docs/FEL_PLAN.md), the plan for FEL input that keeps the base layer
    bit-exact and re-encodes nothing.

### Changed

- **Breaking: HLG max-RGB is measured through the spec's 4:2:0 decode** (`hdr_analyzer_mvp`,
  `mkvdovi`; ROADMAP P8). The chroma curves of the Profile 8.4 composer now run once per chroma
  sample on down-sampled luma, as ETSI GS CCM 001 §5.4.2.3.3 specifies, in `code / 1023` float, and
  the composed chroma is upsampled bilinearly at the stream's chroma location (left or top-left;
  other locations warn and use left). CPU and CUDA stay bit-identical. On two HLG development cuts
  (Blue Lights, Wimbledon) scene L1 max moves by up to 19 codes (mostly down) and the max-RGB average by at most 1 code;
  luma statistics do not change. The sidecar names the new measurement `dovi84-v3` (preset) or
  `dovi84-bt2100-v1-spec420` (bt2100), version still 5; sidecars with the old names `dovi84-v2` and
  `dovi84-bt2100-v1` are re-analyzed with a "pre-spec 4:2:0" message. `mkvdovi` needs an
  `hdr_analyzer_mvp` from this release for every HLG composer, preset included: it looks for the
  mapping name in the analyzer's `--help` and refuses an older analyzer before any analysis. An
  older `mkvdovi` with this analyzer analyzes the file, then rejects the sidecar ("names a Dolby
  Vision 8.4 HLG mapping this mkvdovi does not know (dovi84-v3); it may come from a newer
  analyzer") and fails the file. `--dump-frame-stats` gains a last column, `avg_max_rgb_pq` (the
  frame's unrounded max-RGB average), on PQ runs too; PQ `.bin` files and sidecars are unchanged.
  Measurements: [`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md) (2026-10-06).
- **Breaking: HLG inputs that the 8.4 RPU cannot describe are refused**, including files earlier
  versions converted: tagged full range, a matrix other than BT.2020 non-constant luminance
  (BT.2020 constant luminance included), or primaries other than BT.2020, from MediaInfo or
  ffprobe tags. The check runs before any work and lists every
  source and value. Untagged fields are accepted with a warning that states the assumption
  (limited range, BT.2020 NCL). The analyzer now also warns on BT.2020 constant luminance.
- **`mkvdovi --verify` is stricter for HLG output**: a missing or unloadable L1 sidecar fails the
  verification instead of skipping its checks, and every RPU frame must carry the composer the
  sidecar names.
- **Profile 7 FEL input is refused**, with or without `--mdfix`. The file fails with an error
  before any temporary work, the source is kept, and a multi-file run continues with the next
  file. Profile 7 MEL, Profile 8, HDR10, HDR10+ and HLG inputs behave as before, and
  `mkvdovi inspect` still reports FEL.
- **`--hwaccel` in `mkvdovi` only selects GPU decode and analysis** in the spawned
  `hdr_analyzer_mvp`. It no longer selects an encoder.
- **Scene detection is rebuilt.** The old detector compared a smoothed histogram distance with a
  fixed threshold (0.3). On grainy or busy pictures the distance is above that threshold on
  about half of all frames, so a cut was placed whenever the 24-frame minimum scene length
  allowed one: 53 scenes where the authored shot list of a 35mm war-film cut has 10. Flashes
  were cut as well. Cuts are now chosen after analysis: a cut must separate the frames before
  it from the frames after it (a flash of up to 3 frames does not), and must exceed 16 times
  the local frame-to-frame level. Of two candidates closer than the minimum scene length the
  stronger one wins. On ten real-content cuts with authored retail shot lists (168 cuts) the
  analyzer now matches 142 with 13 extra cuts; before it matched 91 with 228 extra. Two cuts
  where the whole picture changes on every frame (a spinning capsule, a lightning storm) still
  miss most boundaries ([`docs/TECHNICAL_REFERENCE.md`](docs/TECHNICAL_REFERENCE.md) §1.2).
  - Defaults changed: `--scene-threshold` 0.3 → 3.0 (it is now a floor on the distance),
    `--min-scene-length` 24 → 12. `--scene-smoothing` is accepted and ignored.
  - Scene boundaries, and with them per-scene L1 and the final RPU, change for most sources.
    Per-frame measurements do not change. The sidecar layout and version are unchanged, so
    `mkvdovi` reuses existing measurement files with their old boundaries; delete them to
    re-analyze.
- **`--peak-estimator robust` runs on the GPU.** The CUDA kernel now gathers the cross-quad
  difference histogram the estimator needs, so `--hwaccel cuda` no longer falls back to CPU
  analysis for it. Output is identical to the CPU path (`.bin`, sidecar and frame statistics;
  checked by `scripts/cuda-parity.sh` in both peak domains for PQ and HLG, and on a 2,855-frame
  retail cut). Speed is the same as with the default estimator. That step did not change the
  estimator (the next entry does); it stays opt-in. `--pre-denoise median3` is still CPU-only.
- **`--peak-estimator robust` uses a new rule that keeps the flat highlights the old one
  lowered.** The old rule inferred the size of the brightest area from one tail count and
  subtracted the expected maximum of that many grain samples. It lowered real highlights: a
  flat 2x2 to 32x32 highlight and one- and three-frame flashes read 14 to 246 codes low, also
  on a clean clip. The new rule describes the pixels at the top of the 4096-bin PQ histogram
  by a centre and a width and removes the measured grain variance from the width:
  `peak = centre + (raw - centre) * sqrt(1 - sigma^2 / width^2)`, with the correction limited
  to 5.2 sigma. A top as narrow as the grain reads its centre, and a top much wider than the
  grain is picture and is nearly kept. A flat top is kept. A small group of pixels above a gap
  is kept exactly when it has one value, and reads its mean when it is no wider than sigma
  ([`docs/TECHNICAL_REFERENCE.md`](docs/TECHNICAL_REFERENCE.md) §2.4). It works on the same
  kernel statistics as before, so CPU and CUDA output stay identical (`scripts/cuda-parity.sh`),
  and the default `max` estimator is unchanged.
  Measured on 25 real-content cuts, 12-bit PQ codes, against the default `max`:
  - Flat, noise-free synthetic highlights about 1500 codes above the picture (2x2, 8x8, 32x32,
    one- and three-frame flashes, a moving 2x2) on a clean and a grainy clip: all 12 segments
    read +0.0.
  - Grainy clip against the raw maximum of its clean twin, per frame, on three pairs: bias
    +102.7 → +23.4, +41.0 → +17.0 and +83.3 → +50.5 (the last has a saturated, partly clipped
    highlight). Mean absolute error 108.4 → 60.3, 41.0 → 18.6 and 85.3 → 70.7. The low bias
    is partly over- and under-correction cancelling: on the first pair the largest error
    (251.3 → 248.6) is now an under-read, and 27% of the frames read more than 10 codes below
    the clean twin (27% on the third pair, 2% on the second). The clean twin's raw maximum
    carries some noise itself.
  - One heavy-grain retail cut against its embedded L1, per shot: bias +76.1 → +54.4,
    largest error 226.6 → 138.6.
  - Limits, all in [`docs/TECHNICAL_REFERENCE.md`](docs/TECHNICAL_REFERENCE.md) §2.4. Not
    every highlight is kept: one to three flat pixels, or a few pixels that carry grain, within
    a few sigma of the highest grain pixel are corrected as grain and read up to 5.2 sigma
    low. The rule also lowers clean digital content, by 3 to 21 codes per frame on average
    (36 on an HLG cut); on single frames by up to 332 codes, and by 343 on a grainy cut, which
    is the limit of the correction. No content floor is applied: on 4,259 of 23,519 frames
    the result lies below the frame's 99.99th percentile. Frame-to-frame variation inside
    shots is higher than with `max` on 24 of 25 cuts (10% on average, 38% at worst), and the
    distance of the shot peak from the shot's typical frame grows on 15 cuts and shrinks on 7.
    Against embedded L1, three cuts whose `max` bias was near zero or negative move further
    down (+1.1 → -47.6, -9.2 → -65.7, -90.1 → -139.4; largest error 98 → 246, 502 → 652,
    1499 → 1624); on all three the embedded L1 describes a composed two-layer picture, so that
    reference is approximate.
    Two constants of the rule were chosen on these cuts. The estimator stays opt-in.
- With `--peak-estimator max` or `percentile`, the CPU path no longer runs the grain
  estimator. The `robust_pq`, `sigma_pq`, `correction_pq` and `n_eff` columns of
  `--dump-frame-stats` are then neutral (raw maximum, 0, 0, 0), as they already were with
  `--hwaccel cuda`. `--peak-estimator robust` with `--downscale` 2 or 4 prints a warning: the
  rule is specified for full-resolution analysis.
- The CPU path computes the PQ max-RGB mix and each pixel's histogram bin in the kernel's f32
  arithmetic instead of f64. This removes rare one-bin differences between the backends. The
  default peak moves by less than 0.1 of a 12-bit code (measured on three real-content cuts
  whose peak changed at all; twenty others are unchanged).
- `--dump-frame-stats` also writes the scene-detection series (`scene_diff`, `scene_score`,
  `scene_baseline`, `scene_start`).
- `l1_diff --scenes` matches cuts one to one (two analyzer cuts can no longer both count as a
  match for one reference cut), prints the number of extra cuts, and no longer counts frame 0
  as a cut on either side.
- The synthetic L1 regression clip has shots of 61, 29, 31, 43, 41 and 37 frames instead of six
  shots of 24. Its scene reference now comes from the clip's construction, not from analyzer
  output. The old clip could not detect a detector that cuts every 24 frames.

### Fixed

- **The L1 average of a scene that changes over time was wrong.** Each frame's average passed a
  forward-only smoothing filter before the scene mean was taken, so the scene average leaned
  toward the first frames of the scene: a 24-frame fade from PQ code 168 to 2973 read 893 where
  the frame mean is 1569, and a one-frame flash leaked into the frames after it. The L1 sidecar
  now stores the unfiltered frame means (**sidecar version 4**, same layout), and `mkvdovi` writes
  the scene average from those. Scenes that do not change over time keep their average. On real clips only the L1 average in
  the final RPU moves; fireworks and other brightening or fading scenes move most
  ([`docs/VALIDATION.md`](docs/VALIDATION.md) §9). The madVR `.bin` is byte-identical to 0.5.1.
- `mkvdovi` reuses a version 1–3 sidecar with a warning that its averages were smoothed. Delete
  the measurements file to re-analyze.
- On resume, `mkvdovi` regenerates the RPU when the generation settings differ from the
  interrupted run (for example after a re-analysis changed L1). Before, an RPU that was already
  complete in the temp directory was reused whatever the measurements now said. A muxed output
  left by that run is rebuilt as well.

### Added

- **Measured MaxCLL/MaxFALL in L6.** The analyzer writes the content light levels (CTA-861.3,
  active image area, frame average in linear light) to the L1 sidecar as `light_level`, and
  `mkvdovi` uses them for a MaxCLL or MaxFALL the source does not state (or states as 0). This
  mainly affects HLG, which carries no light-level metadata and so far got the defaults
  1000 / 400. Source-stated values are unchanged. MaxCLL needs full-resolution analysis and both
  need every frame analyzed; otherwise the defaults stay
  ([`docs/FORMAT_COMPATIBILITY.md`](docs/FORMAT_COMPATIBILITY.md)). The block is an optional
  addition to sidecar version 4.
- **L1 regression gate in CI.** `scripts/ci/l1-regression-gate.sh` generates a six-shot synthetic
  clip (grain, saturated colour, small specular, ramp, fade, one-frame flash), analyzes it as PQ
  and as HLG, and scores the result against the references in `tools/l1_diff/corpus`. A change
  that moves L1 fails the job until the references are rewritten with `--update` and reviewed.
- **`tools/l1_diff` can fail.** `--max-peak-bias`, `--max-peak-error`, `--max-min-bias`,
  `--max-min-error`, `--max-avg-bias`, `--max-avg-error` (12-bit PQ codes; the average limits
  apply to the max-RGB average) and `--max-scene-mismatches` list every breach and return a
  nonzero exit status. Before, the tool only printed statistics. `--export-reference` writes an
  analyzer run as a reference CSV; the reference peak column may carry decimals. CI now lints and
  tests the tool, which is outside the workspace.
- **CPU/CUDA parity check.** `scripts/cuda-parity.sh` encodes the synthetic clip as PQ and HLG
  HEVC, analyzes each with and without CUDA, and fails unless the measurement files are
  byte-identical, the sidecars agree and the CUDA run reports `gpu: true`. Hosted CI has no GPU,
  so run it on a CUDA host before a change to the analysis or decode path
  (`hdr_analyzer_mvp/tests/cuda_parity.rs`, skipped without its environment variables).
- **Final-RPU baseline.** `scripts/rpu-baseline.sh capture` converts inputs with
  `mkvdovi --keep-source --verify` and stores the RPU extracted from the muxed file with a
  manifest of input identity and tool versions. `compare` fails on any difference outside
  Level 1 and reports Level 1 differences as numbers (`--require-identical-l1` makes them fail).

## [0.5.1] - 2026-10-01

### Performance

- **CUDA analysis is about 2.5–3.9× faster end to end.** On an RTX 4070 with 4K sources, frames ÷
  wall time went from 132 to about 325 fps (HDR10) and from 125 to about 490 fps (HLG), and the analyzer's CPU
  use fell from about one full core to under two thirds of one. L1 output is identical to 0.5.0.
  Measurements and conditions are in [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md).
  - NVDEC frames are analyzed where the decoder leaves them. Before, every frame was downloaded to
    host memory and uploaded again (~4.5 ms per 4K frame). The decoder and the analyzer now share
    the device's primary CUDA context; a frame is downloaded only for scene-cut crop sampling,
    fallback crop detection or CPU analysis. 8-bit and 12-bit sources fall back to CPU analysis as
    before; frames from another CUDA context keep the download path. `HDR_ANALYZER_CUDA_HOST_FRAMES=1` forces it for comparisons.
  - The analysis kernel takes 0.18 ms per 4K frame instead of 2.43 ms (HDR10; HLG 0.33 ms instead of
    2.55 ms): a grid-stride loop with warp reductions replaces up to 32,400 blocks per 4K frame that each cleared and
    flushed 4,383 shared histogram bins, and the fixed-point sums use exact f32 instead of f64.
  - The crop probe decodes on all cores. It used libavcodec's default of one thread, which cost up
    to a minute per run on long-GOP 4K sources. At the end of the file it now drains the decoder,
    so short clips keep the frames a decoder holds back.
  - **Behaviour change:** a failed CUDA call while decoding on NVDEC now stops the analysis with an
    error instead of falling back, because the decoder shares the CUDA context and its later frames
    can no longer be trusted. Rerun with `--hwaccel none`.
  - How it works and how it was measured: [`docs/CUDA_PIPELINE.md`](docs/CUDA_PIPELINE.md).

### Fixed

- **`mkvdovi --verify` could hang forever on long files.** The helper that runs short external tools
  kept their output pipes open without reading them, so a tool that printed more than 64 KB blocked.
  The verifier hit this on a 1,263-scene episode. The same bug left those tools' log files empty;
  they now contain the tools' output.
- **Release binaries no longer depend on the build machine's CPU.** `.cargo/config.toml` sets
  `-C target-cpu=native` for local builds, and the release workflow did not override it, so the
  published binaries (releases up to 0.5.0) were compiled for whatever CPU the GitHub runner had and
  could stop with an illegal-instruction error on a different CPU. The release and CI workflows
  now build for the default baseline CPU. Source builds are unchanged.

## [0.5.0] - 2026-09-30

### Changed

- **HLG now converts to Dolby Vision Profile 8.4 without re-encoding.** `mkvdovi` keeps the HLG
  base layer bit-exact and injects a Profile 8.4 RPU, following the same path as HDR10: analyze
  (CUDA when available), generate, inject, mux. The old path re-encoded to PQ for Profile 8.1, and
  its L1 did not match its own output: the analyzer applied the HLG inverse OETF without the BT.2100
  OOTF, so a 75% HLG signal read 265 nits in L1 but 204 nits in the `zscale`-converted base layer,
  and `zscale` ignored `--hlg-peak-nits`.
- **The analyzer measures HLG through the Dolby Vision 8.4 luma reshaping curve** (the `dolby_vision`
  crate's `Profile84` preset, which `dovi_tool` embeds), clamped to the RPU's source range of PQ
  codes 62–3079. CPU and CUDA share one 1024-entry lookup table. On a lossless grey ramp the curve
  agrees with libplacebo's Dolby Vision renderer to within 3.2 twelve-bit PQ codes.
- **HLG max-RGB is measured on the full Dolby Vision 8.4 decode.** Each pixel goes through the
  luma curve, the preset's two order-3 chroma MMR curves and the RPU's YCbCr-to-RGB matrix, and the
  peak is max(R′, G′, B′), clamped to the same source range (PQ codes 62–3079). CPU and CUDA
  results are bit-identical. On 52 lossless flat colour patches (six primaries and secondaries at
  100% and 75% saturation plus grey, four HLG levels) the decode agrees with libplacebo's Dolby
  Vision render to within 0.59 twelve-bit PQ codes on both paths
  (`scripts/validate_hlg_dv84_color.sh`).
- **Behaviour change: HLG now defaults to `--peak-domain max-rgb`**, like PQ. Earlier versions forced
  luma for HLG. `--peak-domain luma` still selects the luma curve alone. Neutral HLG content reads
  about 2% higher in max-RGB than in luma, because the preset's chroma MMR tints neutrals slightly
  blue (grey code 721: luma 2389, max-RGB 2439; libplacebo 2439.1). Saturated highlights can read
  higher than under the luma domain.
- **L1 sidecar version 3** adds `analysis.luminance_mapping`: `"pq"`, or for HLG `"dovi84-v2"`
  (full 8.4 decode, written for every HLG run in either peak domain) or `"dovi84-v1"` (luma-only
  HLG measurements from pre-release builds). `mkvdovi` and `tools/l1_diff` accept
  versions 1–3. HLG inputs require a version 3 sidecar with `dovi84-v2`, so HLG measurements from
  earlier versions, including luma-only `dovi84-v1`, are re-analyzed and never reused.
- **The analyzer takes the transfer function from the first decoded frame** when the stream-level
  tag is not PQ or HLG. Broadcast HLG (for example BBC iPlayer) signals BT.2020 10-bit in the VUI and
  HLG in the alternative transfer characteristics SEI, and some MKVs carry HLG only in the container
  Colour element. The analyzer refused the first case as SDR and measured the second as PQ. FFmpeg
  applies the SEI to decoded frames, so that case now analyzes as HLG. New `hdr_analyzer_mvp
  --transfer <auto|pq|hlg>` forces the transfer; `mkvdovi` passes `--transfer hlg` for inputs it
  classified as HLG, because older FFmpeg libraries (6.1) drop an HLG tag held only in the container.
- When HLG is tagged only in the MKV colour element (not in the HEVC VUI or SEI), `mkvdovi` writes
  the HLG transfer into the base layer's VUI with FFmpeg's `hevc_metadata` filter, a lossless header
  edit. Without it mkvmerge gives the track Dolby Vision compatibility ID 2 (SDR). `--verify` now
  fails an HLG conversion whose output compatibility ID is not 4.
- `mkvdovi` refuses `--mdfix` on a Profile 8.4 (HLG base layer) input and `--legacy-madvr-l1` on
  HLG input. Leftover temp directories from the old HLG→PQ path are discarded instead of resumed.
  Interrupted 0.4.0 conversions start clean, because the resume fingerprint records the mkvdovi
  version.

### Removed

- `mkvdovi --hlg-crf`, `--hlg-preset` and `--hlg-peak-nits`, and `hdr_analyzer_mvp
  --hlg-peak-nits`. The 8.4 RPU fixes the HLG mapping, and nothing is re-encoded. `--encoder` now
  applies to Profile 7 FEL re-encodes only.

---

## [0.4.0] - 2026-09-15

### Removed

- **Duplicate vendored `dovi_tool` README** (`docs/dovitool.README.md`). Third-party reference docs
  are kept local-only per `.gitignore` policy; nothing in the repo linked to the tracked copy.
- **The transitional `mkvdolby` binary alias is no longer shipped.** Release archives contained a
  `mkvdolby` copy of the converter for one release after the v0.3.0 rename; that compatibility
  window has closed, so the Linux/macOS and Windows archive steps now ship `mkvdovi` only. Scripts
  still invoking `mkvdolby` must be updated. Recognition of leftover `mkvdolby_temp_*` resume
  directories is **retained** — those are on-disk state belonging to users mid-conversion, not a
  distributed product name, and removing it would strand an interrupted run.

### Fixed

- **Windows installer.** `install.ps1` now copies the FFmpeg DLLs and license bundled in the zip,
  so the installed analyzer can start. It also no longer uses a three-argument `Join-Path`, which
  Windows PowerShell 5.1 rejects. Not yet tested on Windows; the documentation recommends the zip
  until it is.
- **FEL compositing chroma correspondence.** Chroma MMR reshaping previously read luma at a flat
  `i*4` index, which is not the 2D co-located position, and substituted neutral Cr when reshaping
  Cb. It now averages the co-located 2×2 luma block and reads the real opposite chroma plane.
  Covered by unit tests only; no FEL sample or independent reference comparison was available, so
  the FEL path remains experimental.
- **Analyzer transfer classification.** The BT.2020 10/12-bit transfer tags share the BT.709 SDR
  curve and are no longer treated as PQ; the analyzer refuses every tagged non-HDR transfer.
- **Metadata-removal step safety fix:** `extract_clean_base_layer` now deletes only its own intermediate
  temporary raw stream (`DV_raw.hevc`) and completion sentinel rather than the input path, preventing any
  possibility of deleting the source MKV in direct input mode.

### Changed

- **Release archives include the documentation.** `docs/`, `CONTRIBUTING.md` and `ROADMAP.md`
  ship next to `README.md`, so its relative links work inside an extracted archive.
- **No silent optimizer-target fallback.** When existing measurements have a missing, unreadable,
  unknown-version, structurally invalid, or mismatched L1 sidecar, `mkvdovi` warns and re-runs the
  analyzer instead of generating L1 through `dovi_tool --madvr-file --use-custom-targets`. That
  path, where L1 max follows optimizer targets and L1 avg is a placeholder, now requires
  `--legacy-madvr-l1`. Every measurements candidate is tried in turn, the analyzer's own
  `<stem>_measurements.bin` first, so a stale shared `measurements.bin` cannot block reuse. The
  sidecar frame count may differ from MediaInfo's input count by 0.1 % (at least 2 frames),
  because MediaInfo estimates it from duration for MKVs without statistics tags. Scenes whose
  average exceeds the peak, which `--peak-domain luma` or a percentile/robust estimator can
  produce, print a warning instead of rejecting the measurements.
- **L1 sidecar version 2.** The analyzer records its version, input identity, sampling settings, GPU
  use, and the committed crop in full-resolution coordinates. `mkvdovi` and `tools/l1_diff` accept
  versions 1 and 2.
- **Resume is bound to the input and settings.** A leftover temp directory is resumed only when its
  `resume.json` fingerprint matches the input name, size, and mtime, the mkvdovi version, and the
  artifact-affecting settings. Otherwise it is discarded with a warning. Temp directories from
  earlier versions, including `mkvdolby_temp_*`, have no fingerprint; they resume with a warning
  instead of being discarded, so an interrupted FEL composite survives the upgrade.
- **Input contract warnings.** The analyzer warns when a stream is tagged full range or with a
  non-BT.2020 matrix, because it assumes limited-range BT.2020 NCL samples.
- **CI/release FFmpeg setup simplified.** One composite action (`.github/actions/setup-ffmpeg`)
  replaces six copy-pasted per-OS install blocks. Windows uses a checksum-verified prebuilt LGPL
  FFmpeg (BtbN) instead of compiling it from source with vcpkg on every run, and tools already on
  the runner images (LLVM, pkg-config, build-essential) are no longer reinstalled. The Windows
  release zip now **bundles the LGPL FFmpeg DLLs and licence**, so the analyzer runs without a
  separate FFmpeg install; CI smoke-tests that bundle on every Windows build.
- **`ffmpeg-next` 8.0 → 9.0** with only the `codec`/`format`/`software-scaling` features. Fixes
  macOS builds against Homebrew's FFmpeg 9 and stops linking the unused avfilter, avdevice and
  swresample libraries. FFmpeg 3.4 through 9.x remain supported.
- **Updated `dolby_vision` crate from 3.3 to 3.4**: incorporates the L11 byte 1 parsing fix, benefiting in-process inspection (`inspect`) and `--mdfix`.
- **Progress and stall detection for `dovi_tool` operations**: `remove`, `convert`, and `demux` now display live byte progress with `--stall-timeout` monitoring.
- **Trademark and provenance hygiene across the documentation.** Removed the promotional
  "only open-source HDR10 → Dolby Vision pipeline" claim from the README hero and replaced
  brand-led feature labels with format-neutral descriptions ("Profile 8.1", "CM v4.0 metadata",
  "RPU metadata"), keeping trademark references nominative and descriptive. Added a
  non-affiliation disclaimer beside the first prominent use, and corrected the trademark
  attribution to Dolby Laboratories Licensing Corporation. Renamed `docs/DOLBY_VISION.md` to
  [`docs/FORMAT_COMPATIBILITY.md`](docs/FORMAT_COMPATIBILITY.md) (all links updated).
- **[`docs/PROVENANCE.md`](docs/PROVENANCE.md) rewritten as auditable facts rather than legal
  conclusions.** Retitled "Implementation Provenance and Source-Material Boundary"; replaced
  absolute claims ("clean", "public standards only", "enforced by review") with scoped,
  knowledge-qualified statements; strengthened the patent disclaimer to state that no
  freedom-to-operate review has been performed and that independent development does not
  establish non-infringement; and recorded a dated review of the licensed-tool EULA (v5.6.4,
  reviewed 2026-07-31): no benchmark or publication restriction exists, so published validation
  material is limited to derived statistics computed by this project, never the tool's
  documentation or raw output. Broadened `.gitignore` so licensee-confidential Dolby
  documentation cannot be committed from anywhere in the tree.

### Added

- **L5 active-area metadata from the committed crop.** HDR10/HLG conversions emit L5 offsets that
  describe the same active area the measurements used. Sampled source L5 keeps precedence for Dolby
  Vision inputs; full-frame content keeps the `dovi_tool` zero default.
- **Frame-coverage verification.** `--verify` compares the RPU frame count with the muxed video
  track and the L1 sidecar, and fails on a mismatch.
- **Measurement provenance output.** Reused measurements print their provenance and warn when they
  were analyzed more coarsely than the resolved `--analysis-quality`.
- **`Cargo.lock` is tracked** for reproducible application builds.
- **Direct MKV input to `dovi_tool`** (`--dovi-input auto|raw|mkv`): when `dovi_tool` 2.3.4+ is detected,
  `mkvdovi` feeds the MKV container directly to `remove`, `convert`, and `demux` subcommands instead of
  extracting a full-size intermediate raw HEVC stream with ffmpeg. Saves substantial disk space and
  runtime by skipping the HEVC extraction pass on Profile 7 MEL → 8.1 discard, `--mdfix`, and Profile 7
  FEL compositing paths. Any direct read failure automatically falls back to ffmpeg extraction with
  advisory logging (`*_mkv.log`).
- **Zero-config hardware auto-detection in `mkvdovi`**: `--hwaccel` now defaults to `auto`,
  which probes for an NVIDIA GPU (`nvidia-smi`, including the WSL2 fallback path) once at startup
  and resolves to `cuda` or `none`. `--analysis-quality` now defaults to `auto`, resolving to
  `accurate` (full-resolution, every-frame) when GPU analysis is actually available — detected via
  the spawned `hdr_analyzer_mvp --version` advertising `+cuda` — and `balanced` otherwise.
  NVENC use for FEL/HLG re-encodes is guarded by an ffmpeg `hevc_nvenc` capability probe with a
  warn-and-fall-back-to-libx265 path instead of a mid-encode failure. Explicit
  `--hwaccel none|cuda` and quality values bypass detection entirely.
- **CUDA-accelerated analysis backend** for `hdr_analyzer_mvp` (opt-in `cuda` cargo feature,
  activated at runtime with `--hwaccel cuda`). NVDEC hardware decode via a proper FFmpeg CUDA
  `AVHWDeviceContext` (with `hevc_cuvid` and software fallbacks) feeds a single-launch
  NVRTC-compiled kernel that computes the v5 luminance histogram, hue histogram, 4096-bin
  peak-domain PQ histogram, max-RGB peaks, and exact per-pixel means on full-resolution frames
  with a sampling stride — no swscale, and only a few KB of results downloaded per frame.
  Validated bit-identical L1 measurements, scene cuts, and MaxCLL against the CPU path;
  ~12× analysis throughput measured on an RTX 4070 (17 → 213 fps at 4K). `--pre-denoise median3`
  and `--peak-estimator robust` remain CPU-only; all failure modes fall back to CPU analysis.
  `mkvdovi --hwaccel cuda` forwards the flag to the analyzer it spawns. No CUDA toolchain is
  needed at build time (driver + NVRTC at runtime only).
- Added Profile 7 FEL to Profile 8.1 conversion in `mkvdovi`: BL+EL compositing applies polynomial
  luma reshaping, MMR chroma reshaping, and NLQ LinearDeadzone residuals before local or Modal-backed
  HEVC re-encoding and fresh RPU generation.
- Added `mkvdovi inspect <file>` for full RPU L1 diagnostics and automatic multi-window preflight
  warnings during Dolby Vision conversion.
- Added `--mdfix` for Profile 7 MEL and Profile 8 inputs. It removes the existing RPU, measures the
  clean base layer, preserves sampled L5 offsets when available, and writes a distinct
  `*.mdfix.DV.mkv` candidate without re-encoding the picture.
- Profile 7 MEL now has a fast `dovi_tool -m 2 convert --discard` path when metadata rebuilding is
  not requested; Profile 8 inputs are skipped unless inspected or repaired.

### Changed (behavioral)

- **Added opt-in grain-robust max-RGB peak estimation.** `--peak-estimator
  <max|percentile|robust>` selects the direct maximum (still the default), a configurable fine
  percentile, or a synthetic-calibrated Gaussian extreme-value correction. `--peak-percentile`
  defaults to P99.99, and `--dump-frame-stats <PATH>` writes selected/raw/percentile/robust peak,
  measured sigma, correction, and effective-tail diagnostics. Deterministic additive-luma, chroma,
  and multiplicative-linear grain fixtures cover σ10 1/2/4/8 plus exact σ=0 behavior. The first
  two-title cm-v2 gate reduced per-shot bias but missed its acceptance envelope, so robust mode did
  not replace the default; see `docs/VALIDATION.md` §7 Finding 5.
- The shared fine PQ histogram now has 4096 bins, improving active-area minimum quantization from
  the former 10-bit grid while also supporting peak-estimator diagnostics. Sidecar version 1 gains
  additive `peak_estimator` and `peak_percentile` metadata.
- **L1 average now uses a true per-pixel PQ mean.** The frame analyzer accumulates full-precision
  Y-luma and max-RGB means in its parallel pixel pass; scene-aware smoothing operates directly on the
  measured Y mean instead of reconstructing it from 256 histogram bin centers.
- **Added explicit noise-rejected L1 minimum measurements.** `--min-percentile <percent>` defaults to
  P0.1 and `0` selects the absolute minimum. Every analyzer run writes `<output>.l1.json` with the
  crop/denoise configuration plus per-frame and per-scene 12-bit PQ min/avg/max measurements.
- `tools/l1_diff` now reads the sidecar and scores minimum plus Y-luma and max-RGB average domains
  against reference L1 CSV data. Historical `.bin` files without sidecars retain peak and embedded
  average scoring; minimum and max-RGB average are reported unavailable. The sidecar minimum is
  measurement-only and is not wired into RPU generation yet.
- Y-luma and max-RGB means now receive identical temporal smoothing before sidecar serialization.
  Fine minimum histograms are reused per Rayon fold partition instead of allocated per chroma row.
- **Active-area crop detection now uses a multi-frame probe.** `hdr_analyzer_mvp` samples
  `--crop-probes <N>` frames (default 7) across the middle 70% of seekable inputs, rejects
  black/low-signal frames, and commits a tolerance-clustered conservative crop before analysis.
  `--crop-probes 0` selects the hardened in-stream fallback; `--no-crop` remains unchanged.
- Scene cuts now provide reporting-only crop-stability telemetry. Variable-active-area titles use
  the union of observed probe modes so full-frame picture is not cut; per-scene crop application
  remains a follow-up to preserve measurement continuity.
- Dolby Vision inputs and all `--mdfix` runs keep their source by default. New FEL/repair artifacts
  use the v0.3 resume sentinels and live progress reporting; the legacy `mkvdolby_temp_*` resume
  compatibility remains intact for one release.

### Documentation

- **Metadata-source and pipeline corrections.** The installation guide now says MediaInfo supplies
  the L6 and L9 source values and that an `ffprobe`-only setup falls back to defaults. The format
  guide states that HLG is measured before the PQ re-encode, and the CLI reference states that the
  sidecar's per-scene minimum is written to L1.
- The README is rewritten around the HDR10 and HDR10+ to Profile 8.1 workflow, with a compatibility
  table that says which paths re-encode the picture.
- Added [`docs/INSTALLATION.md`](docs/INSTALLATION.md) for release binaries, runtime tools, source
  builds and the CUDA build, and [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md) for the recorded
  benchmark and its unrecorded fields.
- Moved the FEL research notes and the Modal offload notes to
  [`docs/experimental/`](docs/experimental/README.md). None of it is a release feature.
- Removed the vendored `hdr10plus_tool` README (`docs/hdr10plus_tool_README.md`), in line with the
  local-only policy for third-party reference docs.

---

## [0.3.0] - 2026-07-06

### Renamed (breaking)
- **The converter binary `mkvdolby` is now `mkvdovi`** (trademark hygiene: no product name
  embeds the Dolby mark, matching community convention — cf. `dovi_tool`, `libdovi`).
  Transitional support for exactly one release: release archives include a `mkvdolby` copy of
  the binary, resume recognizes leftover `mkvdolby_temp_*` directories, and
  `mkvdovi_hifi_workflow.sh` (renamed from `mkvdolby_hifi_workflow.sh`) still honors the
  `MKVDOLBY_BIN` environment variable.

### Legal & provenance
- Added [`docs/PROVENANCE.md`](docs/PROVENANCE.md): clean-room statement, the public standards
  every piece of domain knowledge derives from, the strict no-leaked-tools policy, and the
  honest patent/trademark framing.
- Fixed placeholder repository URLs in `hdr_analyzer_mvp`'s crate metadata; brought
  `CITATION.cff` up to the current version.

### Changed (behavioral)
- **PQ direct peaks now default to max-RGB.** `hdr_analyzer_mvp` decodes limited-range BT.2020 NCL
  R′G′B′ and stores the maximum channel in `peak_pq_2020`; pass `--peak-domain luma` for the legacy
  Y′ peak. The implicit peak source is also `max` in max-RGB domain, so balanced/aggressive histogram
  smoothing does not replace it with a Y percentile; histogram peak sources remain explicit opt-ins.
  HLG continues to use luma. Synthetic and `cm_analyze` scoring are documented in
  [`docs/VALIDATION.md` §2](docs/VALIDATION.md#2-the-definitional-gap-y-luma-peak-vs-max-rgb-maxscl).
- madVR v6 DCI-P3/BT.709 peaks remain approximated; true per-gamut transforms and HLG max-RGB are
  follow-ups enabled by the new RGB measurement path.

### Reliability & observability (mkvdovi)
- Long file-producing steps (base-layer extract, RPU inject, mux, HLG→PQ encode) now show a
  live **byte-progress bar with throughput and ETA** instead of a bare elapsed spinner, so a
  slow-but-working step is distinguishable from a stalled one. Child output is streamed to the
  step log during the run, surfacing tool warnings as they happen.
- Added a **stall warning**: `--stall-timeout <SECS>` (default 300, `0` disables) flags when the
  current step's output file stops growing — telling a hung tool apart from merely slow storage.
- Added **automatic resume**: an interrupted conversion preserves its `mkvdovi_temp_*` directory,
  and a re-run reuses every completed step (analysis, RPU, extracted base layer, …) via
  `<artifact>.done` completion sentinels. `--no-resume` forces a clean re-run.
- Added **graceful interrupt handling**: `SIGINT`/`SIGTERM`/`SIGHUP` (e.g. a dropped SSH session)
  print a resume hint and exit without deleting partial work. Documented running long conversions
  under `tmux`/`nohup`.

---

## [0.2.0] - 2026-05-31

Quality and observability release for native HDR10, HDR10+, and HLG to Dolby
Vision conversion.

### Highlights
- Corrected HDR10+ peak-source defaults and metadata generation for balanced
  Dolby Vision Profile 8.1 output.
- Added `mkvdolby --analysis-quality <fast|balanced|accurate>` with a new
  balanced default that analyzes every frame at half resolution.
- Added warnings when L6 metadata or L9 source primaries require fallbacks.
- Added advisory warnings when selected HDR10+ scene peaks exceed three times
  the mastering-display peak. Outliers are never clamped silently.
- Hardened `mkvdolby --verify`: installed tools are resolved from `PATH`, and
  post-mux RPU frame JSON is checked for Profile 8, ordered L1 values, sane L6,
  and required CM v4.0 L9/L11/L254 blocks.

### Documentation
- Clarified that generated L2 blocks are neutral compatibility trims, not panel
  calibration controls.
- Clarified that authored L8 creative trims remain outside the default
  conversion workflow.
- Documented the specialist scope of `scripts/mkvdolby_hifi_workflow.sh`.
- Fixed release archive naming so the one-line installers fetch the uploaded
  versioned assets, and included the specialist helper in Unix archives.

---

## [0.1.0] - 2026-01-23

First public release of the HDR-Analyze suite.

### Highlights
- Complete HDR10/PQ analysis engine with madVR v5/v6 compatible output
- Dolby Vision Content Mapping v4.0 metadata generation (mkvdolby)
- Measurement file verification tool (verifier)
- Cross-platform support: Linux, macOS (Intel + Apple Silicon), Windows

### What's Included
- **hdr_analyzer_mvp**: Core HDR10 frame analysis with scene detection, noise-robust peak detection, and dynamic target nits optimization
- **mkvdolby**: MKV to Dolby Vision Profile 8.1 conversion with CM v4.0 metadata (L1/L2/L6/L9/L11)
- **verifier**: madVR measurement file validation tool

---

## Pre-Release Development History

> The milestones below document internal development prior to the first public release.
> They are not SemVer versions.

### Milestone 5: Dolby Vision CM v4.0 & Toolchain Upgrade

- **Dolby Vision Content Mapping v4.0**: Full CM v4.0 implementation in mkvdolby.
  - Added `--cm-version` flag with `v29` (legacy) and `v40` (default) options.
  - Added `--content-type` flag for L11 metadata (film, live, animation, cinema, gaming, graphics).
  - Added `--reference-mode` flag for L11 reference viewing environment hint.
  - Added `--source-primaries` flag with auto-detection from MediaInfo (BT.2020/P3/709).
  - Generate L2 trim parameters for 100/600/1000 nit target displays.
  - Generate L9 (source primaries) and L11 (content type, reference mode) metadata blocks.
  - All metadata written to `extra.json` for `dovi_tool generate`.
- **Rust Toolchain Upgrade**: Upgraded from pinned Rust 1.82.0 to stable channel (1.93.0).
  - Enables latest dependency updates (e.g., madvr_parse 1.0.3 with Rust 2024 edition).
  - Changed `rust-toolchain.toml` to use `channel = "stable"` instead of fixed version.
- **Test Infrastructure**: Fixed deprecated `cargo_bin` usage in integration tests.

### Milestone 4: Performance & Quality Enhancements

- **PQ Noise Robustness**: Implemented a suite of features to improve measurement stability on noisy or grainy content.
  - Added `--peak-source` flag with `max`, `histogram99`, and `histogram999` options for robust peak detection. `histogram99` is now the default for balanced/aggressive profiles.
  - Implemented per-bin EMA histogram smoothing (`--hist-bin-ema-beta`) with scene-aware resets to prevent cross-scene contamination.
  - Added optional temporal median filtering (`--hist-temporal-median`) for histograms.
  - Added an optional Y-plane `median3` pre-analysis denoiser (`--pre-denoise`).
- **Future-aware Target-Nits Smoothing**: Implemented bidirectional EMA smoothing with per-scene resets and delta caps to reduce flicker and pumping in `target_nits`. This is now the default smoothing strategy.
- **Performance & Parallelization**:
  - Parallelized histogram accumulation using `rayon` to improve throughput on multi-core systems.
  - Added `--analysis-threads` flag to control worker count.
  - Added `--profile-performance` flag to emit per-stage throughput metrics for performance analysis.

### Milestone 3: Advanced Optimization & Format Support

- **Scene-Aware Optimizer**: Enhanced the optimizer with configurable profiles (`conservative`, `balanced`, `aggressive`) and a dynamic clipping heuristic that uses per-scene knee smoothing to prevent banding.
- **Hue Histogram**: Implemented a real 31-bin chroma-derived hue histogram from the U/V planes, replacing the previous zeroed-out placeholder. The verifier was also extended to validate its distribution.
- **madVR v6 Gamut Peaks**: Replaced the simple duplication of BT.2020 peaks with a gamut-aware approximation for DCI-P3 (99%) and BT.709 (95%) peaks.

### Milestone R: Codebase Modularization

- **Refactored `main.rs`**: Successfully refactored the monolithic `main.rs` file (originally ~1860 lines) into a modular structure with a thin (63-line) entry point.
- **Created Modules**: Logic was separated into distinct modules with single responsibilities:
  - `cli.rs`: Command-line interface definition.
  - `ffmpeg_io.rs`: FFmpeg initialization and I/O.
  - `pipeline.rs`: Main orchestration logic.
  - `writer.rs`: madVR measurement file writing.
  - `analysis/`: Modules for frame, scene, histogram, and HLG analysis.
- **Preserved Behavior**: All unit tests were migrated and passed, ensuring behavior was preserved post-refactor.

### Milestone 2: Core Accuracy and Stabilization

- **Baseline & Harness**: Established a baseline test pack and created the `tools/compare_baseline` harness for regression testing.
- **Core Analysis Features**:
  - Implemented robust active-area (black bar) detection and cropping.
  - Ensured correct v5 histogram semantics and limited-range normalization.
  - Implemented a native histogram-distance scene detection algorithm with threshold and smoothing controls.
- **CLI & Usability**:
  - Added support for both positional and flag-based (`-i/--input`) input.
  - Enhanced the `verifier` tool with additional checks for FALL metrics and data consistency.

### Milestone 1: Initial Implementation

- **Native FFmpeg Pipeline**: Initial version of the tool using `ffmpeg-next` for a native Rust video processing pipeline.
- **madVR v5/v6 Output**: Core support for writing madVR-compatible `.bin` measurement files.
- **Basic Optimizer**: First implementation of the dynamic target nits optimizer.
