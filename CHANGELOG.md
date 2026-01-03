# Changelog

This document provides a historical record of completed milestones, feature implementations, and significant refactoring efforts for the `hdr-analyze` project.

---

## V1.4: Performance & Quality Enhancements

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

## V1.3: Advanced Optimization & Format Support

- **Scene-Aware Optimizer**: Enhanced the optimizer with configurable profiles (`conservative`, `balanced`, `aggressive`) and a dynamic clipping heuristic that uses per-scene knee smoothing to prevent banding.
- **Hue Histogram**: Implemented a real 31-bin chroma-derived hue histogram from the U/V planes, replacing the previous zeroed-out placeholder. The verifier was also extended to validate its distribution.
- **madVR v6 Gamut Peaks**: Replaced the simple duplication of BT.2020 peaks with a gamut-aware approximation for DCI-P3 (99%) and BT.709 (95%) peaks.

## Milestone R: Codebase Modularization

- **Refactored `main.rs`**: Successfully refactored the monolithic `main.rs` file (originally ~1860 lines) into a modular structure with a thin (63-line) entry point.
- **Created Modules**: Logic was separated into distinct modules with single responsibilities:
  - `cli.rs`: Command-line interface definition.
  - `ffmpeg_io.rs`: FFmpeg initialization and I/O.
  - `pipeline.rs`: Main orchestration logic.
  - `writer.rs`: madVR measurement file writing.
  - `analysis/`: Modules for frame, scene, histogram, and HLG analysis.
- **Preserved Behavior**: All unit tests were migrated and passed, ensuring behavior was preserved post-refactor.

## V1.2: Core Accuracy and Stabilization

- **Baseline & Harness**: Established a baseline test pack and created the `tools/compare_baseline` harness for regression testing.
- **Core Analysis Features**:
  - Implemented robust active-area (black bar) detection and cropping.
  - Ensured correct v5 histogram semantics and limited-range normalization.
  - Implemented a native histogram-distance scene detection algorithm with threshold and smoothing controls.
- **CLI & Usability**:
  - Added support for both positional and flag-based (`-i/--input`) input.
  - Enhanced the `verifier` tool with additional checks for FALL metrics and data consistency.

## Initial Implementation

- **Native FFmpeg Pipeline**: Initial version of the tool using `ffmpeg-next` for a native Rust video processing pipeline.
- **madVR v5/v6 Output**: Core support for writing madVR-compatible `.bin` measurement files.
- **Basic Optimizer**: First implementation of the dynamic target nits optimizer.
