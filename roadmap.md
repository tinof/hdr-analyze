# HDR-Analyze Roadmap

This document outlines the development roadmap for `hdr-analyze`. It focuses on upcoming features, quality improvements, and long-term goals. For a detailed history of completed work, see [`docs/CHANGELOG.md`](docs/CHANGELOG.md). For development workflows and research, see [`DEVELOPMENT_WORKFLOW.md`](DEVELOPMENT_WORKFLOW.md) and [`docs/RESEARCH.md`](docs/RESEARCH.md) respectively.

---

## Prioritized Task Order

The following tasks are prioritized to deliver the most significant improvements to output quality and accuracy first.

1.  **Dynamic Clipping Heuristics Calibration**: Finalize the optimizer profiles (`conservative`, `balanced`, `aggressive`) by calibrating the dynamic clipping knee based on robust peak statistics (P99/P99.9) and APL categories.
2.  **Scene Detection (Hybrid Metric)**: Implement a hybrid scene detection metric that fuses histogram distance with optical flow (e.g., Farnebäck) to improve accuracy, especially on grainy content. This should be gated behind a feature flag until it demonstrates a clear F1 score improvement.
3.  **Native HLG Support Validation**: Complete the validation of the native HLG analysis pipeline by comparing its output against the legacy `zscale` re-encode workflow on a dedicated test corpus.
4.  **Benchmark Corpus & CI Integration**: Establish an official benchmark corpus with ground-truth annotations. Integrate the `compare_baseline` tool and `dovi_tool` smoke tests into the CI pipeline to catch regressions automatically.
5.  **madVR v6 Gamut Peaks (Full RGB Conversion)**: Replace the current luminance-based approximation for DCI-P3 and BT.709 gamut peaks with a full, color-accurate RGB transformation pipeline.
6.  **Dolby Vision XML Export**: Implement a feature to directly export Dolby Vision metadata as CMv2.9 and CMv4.0 XML files, ensuring compatibility with professional tools like DaVinci Resolve and Dolby Metafier.

---

## Phase A: madVRhdrMeasure Feature Parity

**Objective**: Achieve feature parity with `madVRhdrMeasure` for key output fidelity metrics.

- [ ] **Scene & Target Nits Feature Audit**: Perform a detailed comparison against `madVRhdrMeasure` to identify gaps in long-term smoothing, scene merge logic, and other heuristics.
- [ ] **Reference Comparison Harness**: Create an integration test that runs both `hdr-analyze` and `madVRhdrMeasure` on a reference clip and automatically diffs the key statistics (scene boundaries, MaxCLL/FALL, optimizer targets) to within a defined tolerance.

---

## Phase B: Dolby Vision `cm_analyze` Parity

**Objective**: Achieve parity with Dolby's `cm_analyze` for professional metadata generation.

- [ ] **Long-Play / Shot Metadata Mode**: Add support for per-shot analysis with edit-offset handling to correctly manage dissolves and transitions.
- [ ] **Dolby XML Export Path**: Implement direct writing of Level 1, 2, and 8 Dolby Vision XML, compatible with Netflix's Metafier validator.
- [ ] **Canvas & Aspect Metadata**: Capture and embed mastering display specs, active image area, and target trims.
- [ ] **Validation Workflow**: Integrate Metafier (or an open-source equivalent) checks into CI to validate generated XML against standard QC rules.

---

## Phase C: Novel Research & Quality Enhancements

**Objective**: Explore and implement state-of-the-art techniques to push beyond existing tool capabilities.

- [ ] **Adaptive Scene Detection Research**: Evaluate advanced scene detection methods (e.g., ML-based models like TransNetV2, multi-metric fusion) to eliminate manual threshold tuning.
- [ ] **Temporal Consistency Modeling**: Prototype a "future-aware" optimizer that uses a larger context window to minimize flicker and create more stable `target_nits`.
- [ ] **Benchmark Corpus Expansion**: Curate a diverse public dataset of HDR10 and HLG content with ground-truth annotations for ongoing validation.

---

## Future Enhancements

- [ ] **Hardware Decode Contexts**: Implement full support for VAAPI and VideoToolbox for hardware-accelerated decoding on Linux and macOS.
- [ ] **Broader CI Coverage**: Expand CI to build and test on multiple platforms (Linux, macOS, Windows) and run a more comprehensive integration test suite.
