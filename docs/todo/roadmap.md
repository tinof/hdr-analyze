# HDR-Analyze Roadmap

This document outlines the development roadmap for `hdr-analyze`. It focuses on upcoming features, quality improvements, and long-term goals. For a detailed history of completed work, see [`docs/CHANGELOG.md`](../../CHANGELOG.md) (noting V1.4 features like robust noise handling and scene-aware smoothing are now live).

---

## Recently Completed / In Validation (V1.4)

The following core features have been implemented and are currently being validated in production:

-   **Dynamic Clipping Heuristics**: Optimizer profiles (`conservative`, `balanced`, `aggressive`) and dynamic knee detection are implemented. *Next step: Long-term qualitative tuning.*
-   **Native HLG Support**: In-memory HLG-to-PQ conversion is active. *Next step: Formal regression testing against reference streams.*
-   **Noise Robustness**: Percentile-based peaks (P99/P99.9) and histogram smoothing are live.

---

## Prioritized Task Order (Next Steps)

The following tasks are prioritized to stabilize the V1.4 codebase and enable safe future expansion.

1.  **Benchmark Corpus & CI Integration (Critical)**:
    -   Establish an official benchmark corpus with ground-truth annotations.
    -   Integrate the `compare_baseline` tool and `dovi_tool` smoke tests into the CI pipeline to catch regressions in the V1.4 logic automatically.
    -   *Why*: Essential to safely refactor or tune the complex heuristics added in V1.4.

2.  **Scene Detection (Hybrid Metric)**:
    -   Implement a hybrid scene detection metric that fuses the current histogram distance with optical flow (e.g., Farnebäck) to improve cut accuracy, especially on grainy content.
    -   *Status*: Currently a prototype flag (`--scene-metric hybrid`) that falls back to histogram-only.

3.  **madVR v6 Gamut Peaks (Full RGB Conversion)**:
    -   Replace the current V1.3 luminance-based approximation (0.99x/0.95x scaling) for DCI-P3 and BT.709 peaks.
    -   Implement a color-accurate RGB transformation pipeline for precise per-gamut peak measurement.

4.  **Dolby Vision XML Export**:
    -   Implement a feature to directly export Dolby Vision metadata as CMv2.9 and CMv4.0 XML files, ensuring compatibility with professional tools like DaVinci Resolve and Dolby Metafier.

---

## Phase A: madVRhdrMeasure Feature Parity

**Objective**: Achieve feature parity with `madVRhdrMeasure` for key output fidelity metrics.

-   [ ] **Reference Comparison Harness**: Create an integration test that runs both `hdr-analyze` and `madVRhdrMeasure` on a reference clip and automatically diffs the key statistics (scene boundaries, MaxCLL/FALL, optimizer targets) to within a defined tolerance.
-   [ ] **Long-term Smoothing Audit**: Verify if the V1.4 bidirectional EMA implementation matches the "feel" of the reference tool's temporal stability.

---

## Phase B: Dolby Vision `cm_analyze` Parity

**Objective**: Achieve parity with Dolby's `cm_analyze` for professional metadata generation.

-   [ ] **Long-Play / Shot Metadata Mode**: Add support for per-shot analysis with edit-offset handling to correctly manage dissolves and transitions.
-   [ ] **Canvas & Aspect Metadata**: Capture and embed mastering display specs, active image area, and target trims.
-   [ ] **Validation Workflow**: Integrate Metafier (or an open-source equivalent) checks into CI to validate generated XML against standard QC rules.

---

## Phase C: Novel Research & Quality Enhancements

**Objective**: Explore and implement state-of-the-art techniques to push beyond existing tool capabilities.

-   [ ] **Adaptive Scene Detection Research**: Evaluate advanced scene detection methods (e.g., ML-based models like TransNetV2, multi-metric fusion) to eliminate manual threshold tuning.
-   [ ] **Temporal Consistency Modeling**: Prototype a "future-aware" optimizer that uses a larger context window to minimize flicker and create more stable `target_nits`.

---

## Future Enhancements

-   [ ] **Hardware Decode Contexts**: Implement full support for VAAPI and VideoToolbox for hardware-accelerated decoding on Linux and macOS.
-   [ ] **Broader CI Coverage**: Expand CI to build and test on multiple platforms (Linux, macOS, Windows) and run a more comprehensive integration test suite.
