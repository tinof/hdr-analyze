# Toward `cm_analyze` parity: technical gap analysis

This is a gap analysis. It compares the metadata this project generates with the metadata Dolby's
`cm_analyze` produces, level by level, and records what is known to differ.

Two questions are kept apart. Format compatibility means the generated RPU carries the CM v4.0
metadata levels that Dolby Vision playback devices read. Algorithm parity means the values in those
levels match what `cm_analyze` computes on the same pixels. The project has format compatibility. It
does not have algorithm parity and does not claim it. The analyzer is an independent implementation
based on published standards and measurement; it does not include, reverse-engineer, or redistribute
Dolby code, lookup tables, tone curves, or binaries (see [PROVENANCE.md](PROVENANCE.md)). Differences
are measured in [VALIDATION.md](VALIDATION.md), not asserted.

Known differences today:

- The default direct peak reads hot on grainy content: +92.6 and +74.4 codes per shot against
  `cm_analyze --analysis-version 2` on two real-content samples.
- `cm_analyze`'s default CM v4 L1 applies a peak floor at PQ(100 nits) and an anchored average. This
  analyzer reports measured values instead.
- The measured L1 is not what the RPU carries in every scene. `dovi_tool generate` writes a minimum
  above 12 codes as 12, a maximum below 2081 as 2081 and an average below 819 as 819. Of 435 test
  scenes, 135 have a measured minimum above 12 and 24 a maximum below 2081. The retail RPUs of the
  test cuts carry minima up to 251 codes.
- `source_min_pq` / `source_max_pq` for Profile 8.1 are converted from the mastering display
  values (since 2026-10-04). Before, they came from the generator's coarse lookup on L6.
- There is no L4, L2 trims are neutral, L3 is the generator's neutral block under CM v4.0, and L8 is
  not derived. Every retail RPU of the test cuts carries L4 on every frame and non-neutral L2.
- For HDR10+ inputs L1 is derived from the HDR10+ metadata by `dovi_tool`, not measured.
- L5 comes from one crop for the whole file, and there is no XML export.

Status and prioritization live in the [roadmap](../ROADMAP.md); current conversion usage lives in
[FORMAT_COMPATIBILITY.md](FORMAT_COMPATIBILITY.md). Profile 7 FEL inputs are refused since
2026-10-03; the compositor was removed and the no-re-encode design is tracked in
[FEL_PLAN.md](FEL_PLAN.md).

## What `cm_analyze` produces

Dolby Vision mastering carries Display Management metadata in an RPU. The relevant levels are:

- L1: min / avg / max luminance of the active picture area, organized and stabilized by shot.
- L5: active-area letterbox/pillarbox offsets.
- L6: ST.2086 mastering-display metadata plus MaxCLL/MaxFALL.
- L2/L3/L8: target-display creative trims and offsets.
- L4: temporal filtering / shot anchoring used to stabilize L1.
- L9: source/mastering-display primaries.
- L11: content type and reference-mode hint.
- L254: Content Mapping algorithm version metadata.

The central parity problem is accurate, temporally stable L1 over the correct active area. Creative
trims are a separate, higher-risk tone-mapping problem.

## Current implementation

| Stage | Implementation | Current output |
|-------|----------------|----------------|
| Per-frame analysis | `hdr_analyzer_mvp/src/analysis/frame.rs` (CPU), `analysis/gpu.rs` + `analysis/kernels.cu` (CUDA), `analysis/hlg.rs` (HLG through the Profile 8.4 decode) | Direct/percentile/grain-robust peak, true Y/max-RGB means, robust 4096-bin minimum, 256-bin luma histogram, 31-bin hue histogram |
| Peak selection | `analysis/frame.rs`, `analysis/histogram.rs` | Direct `max` (default), opt-in fine-histogram percentile or grain-robust max-RGB, or Y-based P99/P99.9 peak |
| Active area | `crop.rs`, `ffmpeg_io.rs`, `pipeline.rs` | Multi-position crop probe with low-signal rejection, tolerance clustering, and conservative variable-AR union |
| Scene detection | `analysis/scene.rs` | Histogram-distance cuts and minimum scene length |
| Optimizer | `optimizer.rs` | madVR `target_nits`; this is not itself a Dolby metadata level |
| DV configuration | `mkvdovi/src/metadata/` | Neutral L2, L6, L9, L11 |
| RPU assembly | `mkvdovi/src/pipeline/dovi_steps.rs` | `dovi_tool generate` with explicit per-scene L1 shots from the sidecar, L5 from the committed crop, L254 from `dovi_tool` |

Key facts:

- PQ direct peaks default to limited-range BT.2020 NCL **max-RGB**. `--peak-domain luma` retains the
  legacy Y′ peak. HLG also defaults to max-RGB, measured on the full Dolby Vision 8.4 decode. `--peak-estimator robust` enables the synthetic-calibrated
  grain correction; `max` remains the estimator default because the first real-content acceptance
  round did not reach the predeclared parity envelope. Histogram percentile sources and APL remain
  Y-based.
- `avg_pq` is a full-precision per-pixel Y-luma mean over the active area. The sidecar also records a
  measured max-RGB mean; the RPU's L1 average is that per-scene max-RGB mean, and the Y-luma mean
  stays in the `.bin` and sidecar for diagnostics.
- The analyzer measures a robust active-area minimum from a 4096-bin fine-PQ histogram after
  selected denoising. It defaults to P0.1; `--min-percentile 0` selects the absolute minimum.
- `madvr_parse::MadVRFrame` has no minimum field, and `dovi_tool` hardcodes `min_pq = 0` for madVR
  generator input; it does **not** infer L1 minimum from the histogram. `mkvdovi` therefore bypasses
  the madVR path and passes the sidecar's per-scene minimum, max-RGB mean, and maximum as explicit
  generator shots.
- The generator then clamps each shot (`dolby_vision` crate, checked on `dovi_tool` 2.3.4 output):
  minimum to at most 12 codes (0.00026 nits), maximum to at least 2081 (100 nits), average to at
  least 819 (2.43 nits; `mkvdovi` requests the CM v2.9 floor, the CM v4.0 floor would be 1229) and
  below the maximum. Measured (min, max, avg) = (100, 1500, 500) is written as (12, 2081, 819);
  (5, 2500, 1000) passes unchanged. These are the generator's rules; ETSI GS CCM 001 §6.2.2 defines
  the L1 fields over 0–4095 without them. The sidecar keeps the measured values.
- For Profile 8.1 the generator sets `source_min_pq` / `source_max_pq` from L6 by lookup: a
  mastering peak other than 1000/2000/4000/10000 nits gives 3079 (1000 nits), and a mastering minimum
  other than ≤ 0.001 or exactly 0.005 nits gives 0. Since 2026-10-04 `mkvdovi` passes the two
  fields explicitly, as the PQ codes of the mastering minimum and peak (the `dolby_vision` crate's
  conversion for a Dolby CM XML), so the lookup applies only to a mastering range that is not
  usable. For Profile 8.4 the preset's 62/3079 is used regardless of L6. `--verify` checks the
  source range on every frame and reports, per field, the scenes whose L1 the generator's limits
  changed.
- HDR10+ inputs are not analyzed. `dovi_tool generate --hdr10plus-json` takes L1 from the first
  frame of each HDR10+ scene: minimum 0, average = PQ of the linear-light max-RGB mean rounded to
  whole nits, maximum from the selected peak.
- Seven seek-based crop probes are used by default across 15% to 85% of seekable inputs. Black/low-signal
  frames are rejected, candidates are clustered within two pixels, and multiple aspect-ratio modes
  use their union. Scene cuts are monitored but do not change the committed crop.
- The committed crop is recorded in full-resolution coordinates (sidecar v2) and emitted as L5 offsets.
- L2 trims are neutral (`2048`). L9 detection prefers mastering-display primaries, then container
  primaries, and warns before falling back to BT.2020.
- With `dovi_tool generate --use-custom-targets` and optimizer targets present, the generated
  per-frame L1 maximum follows optimizer `target_pq`, not the analyzer's measured peak. That path
  requires `mkvdovi --legacy-madvr-l1`.
- The shot maximum is the maximum of its frame peaks, so one retained grain spike can set a whole
  shot. Robust aggregation is investigated together with spatial-support peak estimation.

## Per-level gap table

| Level | Current state | Remaining gap | Roadmap |
|-------|---------------|---------------|---------|
| **L1 max** | PQ max-RGB direct peak measured and scored; opt-in percentile and synthetic-calibrated grain-robust estimators. Delivered with a floor of 2081 (24 of 435 test scenes raised) | Robust mode reduced real-content per-shot bias from +92.6 to +80.4 and from +74.4 to +66.4 codes; isolated-tail frames selected by fold-max remain the open gap, so shot aggregation is part of the fix. Spatial-support statistics measured 2026-10-03 remove 55 to 70% of the grain excess on a clean/grainy pair but lose real highlights; none passes ([roadmap](../ROADMAP.md) WS1). Target-gamut transforms. Sub-100-nit shot maxima are not delivered | WS1 / P10 |
| **L1 avg** | Per-scene max-RGB mean delivered in the RPU with a floor of 819 (matches cm v2 shot averages within ~10 codes); Y mean also recorded in the sidecar. HDR10+ inputs: derived from HDR10+ metadata, 156 to 505 codes (median 264) above the measured mean on the one test cut | Hybrid HDR10+ mode with a measured average; cm v4's "avg" is an anchored constant, not a mean. Retail averages sit at exactly 819 on most shots of two test cuts, so the floor is kept | P9 / WS1 |
| **L1 min** | Noise-rejected active-area minimum measured per scene and passed to the generator, which writes any value above 12 codes as 12 (135 of 435 test scenes). HDR10+ inputs: 0 | The measured minimum does not reach the RPU when it is above 12 codes, while retail RPUs carry minima up to 251. `--verify` reports the clamped scenes per field (2026-10-04); open: decide whether to write L1 without the generator's clamp. No open renderer reads L1 min, so the effect can only be checked on a device | P10 |
| **L4** | None; optimizer smooths madVR `target_nits`, not L1. `--mdfix` drops an authored L4 | Every retail RPU of the test cuts has L4 on every frame; its anchor follows the per-frame average (r = 0.94 to 0.99 in eight of nine, 0.71 in one). Emission is deferred until something can validate it: no open renderer reads L4 | WS2 / P11 |
| **L5** | Offsets from the committed crop (HDR10/HLG); sampled source L5 for Dolby Vision inputs | Per-scene offsets for changing aspect ratios | P3 / WS3 |
| **L6 / source range** | Container/MediaInfo values with warned fallbacks; measured MaxCLL/MaxFALL fill fields the source does not state. `source_min_pq` / `source_max_pq` for 8.1 from the mastering values (2026-10-04) | Generator L1 limits remain (P10 step 3) | P6 / P10 |
| **L2/L3/L8** | Neutral L2 for 100/600/1000-nit targets; neutral L3 from the generator under CM v4.0; no L8. `--mdfix` replaces authored L2 with neutral L2 | Authored L2 in the retail RPUs is far from neutral (up to 928 codes on slope and 1189 on power at the 100-nit target on one MEL title). Generated trims stay behind an evaluation gate and device A/B; whether `--mdfix` keeps authored levels is an open decision | WS4 / WS7 / P11 |
| **L9** | Auto-detected with CLI override | Maintain and expand inconsistent-source diagnostics | P5 / P6 |
| **L11/L254** | Emitted | Maintain validation coverage | None |
| **XML** | No export | Resolve/Metafier-compatible metadata interchange | WS5 |

## Accuracy gaps

### Max-RGB peak

The analyzer decodes limited-range BT.2020 NCL and tracks the maximum R′/G′/B′ PQ signal alongside
Y′. This closed the large saturated-highlight definition gap.

Real-content validation (2026-07-08, [VALIDATION.md](VALIDATION.md) §7) settled the input-preparation
question and reframed the remaining gap:

- **Chroma reconstruction is not a material error source.** cm_analyze ingests raw 4:2:0 directly;
  neighbor- and spline-prepped 4:4:4 inputs produce *identical* cm peaks, both within ~10 codes of
  native-4:2:0 ingest. The earlier attribution of the +12.8-code FEL-asset bias to
  nearest-neighbor-vs-spline resampling was wrong; the offset lives in the YCbCr→RGB
  conversion/rounding path and nearest-neighbor chroma sharing stays.
- **cm_analyze's default (CM v4) L1 is not a raw measurement**: its peak has an exact floor at
  PQ(100 nits) = code 2081 and its average is anchored. Measurement-style comparisons must use
  `--analysis-version 2`, which matches Dolby-authored embedded L1 to ~+16 codes.
- **The open gap is grain robustness.** Against cm v2 on identical BL pixels, our direct max reads
  +92.6 codes hot on heavy-grain content and +74.4 on milder content. The first robust estimator
  combines a 4096-bin max-RGB histogram, cross-quad noise estimate, inferred Gaussian support, and
  noise-adjusted content floor. It passes deterministic additive-luma, chroma, and
  multiplicative-linear grain truth, but the one-shot real-content gate improved bias to
  +80.4/+66.4 codes. Frames with a single extreme-tail pixel intentionally retain the raw peak, and
  per-shot fold-max can select them. Robust mode is therefore explicit opt-in, not the default;
  spatial-support handling requires a separate validated design.

The v6 `peak_pq_dcip3` and `peak_pq_709` fields remain approximations. They are madVR-only fields and
are not consumed by the Dolby Vision v5 conversion path.

### Average and minimum

The analyzer accumulates Y-luma and max-RGB PQ sums and the processed-pixel count in the same Rayon
reduction as the histogram. The averages are not reconstructed from histogram bin centers. The L1 sidecar stores both means
unfiltered, per frame and per scene (sidecar version 4). Until version 3 each frame mean passed the
histogram EMA first, which pulled a scene average toward the scene's first frames
([VALIDATION.md](VALIDATION.md) §9). Only the madVR `.bin` frame average is still smoothed, with
scene resets. The robust minimum is an unsmoothed spatial-percentile measurement.

The sidecar minimum is P0.1 by default over the active area after selected denoising. Scene minimum is
the minimum of the already noise-rejected per-frame values, so a real raised-black excursion remains
visible while isolated dark pixels do not dominate. Synthetic tests cover a uniform 0.05-nit floor,
sparse dark contamination, and the `--min-percentile 0` absolute-minimum control. `mkvdovi` passes
this measured value to the generator as each scene's L1 minimum, and the generator writes at most 12
codes (0.00026 nits). A measured raised black therefore stays in the sidecar and does not reach the
RPU. An earlier version of this document said it was delivered; that was wrong. Whether to write L1
without the clamp is roadmap item P10.

For HDR10+ inputs the average is not measured. On the HDR10+ test cut (11 scenes, full-frame
picture) the generator's average, PQ of the linear-light mean rounded to whole nits, is 156 to 505
codes (median 264) above the analyzer's mean of PQ max-RGB in every scene; the rounding alone moves
a 0.7-nit scene by 71 codes. A linear-light mean lies above a mean of PQ values by construction, but
how much of the gap that explains and how much is the HDR10+ producer's own measurement was not
separated.

### Active area and temporal stability

Multi-position crop probing fixes the former first-frame failure mode. Variable-aspect-ratio inputs
currently use a conservative union so picture is never cut. Per-scene crop application remains a
follow-up because changing the sample area can itself create measurement discontinuities.

L1 is emitted per scene, but there is no shot anchoring or L4-style temporal filtering yet. Shot aggregation and optional L4-style anchoring
must be compared against reference shot boundaries and checked for pumping around cuts and fades.

The nine retail RPUs of the test cuts were checked on 2026-10-03. L1 changes only on frames that
start a shot (0 changes elsewhere), so scene-constant L1 matches authored practice on this material.
L4 is present on every frame and changes inside shots. Its anchor correlates with the analyzer's
unfiltered per-frame average at r = 0.94 to 0.99 in eight RPUs and 0.71 in one, and equally well
with the luma average, so the correlation does not decide between the two average domains. No test
cut contains a dissolve or a fade, so how authored metadata treats transitions is not measured.

### Trims

Neutral L2 remains the safe default. Any non-neutral L2/L8 derivation must begin with a documented open
operator such as ITU-R BT.2390 and remain opt-in unless blinded real-content comparisons show it is at
least as good as neutral output.

Authored trims are not neutral. In the retail RPUs of the test cuts (all CM v2.9, no L3 or L8) the
100-nit L2 has slope 1120 to 1384 and power 859 to 1149 on one MEL title and differs from the
neutral 2048 by typically 100 to 400 codes on the others; one cut has no L2. `--mdfix` regenerates
the RPU and replaces these with neutral L2 and no L4. Trims were authored for the authored L1 and
shot list, so keeping them under newly measured L1 is not obviously right either; the decision is
roadmap item P11. The open display-mapping curves read no trims, so generated trims cannot be scored
without a device.

## Validation methodology

The core validation foundation is shipped:

- `tools/l1_diff` aligns per-frame reference and generated L1 values and reports bias plus absolute
  mean/median/p95/max errors in PQ codes and nits. It reads the sidecar to score minimum and both
  average domains while retaining the existing peak comparison from the madVR file. For historical
  `.bin` files without a sidecar it falls back to the embedded average and reports sidecar-only
  metrics unavailable; an explicit invalid `--sidecar` remains an error.
- `hdr_analyzer_mvp/tests/synthetic_accuracy.rs` validates constructed lossless PQ and saturated
  max-RGB signals and runs through workspace CI.
- [VALIDATION.md](VALIDATION.md) records comparisons against synthetic truth, embedded retail-style
  L1, and licensed `cm_analyze` output on identical pixels.

A small numerical L1 regression runs in CI: `scripts/ci/l1-regression-gate.sh` analyzes a
generated six-shot clip (`tools/l1_diff/corpus`, PQ and HLG) and scores it with `l1_diff` limits
against committed references. The references are the analyzer's own earlier output, so the gate
detects change; it does not measure accuracy. `tools/l1_diff` and `tools/compare_baseline` stay
excluded from the workspace, so `cargo test --workspace` does not execute either utility; the CI
gate job lints and tests `l1_diff` itself. Remaining validation work is to grow the
redistributable corpus.

All of the above scores PQ codes. What a code error costs on a display depends on the tone curve
and on whether the shot exceeds the panel: recomputed with open curves for a 1000-nit shot on a
1000-nit panel, L1 max +75 / +140 codes lowers the peak by 14.6% / 24.4% with libplacebo's spline
and by 2.5% / 4.5% with the ITU-R BT.2390 EETF, and changes nothing when L1 max stays below the
panel peak. A display-mapping simulator that reports such numbers per shot is planned (roadmap WS7).
Open curves are not a Dolby display, and they read only L1 max, L1 avg and the source range.

For every accuracy change:

- Align reference and generated output by frame/shot and report distributions, not a binary parity
  claim.
- Use known PQ/HLG patterns for absolute correctness and real masters for relative behavior.
- Define tolerances before changing defaults.
- Keep reference masters private; publish reproduction commands and derived statistics only.

## Non-goals and risks

- No proprietary Dolby source, LUTs, exact tone curves, or binary blobs. See
  [PROVENANCE.md](PROVENANCE.md).
- No silent peak clamping; anomalous source metadata produces advisory warnings.
- No default creative trims without successful A/B validation.
- No parity claim based on one title, one metric, or visual inspection alone.

## References

- [dovi_tool generator documentation](https://github.com/quietvoid/dovi_tool/blob/main/docs/generator.md)
- [Dolby Vision Metadata Levels](https://professionalsupport.dolby.com/s/article/Dolby-Vision-Metadata-Levels)
- ITU-R BT.2100, BT.2390, and BT.2408
- SMPTE ST.2084 and ST.2086; CTA-861.3
- [TECHNICAL_REFERENCE.md](TECHNICAL_REFERENCE.md) and [VALIDATION.md](VALIDATION.md)
