# HDR-Analyze Roadmap

This document outlines the development roadmap for `hdr-analyze`: current status, near-term engineering,
and the long-term path toward **Dolby Vision `cm_analyze` parity**. For completed history, see
[`CHANGELOG.md`](CHANGELOG.md). For the detailed parity gap analysis and design, see
[`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md).

> **North star:** approach the accuracy and feature set of Dolby Vision's `cm_analyze` with a fully
> open-source, research-based analyzer — experimental, but engineering-grounded. Parity is something we
> **measure**, never assert.

> **Versioning:** [Semantic Versioning](https://semver.org/). During the `0.x` series, minor version
> bumps may include breaking changes.

---

## Conversion Quality (current priority)

Perfecting HDR10 / HDR10+ / HLG → Dolby Vision conversion is the active focus. Full findings and
rationale in [`docs/CONVERSION_QUALITY.md`](docs/CONVERSION_QUALITY.md). External tools are current
(`dovi_tool 2.3.2`, `hdr10plus_tool 1.7.1`).

> **Decision:** the default output becomes **source-honest / reference-accurate** — accurate content L1,
> neutral trims, no optimizer target-nits baked into the RPU; the display does its own mapping. The
> madMeasureHDR-style per-display optimizer (e.g. "680 nits") becomes an **opt-in `--target-nits`**.

- [ ] **P0 — Verify empirically** *(gate)*: `dovi_tool info` on a generated RPU to confirm exactly what
  we emit and what `--use-custom-targets` does (= [WS0](docs/CM_ANALYZE_PARITY.md)).
- [ ] **P1 — Source-honest default**: stop baking optimizer targets by default; consider full-res
  analysis as the quality-first default.
- [ ] **P2 — Robust L1 min + true-mean avg** (targets raised blacks) — see [WS1](docs/CM_ANALYZE_PARITY.md).
- [ ] **P3 — Emit L5 active area** from detected crop (letterbox/levels) — see [WS3](docs/CM_ANALYZE_PARITY.md).
- [ ] **P4 — `--target-nits` opt-in** display-targeted mode (the 680-nits workflow).
- [ ] **P5 — Robustness**: `hdr10plus_tool --skip-reorder` fallback; HLG `master-display` primaries →
  BT.2020; harden L9 detection.
- [ ] **P6 — Problematic-source detection** (warn + default to the safe path).

---

## Current Status (v0.2.0)

Shipped and in validation:

- **Three binaries released** for Linux, macOS (Intel + Apple Silicon), and Windows —
  `hdr_analyzer_mvp`, `mkvdovi`, `verifier` (plus `mkvdovi_hifi_workflow.sh` on Unix).
- **Dolby Vision CM v4.0** is the default `mkvdovi` output: L1 (from HDR10+ or `hdr_analyzer_mvp`),
  neutral L2 compatibility trims, L6, L9 (auto-detected primaries), L11 (content type / reference mode),
  L254 (via `dovi_tool`).
- **HDR10+ peak mapping** with corrected `--peak-source` defaults; advisory warnings (never silent
  clamps) when scene L1 peaks exceed 3× the mastering-display peak.
- **`--analysis-quality`** presets (`fast` / `balanced` (default) / `accurate`).
- **`--verify`** post-mux validation hardened (Profile 8, ordered L1, sane L6, required L9/L11/L254).
- **Noise robustness**: percentile peaks (P99/P99.9), per-bin EMA, temporal median, `median3` denoise.
- **Native HLG** in-memory HLG→PQ; **NVIDIA CUDA** analysis hint; stable Rust toolchain.
- **Docs**: README slimmed to a front door; full reference split into `docs/` (CLI, Dolby Vision,
  technical). Crop single-frame limitation documented (issue #3).

---

## Near-term Engineering

Practical improvements, independent of the parity workstreams below:

1. **Benchmark corpus & CI regression** *(also gates parity — see WS0)*: curated reference clips with
   ground-truth L1, `dovi_tool` smoke tests, and `tools/compare_baseline` wired into CI.
2. **Crop detection robustness** (issue [#3](https://github.com/tinof/hdr-analyze/issues/3)): stream-level
   multi-frame probing instead of trusting the first analyzed frame; per-scene crop as a follow-up.
3. **mkvdovi UX**: `--dry-run` (preview commands), `--keep-temp` / `--keep-logs` for debugging.
4. **Hybrid scene metric**: finish `--scene-metric hybrid` (histogram + optical flow); currently a
   prototype that falls back to histogram-only.
5. **Hardware decode contexts**: full VAAPI / VideoToolbox *decode* paths (encode via VideoToolbox already
   works on Apple Silicon).

---

## Path to `cm_analyze` Parity

Dependency-ordered workstreams. Full detail, per-level gap table, and validation methodology in
[`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md). Nothing claims parity until **WS0** can measure
it.

- [ ] **WS0 — Validation foundation** *(prerequisite for all parity claims)*: reference corpus from real
  DV masters (reference L1 via `dovi_tool extract-rpu` + export) and ITU/Dolby test patterns; an L1 diff
  harness (extending `tools/compare_baseline`) reporting min/avg/max error in PQ; CI smoke comparison.
- [ ] **WS1 — L1 accuracy**: robust low-percentile **min** (active area, noise-rejected), true
  full-precision **mean** avg, luminance/**max-RGB peak** (subsumes "v6 full-RGB gamut peaks"), and
  active-area correctness (consumes the multi-frame crop from issue #3).
- [ ] **WS2 — Shot model & temporal stability**: per-shot L1 aggregation and an optional L4-style
  temporal filter / shot anchor; promote the hybrid scene metric once it beats histogram-only on WS0.
- [ ] **WS3 — L5 active-area emission**: emit correct L5 offsets from the detected crop instead of
  `dovi_tool` defaults.
- [ ] **WS4 — L2/L8 trim derivation** *(experimental, opt-in, A/B-gated)*: derive target-display trims
  from an open tone-mapping baseline (ITU-R BT.2390 EETF). **Neutral L2 stays the default**; non-neutral
  output ships only if blinded A/B on real content shows it is at least as good as neutral.
- [ ] **WS5 — Dolby Vision XML export**: DV metadata XML for DaVinci Resolve / Dolby Metafier; doubles as
  an independent validation/interchange path.

---

## Done

- [x] **CM v4.0 metadata generation** (L1/L2/L6/L9/L11 via `extra.json` for `dovi_tool`) — see
  [`docs/cmv40_upgrade_implementation_plan.md`](docs/cmv40_upgrade_implementation_plan.md).
- [x] **Release packaging** of all three binaries; README/docs refresh.
- [x] **Native HLG**, **noise-robust peaks**, **scene-aware optimizer**, **CUDA analysis hint**.

---

## Explicit Non-Goals

- No proprietary Dolby code, LUTs, tone curves, or binary blobs (see
  [`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md#7-non-goals--risk-register)).
- No silent peak clamping (advisory warnings only).
- No claim of `cm_analyze` parity without WS0 measurements to back it.
