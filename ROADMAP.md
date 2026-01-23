# HDR-Analyze Roadmap

This document outlines the development roadmap for `hdr-analyze`. It focuses on upcoming features, quality improvements, and long-term goals. For a detailed history of completed work, see [`CHANGELOG.md`](CHANGELOG.md) (noting Milestone 4 features like robust noise handling and scene-aware smoothing are now live).

> **Versioning**: This project follows [Semantic Versioning](https://semver.org/). 
> The first public release is `v0.1.0`. During the `0.x` series, minor version bumps may include breaking changes.

---

## Recently Completed / In Validation (v0.1.0)

The following core features have been implemented and are currently being validated in production:

-   **Dolby Vision CM v4.0 (NEW)**: Full Content Mapping v4.0 implementation in mkvdolby with:
    - L2 trim parameters for 100/600/1000 nit target displays
    - L9 source primaries (auto-detected from MediaInfo)
    - L11 content type and reference mode hints
    - CM v4.0 is now the default for all mkvdolby runs
-   **Dynamic Clipping Heuristics**: Optimizer profiles (`conservative`, `balanced`, `aggressive`) and dynamic knee detection are implemented.
-   **Native HLG Support**: In-memory HLG-to-PQ conversion is active.
-   **Noise Robustness**: Percentile-based peaks (P99/P99.9) and histogram smoothing are live.
-   **NVIDIA CUDA Support**: Added `--hwaccel` flag to `mkvdolby` for accelerated analysis.
-   **Rust 1.93 Toolchain**: Upgraded from pinned 1.82 to stable channel.

---

## Prioritized Task Order (Next Steps)

The following tasks are prioritized to ship CM v4.0 as a stable product and enable safe future expansion.

1.  **Release Packaging & Distribution (Critical)**:
    -   Update GitHub release workflow to include `mkvdolby` and `verifier` binaries (currently only packages `hdr_analyzer_mvp`).
    -   Update README documentation to reflect mkvdolby as a core component.
    -   *Why*: CM v4.0 is implemented but not shipped to users via releases.

2.  **Benchmark Corpus & CI Integration (Critical)**:
    -   Establish an official benchmark corpus with ground-truth annotations.
    -   Add regression tests for CM v4.0 metadata generation.
    -   Integrate `compare_baseline` and `dovi_tool` smoke tests into CI.
    -   *Why*: Essential to safely tune complex heuristics without breaking CM v4.0.

3.  **mkvdolby UX Improvements**:
    -   Add `--dry-run` mode to preview commands without execution.
    -   Add `--keep-temp` / `--keep-logs` for debugging failed conversions.
    -   Improve dependency checks (include `hdr10plus_tool` when needed).
    -   Consider safer defaults (keep source by default, require `--delete-source`).

4.  **Scene Detection (Hybrid Metric)**:
    -   Implement hybrid scene detection fusing histogram distance with optical flow.
    -   *Status*: Prototype flag (`--scene-metric hybrid`) falls back to histogram-only.

5.  **madVR v6 Gamut Peaks (Full RGB Conversion)**:
    -   Replace luminance-based approximation with color-accurate RGB transformation.

---

## Phase A: madVRhdrMeasure Feature Parity

**Objective**: Achieve feature parity with `madVRhdrMeasure` for key output fidelity metrics.

-   [ ] **Reference Comparison Harness**: Create an integration test that runs both `hdr-analyze` and `madVRhdrMeasure` on a reference clip and automatically diffs the key statistics (scene boundaries, MaxCLL/FALL, optimizer targets) to within a defined tolerance.
-   [ ] **Long-term Smoothing Audit**: Verify if the V1.4 bidirectional EMA implementation matches the "feel" of the reference tool's temporal stability.

---

## Phase B: Dolby Vision `cm_analyze` Parity

**Objective**: Achieve parity with Dolby's `cm_analyze` for professional metadata generation.

-   [x] **CM v4.0 Metadata Generation**: Generate L1/L2/L6/L9/L11 metadata via `extra.json` for `dovi_tool`.
-   [ ] **Dolby Vision XML Export**: Direct XML export for DaVinci Resolve and Dolby Metafier compatibility.
-   [ ] **Long-Play / Shot Metadata Mode**: Per-shot analysis with edit-offset handling for dissolves.
-   [ ] **Canvas & Aspect Metadata**: Capture mastering display specs and active image area.

---

## Phase C: Novel Research & Quality Enhancements

**Objective**: Explore and implement state-of-the-art techniques to push beyond existing tool capabilities.

-   [ ] **Adaptive Scene Detection Research**: Evaluate advanced scene detection methods (e.g., ML-based models like TransNetV2, multi-metric fusion) to eliminate manual threshold tuning.
-   [ ] **Temporal Consistency Modeling**: Prototype a "future-aware" optimizer that uses a larger context window to minimize flicker and create more stable `target_nits`.

---

## Future Enhancements

-   [ ] **Hardware Decode Contexts**: Implement full support for VAAPI and VideoToolbox for hardware-accelerated decoding on Linux and macOS.
    -   *Status*: **Partial**. `img_convert` pipeline in `mkvdolby` now supports `videotoolbox` encoding on macOS. Decode contexts for analysis are deferred.
-   [ ] **Broader CI Coverage**: Expand CI to build and test on multiple platforms (Linux, macOS, Windows) and run a more comprehensive integration test suite.
