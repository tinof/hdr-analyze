# mkvdovi pipeline internals

Reference for changing `mkvdovi`. `CLAUDE.md` keeps the rules in one line each and points here for
the reasons and the code paths. What users see: [FORMAT_COMPATIBILITY.md](FORMAT_COMPATIBILITY.md)
(accepted, refused and converted inputs; the L1 sidecar schema per version) and
[CLI_REFERENCE.md](CLI_REFERENCE.md) (flags).

## Code map

- `mkvdovi/src/main.rs`: file discovery and sorting, early `inspect` dispatch, per-file
  orchestration through `pipeline::convert_file`.
- `rpu_check.rs`: MEL/FEL/P8 classification and RPU diagnostics; it is what detects FEL.
- `external.rs`: tool checks and invocation.
- `metadata/`: `CmV40Config`, L2/L5/L9/L11 generation; submodules `format`, `probe`, `rpu_config`,
  `sidecar`, `static_metadata`.
- `pipeline/`: `convert_file` in `mod.rs`; by phase `temp_dir`, `measure`, `config`, `deliver`;
  plus `analyzer`, `dovi_steps`, `hdr10plus`.
- `verify.rs`, `progress.rs`.

## External tools

- `external::check_dependencies` checks the tools at runtime: `ffmpeg`, `mkvmerge`, `dovi_tool`, and
  either `mediainfo` or `ffprobe`. HDR10+ processing also runs `hdr10plus_tool`; keep it in `PATH`
  for HDR10+ inputs. `inspect` dispatches before the dependency checks.
- `dovi_tool` 2.3.2 is the floor: the existing orchestration relies on its `inject-rpu` padding
  fix. 2.3.4+ lets mkvdovi pass the MKV directly to remove/convert/demux
  (`--dovi-input auto|raw|mkv`; auto = on when `dovi_tool --version` reports 2.3.4+), with an
  automatic per-step fallback to ffmpeg extraction. Direct attempts log to `*_mkv.log` in the temp dir.
- FFmpeg 9's `dovi_split`/`dovi_rpu` filters do not replace `dovi_tool`: they have no profile
  conversion, injection or generation; `dovi_split=mode=bl` is slower than `remove` on the
  direct-MKV path and changes the bytes; `dovi_rpu=strip` misses RPUs before EOS. Evaluation and
  measurements: [FFMPEG9_DOVI_EVALUATION.md](FFMPEG9_DOVI_EVALUATION.md) (2026-10-13).
- `dovi_tool` 2.3.4 parses Level 253 ext-metadata blocks, but the `dolby_vision` crate 3.4.0 used
  in-process (inspect sampling, MEL/FEL classification) does not yet. Bump the crate again when it
  releases L253 support.
- The `dolby_vision` crate (3.4.0) stores reshaping pivots delta-coded, as read from the bitstream;
  accumulate them before comparing with sample values.
- **Runners** (`external.rs`): `run_command`/`run_command_with_spinner` must leave the child's pipes
  attached and let `wait_with_output` drain them. Taking `child.stdout`/`stderr` out first
  deadlocks once a tool writes more than 64 KB (this hung `--verify` on a 1,263-scene episode until
  0.5.1). Long-running steps use `run_command_with_progress`/`run_command_live`, which drain on
  threads.

## Inputs and the source file

- With no input arguments, mkvdovi recursively processes `.mkv` files from the current directory.
  It skips `mkvdovi_temp_*` and legacy `mkvdolby_temp_*` paths and files already ending in
  `.DV.mkv`. Explicit `--mdfix` allows a DV input and writes a distinct `*.mdfix.DV.mkv` candidate.
- **A successful conversion deletes the source input by default**; `--keep-source` prevents it.
  All DV and repair inputs keep their source by default.
- **mkvdovi never encodes video.** Every path copies the video stream bit-exactly. There is no
  compositor, no `composite-pipe`, no `--fel-*`/`--encoder` flag and no Modal/NVENC/libx265/
  VideoToolbox encode; do not add one back.
- **Profile 7 FEL is refused**, with or without `--mdfix`: a per-file hard error before any temp
  work. The source is never deleted, and a multi-file run continues with the next file. The BL+EL
  compositor was removed because it did not match ETSI GS CCM 001 on real discs (measured mean
  chroma error 69 to 534 10-bit codes on seven of eight FEL test cuts). The plan for FEL without a
  re-encode is [FEL_PLAN.md](FEL_PLAN.md). `inspect` still reports FEL.
- Profile 7 MEL uses a fast metadata-only discard path unless `--mdfix` is requested. MEL/Profile 8
  `--mdfix` rebuilds metadata from a clean base layer.
- **PQ range contract.** PQ inputs tagged full range (range only) are refused for every input whose
  8.1 RPU mkvdovi generates: HDR10, HDR10+, MEL/P8 under `--mdfix`
  (`metadata::check_pq_range_contract`, before any temp work). Its sources include the first
  decoded frame of a video-only ffprobe call, because an MKV can tag a full-range stream as limited.
  Untagged passes. The MEL fast path is not checked.

## Interruption and resume

- Extract, inject and mux show a live byte-progress bar (throughput and ETA) and warn after
  `--stall-timeout` (default 300 s, `0` disables) when the output file stops growing.
- An interrupted run (for example SSH `SIGHUP`) keeps `mkvdovi_temp_*` and prints a resume hint. A
  re-run **auto-resumes** by reusing completed steps, gated by `<artifact>.done` sentinels plus a
  `resume.json` fingerprint (input name/size/mtime, mkvdovi version, and the artifact-affecting
  settings from `pipeline::resume_settings`) (`resume.rs`).
  - A temp dir with a different fingerprint is discarded. One with no fingerprint (older mkvdovi,
    legacy `mkvdolby_temp_*`) resumes with a warning, except under a non-preset `--hlg-composer`,
    where it is discarded.
  - A non-preset HLG composer adds ` hlg_composer=<luminance_mapping>` to the settings string (only
    then, so default fingerprints are unchanged); likewise ` hdr10plus_l1=hybrid` only for an
    HDR10+ input under that flag. **Add any new artifact-affecting flag to `resume_settings`.**
  - The fingerprint does not cover the measurements. `config::withdraw_stale_rpu` (called from
    `prepare_configuration`) compares the regenerated `extra.json` with the one in the temp dir and,
    when they differ, withdraws the `RPU.bin`/`BL_RPU.hevc` sentinels (`resume::invalidate`) and the
    `mux.done` marker, so a re-analysis is never paired with a stale RPU.
- `--no-resume` forces a clean run. Run long conversions under `tmux`/`nohup`.

## Measurements and the analyzer

- **Finding the analyzer** (`pipeline::analyzer_executable`): next to the mkvdovi executable, then
  `target/release/hdr_analyzer_mvp` relative to the current directory, then `PATH`.
- **Reusing measurements.** For HDR10 without valid measurements, mkvdovi runs `hdr_analyzer_mvp`.
  Every existing candidate is tried in turn (the analyzer's own `<stem>_measurements.bin` first, a
  shared `measurements.bin` last). The first whose `.l1.json` sidecar validates against the input
  is reused: name, size, contiguous scenes, and the MediaInfo frame count within
  `metadata::frame_count_tolerance` (MediaInfo estimates counts from duration for untagged MKVs).
  Otherwise analysis runs again. Scenes outside min <= avg <= max are a warning, not a rejection:
  `--peak-domain luma` and the percentile/robust estimators legitimately produce them.
- `--legacy-madvr-l1` is the only way to reach the old `--madvr-file --use-custom-targets` L1
  (optimizer targets as max, placeholder avg). It is HDR10-only and refused for HLG.
- **`--analysis-quality`** controls sampling (downscale/sample-rate): `auto` (default) = `accurate`
  when GPU analysis is available, else `balanced`; `fast` = half resolution, every 3rd frame;
  `balanced` = half resolution, every frame; `accurate` = full resolution, every frame. CUDA is the
  main pipeline: `accurate` is the only preset where CPU and CUDA give identical output (above
  downscale 1 CUDA strides while the CPU resizes), and on CUDA every preset runs at NVDEC speed, so
  `resolve_auto_settings` warns on an explicit `balanced`/`fast` with GPU analysis. Under
  `accurate`, a candidate whose sidecar records coarser sampling is rejected and re-analyzed
  (`rejects_coarser_sidecar`); other presets reuse it with a warning. After a fresh analysis,
  `warn_if_gpu_analysis_missing` warns when `--hwaccel cuda` with a `+cuda` analyzer produced
  `analysis.gpu: false`.
- **`--hwaccel`** defaults to `auto`: `pipeline::resolve_auto_settings` (called once from
  `main.rs`) probes `nvidia-smi` (also `/usr/lib/wsl/lib/nvidia-smi` on WSL2) and resolves to `cuda`
  or `none` before any file is processed, so downstream code only sees concrete values. GPU
  analysis availability is probed through `hdr_analyzer_mvp --version` containing `+cuda` (set from
  the analyzer's `cuda` feature in its `cli.rs` `VERSION` const; keep that contract on both sides).
  `--hwaccel` only selects GPU decode and analysis in the analyzer; it never selects an encoder.
  An explicit `mkvdovi --hwaccel cuda` is forwarded to the spawned analyzer (GPU analysis when that
  binary was built with `--features cuda`).
- **HLG analyzer contract.** mkvdovi always passes `--transfer hlg` for inputs MediaInfo classifies
  as HLG and hard-fails on an analyzer without the flag, a cross-binary contract like the `+cuda`
  probe. For HLG it requires the analyzer's `--help` to contain the selected composer's
  `Composer::luminance_mapping()` name, for every composer including preset
  (`mkvdovi/src/pipeline/measure.rs` `run_fresh_analysis`, `external::analyzer_lists_option`), and
  then always passes `--hlg-composer`. An analyzer that does not name it (pre-P8) is refused before
  any analysis. The analyzer's help text is therefore a cross-binary contract too. Under
  `--hdr10plus-l1 hybrid` a fresh analysis needs the analyzer's `--help` to contain the capability
  `l1-sidecar-frame-peak` (`ANALYZER_FRAME_PEAK_CAPABILITY`, printed in `hdr_analyzer_mvp/src/cli.rs`):
  an analyzer that writes sidecars older than v6 is refused before it runs, so it cannot replace
  existing measurements. The name is a capability, not a version, and stays for later sidecars.

## L1 sidecar contract

The field-by-field schema per version is in FORMAT_COMPATIBILITY.md ("L1 measurement sidecar").
This section covers the code on each side.

- The analyzer writes per-scene L1 stats to `<measurements>.l1.json`
  (`hdr_analyzer_mvp/src/l1_sidecar.rs`, currently `version: 6`); mkvdovi reads it through
  `metadata::load_l1_sidecar` for source-honest per-scene L1. Version 2's full-resolution `crop`
  becomes mkvdovi's L5 offsets.
- **Versions accepted:** mkvdovi and `tools/l1_diff` accept 1–6 (`metadata::L1_SIDECAR_MAX_VERSION`).
  mkvdovi reuses a pre-v4 sidecar with an advisory about smoothed averages, and a pre-v5 sidecar
  only on an exact MediaInfo frame count. A missing, invalid or unknown-version sidecar produces a
  visible warning and re-analysis, never a silent fallback. A schema change must bump the version
  on both sides.
- **Averages (v4):** frame and scene averages are unfiltered (`FrameL1Measurement::avg_luma_pq` /
  `avg_max_rgb_pq`); only `MadVRFrame::avg_pq` in the `.bin` still goes through the histogram EMA.
- **Light levels (v4, max-RGB runs):** the optional `light_level` block (MaxCLL/MaxFALL in nits
  over the crop; FALL is a linear-light mean of the 4096-bin PQ histogram) fills the L6 fields the
  source does not state (`metadata::resolve_light_levels`). It needs `sample_rate` 1, and
  `downscale` 1 for MaxCLL; it never changes a source-stated value; HLG MaxCLL is capped at 1000.
- **Frame peak (v6):** `frames.max_pq_12bit` is `MadVRFrame::peak_pq_2020` after `select_peak_pq`, the
  value each scene's `max_pq_12bit` folds, and `peak_source` is the source that pass applied
  (`pipeline::scenes::applied_peak_source_for`). `hybrid::check_measurements` needs v6 and `max`, so
  older measurements are re-analyzed under `--hdr10plus-l1 hybrid` and reused otherwise. The sidecar's
  `peak_source` is the analyzer's flag, not mkvdovi's `--peak-source`, which selects an HDR10+ statistic.
- **Stream frames (v5):** an open-GOP cut at a CRA starts with RASL pictures that no decoder
  outputs, but `dovi_tool inject-rpu` maps RPU `n` to presentation picture `n`, and those pictures
  come first. The analyzer counts them from the packets' NAL types (`leading_pictures.rs`) and fails
  on any other difference between decoded frames and pictures (packets that carry a slice)
  (`pipeline::account_stream_frames`). `metadata::generate_extra_json` shifts the shots by the count
  (`L1Sidecar::stream_frame_count` = RPU length), `verify.rs` compares in stream frames, and
  `tools/l1_diff` shifts our frames by the count against a stream-frame reference and labels
  `--export-reference` rows in stream frames. Inject output containing "mismatched lengths" is a
  hard failure for a measured RPU.
- **HLG names:** HLG inputs require v3 or later and exactly the selected composer's name in
  `analysis.luminance_mapping` (`metadata::check_luminance_mapping`). A sidecar of the other
  composer, an unknown `dovi84-*` name or the luma-only `dovi84-v1` (pre-release builds) is
  re-analyzed with a visible message. A legacy name (`dovi84_composer::LEGACY_LUMINANCE_MAPPINGS`:
  `dovi84-v2`, `dovi84-bt2100-v1`, the per-pixel chroma decodes before P8) is re-analyzed with a
  "pre-spec 4:2:0" message. Each new name of an existing field kept the version number (`dovi84-v2`
  kept 3, `dovi84-bt2100-v1` kept 4, the P8 names `dovi84-v3` and `dovi84-bt2100-v1-spec420` kept
  5). The names come from `Composer::luminance_mapping`.

## HDR10+

- L1 comes from the source HDR10+ metadata. Under the opt-in `--hdr10plus-l1 hybrid` the analyzer
  also runs and L1 comes from union pieces (HDR10+ starts ∪ analyzer cuts; HDR10+ peak with a
  reported fallback to the measured peak of the piece's own frames; measured average and minimum;
  `pipeline/hybrid.rs`).
- The panel peak is **not** passed as a `--trim-targets` override.
- HDR10+ scene peaks above 3× the mastering-display peak produce advisory warnings only. **Never
  add a silent clamp.**
- Use `mkvdovi` directly for HDR10+ sources. `scripts/mkvdovi_hifi_workflow.sh` is a specialist
  comparison helper for inputs that **already** contain DV metadata.

## HLG → Profile 8.4 (no re-encode)

- The HLG stream is copied bit-exactly and gets an 8.4 RPU (`metadata::dv_profile_for`). L1 comes
  from the analyzer's 8.4-decode measurement through the selected composer. There is no HLG→PQ
  encode path any more. `--mdfix` on an 8.4 input is refused.
- `--hlg-composer bt2100` is the default, switched by owner decision before the playback test. The
  P8 playback test of 2026-10-13 (part of WS6, whose `code / 1023` question the
  WS6.6 canary answered for the C9 on 2026-10-29: `code / 1023`) showed that
  an LG OLED C9, through the TV's own player and through a Shield, applies the RPU composer: a
  deliberately green canary composer rendered green, and the preset's tint was visible next to a
  neutral bt2100. `preset` keeps the RPU exactly as `dovi_tool generate` writes it and is the
  fallback. Design: [HLG_COMPOSER.md](HLG_COMPOSER.md).
- For bt2100, `generate_rpu` rewrites the composer with `dovi84_composer::rewrite_rpu_file` before
  the `RPU.bin` sentinel. The rewrite refuses an RPU whose frames do not carry the preset mapping or
  that does not round-trip byte for byte through the `dolby_vision` crate, and writes atomically.
- **Colour contract:** every HLG input (any composer) tagged full range, with a matrix other than
  BT.2020 NCL (BT.2020 CL included) or primaries other than BT.2020 in MediaInfo or ffprobe is
  refused before any temp work (`metadata::check_hlg_colour_contract`). Untagged fields pass with a
  warning.

## `--verify`

- Resolves tools from `PATH`, validates structured RPU frame JSON, and hard-fails: malformed
  Profile 8 / L1 / L6 / CM v4.0 L9/L11/L254 metadata; a frame 0 whose CM version differs from
  `--cm-version` (`pipeline::expected_cm_version`: CM v4.0 metadata under v29, none under v40); an
  RPU frame count that differs from the muxed video track (MediaInfo `FrameCount`) or the L1
  sidecar. L6 MaxCLL/MaxFALL 0 in a passed-through Profile 7 MEL RPU is only a warning.
- HLG output: hard-fails when there are no measurements or the sidecar does not load, and checks
  that **every** RPU frame carries the composer the sidecar names (`dovi84_composer::check_rpu_file`).
- Generated RPU (`verify::DeliveryExpectation`; not the MEL passthrough, whose RPU can carry levels
  the crate cannot parse, such as L253): parses the extracted RPU in-process (frame count from the
  parse) and hard-fails a frame whose source range differs from `metadata::expected_source_range`,
  or whose L1 differs from the sidecar scene's L1 after the generator's clamp
  (`ExtMetadataBlockLevel1::from_stats_cm_version(.., V29)`). The clamps themselves are only
  reported.
- The pipeline passes no sidecar for HDR10+ without `--hdr10plus-l1 hybrid` and under
  `--legacy-madvr-l1`, because then the RPU's L1 does not come from it.

## RPU generation

- **Generator clamps:** `dovi_tool generate` clamps L1. Min is written as at most 12 codes, max as
  at least 2081 (100 nits), and avg as at least 819, because mkvdovi sets
  `l1_avg_pq_cm_version: V29` under CM v4.0 too (`metadata::generate_extra_json`). A measured value
  outside those limits does not reach the RPU.
- **Source range, 8.1:** mkvdovi passes `source_min_pq`/`source_max_pq` explicitly in `extra.json`
  (`metadata::source_range_pq_81`: `dolby_vision::utils::nits_to_pq_12_bit` of the mastering
  min/max, the crate's CM XML convention). Without them `dovi_tool generate` falls back to a coarse
  L6 lookup (mastering max other than 1000/2000/4000/10000 nits → 3079; mastering min other than
  ≤0.001 or exactly 0.005 nits → 0). That fallback still applies to implausible mastering values
  (peak outside `dovi_rpu_builder::MASTERING_PEAK_NITS` 100–10000, min outside
  `MASTERING_MIN_NITS` 0–1), with a warning, because a wrong-units SEI would otherwise set a
  0.1-nit source range.
- **Source range, 8.4: never pass it.** `change_source_levels` overrides on any value, and both HLG
  composers and the HLG measurement assume the preset's 62/3079 (`metadata::source_range_pq_84`).
- `extra.json` itself comes from `dovi_rpu_builder` (`RpuParams::extra_json`,
  `metadata::generate_extra_json`); mkvdovi still runs `dovi_tool generate`.
- **`--mdfix`** regenerates the RPU with `dovi_tool generate`: authored L2 trims are replaced by
  neutral ones, L3 becomes neutral, L4/L8/L10 are dropped (the generator emits none), L5 is one
  sampled value, and L6 comes from the container, not the source RPU (P12). Under
  `--cm-version v29` the rebuild is a CM v2.9 RPU: authored L2 is dropped too, and L3, L9, L11 and L254 are
  dropped, not rebuilt. After each "Rebuilding … (--mdfix)." line it prints which authored levels
  it replaces or drops (`pipeline/mdfix_notice.rs`; `-q` hides it). Under v29 the notice names L3,
  L9, L11 and L254 as dropped ("a CM v2.9 RPU has no CM v4.0 levels").

### Generated DV metadata levels (CM v4.0 by default)

- **L1** per-frame luminance (from HDR10+ or the analyzer) · **L2** trims for 100/600/1000-nit
  targets · **L6** static mastering metadata (MaxCLL/MaxFALL) · **L9** source primaries
  (auto-detected) · **L11** content type + reference mode.
- `--cm-version v29` writes `"cm_version": "V29"` (`metadata::generate_extra_json`; without it
  `dovi_tool generate` defaults to CM v4.0, which was ROADMAP P13). The RPU then carries L1, L5 and
  L6 only: no L2 and no CM v4.0 levels (L3, L9, L11, L254).
- Defaults: `--cm-version v40` (or `v29`); `--content-type movies` (valid: `default`, `movies`,
  `game`, `sport`, `user-generated-content`; `cinema`/`film` alias `movies`, `gaming` aliases
  `game`); `--reference-mode false`; `--source-primaries` auto (`0=P3-D65, 1=BT.709, 2=BT.2020`).
  `-v/--verbose` shows raw tool output; `-q/--quiet` is minimal.
- Progress uses `indicatif` spinners with TTY detection (disabled automatically in CI and
  non-interactive runs).

## What a libplacebo render can test

libplacebo (mpv, VLC, ffmpeg) reads only L1 max/avg and `source_min_pq`/`source_max_pq` from an RPU
for display mapping, plus the reshaping/NLQ. It ignores L1 min, L2, L3, L5, L6, L8, L9 and L11, so a
libplacebo render cannot test those levels.
