# HDR-Analyze Roadmap

This is the single source of truth for active `hdr-analyze` work. Completed changes belong in
[`CHANGELOG.md`](CHANGELOG.md); current user-facing behavior belongs in
[`docs/FORMAT_COMPATIBILITY.md`](docs/FORMAT_COMPATIBILITY.md); technical accuracy analysis belongs in
[`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md).

> **North star:** a fully open-source, research-based analyzer whose measurement accuracy stands up
> against the reference tooling it is compared to. Parity is measured, never asserted.

Status meanings: **Open** has not shipped; **Partial** has useful pieces in place but does not meet
the stated outcome; **Core complete** meets the original gate but retains named follow-up work;
**Deferred** is intentionally not scheduled. Items with no follow-up left move to the changelog.

## Current status (updated 2026-10-01)

The v0.3.0 release shipped the `mkvdolby` → `mkvdovi` rename, published measured accuracy in
[`docs/VALIDATION.md`](docs/VALIDATION.md), and made PQ direct peaks default to BT.2020 NCL max-RGB.
Since then, measured per-scene L1 reaches the RPU by default, and v0.4.0 adds L5 from the
committed crop, measurement and resume provenance, frame-coverage verification, an analyzer input
contract, and FEL chroma fixes (see [`CHANGELOG.md`](CHANGELOG.md)).

An external review on 2026-09-15 re-audited this inventory against the code. Its findings are folded
into the tables and priorities below.

v0.5.0 converts HLG to Profile 8.4 without re-encoding and measures HLG max-RGB on the full 8.4
decode. v0.5.1 made CUDA analysis 2.5–3.9× faster with identical L1 output. Two consequences for
the work below:

- `--peak-estimator robust` runs on CPU and CUDA with identical output since 2026-10-02;
  `--pre-denoise median3` is still CPU-only. A new estimator has to run on both backends with
  identical results, or the hosts that use the fast path never get it.
- Every measurement change is now checked by two gates before it merges: the L1 regression gate
  in CI (`scripts/ci/l1-regression-gate.sh`) and, for changes to the analysis or decode path, the
  local CPU/CUDA parity check (`scripts/cuda-parity.sh`). Hosted CI has no GPU, so the second one
  is run by hand on a CUDA host.

## Progress log

Newest first. One line per step that changed the state of a roadmap item; details are in the
pull request and in [`CHANGELOG.md`](CHANGELOG.md).

| Date | Step | Items | Where |
|------|------|-------|-------|
| 2026-10-02 | `--peak-estimator robust` runs in the CUDA kernel with output identical to the CPU (kernel fills the cross-quad difference histogram; CPU max-RGB mix and histogram binning moved to the kernel's f32 arithmetic). No algorithm change. A real-content run now takes the same time as the default estimator, so estimator candidates can be scored in minutes. First scores of the unchanged estimator: clean/grain pair bias +102.7 → −1.1 codes per frame, but the largest error grows (251 → 333) and shot-to-shot spikes get larger; one grainy retail cut +76.1 → +64.0 against its embedded L1. | WS1 | [#21](https://github.com/tinof/hdr-analyze/pull/21) |
| 2026-10-02 | Scene detection rebuilt: cuts chosen after analysis from a flash-tolerant score and the local frame-to-frame level, strongest candidate first. Authored shot lists of ten real-content cuts: 142 of 168 cuts matched with 13 extra (before: 91 matched, 228 extra). The fixed threshold was saturated by grain on about half of all frames; picture type played no role. Open: pictures that change completely on every frame. The CI clip now has unequal shot lengths and a constructed scene reference. | WS2, E1 | [#20](https://github.com/tinof/hdr-analyze/pull/20) |
| 2026-10-01 | Measurement study for the grain-robust peak started: how isolated the raw peak is on the available clips, spatial-support and temporal candidates, CUDA feasibility. Blocker for promotion: the two grainy titles and their `cm_analyze` v2 output from [VALIDATION.md §7](docs/VALIDATION.md) are not on the development host. | WS1, WS2 | in progress |
| 2026-10-01 | Scene averages from unfiltered frame means (sidecar version 4). `mkvdovi` regenerates a stale RPU on resume. | WS2 | [#18](https://github.com/tinof/hdr-analyze/pull/18) |
| 2026-10-01 | Final RPU checked against the sidecar on real HDR10 and HLG clips; 0.5.1 final-RPU baselines captured for five clips. | P0, WS6 | [#17](https://github.com/tinof/hdr-analyze/pull/17) |
| 2026-10-01 | `l1_diff` limits, L1 regression gate in CI, CPU/CUDA parity check, final-RPU baseline script. The gate's first CI run exposed a lossy test-clip encode on newer FFmpeg; fixed. | E1, WS0 | [#17](https://github.com/tinof/hdr-analyze/pull/17) |

## Development principles

- **Three achievements, three kinds of evidence.** Measuring source luminance accurately is shown by
  synthetic ground truth and reference-analyzer scores. Generating valid metadata is shown by
  structural checks on the final muxed file. Better-looking playback is shown only by matched playback
  tests on real hardware. The project has evidence for the first two; claims about the third need WS6.
- **Parity is a benchmark, not proof of better playback.** Matching a reference analyzer's numbers
  does not by itself show that the resulting tone mapping looks better.
- **Correctness before features.** Conversion correctness and playback validation come before new
  capabilities.
- **No silent degradation.** A path that produces weaker metadata must be explicit and visible.
- **Source-faithful defaults.** Measured L1 and neutral trims stay the default; display-targeted
  behavior stays opt-in.
- **Measured research claims only.** Unmeasured expectations, such as the retired "80-90% of FEL
  intent" estimate, are not targets.

## Priorities

| Priority | Work | IDs |
|----------|------|-----|
| **First milestone** | Dependable HDR10/HDR10+ → Profile 8.1: source-derived L1 on every path, L5 delivery, measurement and resume provenance, stronger completeness checks, input contract. Code landed 2026-09-15; a real-content end-to-end run is required before release. Source retention was reviewed and the current default (delete a non-DV source after success) is **kept by decision**. | P0, P1, P3, P7, E6, E7 |
| **Second** | Regression gates: `l1_diff` limits in CI, CPU/CUDA parity check, final-RPU baseline capture. Landed 2026-10-01. | E1, WS0 |
| **Done first under Third** | Unbiased scene averages (sidecar version 4), 2026-10-01. The gates above caught and scored the change: the CI gate failed until its references were rewritten, and the final-RPU comparison showed only the L1 average moving. | WS2 |
| **Third** | Grain handling with spatial support and temporal persistence, robust shot aggregation, unbiased scene averages, scene-boundary quality. Designed for CPU and CUDA together. A new estimator ships opt-in and becomes the default only after the `cm_analyze` v2 gate, the final-RPU comparison and a matched playback test all pass. | WS1, WS2 |
| **Then** | Final-RPU regression corpus and a written Shield/TV playback test procedure. The corpus can grow while the third item is in progress; the playback procedure needs its candidate. | WS6 |
| **Fourth** | Independent validation of FEL compositing; explicit FEL-discard mode | F1, F2 |
| **Research track** | Small FEL metadata-fit feasibility experiment | R1 |
| **Conditional** | Profile 5 through an established encoder, only if matched playback tests justify it | R2, WS5 |

**Deferred:** display-target optimizers (P4), automatic creative trims (WS4), mandatory optical-flow
scene detection (E4), and broader hardware acceleration (E5). Neutral trims stay.

## Conversion quality

| ID | Status | Work |
|----|--------|------|
| **P0** | **Core complete** | Measured per-scene L1 (minimum, max-RGB mean, maximum) reaches the RPU as explicit `dovi_tool generate` shots and bypasses optimizer targets. A missing, invalid, or mismatched sidecar re-runs analysis. The old `--madvr-file --use-custom-targets` generation, where L1 max follows optimizer `target_pq` and L1 avg is a placeholder, is reachable only through `--legacy-madvr-l1`. Checked 2026-10-01 on the final RPU of one real HDR10 clip (33 scenes), three real HLG clips (96 scenes) and the synthetic HDR10 clip: every scene's L1 equals the sidecar after `dovi_tool`'s spec limits (minimum at most 12, maximum at least 2081, average at least 819). |
| **P1** | **Partial** | Source-honest generation is the default. Open: decide whether full-resolution every-frame analysis becomes the CPU default. Today `auto` resolves to `accurate` only with CUDA analysis and to `balanced` otherwise; measure the CPU cost first. |
| **P3** | **Core complete** | L5 offsets come from the committed full-resolution crop (sidecar v2); sampled source L5 keeps precedence for Dolby Vision inputs. Open: changing aspect ratios need per-scene offsets, treated as a separate validation problem. |
| **P4** | **Deferred** | Opt-in `--target-nits` display-targeted workflow wired into optimizer behavior. |
| **P5** | **Partial** | L9 detection prefers mastering-display primaries and has an override. Still needed: `hdr10plus_tool extract --skip-reorder` fallback. (The HLG→PQ mastering-primaries item is gone: HLG now converts to Profile 8.4 without re-encoding.) |
| **P6** | **Partial** | Warnings exist for L6/L9 fallbacks and suspicious HDR10+ scene peaks. Add broader missing/inconsistent-source detection. |
| **P7** | **Core complete** | Analyzer input contract: only PQ and HLG transfers are analyzed; tagged SDR transfers, including the BT.2020 10/12-bit tags that share the BT.709 curve, are refused. Full-range and non-BT.2020 matrix tags warn. Open: normalize full-range input instead of warning. |

## `cm_analyze` parity

The detailed gap table and validation method live in
[`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md). These workstreams are dependency ordered.

| ID | Status | Work |
|----|--------|------|
| **WS0** | **Core complete** | `tools/l1_diff`, synthetic ground-truth tests, embedded-L1 comparison, and licensed `cm_analyze` scoring have shipped. `l1_diff` takes limits (`--max-peak-bias`, `--max-peak-error` and the same for minimum and max-RGB average) and exits nonzero on a breach; CI runs it on a generated six-shot PQ and HLG clip against committed references. Open: grow the corpus. |
| **WS1** | **Partial** | Measurement core, measured-minimum delivery, and the max-RGB-mean average domain have shipped. Open: a grain-robust peak. Raw max-RGB reads +92.6 / +74.4 codes hot against `cm_analyze` v2; the opt-in robust estimator reached +80.4 / +66.4 and missed its promotion gate ([VALIDATION.md §7](docs/VALIDATION.md)). Investigate spatial support and temporal persistence together while preserving genuine small speculars; do not enable the current robust estimator by default. The new estimator must run on CPU and CUDA with identical output (integer counts, fixed-point sums, order-independent reductions; see [`docs/CUDA_PIPELINE.md`](docs/CUDA_PIPELINE.md)), as the current `robust` estimator does since 2026-10-02 (`median3` is still CPU-only). Its frozen synthetic gate must include what temporal persistence can wrongly remove: a small specular, a one-frame flash, a highlight on the first or last frame of a shot, a 2–3 frame specular and a fade. HLG max-RGB shipped in v0.5.0. Also open: true target-gamut peaks. |
| **WS2** | **Partial** | Initial shot aggregation shipped: the shot maximum is the maximum of its frame peaks, so one retained grain spike can set a whole shot. Scene averages are unbiased since sidecar version 4 (2026-10-01): they are the mean of unfiltered frame means; before, a forward-only EMA made a 24-frame fade read 893 codes instead of 1569 ([VALIDATION.md §9](docs/VALIDATION.md)). The `cm_analyze` v2 average comparison has not been repeated for version 4. Scene boundaries (2026-10-02): the detector chooses cuts after analysis from a flash-tolerant score against the local frame-to-frame level; 142 of 168 authored cuts matched with 13 extra on ten real-content cuts. Open: robust aggregation (investigated with WS1), an optional L4-style temporal filter, and a second scene signal for pictures that change completely on every frame (E4), promoted only after it beats histogram-only against reference boundaries. |
| **WS4** | **Deferred; experimental** | Optional L2/L8 trims from an open tone-mapping baseline such as ITU-R BT.2390. Neutral trims remain the default unless blinded A/B testing demonstrates an improvement. |
| **WS5** | **Open; conditional** | CM metadata XML export. Worthwhile only if the external Profile 5 authoring route (R2) is chosen. |
| **WS6** | **Open** | Final-RPU regression corpus covering grain, saturated highlights, raised blacks, fades, flashes, rapid cuts, and changing aspect ratios. Checks run on the RPU extracted from the muxed file. Add a Shield/TV playback procedure comparing matched material from the same master, recording player, firmware, TV picture mode, and HDMI path. |

## Dolby Vision profile work

| ID | Status | Work |
|----|--------|------|
| **F1** | **Partial; experimental** | FEL compositing. Chroma MMR now uses the co-located 2×2 luma mean and the real opposite chroma plane (fixed 2026-09-15; the earlier code used a flat `i*4` luma index and neutral Cr when reshaping Cb). Still unvalidated against an independent reference decode with exact frame alignment. |
| **F2** | **Open** | Explicit FEL-discard mode: keep the untouched BL, analyze it, and generate BL-appropriate metadata. Retaining FEL-targeted metadata after dropping the EL can produce wrong brightness on some titles. |
| **R1** | **Research** | FEL → metadata fit. It is not possible generally or losslessly: FEL carries residual picture data, and identical BL values that need different reconstructed values at different positions cannot share one pixel-value transform. A 10-bit display does not remove that limit. Order: (1) independent FEL reference and exact frame alignment; (2) minimal decoder/profile feasibility test: does the target playback device apply a non-identity fitted transform in Profile 8.1, and does the bitstream meet profile constraints; (3) residual collision and spatial analysis; (4) fitting on a few representative scenes; (5) quantized-output comparison against BL-only and reference playback. Depends on F1. |
| **R2** | **Conditional** | Profile 5. Genuine Profile 5 uses the IPT signal representation, so HDR10 BL → Profile 5 requires decoding, color transformation, and re-encoding. Feasible through an established encoder (Resolve Studio, Dolby Encoding Engine); an open-source Profile 5 encoder in this project is substantial separate work; changing profile flags on untouched HEVC is not a target. Precondition: a matched HDR10 / P8.1 / correctly encoded P5 playback comparison from the same master with the playback chain recorded. |

## Engineering backlog

| ID | Status | Work |
|----|--------|------|
| **E1** | **Core complete** | `tools/l1_diff` runs in CI as a numerical regression gate (`scripts/ci/l1-regression-gate.sh`, references in `tools/l1_diff/corpus`); `scripts/cuda-parity.sh` checks CPU against CUDA output on a GPU host; `scripts/rpu-baseline.sh` captures and compares final RPUs. Synthetic accuracy runs in workspace CI. Open: expand the corpus (feeds WS6); a self-hosted GPU runner would automate the parity check. |
| **E2** | **Partial** | Seven-position crop probing, low-signal rejection, modal voting, and variable-AR union shipped in PR [#4](https://github.com/tinof/hdr-analyze/pull/4), closing issue [#3](https://github.com/tinof/hdr-analyze/issues/3). Per-scene crop application remains a continuity-sensitive follow-up. |
| **E3** | **Open** | Add `mkvdovi --dry-run`, `--keep-temp`, and `--keep-logs`. |
| **E4** | **Deferred** | Replace the `--scene-metric hybrid` histogram-only placeholder with histogram + optical-flow fusion and validate it against WS0 references. |
| **E5** | **Deferred** | Add proper VAAPI and VideoToolbox decode device contexts and hardware-frame transfer. |
| **E6** | **Core complete** | Provenance. Sidecar v2 records analyzer version, input name and size, dimensions, transfer, downscale, sample rate, GPU use, and crop. Resume temp directories carry a fingerprint of input name, size, and mtime, mkvdovi version, and artifact-affecting settings. Open: record external tool versions (`dovi_tool`, `mkvmerge`, `ffmpeg`) for each output. |
| **E7** | **Partial** | Completeness. `--verify` fails when the RPU frame count differs from the muxed video track or the L1 sidecar, and warns when output and input frame counts differ. Open: direct-MKV `dovi_tool` steps still accept exit status plus non-empty output, and verification is opt-in. |

## Explicit non-goals

- No proprietary Dolby code, LUTs, tone curves, or binary blobs in this repository; see
  [`docs/PROVENANCE.md`](docs/PROVENANCE.md) for the provenance statement and its stated limits.
- No silent peak clamping; suspicious values produce advisory warnings.
- No `cm_analyze` parity claim without reproducible measurements.

During the `0.x` series, minor releases may include breaking changes under the project's documented
[semantic-versioning](https://semver.org/) policy.
