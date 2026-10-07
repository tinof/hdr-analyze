# HDR-Analyze Roadmap

This is the single source of truth for active `hdr-analyze` work. Completed changes belong in
[`CHANGELOG.md`](CHANGELOG.md); current user-facing behavior belongs in
[`docs/FORMAT_COMPATIBILITY.md`](docs/FORMAT_COMPATIBILITY.md); technical accuracy analysis belongs in
[`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md). The full text of each progress step and the
measurement detail behind the items are in [`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md).

> **North star:** a fully open-source, research-based analyzer whose measurement accuracy stands up
> against the reference tooling it is compared to. Parity is measured, never asserted.

Status meanings: **Open** has not shipped; **Partial** has useful pieces in place but does not meet
the stated outcome; **Core complete** meets the original gate but retains named follow-up work;
**Deferred** is intentionally not scheduled. Items with no follow-up left move to the changelog.

**How to update.** Change the item's status and open steps in place. Add one row to the progress
log (one sentence plus the PR link) and the full text of the step to
[`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md). Measurements go to the log or to the doc the item
links, not into the item. `/roadmap-next` keeps **Next up** current and adds a **Checked** bullet
(date, commit, `file:line`) to each item it examined, so a later session re-checks only what
changed since that commit.

## Current status (updated 2026-10-05)

The v0.3.0 release shipped the `mkvdolby` → `mkvdovi` rename, published measured accuracy in
[`docs/VALIDATION.md`](docs/VALIDATION.md), and made PQ direct peaks default to BT.2020 NCL max-RGB.
Since then, measured per-scene L1 reaches the RPU by default, and v0.4.0 adds L5 from the
committed crop, measurement and resume provenance, frame-coverage verification, an analyzer input
contract, and FEL chroma fixes (see [`CHANGELOG.md`](CHANGELOG.md)).

An external review on 2026-09-15 re-audited this inventory against the code. Its findings are folded
into the items and priorities below.

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

A review on 2026-10-03 checked the conversion paths against the public specifications, the
`dolby_vision` crate, `dovi_tool` 2.3.4 output and the retail RPUs of the test cuts. Three results
change the order of work:

- The Profile 7 FEL compositor produced wrong pixels on real discs and was removed. `mkvdovi`
  refuses Profile 7 FEL inputs and no longer encodes video on any path. A design that keeps the base
  layer bit-exact is tracked in [`docs/FEL_PLAN.md`](docs/FEL_PLAN.md).
- HLG inputs get wrong metadata on every file: the Profile 8.4 composer tints neutrals (P8).
- The L1 average derived from HDR10+ metadata read 156 to 505 codes above the measured one in all
  11 scenes of the one HDR10+ test cut (P9).
- What the generator writes is not what the analyzer measured: `dovi_tool generate` clamps L1 and
  derived the source range from a coarse lookup (P10; the source range is fixed since 2026-10-04). Statements that the measured minimum is
  delivered were wrong and are corrected here and in
  [`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md).

Quality has so far been scored only in PQ codes. The same review recomputed, with open tone curves,
what an L1 error costs on a display (WS7); that scale now decides which measurement work is worth
doing.

### Next up (checked 2026-10-06 at main@df54ad4)

1. **E11:** `l1_diff` shifts by `leading_skipped_frames` (`fix-l1-diff-open-gop`, `tools/l1_diff`
   only). Gate: the E11 Fix, with a synthetic open-GOP sidecar test. No file overlap with the P8
   branch and no CUDA gate; unblocks scoring Joker against its retail L1 (see E11 Checked). Run the
   L1 regression gate after P8's benchmark, not during it.
2. **P9 step 0:** frame alignment of pixel measurements to HDR10+ scenes on the HDR10+-only cuts,
   before any hybrid flag (see P9 Checked). Shares `mkvdovi/src/{metadata,pipeline}.rs` with the
   P8 branch, so start it after P8 lands.
3. **E8 + E9** (one branch): needs CUDA parity and edits `cli.rs` like the P8 branch; after P8.

### Waiting for the owner

Steps that need a decision or a hardware test from the owner. Other steps of the same item stay
startable. `/roadmap-next` reads this list; remove an entry when it is settled.

- **P8: playback test.** Do Shield/TV devices apply the bt2100 composer, and how visible is the
  preset's tint (WS6 procedure)? The chroma-siting comparison does not wait for this.
- **P9: default change.** Whether hybrid mode becomes the default, after its development-tier score.
- **P10 step 3: write L1 in-process.** Needs WS7 or playback evidence first.
- **P11: targeted repair or full regeneration.** The notice that lists the dropped authored levels
  does not wait for this.

## Progress log

Newest first. One line per step that changed the state of a roadmap item. The full text is in
[`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md); details are in the pull request and in
[`CHANGELOG.md`](CHANGELOG.md).

| Date | Step | Items | Where |
|------|------|-------|-------|
| 2026-10-06 | CUDA as main pipeline: warn on `balanced`/`fast` with GPU, re-analyze coarser sidecars under `accurate`, warn on CPU-analyzed CUDA run; CI lints `cuda`. | P1, E1 | [#31](https://github.com/tinof/hdr-analyze/pull/31) |
| 2026-10-06 | Review with CUDA as the main pipeline: `accurate` is the only parity-checked preset; release binaries and hosted CI never build the `cuda` feature; P1, E1, E2 amended, E12 opened. | P1, E1, E2, E12 | this file |
| 2026-10-06 | `l1_diff` lines references up with open-GOP cuts by `leading_skipped_frames`; exports are labelled in stream frames. | E11 | [#30](https://github.com/tinof/hdr-analyze/pull/30) |
| 2026-10-06 | Analyzer measures HLG max-RGB through the spec 4:2:0 decode (`spec-float`); gate passed at 0.000 codes, CPU = CUDA; new sidecar names, older sidecars re-analyzed. | P8 | [#29](https://github.com/tinof/hdr-analyze/pull/29) |
| 2026-10-06 | Spec fixed-point arithmetic measured: `bt2100` is not neutral at `code / 1024` (spread 5.9 codes); the decode change targets the spec structure in `code / 1023` (`spec-float`). | P8, WS6 | [log](docs/ROADMAP_LOG.md) |
| 2026-10-05 | 4:2:0 chroma of the HLG decode measured against the spec composer and libplacebo: the analyzer differs from the spec by up to 18 codes of L1 max per scene; spec decode step opened. | P8 | [log](docs/ROADMAP_LOG.md) |
| 2026-10-05 | Real material per format added: four HDR10+ titles (one same-master DV MEL + HDR10+ pair), four HLG cuts, six HDR10/MEL titles; 29 holdout cuts. | WS8, P9, P8, WS1 | [log](docs/ROADMAP_LOG.md) |
| 2026-10-05 | Coverage of real material per format reviewed; HDR10+ and HLG material comes before P9 and P8 work. | WS8, P9, P8 | this file |
| 2026-10-04 | L1 lines up with the picture in open-GOP cuts (RASL leading pictures); sidecar v5. | E7 | [#26](https://github.com/tinof/hdr-analyze/pull/26) |
| 2026-10-04 | 8.1 source range from the mastering display; `--verify` compares delivered with measured L1. | P10, E10 | [#25](https://github.com/tinof/hdr-analyze/pull/25) |
| 2026-10-03 | FEL compositor and re-encode removed; FEL inputs refused. | FEL | [FEL_PLAN](docs/FEL_PLAN.md) |
| 2026-10-03 | Review of all conversion paths; priorities reordered; new items. | P8–P11, E8–E10, WS7, WS8 | this file |
| 2026-10-02 | Robust peak estimator reads the shape of the histogram top. | WS1, WS2 | [#22](https://github.com/tinof/hdr-analyze/pull/22) |
| 2026-10-02 | `--peak-estimator robust` in the CUDA kernel, identical to CPU. | WS1 | [#21](https://github.com/tinof/hdr-analyze/pull/21) |
| 2026-10-02 | Scene cuts chosen after analysis; 142 of 168 authored cuts matched. | WS2, E1 | [#20](https://github.com/tinof/hdr-analyze/pull/20) |
| 2026-10-01 | Grain-robust peak measurement study started. | WS1, WS2 | in progress |
| 2026-10-01 | Unfiltered scene averages (sidecar v4); stale RPU regenerated on resume. | WS2 | [#18](https://github.com/tinof/hdr-analyze/pull/18) |
| 2026-10-01 | Final RPU checked against the sidecar; 0.5.1 RPU baselines captured. | P0, WS6 | [#17](https://github.com/tinof/hdr-analyze/pull/17) |
| 2026-10-01 | `l1_diff` limits, L1 gate in CI, CUDA parity check, RPU baseline script. | E1, WS0 | [#17](https://github.com/tinof/hdr-analyze/pull/17) |

## Development principles

- **Three achievements, three kinds of evidence.** Measuring source luminance accurately is shown by
  synthetic ground truth and reference-analyzer scores. Generating valid metadata is shown by
  structural checks on the final muxed file. Better-looking playback is shown only by matched playback
  tests on real hardware. The project has evidence for the first two; claims about the third need WS6.
- **Parity is a benchmark, not proof of better playback.** Matching a reference analyzer's numbers
  does not by itself show that the resulting tone mapping looks better.
- **Correctness before features.** Conversion correctness and playback validation come before new
  capabilities.
- **Delivered output decides the order.** A defect that makes the delivered file wrong for every
  input of a type comes before any accuracy improvement. What counts is the value in the final RPU
  and the pixels a display reconstructs from it, not the value the analyzer measured.
- **The video stream is never re-encoded.** Every `mkvdovi` path copies the video bit-exactly; a
  feature that needs an encode is out of scope.
- **No silent degradation.** A path that produces weaker metadata must be explicit and visible.
- **Source-faithful defaults.** Measured L1 and neutral trims stay the default; display-targeted
  behavior stays opt-in.
- **Measured research claims only.** Unmeasured expectations, such as the retired "80-90% of FEL
  intent" estimate, are not targets.

## Priorities

Order (2026-10-03): defects that make delivered output wrong for every file of an input type; then
the evaluation infrastructure every later decision needs; then improvements with measured evidence;
then research. Items 1 to 5 are about what is delivered, 6 and 7 about how it is judged. The
evidence for each priority is in its items and in
[`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md#priority-table).

1. **HLG → Profile 8.4 composer** (P8, P7). `--hlg-composer bt2100` and the input refusal landed
   2026-10-03; bt2100 is the default by owner decision. Gate: the WS6 playback test shows that
   devices apply a mapping that is not the preset; `--hlg-composer preset` is the fallback.
2. **HDR10+ → Profile 8.1 hybrid mode** (P9): scene list and peak from HDR10+; average, minimum and
   crop from pixels. Gate: opt-in, scored on the development tier before any default change.
   HDR10+ material is in (WS8 item 5, 2026-10-05), including a same-master Dolby Vision pair.
3. **Deliver what was measured** (P10): source range and the measured-against-delivered report
   landed 2026-10-04; open: decide whether to write L1 in-process. Gate: unclamped L1 needs WS7 or
   a playback test.
4. **Narrow code defects** (E8, E9, P1, E10): histogram percentile reader, `--pre-denoise` values,
   half-resolution CPU default, L1 scaling note. Gate: unit tests for the reader and the option;
   measure the CPU default first.
5. **`--mdfix` replaces the whole RPU** (P11): targeted repair or full regeneration. Open decision.
6. **Evaluation in displayed-picture units** (WS7): histogram-domain display-mapping simulator, then
   ColorVideoVDP on a few cuts. Gate: the simulator reproduces the recomputed numbers in WS7.
7. **Test material** (WS8): grain on synthetic highlights, unaligned positions, a same-master grainy
   twin, transitions. Needed before 9 can be scored. Real material per format (item 5) landed
   2026-10-05.
8. **Scene detection** (WS2, E4): second signal for whole-picture motion; strong cuts closer than
   12 frames. Gate: precision and recall against authored shot lists, no loss on the other cuts.
9. **Research, default unchanged** (WS1, WS2, WS4): grain-robust peak; per-frame L1 inside detected
   transitions; L4; automatic trims. Each stays opt-in or unbuilt until its gate is met.
10. **Profile 7 FEL: placeholder** (FEL). See [`docs/FEL_PLAN.md`](docs/FEL_PLAN.md).
- **Conditional** (R2, WS5): Profile 5 through an established encoder, only if matched playback
  tests justify it.

**Landed before this order:** dependable HDR10/HDR10+ → Profile 8.1 with source-derived L1, L5,
provenance, completeness checks and the input contract (P0, P1, P3, P7, E6, E7; code 2026-09-15;
source retention reviewed and the default, delete a non-DV source after success, **kept by
decision**); regression gates (E1, WS0; 2026-10-01); unbiased scene averages (WS2; 2026-10-01).
The final-RPU regression corpus and the written Shield/TV playback procedure (WS6) grow alongside
items 1 to 3, which each need a playback check.

**Deferred:** display-target optimizers (P4), automatic creative trims (WS4, behind the WS7 gate),
and broader hardware acceleration (E5). Neutral trims stay.

## Conversion quality

### P0: measured per-scene L1 in the RPU

- **Status:** Core complete.
- Measured per-scene L1 (minimum, max-RGB mean, maximum) reaches the RPU as explicit
  `dovi_tool generate` shots and bypasses optimizer targets. A missing, invalid or mismatched
  sidecar re-runs analysis. The old optimizer-target generation is reachable only through
  `--legacy-madvr-l1`.
- Checked 2026-10-01 on the final RPU of real HDR10 and HLG clips: every scene's L1 equals the
  sidecar after the generator's limits. What those limits do to the measured values is P10.

### P1: CPU analysis default

- **Status:** Partial. Source-honest generation is the default. CUDA is the main pipeline;
  `accurate` (downscale 1, sample-rate 1) is the only parity-checked measurement. On CUDA,
  `balanced`/`fast` sample with a stride and the CPU resizes, so those presets differ by backend and
  no gate covers them ([`docs/CUDA_PIPELINE.md`](docs/CUDA_PIPELINE.md)). They also save no time on
  CUDA, where the run is NVDEC-bound, and they lose the measured MaxCLL (`fast` also MaxFALL).
- **Open:**
  1. Decide the CPU fallback default. `auto` resolves to `accurate` only with CUDA, otherwise
     `balanced`, which downsizes with `FAST_BILINEAR` at half resolution and averages small
     highlights. Measure the CPU cost of `accurate` and the effect of `FAST_BILINEAR` on L1 max, then
     flip CPU `auto` to `accurate` (the same file then gives the same L1 on any host) or keep
     `balanced` with a visible note. `fast` puts cuts on a 3-frame grid and misses peaks on skipped
     frames.
- **Done 2026-10-06** (former open items 2–4; evidence: unit tests `coarser_sampling`,
  `rejects_coarser_sidecar`, `analysis_quality_notice`, `gpu_analysis_missing` in
  `mkvdovi/src/pipeline.rs`, e2e test `accurate_reanalyzes_coarser_measurements` in
  `mkvdovi/tests/hlg_profile84.rs`): mkvdovi warns on an explicit `balanced`/`fast` with GPU
  analysis; under `accurate` it re-analyzes a coarser sidecar instead of reusing it; it warns when a
  run that expected GPU analysis comes back with `analysis.gpu: false`.

### P3: L5 from the crop

- **Status:** Core complete. L5 offsets come from the committed full-resolution crop (sidecar v2);
  sampled source L5 keeps precedence for Dolby Vision inputs.
- **Open:** changing aspect ratios need per-scene offsets, treated as a separate validation problem.

### P4: display-targeted workflow

- **Status:** Deferred. Opt-in `--target-nits` display-targeted workflow wired into optimizer
  behavior.

### P5: L9 primaries

- **Status:** Partial. L9 detection prefers mastering-display primaries and has an override.
- **Open:** `hdr10plus_tool extract --skip-reorder` fallback. (The HLG→PQ mastering-primaries item
  is gone: HLG now converts to Profile 8.4 without re-encoding.)

### P6: source consistency warnings

- **Status:** Partial. Warnings exist for L6/L9 fallbacks and suspicious HDR10+ scene peaks.
- **Open:** broader missing/inconsistent-source detection.

### P7: analyzer input contract

- **Status:** Core complete. Only PQ and HLG transfers are analyzed; tagged SDR transfers, including
  the BT.2020 10/12-bit tags that share the BT.709 curve, are refused. Full-range and non-BT.2020
  matrix tags warn.
- **Open:** for PQ, normalize full-range input or refuse it. (The HLG half shipped with P8:
  `mkvdovi` refuses full-range or non-BT.2020 HLG, `metadata::check_hlg_colour_contract`; the
  analyzer itself still only warns.)

### P8: HLG → Profile 8.4 composer

- **Status:** Landed; default bt2100; playback test open.
- `--hlg-composer bt2100` (2026-10-03) is fitted to the BT.2100 / BT.2408 1000-nit HLG-to-PQ
  conversion and keeps neutrals neutral; the HLG colour input contract is enforced. Design and
  measurements: [`docs/HLG_COMPOSER.md`](docs/HLG_COMPOSER.md). The preset it replaces tints
  neutrals (75% grey ΔE_ITP 6.8, up to 10.3 near black) and decodes nominal white to about
  1150 nits.
- **Open:** the WS6 playback test must show that devices apply a composer that is not the preset,
  and how visible the tint is on a TV. Real HLG material for both: five cuts since 2026-10-05
  (live sport at 50p, a concert, a nature series, a broadcast capture, a drama; WS8 item 5).
- **Done 2026-10-06: spec 4:2:0 decode in the analyzer** (both composers;
  [#29](https://github.com/tinof/hdr-analyze/pull/29)). Max-RGB runs the chroma MMR at chroma resolution on down-sampled
  luma in `code / 1023` f32 (`spec-float`; `code / 1024` leaves `bt2100` 5.9 codes off neutral) and
  upsamples the composed chroma bilinearly at the stream's chroma location; CPU and CUDA identical.
  New sidecar names `dovi84-v3` / `dovi84-bt2100-v1-spec420`; older sidecars are re-analyzed, and
  mkvdovi refuses an analyzer whose `--help` does not name the mapping. Gate
  (`scripts/validate_hlg_chroma_siting.sh --gate`): 0.000 codes on the synthetic patterns and the
  five cuts. Scene L1 max moves by up to 19 codes on the two cuts compared. Evidence: [log](docs/ROADMAP_LOG.md) 2026-10-06.

### P9: HDR10+ → Profile 8.1 L1

- **Status:** Open.
- `dovi_tool generate --hdr10plus-json` takes L1 from the first frame of each HDR10+ scene
  (minimum 0, average rounded to whole nits, no measured crop). On the HDR10+ test cut the average
  reads 156 to 505 codes above the analyzer's mean; the cause is not separated.
- **Prerequisite met (2026-10-05):** four HDR10+ titles from two studios and one streamer
  (WS8 item 5). Alita: Battle Angel carries Dolby Vision MEL and HDR10+ on the same disc, so its
  retail L1 can score an HDR10+-derived L1 frame by frame.
- **Finding for the fallback:** both HDR10+ Profile A discs (Alien, Alita) carry MaxSCL 0 on every
  frame, but the percentile distribution (99th percentile 90 to 129 nits per scene on Alita) and
  the average are present. A peak taken from MaxSCL is unusable on such sources, so the pixel
  fallback is required, not optional; whether the percentiles can stand in is open.
- **Plan:** opt-in hybrid mode (HDR10+ scene list and peak; average, minimum and L5 from pixels),
  scored on the development tier before any default change, with a pixel fallback for missing or
  implausible HDR10+ statistics. The panel peak is still not passed as a trim target, and
  suspicious scene peaks still only warn.
- **Checked 2026-10-06 @ab13527:** a hybrid flag is premature: `pipeline.rs:1649` passes
  `--hdr10plus-json` (measured shots ignored; no `--verify` sidecar for HDR10+, `pipeline.rs:901`);
  `metadata.rs` detects Dolby Vision before HDR10+, so Alita takes the MEL path; the sidecar has no
  per-frame peak, and mkvdovi reads only per-frame minima (`metadata.rs:986` `L1SidecarFrames`).
  All 45 development manifests say `shotlist_checked: false`.

### P10: measured against delivered

- **Status:** Partial. Steps 1 and 2 landed 2026-10-04.
- `dovi_tool generate` clamps L1 (minimum ≤ 12, maximum ≥ 2081, average ≥ 819). Numbers:
  [`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md).
- **Done:**
  - (1) Profile 8.1 RPUs carry `source_min_pq` / `source_max_pq` converted from the mastering
    display, instead of the generator's L6 lookup. Implausible mastering values (peak outside
    100–10000 nits, minimum above 1 nit) keep the lookup, with a warning.
  - (2) `--verify` checks the source range and the clamped measured L1 on every frame of a
    generated RPU, and reports per field how many scenes the generator's limits changed.
- **Open:** (3) decide whether to write L1 in-process so the measured minimum and sub-100-nit
  maxima reach the RPU. Step 3 needs evidence from WS7 or playback; raising the average floor to
  1229 is not planned.
- **Also open:** generated L2 carries `ms_weight` 2048 where retail RPUs carry 512 or 0; the effect
  is unverified.

### P11: `--mdfix` and authored metadata

- **Status:** Open; decision.
- `--mdfix` regenerates the whole RPU: authored L2 and L4 become neutral L2 and no L4, authored L5
  is reduced to one sampled value, L6 comes from the container. Retail trims are far from neutral.
- **Decision:** a targeted repair that keeps unaffected levels, against the finding that trims were
  authored for the authored L1 and shot list. Until decided, `--mdfix` should state in its output
  which authored levels it drops.
- **Checked 2026-10-05 @9edee09:** the source RPU is classified from a 240-frame sample
  (`rpu_check::extract_rpu_sample`), so the dropped-levels notice can only name levels seen in
  that sample unless the step reads the whole RPU.

## `cm_analyze` parity

The detailed gap table and validation method live in
[`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md). These workstreams are dependency ordered.

### WS0: L1 scoring tools

- **Status:** Core complete. `tools/l1_diff`, synthetic ground-truth tests, embedded-L1
  comparison and licensed `cm_analyze` scoring have shipped. `l1_diff` takes limits
  (`--max-peak-bias`, `--max-peak-error` and the same for minimum and max-RGB average) and exits
  nonzero on a breach; CI runs it on a generated six-shot PQ and HLG clip against committed
  references.
- **Open:** grow the corpus.

### WS1: grain-robust peak

- **Status:** Partial. Measurement core, per-scene minimum (clamped by the generator, P10) and the
  max-RGB-mean average domain have shipped. HLG max-RGB shipped in v0.5.0.
- **Open:** a grain-robust peak. Raw max-RGB reads hot against `cm_analyze` v2
  ([VALIDATION.md §7](docs/VALIDATION.md)). The current opt-in `robust` rule
  ([TECHNICAL_REFERENCE.md §2.4](docs/TECHNICAL_REFERENCE.md)) removes part of the grain bias but
  lowers small highlights near the grain and clean content, and has not been scored against
  `cm_analyze`. No histogram-only or spatial statistic tried so far passes.
- **Finding (2026-10-05):** the default `max` peak also reads high on clean retail sources:
  per-shot bias against retail L1 is +148 to +162 codes on Alita: Battle Angel (digital, largely
  computer-generated) and +88 and +162 on Skyfall (both Profile 7 MEL). Grain does not explain
  that. Find what does (for example the reference's spatial or temporal treatment of small
  highlights) before more grain-estimator work.
- **Next:** explain the clean-source over-read, then the display-unit scale (WS7). Do not enable
  the robust estimator by default.
- **Rules for a new estimator:** identical output on CPU and CUDA (see
  [`docs/CUDA_PIPELINE.md`](docs/CUDA_PIPELINE.md)); its frozen synthetic gate includes a small
  specular, a one-frame flash, a highlight on the first or last frame of a shot, a 2–3 frame
  specular and a fade.
- **Also open:** true target-gamut peaks.

### WS2: shots and scene averages

- **Status:** Partial. The shot maximum is the maximum of its frame peaks. Scene averages are
  unbiased since sidecar v4 ([VALIDATION.md §9](docs/VALIDATION.md)); the `cm_analyze` v2 average
  comparison has not been repeated for v4. Cuts are chosen after analysis (2026-10-02). Retail RPUs
  change L1 only at shot starts, so shot-constant L1 is kept.
- **Open, in this order:**
  1. A second scene signal for whole-picture motion (E4), and strong cuts closer than
     `--min-scene-length` 12, by a confidence exception, not a lower global minimum.
  2. Research, opt-in: per-frame L1 inside detected dissolves and fades; needs a per-frame peak in
     the sidecar (a version bump) and transition test material (WS8), and can cause pumping.
  3. L4 emission, deferred until something can validate it (no open renderer reads L4).
  4. Robust aggregation (investigated with WS1).

### WS4: automatic trims

- **Status:** Deferred; experimental. Optional L2/L8 trims from an open tone-mapping baseline such
  as ITU-R BT.2390. Neutral trims remain the default unless blinded A/B testing demonstrates an
  improvement. Not started before WS7 exists, and WS7 alone cannot pass it: no open renderer applies
  trims, so a trim can only be validated on a device.

### WS5: CM metadata XML export

- **Status:** Open; conditional. Worthwhile only if the external Profile 5 authoring route (R2) is
  chosen.

### WS6: final-RPU corpus and playback procedure

- **Status:** Open.
- Final-RPU regression corpus covering grain, saturated highlights, raised blacks, fades, flashes,
  rapid cuts and changing aspect ratios; checks run on the RPU extracted from the muxed file.
- A Shield/TV playback procedure comparing matched material from the same master, recording player,
  firmware, TV picture mode and HDMI path. It is the gate for P8 and for the default changes of P9
  and P10.
- **Open question (2026-10-06):** do devices evaluate the 8.4 composer at `code / 1023` (as
  libplacebo and the `bt2100` fit) or at the spec's `code / 1024`? Under the latter `bt2100` is
  5.9 codes off neutral, probably too little to see on a neutral ramp, so the test needs a pattern
  that separates the two. A `code / 1024` answer means a refit (new composer and RPU).

### WS7: evaluation in displayed-picture units

- **Status:** Open.
- **Plan:** (1) a histogram-domain display-mapping simulator in a tool crate outside the workspace:
  per-frame PQ histograms and an L1 stream through libplacebo's spline, the SMPTE ST 2094-10 curve
  and the ITU-R BT.2390 EETF for 100-, 600- and 1000-nit panels, comparing candidate with reference
  metadata as change in displayed luminance, clipped highlight share and frame-to-frame steps. It
  needs a per-frame histogram dump from the analyzer (integer counts, identical on CPU and CUDA).
  (2) ColorVideoVDP on rendered output of a few cuts.
- **Gate:** the simulator reproduces the numbers recomputed on 2026-10-03
  ([`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md)): for a 1000-nit shot on a 1000-nit
  panel, L1 max +75 / +140 codes costs 14.6% / 24.4% at the peak with the spline, and nothing while
  L1 max stays below the panel peak.
- **Limits to state with every result:** open curves are not a Dolby display; libplacebo reads only
  L1 max/avg and the source range, so trims, L1 min, L4 and L11 cannot be evaluated in the open;
  pin the libplacebo version; host limits in [`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md#ws7-open).

### WS8: test material

- **Status:** Open. Fixes the 2026-10-03 review asked for:
  1. Grain on the synthetic highlights and odd offsets (today they carry no grain and fill exactly
     one chroma quad).
  2. A grainy twin from the same master as the clean one (today "clean is truth" holds only to
     about ±50 codes).
  3. Synthetic cross-dissolves and fades built from real shots (blended in PQ code and in linear
     light) with their intervals as truth, and retail cuts with authored transitions.
  4. A held-out split: synthetic references and open implementations for development, licensed
     reference output for scoring only ([`docs/PROVENANCE.md`](docs/PROVENANCE.md)).
  5. **Done 2026-10-05:** real material per format. The development tier holds 45 entries (it
     lives outside the repository, see CLAUDE.md "Testing quirks"); 29 further cuts are held out,
     unanalyzed, for a `cm_analyze` qualification run. Added on 2026-10-04 and 2026-10-05, no
     Profile 7 FEL:
     - **HDR10+:** Deadloch (Amazon WEB-DL, Profile B), The Shining (disc, DV MEL + HDR10+),
       Alien (disc, Profile A) and Alita: Battle Angel (disc, DV MEL + HDR10+ Profile A on the same
       master, so its retail L1 is an authored reference for exactly the frames the HDR10+
       metadata describes).
     - **HLG:** Wimbledon 2024 and Glastonbury 2025 (iPlayer), The Green Planet (iPlayer, nature),
       a Champions League broadcast capture (50p, starts at an open-GOP CRA).
     - **HDR10 and Profile 7 MEL / 8.1:** Joker: Folie à Deux (MEL, 1000 nits, starts at a CRA),
       Skyfall (MEL, per-shot L1), Exodus: Gods and Kings (1100-nit master), Star Trek (2009, no
       MaxCLL), Inside Out 2 (Disney+ 8.1, animation; its L1 is static and is not used as a
       reference) and The Revenant (bright snow; an injected hybrid, its metadata is not used).
     Measurements and what was rejected as a reference: [`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md).

## Dolby Vision profile work

### FEL: Profile 7 FEL

- **Status:** Placeholder. The BL+EL compositor and its re-encode were removed on 2026-10-03
  because the composed picture did not match ETSI GS CCM 001. `mkvdovi` refuses Profile 7 FEL
  inputs, keeps the source and continues with the next file.
- The only FEL work of interest keeps the base layer bit-exact and carries FEL information, as far
  as possible, in the Profile 8.1 RPU: [`docs/FEL_PLAN.md`](docs/FEL_PLAN.md). This item replaces
  F1, F2 and R1. Profile 7 MEL conversion is unchanged (see P11 for `--mdfix`).

### R2: Profile 5

- **Status:** Conditional. Genuine Profile 5 uses IPT, so HDR10 BL → Profile 5 needs decoding,
  colour transformation and re-encoding: feasible through an established encoder (Resolve Studio,
  Dolby Encoding Engine), not in this project. Changing profile flags on untouched HEVC is not a
  target.
- **Precondition:** a matched HDR10 / P8.1 / correctly encoded P5 playback comparison from the same
  master with the playback chain recorded.

## Engineering backlog

### E1: regression gates

- **Status:** Core complete. `tools/l1_diff` runs in CI (`scripts/ci/l1-regression-gate.sh`,
  references in `tools/l1_diff/corpus`); `scripts/cuda-parity.sh` checks CPU against CUDA on a GPU
  host; `scripts/rpu-baseline.sh` captures and compares final RPUs. Synthetic accuracy runs in
  workspace CI. Since 2026-10-06 the CI lint job runs clippy on the analyzer with `--features cuda`
  (no GPU or toolkit; first hosted run passed on #31).
- **Open:** expand the corpus (feeds WS6). A self-hosted GPU runner for `scripts/cuda-parity.sh`,
  now a prerequisite because the main analysis path is untested in hosted CI. Spike: an NVRTC
  compile-only check of `kernels.cu` without a GPU.

### E2: crop detection

- **Status:** Partial. Seven-position crop probing, low-signal rejection, modal voting and
  variable-AR union shipped in PR [#4](https://github.com/tinof/hdr-analyze/pull/4), closing issue
  [#3](https://github.com/tinof/hdr-analyze/issues/3).
- **Open:** per-scene crop application, a continuity-sensitive follow-up.
- **Also open:** the crop probe decodes on the CPU in CUDA runs; measure its cost on a full episode,
  then decide an NVDEC probe. A kernel crop reduction must reproduce `detect_crop` exactly (every
  10th sample, at least 10% non-black per row and column, `crop.rs`); per-scene crop application
  needs a second pass, because the crop is fixed before the histograms are built (affordable on
  NVDEC, not on the CPU).

### E3: `mkvdovi` debug options

- **Status:** Open. Add `mkvdovi --dry-run`, `--keep-temp` and `--keep-logs`.

### E4: second scene signal

- **Status:** Open. `--scene-metric hybrid` is a histogram-only placeholder. Missed cuts on
  whole-picture motion (WS2) need a signal that explains motion: compare a spatial-thumbnail
  residual first, then a motion-compensated residual; both are sensitive to flashes, exposure
  changes and low texture. Score precision and recall on held-out authored shot lists. Full
  optical-flow fusion stays deferred.

### E5: hardware decode contexts

- **Status:** Deferred. Add proper VAAPI and VideoToolbox decode device contexts and hardware-frame
  transfer.

### E6: provenance

- **Status:** Core complete. Sidecar v2 records analyzer version, input name and size, dimensions,
  transfer, downscale, sample rate, GPU use and crop. Resume temp directories carry a fingerprint
  of input name, size and mtime, mkvdovi version and artifact-affecting settings.
- **Open:** record external tool versions (`dovi_tool`, `mkvmerge`, `ffmpeg`) for each output.

### E7: completeness

- **Status:** Partial. `--verify` fails when the RPU frame count differs from the muxed video track
  or the L1 sidecar, and warns when output and input frame counts differ.
- Since 2026-10-04 the analyzer accounts for every picture of the stream, so measured L1 lines up
  with the picture. It counts the undecodable RASL pictures at the start of an open-GOP cut, and
  any other loss is an error. A measured RPU whose length differs from the video is refused at
  inject.
- **Open:** direct-MKV `dovi_tool` steps still accept exit status plus non-empty output, and
  verification is opt-in.
- **Found 2026-10-07 (E13 baselines):** `--verify` hard-fails the Profile 7 MEL passthrough of a
  retail disc whose RPU carries L6 MaxCLL and MaxFALL 0 ("must be a positive integer"); 0 means
  "unknown" in the static metadata, so the check may be too strict for a passthrough RPU. Not yet
  investigated.

### E8: histogram percentile reader

- **Status:** Open. The 256-bin luma histogram is split into 64 bins below PQ(100 nits) and 192
  above, but `compute_histogram_percentile_pq` and `find_highlight_knee_nits`
  (`hdr_analyzer_mvp/src/analysis/histogram.rs`) convert a bin index back with bin/255, so a
  1000-nit value reads as about 305 nits. It reaches the output only with
  `--peak-source histogram99/histogram999` or `--peak-domain luma` with a non-conservative
  optimizer profile.
- **Checked 2026-10-06 @df54ad4:** `find_highlight_knee_nits` feeds the optimizer's `target_nits`
  on every default run (`optimizer.rs:152`), so the fix changes the default `.bin`. CUDA parity
  (`cuda_parity.rs:94`) and the L1 gate (`scripts/ci/l1-regression-gate.sh:76`) both pass
  `--disable-optimizer`, so a round-trip and optimizer unit test is the gate. The bin edges are
  written in `frame.rs:667` and `gpu.rs` `build_luminance_bin_lut`; `frame.rs:970` bins uniformly.
- **Fix:** one shared bin-edge function for writer and readers, with a round-trip test.

### E9: `--pre-denoise` values

- **Status:** Open. `--pre-denoise` accepts any string and its help text names `nlmeans`, but only
  `median3` is implemented; every other value silently does nothing and is still written into the
  sidecar provenance. Restrict the values to `off` and `median3`.

### E10: L1 code scaling

- **Status:** Open; unverified. The analyzer converts normalized PQ to a 12-bit code with ×4095;
  ETSI GS CCM 001 §6.2.2 encodes L1 as `clip(round(x × 4096))`. The difference is at most one
  code. The generator's own conversion (`dolby_vision::utils::nits_to_pq_12_bit`, also used for
  the P10 source range) is ×4095 too, so analyzer and generator agree (checked 2026-10-04). Still
  open: a check against retail RPUs.

### E11: `l1_diff` and open-GOP cuts

- **Status:** Done 2026-10-06. `tools/l1_diff` shifts our frames by
  `source.leading_skipped_frames` (sidecar v5) when the reference covers the stream frames, re-bases
  `--per-shot` and `--scenes`, and labels `--export-reference` rows with stream frames. Any other
  count difference is still refused; no measurements are invented for the skipped pictures.
  Evidence: [`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md) (2026-10-06).
- **Still open (supporting, not the gate):** Joker and the Champions League cut (G4) have only v4
  runs; scoring Joker's retail L1 per frame needs a re-analysis with a v5 analyzer.

### E12: CUDA release binaries

- **Status:** Open. `release.yml` builds without `--features cuda`, so release users never get GPU
  analysis, and `auto` resolves to CPU `balanced` even on an NVIDIA host.
- **Plan:** build Linux x64 and Windows x64 with the feature (no toolkit needed at build time;
  macOS already returns an error from `load_cuda_libraries`), smoke-test `--version` for `+cuda`
  and a clean CPU fallback without libcuda. Windows `--features cuda` is untested.
- **Effect:** changes delivered L1 and MaxCLL for NVIDIA release users (`balanced` → `accurate`);
  needs a CHANGELOG entry.

### E13: module size refactor

- **Status:** Open; step 2 in progress (2026-10-07). Nine Rust files pass 1000 lines; with inline
  test modules not counted, `mkvdovi/src/metadata.rs` (1707), `mkvdovi/src/pipeline.rs` (1785, of
  which `convert_file` is about 910), `hdr_analyzer_mvp/src/pipeline.rs` (1348),
  `analysis/frame.rs` (1026) and `analysis/gpu.rs` (951) pass the 800-line limit in CLAUDE.md
  ("Module size").
- **Steps, each its own PR:** (1) this item; (2) safety net: CUDA gate paths accept module
  directories, the module-size rule and Stop-hook note, final-RPU baselines, characterization tests
  for the untested high-risk mkvdovi paths (source deletion, format detection, static L6, the whole
  `extra.json`); (3) mkvdovi: inline tests to `tests.rs`, then pure-move splits of `metadata.rs`
  and `pipeline.rs`, before P9 step 0, which works in both files; (4) split `convert_file` by
  phase, before the P9 hybrid flag; (5) analyzer: inline tests to `tests.rs`, pure-move splits of
  `pipeline.rs` and `analysis/gpu.rs`, before E8 + E9, because E8 moves the default `.bin` and a
  pure move can only be proved byte-identical before it. `tools/l1_diff` stays as it is while the
  local `feat/dev-corpus` branch depends on it.
- **Gate per move:** fmt, clippy (also `--features cuda`), tests; the L1 regression gate without
  `--update`; `scripts/cuda-parity.sh` for analysis paths; for mkvdovi, `scripts/rpu-baseline.sh
  compare --require-identical-l1` against the baselines captured at main@e6d63ed in
  `~/mkvdovi-work/rpu-baseline/refactor-e6d63ed/` (HDR10, HDR10+, HLG, two MEL incl. an open-GOP
  start; `--mdfix` on MEL and Profile 8).

## Checked and kept (2026-10-03)

The review re-examined these decisions and found no evidence to change them:

- **Max-RGB as the L1 domain** and **the L1 average as the mean of PQ max-RGB**, unfiltered.
- **The 819 average floor** (CM v2.9): retail averages sit at exactly 819 on 24 of 32 and 25 of 35
  shots of two retail cuts.
- **Shot-constant L1**: all nine retail RPUs change L1 only at shot starts.
- **Neutral L2 for analysis output.**
- **Nearest-neighbour chroma** for max-RGB: within about 10 codes of native 4:2:0 ingest
  ([VALIDATION.md §7](docs/VALIDATION.md)). A correctly sited reconstruction has not been compared
  on saturated edges; reopen only with such a measurement.
- **Stream copy for MEL and HLG.** With the FEL compositor gone, no path encodes video.

## Explicit non-goals

- No proprietary Dolby code, LUTs, tone curves, or binary blobs in this repository; see
  [`docs/PROVENANCE.md`](docs/PROVENANCE.md) for the provenance statement and its stated limits.
- No silent peak clamping; suspicious values produce advisory warnings.
- No `cm_analyze` parity claim without reproducible measurements.
- No video encoding in `mkvdovi`. Every path copies the video stream bit-exactly; inputs that would
  need a re-encode (Profile 7 FEL today) are refused.
- No Dolby Vision 2 or HDR10+ Advanced output. Neither has public technical content: the
  announcements name features, and no bitstream or metadata specification is published. Revisit
  when one is.

During the `0.x` series, minor releases may include breaking changes under the project's documented
[semantic-versioning](https://semver.org/) policy.
