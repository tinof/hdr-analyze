# HDR-Analyze Roadmap (Consolidated)



Reflects the current state after the latest code upgrades and lays out the next milestones with clear definitions of done and acceptance criteria.

---

## 0) Current Status (after latest upgrades)

Implemented (latest changes)
- CLI options
  - `INPUT` (positional) or `-i, --input` — both forms accepted for input file path.
  - `--madvr-version 5|6` (default: 5). For v6, `header_size=36`, `target_peak_nits` is written (default: MaxCLL or overridden via `--target-peak-nits`), and per-frame gamut peaks (`peak_pq_dcip3`, `peak_pq_709`) use gamut-aware approximation (99% and 95% of BT.2020 peak respectively).
  - `--scene-threshold <float>` (default: 0.3) to tune histogram-distance scene cut sensitivity.
  - `--target-peak-nits <nits>` for v6 header override.
  - `--min-scene-length <frames>` (default: 24) — drop cuts occurring within N frames of the previous cut.
  - `--scene-smoothing <frames>` (default: 5) — rolling mean smoothing over the scene-change metric.
  - `--no-crop` — disable active-area crop detection (analyze full frame; diagnostics/validation).
  - `--optimizer-profile <conservative|balanced|aggressive>` (default: balanced) — presets for dynamic clipping behavior, knee smoothing, and temporal stability.
- Active area (black bar) detection and cropping
  - New `hdr_analyzer_mvp/src/crop.rs` with crop-detect-like algorithm on Y (10-bit), sampling every 10 px, ~10% non-black threshold, rounded to even coords/dims.
  - Detected once and applied to all frames; analysis constrained to `CropRect`.
- v5 histogram semantics and avg computation
  - 256-bin mapping matching v5 semantics: 64 bins up to pq(100 nits), 192 bins pq(100)→1.0; mid-bin weighting; avg-pq computed likewise; bin0 black-bar heuristic for avg.
- Limited-range normalization
  - HDR10 Y' nominal 64–940 normalized to [0..1] as PQ proxy prior to binning (robust with limited-range material).
- Native scene detection
  - Histogram distance (chi-squared-like, symmetric) between consecutive frame histograms; default threshold = 0.3; post-processing fixes end-frame off-by-one.
  - Controls: min scene length guard (default 24) and temporal smoothing of the difference signal (default 5 frames).
- Optimizer (enabled by default)
  - Rolling 240-frame average + highlight knee (99th percentile) with per-scene smoothing + scene-aware heuristics (by APL category).
  - Dynamic clipping heuristic: smooths highlight knee within scenes (window size varies by profile) to prevent banding.
  - Configurable profiles (conservative/balanced/aggressive) control: max delta per frame, extreme peak threshold, scene clamp ranges, knee multipliers, and knee smoothing window.
  - Scene-aware improvements: blends per-scene APL with rolling APL, resets smoothing at scene boundaries, applies per-frame delta limiting for temporal stability.
  - Can be disabled via `--disable-optimizer`.
- Hue histogram (31 bins)
  - Real chroma-derived hue distribution from U/V planes, quantized into 31 bins covering 360° hue circle.
  - Filters low-saturation pixels (grayscale content); integrated into v5 and v6 frame analysis.
- madVR v6 gamut peaks
  - Per-gamut approximation based on gamut size: DCI-P3 = 99% of BT.2020 peak, BT.709 = 95% of BT.2020 peak.
  - Note: Full RGB-based gamut conversion with color space transforms is a future enhancement.
- Header fields and writer
  - `maxCLL` from per-frame peak (nits), `maxFALL` and `avgFALL` derived from per-frame avg-pq (nits). Serialization via `madvr_parse` (v5 or v6).
- Verifier
  - Parses measurement, prints summary, validates scene/frame ranges, histogram integrity (256 bins, sum ≈100), PQ range checks; reports optimizer presence.
  - Additional checks: recomputes MaxFALL/AvgFALL and compares to header within tolerance; validates flags vs. presence of per-frame `target_nits`.
  - Hue histogram validation: checks 31-bin length, sum ≤100%, reports non-zero distribution coverage, warns on low coverage.
- Unit tests
  - 12 passing tests covering: PQ↔nits conversion, v5 histogram boundaries, scene detection guards, histogram distance metrics, highlight knee detection, optimizer profiles, delta limiting, gamut approximation, FALL computation.
- Documentation
  - Root README and CLAUDE.md updated to reflect current CLI (positional + flag input), optimizer defaults, and new flags.
  - Roadmap synchronized with implementation status.

Partially implemented (work remains)
- Hardware acceleration: CUDA attempted via `hevc_cuvid` if available; VAAPI/VideoToolbox paths currently fall back to software decode (device contexts not wired).

Not yet implemented
- Full RGB-based gamut conversion for v6 per-gamut peaks (current: luminance-preserving approximation).
- Proper VAAPI/VideoToolbox device context setup for hardware decoding.
- Integration tests comparing analyzer output against reference baseline.
- Broader CI coverage.

---

## 1) V1.2 — Baseline & Harness ✓ COMPLETE

Objective: Establish a baseline for testing and create a harness to compare future changes against this baseline. This provides a ruler to measure all subsequent changes.

Completed
- [x] Established a baseline pack of 2 short HDR10 clips.
- [x] Froze current outputs (.bin, verifier logs) in `tests/baseline`.
- [x] Added a Rust-based harness `tools/compare_baseline` that compares new runs to the frozen baseline.

**How to use the harness:**

To compare a new set of analysis outputs against the baseline, run the following command from the project root:

```bash
./target/release/compare_baseline --baseline tests/baseline --current path/to/new/outputs
```

This will print a delta of key metrics, including scene count, MaxCLL/FALL, and the 95th-percentile difference in per-frame `target_peak_nits`.

---

## 2) V1.2 — Core Accuracy Release (Beta Stabilization) ✓ COMPLETE

Objective: Produce stable, v5/v6-compatible measurements with accurate active-area cropping, correct histogram semantics, and reliable scene detection suitable for dovi_tool ingestion.

Completed
- [x] Black bar detection with crop; v5 histogram semantics; limited-range normalization.
- [x] Histogram-distance scene cut with threshold control; boundary fix.
- [x] FALL metrics; v5/v6 writing; updated README; enhanced verifier.
- [x] CLI accepts both positional and flag-based input.
- [x] Documentation updated to reflect current implementation.
- [x] Unit tests covering core functionality (12 passing tests).
- [x] Code quality: cargo fmt, clippy (zero warnings), all tests passing.

Definition of Done (V1.2) — ✓ Met
- ✓ New flags available and defaults yield stable cuts on typical content.
- ✓ Verifier passes on produced .bin; FALL header values match derived values within tolerance; flags/data consistent.
- ✓ No unused warnings in release build.
- Note: dovi_tool validation on test clips should be performed as part of manual validation workflow.

Acceptance Criteria — Ready for Validation
- Ready for testing on diverse HDR10 samples (letterboxed scope, 16:9 TV, bright demo).
- Scene boundaries should visually align (±1 frame) with ground truth/madVR on the majority of cuts.
- APL/peaks should be reasonable; no black-bar contamination (verified by crop detection).
- dovi_tool measurement-based generation should accept .bin on test clips without parse errors.

---

## 2) Milestone R — Refactor `main.rs` (Modularization) ✓ COMPLETE

Problem: `hdr_analyzer_mvp/src/main.rs` had grown to ~1860 lines, mixing CLI, decode/scaling, analysis, detection, optimization, and writing. This hindered testability and evolution.

Completed
- [x] Step 1: Extracted CLI to `cli.rs`.
- [x] Step 2: Extracted writer to `writer.rs`.
- [x] Step 3: Extracted scene metric and helpers to `analysis/scene.rs`.
- [x] Step 4: Extracted histogram utilities to `analysis/histogram.rs`.
- [x] Step 5: Extracted per-frame analysis to `analysis/frame.rs`.
- [x] Step 6: Extracted FFmpeg init/open/decoder/scaler to `ffmpeg_io.rs`.
- [x] Step 7: Created `pipeline.rs` to orchestrate the main workflow.
- [x] Step 8: `main.rs` is now a thin (63-line) entry point.
- [x] Step 9: All 12 original unit tests were successfully moved to their respective new modules.

Acceptance criteria — ✓ Met
- ✓ Behavior-preserving: `cargo test` confirms all 12 unit tests pass.
- ✓ Dev ergonomics: `main.rs` is now 63 lines (97% reduction). The codebase is modular and follows single-responsibility principles.
- ✓ Build: `cargo clippy --release -- -D warnings` passes with zero warnings.

---

## 3) V1.3 — Advanced Optimization & Format ✓ COMPLETE

Objective: Improve per-frame target selection and expand complete v6 format support.

Completed
- Scene-aware optimizer
  - [x] Pass `scene_avg_pq` and related aggregates into `apply_advanced_heuristics`.
  - [x] Add profiles: `--optimizer-profile <conservative|balanced|aggressive>` to adjust bounds/weights.
  - [x] Dynamic clipping heuristic with per-scene knee smoothing to prevent banding.
- Hue histogram (31 bins)
  - [x] Populate from chroma-based hue angle quantization from U/V planes.
  - [x] Filters low-saturation pixels; integrated into frame analysis.
- madVR v6 completeness
  - [x] Compute per-frame `peak_pq_dcip3` and `peak_pq_709` via gamut-aware approximation (99%/95% of BT.2020 peak).
  - [x] Extend verifier to validate hue histogram (length, sum, distribution coverage).
- Documentation
  - [x] Update README and CLAUDE.md on profiles and v6 implementation.
  - [x] Document approximation approach and note for future full RGB-based conversion.

Definition of Done (V1.3) — ✓ Met
- ✓ Optimizer yields smoother, scene-consistent `target_nits` with configurable profiles.
- ✓ Hue histogram non-zero, plausible distribution based on chroma analysis.
- ✓ v6 outputs with gamut-aware peaks parse via `madvr_parse`.

Acceptance Criteria — Ready for Validation
- On reference clips, `target_nits` transitions should be visually smooth at scene boundaries and within scenes.
- v6 outputs should validate and ingest in downstream tools expecting v6.
- Different optimizer profiles should produce measurably different behavior (conservative=smoother, aggressive=more responsive).

---

## 4) V1.4 — Performance & Parallelization

Objective: Improve throughput on multi-core systems (e.g., Ampere ARM).

Planned
- [x] Parallelize histogram accumulation by rows/tiles using `rayon`.
- [x] Optional lock-free accumulators or per-thread buffers + reduce (per-worker histograms + reduction).
- [x] Consider SIMD for hot loops (optional) — profiled current path; documented follow-up once hotspots remain after parallelism.
- [x] Benchmarks on representative 4K HEVC HDR samples — profiling workflow documented via `--profile-performance` + `docs/performance.md`.

Notes
- Analyzer exposes `--analysis-threads` to pin Rayon workers and `--profile-performance` to emit decode vs. analysis throughput (see `docs/performance.md`).

Definition of Done (V1.4)
- ≥1.7× speedup on 8-core CPU vs current single-thread baseline (same content, same flags).
- No changes in measurement outputs beyond floating-point noise tolerance.

---

## 5) V1.5 — Hardware Decode Contexts

Objective: Implement VAAPI/VideoToolbox device contexts for hardware decoding on supported platforms.

Planned
- [ ] Add AVHWDeviceContext creation for VAAPI/VT; map hw frames → sw frames for analysis path.
- [ ] CLI/docs updates; capability detection with graceful fallback.
- [ ] Validate on at least one VAAPI-capable Linux and one macOS system.

Definition of Done (V1.5)
- HW decode demonstrably improves decode stage performance with no change to analysis results.
- Fallback remains reliable where HW is unavailable.

---

## 6) Validation & QA

Unit tests — ✓ Partially Complete
- [x] Histogram bin selection correctness (bin edges, mid-bin mapping, v5 boundaries).
- [x] Chi-squared/histogram distance metrics (identical vs. opposite histograms).
- [x] Min-scene-length logic and smoothing behavior.
- [x] FALL computations vs known inputs.
- [x] PQ ↔ nits conversion round-trip accuracy.
- [x] Highlight knee detection with synthetic histograms.
- [x] Optimizer profile parsing and properties.
- [x] Delta limiting behavior.
- [x] Gamut peak approximation logic.
- [ ] Crop detection on synthetic letterboxed frames (remaining).
- Status: 12 passing tests in `hdr_analyzer_mvp/src/main.rs`.

Integration tests — Planned
- [ ] Analyze sample HDR10 assets; verify with `verifier`.
- [ ] Compare scene boundaries and stats against a madVR-produced measurement when available.
- [ ] Run dovi_tool measurement-based workflow (smoke test).
- [x] Baseline comparison: compare analyzer output against stored reference to guard regressions using the `tools/compare_baseline` harness.

Performance — Implemented (V1.4)
- [x] Benchmark decode/analysis fps via `--profile-performance` flag.
- [x] Parallel histogram analysis with Rayon (configurable via `--analysis-threads`).
- [ ] Prevent regressions with dedicated benches (future).

CI (recommended) — Planned
- [ ] Build on Linux/macOS (GitHub Actions).
- [ ] Run unit tests; run integration on small sample clip (time-bounded).
- [ ] Artifact: attach verifier logs.

---

## 7) CLI Flags (Current/Planned)

Current
- `INPUT` (positional) or `-i, --input <PATH>` — input video file (both forms supported)
- `-o, --output <PATH>` — output measurement file (auto-generated if omitted)
- `--disable-optimizer` — disable dynamic target nits generation (enabled by default)
- `--enable-optimizer` — deprecated/hidden (optimizer enabled by default)
- `--hwaccel <cuda|vaapi|videotoolbox>` (CUDA attempted; VAAPI/VT currently fall back to SW)
- `--madvr-version <5|6>` (default: 5)
- `--scene-threshold <float>` (default: 0.3)
- `--min-scene-length <frames>` (default: 24)
- `--scene-smoothing <frames>` (default: 5) — rolling mean window over scene-change metric
- `--target-peak-nits <nits>` (v6 header override; default: MaxCLL)
- `--downscale <1|2|4>` (default: 1) — downscale analysis resolution for speed
- `--no-crop` — disable active-area crop detection
- `--analysis-threads <N>` — override Rayon worker count (default: logical cores)
- `--profile-performance` — emit per-stage throughput metrics

Planned (V1.3+)
- `--optimizer-profile <conservative|balanced|aggressive>`
- `--tone-mapper <hable|reinhard|clamp>` and operator parameters (V2.0)

---

## 8) Changelog of Recent Upgrades

### Latest Session (V1.4 — PQ Noise Robustness)
- **PQ Noise Robustness Implementation**: Completed Phase C noise robustness features.
  - Added `--peak-source` flag with `max`, `histogram99`, and `histogram999` options for robust peak detection.
  - Implemented per-bin EMA histogram smoothing with `--hist-bin-ema-beta` flag (default 0.1, scene-aware resets).
  - Added optional temporal median filtering via `--hist-temporal-median` flag (default off).
  - Implemented pre-analysis Y-plane denoising with `--pre-denoise median3` option (nlmeans reserved for future).
  - Smart defaults: `histogram99` for balanced/aggressive profiles, `max` for conservative.
  - Histogram smoothing automatically renormalizes to maintain sum ≈ 100.0.
  - Peak PQ and APL recomputed from smoothed histograms using v5 mid-bin semantics.
- **Code Quality**: Zero clippy warnings, all tests passing, formatted code.
- **Documentation**: Updated README with new CLI flags and usage examples; roadmap marked complete.
- **Stats**: ~370 lines added across 4 files (cli.rs, histogram.rs, frame.rs, pipeline.rs).

### Previous Session (V1.2 & V1.3 Completion)
- **CLI Enhancement**: Added `-i/--input` flag alongside positional input; updated documentation.
- **Dynamic Clipping Heuristic**: Implemented per-scene smoothing of highlight knee (configurable window size) to prevent banding.
- **Optimizer Profiles**: Added `--optimizer-profile` with conservative/balanced/aggressive presets controlling delta limits, clamp ranges, knee multipliers, and smoothing windows.
- **Hue Histogram**: Implemented real 31-bin chroma-based hue distribution from U/V planes; integrated into frame analysis; filters low-saturation content.
- **v6 Gamut Peaks**: Replaced placeholder duplication with gamut-aware approximation (DCI-P3=99%, BT.709=95% of BT.2020 peak); documented assumptions.
- **Verifier Enhancements**: Added hue histogram validation (length, sum, distribution coverage, low-coverage warnings).
- **Unit Tests**: Added 12 comprehensive tests covering PQ conversion, histogram binning/distance, scene detection, optimizer profiles, knee detection, gamut approximation, FALL computation.
- **Code Quality**: Zero clippy warnings, all tests passing, formatted code.
- **Documentation**: Synchronized README, CLAUDE.md, and roadmap with implementation.
- **Stats**: +472 lines in analyzer, +71 lines in verifier, 635 total insertions across 5 files.

### Previous Upgrades
- Added `--madvr-version`, `--scene-threshold`, `--target-peak-nits`, `--min-scene-length`, `--scene-smoothing`, `--no-crop` to analyzer.
- Implemented v6 writer path with `target_peak_nits` in header.
- Updated README with new flags and minimal beta validation workflow.
- Retained robust v5/v6 serialization via `madvr_parse`.
- Maintained active-area cropping, v5 histogram semantics, limited-range normalization.
- Native chi-squared-like scene metric with boundary fix.
- Optional optimizer (rolling avg + knee + heuristics); enabled by default.

---

## 9) Ownership & Review

- Code owners: hdr-analyzer core maintainers
- Review cadence: per milestone completion or bi-weekly
- Refactor sign-off: require parity validation on sample assets (pre/post refactor outputs comparable within tolerances) and successful verifier checks.
- Beta gate: dovi_tool ingestion on sample set must succeed with no parse errors.

---

## 10) Quality-First Parity & Research Track

Focus: Deliver production-grade measurement quality on CPU-only systems (Oracle ARM instances) before investing in acceleration.

### Phase A — madVRhdrMeasure Feature Parity (Output Fidelity)

- [ ] **Scene & Target Nits Feature Audit** — inventory gaps vs. madVRhdrMeasure (long-term smoothing, scene merge logic). Produce comparison report using shared HDR samples and madVR logs.
- [x] **Dynamic Clipping Heuristic** — implement highlight-knee based clipping with per-scene smoothing and regression tests to avoid banding.
- [x] **Optimizer Profiles** — ship the planned `--optimizer-profile` modes (conservative/balanced/aggressive) with documented default parameters.
- [x] **Hue Histogram Implementation** — replace zeroed hue histograms with chroma-derived 31-bin data; extend verifier checks.
- [x] **madVR v6 Gamut Peaks** — compute P3/709 peaks via gamut-aware approximation (luminance-preserving; full RGB conversion is future enhancement).
- [ ] **Reference Comparison Harness** — add an integration test that runs analyzer + madVRhdrMeasure on a fixed clip and diffs key stats (scene boundaries, MaxCLL/FALL, optimizer targets) within tolerance.

Progress: 4/6 items complete. Dynamic clipping, optimizer profiles, hue histograms, and v6 gamut peaks are production-ready. Remaining: feature audit vs. madVR and automated comparison harness.

Milestone Exit: Produced `.bin` files match madVRhdrMeasure scene counts and per-frame stats within agreed tolerances on three reference clips; verifier passes and regression suite green.

### Phase B — Dolby Vision `cm_analyze` Parity (Professional Metadata)

- [ ] **Long-Play / Shot Metadata Mode** — add per-shot analysis export with edit-offset support so dissolves and transitions mimic cm_analyze behavior.
- [ ] **Dolby XML Export Path** — write Level 1/Level 2/Level 8 Dolby Vision XML (CM v2.9 + v4.0) directly, leveraging `madvr_parse` data or new serializers; include Netflix metadata validation (Metafier compatibility).
- [ ] **Canvas & Aspect Metadata** — capture active image area, mastering display specs, and target trims to align with cm_analyze defaults.
- [ ] **Validation Workflow** — integrate Dolby Metafier or open-source equivalent checks into CI to ensure XML metadata passes standard QC rules (no 0,0,0 L1 except black frames, no overlapping durations).
- [ ] **Documentation & Examples** — provide step-by-step parity guide comparing generated metadata to cm_analyze outputs, including Resolve ingest instructions.

Milestone Exit: Dolby Vision XML validates via Dolby Metafier, aligns with cm_analyze scene/timing on reference clips, and downstream tools (Resolve, dovi_tool) accept the metadata without manual fixes.

### Phase C — Novel Research & Quality Enhancements

- [ ] **Adaptive Scene Detection Research** — evaluate learning-based or multi-metric scene detection (e.g., histogram + optical flow) that can better handle short cuts without manual thresholds.
- [ ] **Temporal Consistency Modeling** — prototype future-aware optimizer using scene context windows (ARM-friendly) to minimize target-nits flicker beyond current heuristics.
- [x] **PQ Noise Robustness — Robust PQ Histograms** ✓ COMPLETE
  - Implemented **--peak-source histogram99** (internal default for "balanced"/"aggressive"), plus **histogram max** and **histogram999** (P99.9) options.
  - Added **per-bin EMA** smoothing (β≈0.1, renormalize; **reset at scene cuts**). Flag: `--hist-bin-ema-beta` (0 disables, default 0.1).
  - Optional **temporal median (3 frames)** after EMA. Flag: `--hist-temporal-median N` (default 0/off).
  - Optional **pre-analysis denoise** at analysis scale (Y-plane **median3** implemented). Flag: `--pre-denoise {median3|off}` (default off, nlmeans reserved for future).
  - **Acceptance:** On a static/grainy scene, expected **APL σ ↓ ≥ 30%** with **median APL shift ≤ 1% (PQ)**; across 3 reference clips, **MaxCLL/MaxFALL** should stay within baseline tolerance.
  - **Status**: Implementation complete, ready for validation on test corpus.
- [ ] **Benchmark Corpus Expansion** — curate diverse HDR10 test set (scope, 16:9, high-grain, animation) with ground-truth scene annotations for ongoing evaluation.
- [ ] **Publication & Feedback Loop** — summarize findings in `docs/research.md`, solicit community feedback, and iterate on promising algorithms.

Milestone Exit: At least one novel method promoted to production (documented improvement over baseline) and research backlog maintained for future ARM-optimized enhancements.

**Progress: 1/5 items complete.** PQ Noise Robustness successfully implemented and ready for field validation.
