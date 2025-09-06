# HDR-Analyze Roadmap (Consolidated)

This document consolidates and supersedes:
- tasks/scene-detection-v2-plan.md
- tasks/to-do.md
- the previous contents of tasks/roadmap.md

It reflects the current state after the latest code upgrades and lays out the next milestones with clear definitions of done and acceptance criteria.

---

## 0) Current Status (after latest upgrades)

Implemented (latest changes)
- CLI options
  - `--madvr-version 5|6` (default: 5). For v6, `header_size=36`, `target_peak_nits` is written (default: MaxCLL or overridden via `--target-peak-nits`), and per-frame gamut peaks (`peak_pq_dcip3`, `peak_pq_709`) are temporarily duplicated from `peak_pq_2020` until real gamut logic lands.
  - `--scene-threshold <float>` (default: 0.3) to tune histogram-distance scene cut sensitivity.
  - `--target-peak-nits <nits>` for v6 header override.
  - `--min-scene-length <frames>` (default: 24) — drop cuts occurring within N frames of the previous cut.
  - `--scene-smoothing <frames>` (default: 5) — rolling mean smoothing over the scene-change metric (set 0 to disable).
  - `--no-crop` — disable active-area crop detection (analyze full frame; diagnostics/validation).
- Active area (black bar) detection and cropping
  - New `hdr_analyzer_mvp/src/crop.rs` with crop-detect-like algorithm on Y (10-bit), sampling every 10 px, ~10% non-black threshold, rounded to even coords/dims.
  - Detected once and applied to all frames; analysis constrained to `CropRect`.
- v5 histogram semantics and avg computation
  - 256-bin mapping matching v5 semantics: 64 bins up to pq(100 nits), 192 bins pq(100)→1.0; mid-bin weighting; avg-pq computed likewise; bin0 black-bar heuristic for avg.
- Limited-range normalization
  - HDR10 Y’ nominal 64–940 normalized to [0..1] as PQ proxy prior to binning (robust with limited-range material).
- Native scene detection
  - Histogram distance (chi-squared-like, symmetric) between consecutive frame histograms; default threshold = 0.3; post-processing fixes end-frame off-by-one.
  - New controls: min scene length guard (default 24) and temporal smoothing of the difference signal (default 5 frames; set 0 to disable).
- Optimizer (optional)
  - Rolling 240-frame average + highlight knee (99th percentile) + scene-aware heuristics (by APL category); writes per-frame `target_nits` when enabled (flags=3).
  - Scene-aware improvements: blends per-scene APL with rolling APL, resets smoothing at scene boundaries, and applies per-frame delta limiting for temporal stability.
  - Enabled by default; can be disabled via `--disable-optimizer`.
- Header fields and writer
  - `maxCLL` from per-frame peak (nits), `maxFALL` and `avgFALL` derived from per-frame avg-pq (nits). Serialization via `madvr_parse` (v5 or v6).
- Verifier
  - Parses measurement, prints summary, validates scene/frame ranges, histogram integrity (256 bins, sum ≈100), PQ range checks; reports optimizer presence.
  - Additional checks: recomputes MaxFALL/AvgFALL and compares to header within tolerance; validates flags vs. presence of per-frame `target_nits`.
- Documentation
  - Root README updated to reflect current behavior, new flags, and a minimal beta validation workflow.

Partially implemented (work remains)
- madVR v6 support: Writer and header present; per-gamut peaks currently duplicated from 2020 (need true P3/709 computation).
- Hardware acceleration: CUDA attempted via `hevc_cuvid` if available; VAAPI/VideoToolbox paths currently fall back to software decode (device contexts not wired).

Not yet implemented
- Hue histogram (31 bins) with meaningful content.
- Optimizer profiles (`--optimizer-profile`) and additional tunables.
- Proper VAAPI/VideoToolbox device context setup for hardware decoding.
- Parallel analysis (rayon) for multi-core throughput.
- Broader unit/integration tests and CI coverage.

---

## 1) V1.2 — Core Accuracy Release (Beta Stabilization)

Objective: Produce stable, v5/v6-compatible measurements with accurate active-area cropping, correct histogram semantics, and reliable scene detection suitable for dovi_tool ingestion.

Already Done
- Black bar detection with crop; v5 histogram semantics; limited-range normalization.
- Histogram-distance scene cut with threshold control; boundary fix.
- FALL metrics; v5/v6 writing; updated README; basic verifier.

Remaining To Complete V1.2
- Documentation
  - [ ] Expand README with additional scene control examples as needed; document v6 gamut caveat (temporary duplication) — partial.

Definition of Done (V1.2)
- New flags available and defaults yield stable cuts on typical content.
- Verifier passes on produced .bin; FALL header values match derived values within tolerance; flags/data consistent.
- dovi_tool measurement-based generation accepts .bin on test clips without parse errors.
- No unused warnings in release build.

Acceptance Criteria
- On 3 diverse HDR10 samples (letterboxed scope, 16:9 TV, bright demo):
  - Scene boundaries visually align (±1 frame) with ground truth/madVR on the majority of cuts.
  - APL/peaks reasonable; no black-bar contamination.
  - dovi_tool run completes and generates RPU.

---

## 2) Milestone R — Refactor `main.rs` (Modularization)

Problem: `hdr_analyzer_mvp/src/main.rs` has grown beyond 1100 lines, mixing CLI, decode/scaling, analysis, detection, optimization, and writing. This hinders testability and evolution.

Target module structure
- `hdr_analyzer_mvp/src/cli.rs` — CLI definition and parsing
- `hdr_analyzer_mvp/src/ffmpeg_io.rs` — init, input open, best stream, decoder setup, scaler setup (incl. future VAAPI/VT device contexts)
- `hdr_analyzer_mvp/src/analysis/mod.rs`
  - `histogram.rs` — v5 binning constants, mapping, accumulation, avg computation, black-bar heuristic
  - `frame.rs` — per-frame analysis entry points operating on Y plane; active-area application
  - `scene.rs` — histogram-distance computation, smoothing, min-length guard, conversion to scenes
  - `crop.rs` — existing crop detection (move/keep; it already exists)
- `hdr_analyzer_mvp/src/optimizer.rs` — rolling avg, knee detection, heuristics; future profiles
- `hdr_analyzer_mvp/src/writer.rs` — v5/v6 writer using `madvr_parse`
- `hdr_analyzer_mvp/src/pipeline.rs` — orchestration (end-to-end run), progress reporting
- `hdr_analyzer_mvp/src/types.rs` (optional) — thin wrappers/types if needed (but reuse `madvr_parse` structs primarily)

Refactor plan (incremental, safe)
- [ ] Step 1: Extract CLI to `cli.rs`; import into `main.rs`.
- [ ] Step 2: Extract writer to `writer.rs` (pure function taking scenes/frames/opts).
- [ ] Step 3: Extract scene metric and helpers to `analysis/scene.rs`.
- [ ] Step 4: Extract histogram utilities to `analysis/histogram.rs`.
- [ ] Step 5: Extract per-frame analysis to `analysis/frame.rs` (uses histogram utils).
- [ ] Step 6: Extract FFmpeg init/open/decoder/scaler to `ffmpeg_io.rs`.
- [ ] Step 7: Create `pipeline.rs` to orchestrate; `main.rs` becomes thin: parse CLI → pipeline::run() → exit.
- [ ] Step 8: Wire unit tests for histogram binning/avg, scene diff metric, and FALL conversions.
- [ ] Step 9: Add benches (optional) for per-frame analysis and histogram accumulation (baseline for perf regressions).

Acceptance criteria
- Behavior-preserving: On sample input, produced `.bin` files parse identically (version/flags/frame+scene counts equal; histogram sums within ±0.5%; APL/peaks within negligible tolerance) compared to pre-refactor baseline.
- Dev ergonomics: `main.rs` ≤ 300 lines; functions ≤ ~150 lines per file; cyclomatic complexity reduced.
- Build: no new warnings in release; CI (when added) passes tests.

Risks and mitigations
- Hidden coupling between steps — mitigate by adding unit tests per module and incremental commits.
- Performance regressions — mitigate by benchmarking before/after; avoid extra allocations.

---

## 3) V1.3 — Advanced Optimization & Format

Objective: Improve per-frame target selection and expand complete v6 format support.

Planned
- Scene-aware optimizer
  - [x] Pass `scene_avg_pq` and related aggregates into `apply_advanced_heuristics`.
  - [ ] Add profiles: `--optimizer-profile <conservative|balanced|aggressive>` to adjust bounds/weights.
- Hue histogram (31 bins)
  - [ ] Populate from chroma-based hue angle quantization; low-cost approach acceptable.
- madVR v6 completeness
  - [ ] Compute per-frame `peak_pq_dcip3` and `peak_pq_709` via gamut conversion (or approximate mapping); remove duplication placeholder.
  - [ ] Extend verifier to validate v6-specific fields where applicable.
- Documentation
  - [ ] Update README on profiles and v6 completeness.

Definition of Done (V1.3)
- Optimizer yields smoother, scene-consistent `target_nits` with less temporal flicker.
- Hue histogram non-zero, plausible distribution.
- v6 outputs with proper gamut peaks parse via `madvr_parse` and external tools.

Acceptance Criteria
- On reference clips, `target_nits` transitions are visually smooth at scene boundaries and within scenes.
- v6 outputs validate and ingest in downstream tools expecting v6.

---

## 4) V1.4 — Performance & Parallelization

Objective: Improve throughput on multi-core systems (e.g., Ampere ARM).

Planned
- [ ] Parallelize histogram accumulation by rows/tiles using `rayon`.
- [ ] Optional lock-free accumulators or per-thread buffers + reduce.
- [ ] Consider SIMD for hot loops (optional).
- [ ] Benchmarks on representative 4K HEVC HDR samples.

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

Unit tests
- [ ] Crop detection on synthetic letterboxed frames.
- [ ] Histogram bin selection correctness (bin edges, mid-bin mapping).
- [ ] Chi-squared detector thresholds; min-scene-length logic; smoothing.
- [ ] FALL computations vs known inputs.

Integration tests
- [ ] Analyze sample HDR10 assets; verify with `verifier`.
- [ ] Compare scene boundaries and stats against a madVR-produced measurement when available.
- [ ] Run dovi_tool measurement-based workflow (smoke test).

Performance
- [ ] Benchmark decode/analysis fps (SW vs HW when available).
- [ ] Prevent regressions with benches.

CI (recommended)
- [ ] Build on Linux/macOS (GitHub Actions).
- [ ] Run unit tests; run integration on small sample clip (time-bounded).
- [ ] Artifact: attach verifier logs.

---

## 7) CLI Flags (Current/Planned)

Current
- `--input`, `--output`
- `--enable-optimizer`
- `--hwaccel <cuda|vaapi|videotoolbox>` (CUDA attempted; VAAPI/VT currently fall back to SW)
- `--madvr-version <5|6>` (default: 5)
- `--scene-threshold <float>` (default: 0.3)
- `--target-peak-nits <nits>` (v6 header override; default: MaxCLL)

Planned (V1.2)
- `--min-scene-length <frames>` (default: 24)
- `--scene-smoothing <frames>` (default: 0 = off)
- `--no-crop`

Planned (V1.3+)
- `--optimizer-profile <conservative|balanced|aggressive>`
- `--tone-mapper <hable|reinhard|clamp>` and operator parameters (V2.0)

---

## 8) Changelog of Recent Upgrades (for context)

- Added `--madvr-version`, `--scene-threshold`, `--target-peak-nits` to analyzer.
- Implemented v6 writer path with temporary duplication of gamut peaks; `target_peak_nits` in header.
- Updated README with new flags and a minimal beta validation workflow.
- Retained robust v5/v6 serialization via `madvr_parse`.
- Maintained active-area cropping, v5 histogram semantics, limited-range normalization.
- Native chi-squared-like scene metric with boundary fix.
- Optional optimizer (rolling avg + knee + heuristics).

---

## 9) Ownership & Review

- Code owners: hdr-analyzer core maintainers
- Review cadence: per milestone completion or bi-weekly
- Refactor sign-off: require parity validation on sample assets (pre/post refactor outputs comparable within tolerances) and successful verifier checks.
- Beta gate: dovi_tool ingestion on sample set must succeed with no parse errors.
