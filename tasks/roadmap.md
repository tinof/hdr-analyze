# HDR-Analyze Roadmap (Consolidated)

This document consolidates and supersedes:
- tasks/scene-detection-v2-plan.md
- tasks/to-do.md
- the previous contents of tasks/roadmap.md

It reflects the current state after the latest code upgrades and lays out the next milestones with clear definitions of done and acceptance criteria.

---

## 0) Current Status (after latest upgrades)

Implemented
- Active area (black bar) detection and cropping
  - New `hdr_analyzer_mvp/src/crop.rs` with crop-detect-like algorithm on Y (10-bit), sampling every 10 px, ~10% non-black threshold, rounded to even coordinates/dimensions.
  - Detected once and applied to all frames; analysis constrained to `CropRect`.
- Correct madVR v5 histogram binning and avg computation
  - 256-bin mapping matching madVR semantics: 64 bins up to pq(100 nits), 192 bins from pq(100) to 1.0; mid-bin weighting; avg-pq computed likewise; black-bar bin0 heuristic applied for avg.
- Limited-range normalization
  - 10-bit Y’ treated as limited range (nominal 64–940); normalized to 0..1 as PQ proxy before binning.
- Native scene detection (basic)
  - Chi-squared distance between consecutive frame histograms; default threshold = 0.3.
  - Fixed scene boundary off-by-one (cut marks first frame of new scene; previous ends at cut-1).
- Header fields
  - `maxCLL` from per-frame peak; `maxFALL` and `avgFALL` from per-frame avg-pq converted to nits.
- Build/verify
  - Project builds successfully; `verifier` reads and validates produced .bin files.

Not yet implemented
- CLI controls for scene detector (threshold, min scene length, toggles), and crop disable.cargo
- Temporal smoothing/rolling window for scene detector; minimum scene duration guard.
- Hue histogram (31 bins) content.
- Scene-aware optimizer (use `scene_avg_pq` in decisions).
- madVR v6 format: per-gamut frame peaks (`peak_pq_dcip3`, `peak_pq_709`) and optional `target_peak_nits` in header.
- Proper VAAPI/VideoToolbox device context setup for hardware decoding.
- Optional RGB→PQ luminance analysis path (exactness vs Y’-proxy).
- Cleanup of unused functions/warnings; tests/docs.

---

## 1) V1.2 — Core Accuracy Release

Objective: Produce madVR v5-compatible measurements with accurate active-area cropping, correct histogram semantics, and reliable native scene detection suitable for dovi_tool ingestion.

Already Done
- Black bar detection (crop once; constrain analysis)
- v5 histogram semantics and avg-pq computation
- Limited-range normalization
- Native chi-squared scene detection; boundary fix
- FALL metrics in header

Remaining To Complete V1.2
- Scene detector controls
  - `--scene-threshold <float>` (default: 0.3)
  - `--min-scene-length <frames>` (default: 24)
  - Optional `--scene-smoothing <frames>` to average diffs (default: 0 = off)
- Crop toggle
  - `--no-crop` to disable crop detection
- Verifier enhancements
  - Recompute/print derived FALL from histogram avg-pq to sanity-check header values; verify flags vs data consistency
- Cleanup
  - Remove legacy `analyze_native_frame` or prefix unused args with `_`
  - Silence dead-code warnings
- Documentation
  - Update README usage, flags, and verification steps

Definition of Done (V1.2)
- CLI supports threshold/min-duration/no-crop; defaults yield stable cuts on typical content.
- Verifier passes on produced .bin; FALL header and derived FALL match within reasonable tolerance.
- dovi_tool measurement-based generation accepts .bin and produces stable DV RPU on test clips.
- No unused warnings in release build.

Acceptance Criteria
- On 3 diverse HDR10 samples (letterboxed scope, 16:9 TV show, bright demo reel):
  - Scene boundaries visually align (±1 frame) with ground truth/madVR on majority of cuts.
  - APL and peaks look reasonable; no evident black-bar contamination.
  - dovi_tool run completes without parse/format errors.

---

## 2) V1.3 — Advanced Optimization & Format

Objective: Improve per-frame target selection with scene-aware logic and expand format support.

Planned
- Scene-aware optimizer
  - Pass `scene_avg_pq` into `apply_advanced_heuristics`
  - Strategy selection by scene: dark/medium/bright baseline; refine with rolling avg and per-frame highlights
  - Optional CLI: `--optimizer-profile <conservative|balanced|aggressive>`
- Hue histogram (31 bins)
  - Populate meaningful 31-bin hue histogram (e.g., angle quantization from chroma; low-cost approach acceptable)
- madVR v6 format (optional but recommended)
  - CLI: `--format-version 6`
  - Compute per-frame `peak_pq_dcip3` and `peak_pq_709` via gamut conversion
  - Write v6 header with `target_peak_nits` when applicable
- Hardware acceleration
  - Implement VAAPI/VideoToolbox device contexts; document CUDA availability/constraints

Definition of Done (V1.3)
- Optimizer uses `scene_avg_pq` and rolling averages; produces smoother, scene-consistent `target_nits`.
- Hue histogram block filled (non-zero; plausible distribution).
- Optionally, v6 output selected by CLI; per-gamut peaks present and parseable; verifier extended to validate v6 fields.
- HW accel working paths (at least one of VAAPI or VideoToolbox) validated on a supported platform.

Acceptance Criteria
- On test clips, optimizer reduces temporal flicker of `target_nits`; scene transitions feel smooth.
- v6 files parse via `madvr_parse` and any external tools expecting v6.
- Performance improved where HW accel is enabled compared to software decode.

---

## 3) V2.0 — Perceptual Engine

Objective: Configurable tone-mapping operators and a more expressive metadata generation pipeline.

Planned
- Tone mapping operators
  - `tonemap/` module with `hable.rs`, `reinhard.rs`
  - CLI: `--tone-mapper <hable|reinhard|clamp>` (+ parameters per operator)
- Optimizer outputs operator parameters
  - Not just `target_nits`; produce a parameter set (contrast, shoulder, knee) per scene/frame based on analysis
- Optional fidelity path
  - Add an RGB-based analysis path computing PQ luminance using BT.2020 coefficients prior to histogramming; keep Y’ path as fast default
  - CLI toggle to choose analysis mode

Definition of Done (V2.0)
- Operators selectable; parameterized; metadata reflects operator choice.
- Documentation explains tradeoffs and recommended profiles.

Acceptance Criteria
- Visual validation: chosen operators yield expected behavior on reference scenes (highlight roll-off, shadow detail).
- Performance acceptable with Y’ path; RGB analysis path documented with expected overhead.

---

## 4) Technical Specifications (Reference)

4.1 Histogram Binning (madVR v5 semantics)
- Bins: 256 total
  - 0..63: PQ range [0, pq(100 nits)] with equal step; mid-bin used for averaging
  - 64..255: PQ range [pq(100 nits), 1.0] with equal step; mid-bin used
- Avg-pq computation
  - Weighted sum of mid-bin PQ values by percent; adjust by sum of histogram bars
  - Black-bar heuristic: skip bin 0 when it is between ~2% and ~30% for avg computation

4.2 Black Bar Detection (implemented)
- Scan Y’ plane (10-bit), sample every 10 px
- Non-black if normalized limited-range value > ~0.01
- Row/column active if ≥10% samples non-black
- Round coordinates and sizes to even; clamp within frame

4.3 Dynamic Metadata Fields
- Header v5: version=5, header_size=32, frame/scene counts, flags (2: no custom target; 3: with per-frame target), maxCLL, maxFALL, avgFALL
- Per-frame: `peak_pq_2020`, 256-bin luminance histogram, 31-bin hue histogram (required by lib)
- If flags==3: custom per-frame `target_nits` block
- v6 (planned): `peak_pq_dcip3`, `peak_pq_709`; header `target_peak_nits`

4.4 FALL Calculations
- FALL per-frame = inversePQ(avg_pq) in nits
- `maxFALL` = ceil(max per-frame FALL), `avgFALL` = ceil(mean per-frame FALL)

---

## 5) Work Plan & Milestones

Milestone A — Finalize V1.2 (Core Accuracy)
- [ ] Add CLI flags: `--scene-threshold`, `--min-scene-length`, `--scene-smoothing`, `--no-crop`
- [ ] Implement min-scene-length guard (drop cuts inside N frames)
- [ ] Optional: smoothing window for scene diff (rolling average)
- [ ] Verifier: recompute FALL, validate flags vs data
- [ ] Cleanup: remove dead code/warnings; doc updates

Milestone B — V1.3 (Optimizer & Format)
- [ ] Pass `scene_avg_pq` to optimizer; strategy selection by scene type
- [ ] Fill hue histogram (31 bins) from chroma-based hue angle quantization
- [ ] Add `--format-version 6`; compute P3/709 peaks; write v6 header (`target_peak_nits` when provided)
- [ ] Implement VAAPI/VideoToolbox device contexts; doc CUDA/VAAPI/VT usage

Milestone C — V2.0 (Perceptual Engine)
- [ ] Implement `tonemap/` module with Hable/Reinhard operators
- [ ] CLI: `--tone-mapper` with operator-specific parameters
- [ ] Optimizer outputs operator parameters, not just `target_nits`
- [ ] Optional: RGB→PQ luminance analysis mode

---

## 6) Validation & QA

- Unit tests
  - Crop detection on synthetic letterboxed frames
  - Histogram bin selection correctness (bin edges, mid-bin mapping)
  - Chi-squared detector thresholds; min-scene-length logic
  - FALL computations vs known inputs
- Integration tests
  - Run analyzer on sample HDR10 assets; verify with `verifier`
  - Compare scene boundaries and statistics to a madVR-produced measurement (when available)
  - Run dovi_tool measurement-based workflow to ensure end-to-end compatibility
- Performance
  - Benchmark decode/analysis fps on software vs HW-accelerated paths

---

## 7) CLI Flags (Planned/Current)

Current
- `--input`, `--output`
- `--enable_optimizer`
- `--hwaccel <cuda|vaapi|videotoolbox>` (CUDA path attempts; VAAPI/VT currently fall back to SW)

Planned (V1.2)
- `--scene-threshold <float>` (default: 0.3)
- `--min-scene-length <frames>` (default: 24)
- `--scene-smoothing <frames>` (default: 0 = off)
- `--no-crop`

Planned (V1.3+)
- `--format-version <5|6>` (default: 5)
- `--optimizer-profile <conservative|balanced|aggressive>`
- `--tone-mapper <hable|reinhard|clamp>` and operator parameters (V2.0)

---

## 8) Changelog of Recent Upgrades (for context)

- Added `crop.rs` and integrated active-area cropping into analysis
- Implemented madVR v5 histogram binning and avg-pq computation
- Normalized Y’ limited range; used as PQ proxy
- Replaced SAD with chi-squared scene metric; fixed off-by-one scene boundaries
- Computed and wrote `maxFALL` and `avgFALL` in header (v5)
- Ensured .bin outputs parse via `madvr_parse`; `verifier` can read and validate

---

## 9) Ownership & Review

- Code owners: hdr-analyzer core maintainers
- Review cadence: per milestone completion or bi-weekly
- Sign-off: require verification on sample assets and dovi_tool ingestion for each milestone
