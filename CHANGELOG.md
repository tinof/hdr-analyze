# Changelog

This document provides a historical record of completed milestones, feature implementations, and significant refactoring efforts for the `hdr-analyze` project.

---

## [Unreleased]

### Changed (behavioral)
- **Active-area crop detection now uses a multi-frame probe.** `hdr_analyzer_mvp` samples
  `--crop-probes <N>` frames (default 7) across the middle 70% of seekable inputs, rejects
  black/low-signal frames, and commits a tolerance-clustered conservative crop before analysis.
  `--crop-probes 0` selects the hardened in-stream fallback; `--no-crop` remains unchanged.
- Scene cuts now provide reporting-only crop-stability telemetry. Variable-active-area titles use
  the union of observed probe modes so full-frame picture is not cut; per-scene crop application
  remains a follow-up to preserve measurement continuity.

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
