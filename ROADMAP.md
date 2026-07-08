# HDR-Analyze Roadmap

This is the single source of truth for active `hdr-analyze` work. Completed changes belong in
[`CHANGELOG.md`](CHANGELOG.md); current user-facing behavior belongs in
[`docs/DOLBY_VISION.md`](docs/DOLBY_VISION.md); technical accuracy analysis belongs in
[`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md).

> **North star:** approach the accuracy and feature set of Dolby Vision's `cm_analyze` with a fully
> open-source, research-based analyzer. Parity is measured, never asserted.

Status meanings: **Open** has not shipped; **Partial** has useful pieces in place but does not meet
the stated outcome; **Core complete** meets the original gate but retains named follow-up work.

## Current status (v0.3.0)

The v0.3.0 release shipped the `mkvdolby` → `mkvdovi` rename (one-release compat), published measured
accuracy in [`docs/VALIDATION.md`](docs/VALIDATION.md), and made PQ direct peaks default to BT.2020 NCL
max-RGB. Against the parity workstreams below, **WS0 (validation foundation) is core complete** and the
**WS1 max-RGB peak has shipped**. Full shipped history lives in [`CHANGELOG.md`](CHANGELOG.md).

## Conversion quality

The current priority is accurate HDR10 / HDR10+ / HLG → Dolby Vision conversion. The intended default
is source-honest metadata: content-derived L1, neutral trims, and no display-specific optimizer target
baked in by default. Display-targeted behavior remains opt-in.

| ID | Status | Work |
|----|--------|------|
| **P0** | **Partial** | `--use-custom-targets` with optimizer data is confirmed to replace per-frame L1 max with optimizer `target_pq`, not measured peak. Isolate the remaining flag combinations and design source-honest frame edits before changing generation. |
| **P1** | **Open** | Make source-honest generation the default: stop passing optimizer targets by default and decide, from measurements, whether full-resolution analysis should become the quality-first preset. |
| **P2** | **Core complete** | PQ max-RGB peaks, robust stream-level crop probing, true per-pixel Y/max-RGB means, and a noise-rejected active-area minimum have shipped. The measured minimum remains in the JSON sidecar until WS1/P0 can deliver it safely into the RPU. |
| **P3** | **Open** | Emit L5 active-area offsets from the committed crop instead of accepting `dovi_tool` defaults. |
| **P4** | **Open** | Add an opt-in `--target-nits` display-targeted workflow and wire the chosen display peak into optimizer behavior. |
| **P5** | **Partial** | L9 detection now prefers mastering-display primaries and has an override. Still needed: `hdr10plus_tool extract --skip-reorder` fallback and BT.2020 mastering primaries for HLG→PQ output. |
| **P6** | **Partial** | Warnings exist for L6/L9 fallbacks and suspicious HDR10+ scene peaks. Add broader missing/inconsistent-source detection and route problematic inputs to the source-honest path once P1 exists. |

## `cm_analyze` parity

The detailed gap table and validation method live in
[`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md). These workstreams are dependency ordered.

| ID | Status | Work |
|----|--------|------|
| **WS0** | **Core complete** | `tools/l1_diff`, synthetic ground-truth tests, embedded-L1 comparison, and licensed `cm_analyze` scoring have shipped. Grow the corpus and add an automated `l1_diff` regression gate; the utility is currently excluded from workspace CI. |
| **WS1** | **Partial** | P2 measurement core is complete. Remaining delivery/accuracy work is measured-minimum RPU wiring, validation-driven average-domain selection, a grain-robust max-RGB peak estimator (real-content validation measured direct max +74…+93 codes hot vs `cm_analyze` v2; [VALIDATION.md §7](docs/VALIDATION.md)), true target-gamut peaks, and HLG max-RGB. |
| **WS2** | **Open** | Add per-shot L1 aggregation and an optional L4-style temporal filter/shot anchor. Promote hybrid scene detection only after it beats histogram-only against reference boundaries. |
| **WS3** | **Open** | Emit and validate L5 active-area metadata (P3). |
| **WS4** | **Open; experimental** | Derive optional L2/L8 trims from an open tone-mapping baseline such as ITU-R BT.2390. Neutral trims remain the default unless blinded A/B testing demonstrates an improvement. |
| **WS5** | **Open** | Export Dolby Vision metadata XML for Resolve/Metafier interchange and independent validation. |

## Engineering backlog

| ID | Status | Work |
|----|--------|------|
| **E1** | **Partial** | Expand the benchmark corpus and wire `tools/l1_diff` or `tools/compare_baseline` into CI as a numerical regression gate. Synthetic accuracy already runs in workspace CI. |
| **E2** | **Partial** | Seven-position crop probing, low-signal rejection, modal voting, and variable-AR union shipped in PR [#4](https://github.com/tinof/hdr-analyze/pull/4), closing issue [#3](https://github.com/tinof/hdr-analyze/issues/3). Per-scene crop application remains a continuity-sensitive follow-up. |
| **E3** | **Open** | Add `mkvdovi --dry-run`, `--keep-temp`, and `--keep-logs`. |
| **E4** | **Open** | Replace the `--scene-metric hybrid` histogram-only placeholder with histogram + optical-flow fusion and validate it against WS0 references. |
| **E5** | **Open** | Add proper VAAPI and VideoToolbox decode device contexts and hardware-frame transfer. |

## Explicit non-goals

- No proprietary Dolby code, LUTs, tone curves, or binary blobs; see
  [`docs/PROVENANCE.md`](docs/PROVENANCE.md).
- No silent peak clamping; suspicious values produce advisory warnings.
- No `cm_analyze` parity claim without reproducible measurements.

During the `0.x` series, minor releases may include breaking changes under the project's documented
[semantic-versioning](https://semver.org/) policy.
