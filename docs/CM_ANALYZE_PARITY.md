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
- There is no L4 temporal anchoring, L2 trims are neutral, and L3/L8 are not derived.
- L5 comes from one crop for the whole file, and there is no XML export.

Status and prioritization live in the [roadmap](../ROADMAP.md); current conversion usage lives in
[FORMAT_COMPATIBILITY.md](FORMAT_COMPATIBILITY.md).

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
| Per-frame analysis | `hdr_analyzer_mvp/src/analysis/frame.rs` | Direct/percentile/grain-robust peak, true Y/max-RGB means, robust 4096-bin minimum, 256-bin luma histogram, 31-bin hue histogram |
| Peak selection | `analysis/frame.rs`, `analysis/histogram.rs` | Direct `max` (default), opt-in fine-histogram percentile or grain-robust max-RGB, or Y-based P99/P99.9 peak |
| Active area | `crop.rs`, `ffmpeg_io.rs`, `pipeline.rs` | Multi-position crop probe with low-signal rejection, tolerance clustering, and conservative variable-AR union |
| Scene detection | `analysis/scene.rs` | Histogram-distance cuts and minimum scene length |
| Optimizer | `optimizer.rs` | madVR `target_nits`; this is not itself a Dolby metadata level |
| DV configuration | `mkvdovi/src/metadata.rs` | Neutral L2, L6, L9, L11 |
| RPU assembly | `mkvdovi/src/pipeline.rs` | `dovi_tool generate` with explicit per-scene L1 shots from the sidecar, L5 from the committed crop, L254 from `dovi_tool` |

Key facts:

- PQ direct peaks default to limited-range BT.2020 NCL **max-RGB**. `--peak-domain luma` retains the
  legacy Y′ peak; HLG remains luma-based. `--peak-estimator robust` enables the synthetic-calibrated
  grain correction; `max` remains the estimator default because the first real-content acceptance
  round did not reach the predeclared parity envelope. Histogram percentile sources and APL remain
  Y-based.
- `avg_pq` is a full-precision per-pixel Y-luma mean over the active area. The sidecar also records a
  measured max-RGB mean so validation can compare domains without changing the RPU definition.
- The analyzer measures a robust active-area minimum from a 4096-bin fine-PQ histogram after
  selected denoising. It defaults to P0.1; `--min-percentile 0` selects the absolute minimum.
- `madvr_parse::MadVRFrame` has no minimum field, and `dovi_tool` hardcodes `min_pq = 0` for madVR
  generator input; it does **not** infer L1 minimum from the histogram. `mkvdovi` therefore bypasses
  the madVR path and passes the sidecar's per-scene minimum, max-RGB mean, and maximum as explicit
  generator shots.
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
| **L1 max** | PQ max-RGB direct peak measured and scored; opt-in percentile and synthetic-calibrated grain-robust estimators | Robust mode reduced real-content per-shot bias from +92.6 to +80.4 and from +74.4 to +66.4 codes; isolated-tail frames selected by fold-max remain the open gap, so shot aggregation is part of the fix. Spatial support or separately validated shot aggregation is needed before a default change; target-gamut transforms; HLG max-RGB | P2 / WS1 |
| **L1 avg** | Per-scene max-RGB mean delivered in the RPU (matches cm v2 shot averages within ~10 codes); Y mean also recorded in the sidecar | Revisit when new validation evidence exists; cm v4's "avg" is an anchored constant, not a mean | WS1 |
| **L1 min** | Noise-rejected active-area minimum delivered per scene in the RPU | Maintain validation coverage | WS1 |
| **L4** | None; optimizer smooths madVR `target_nits`, not L1 | Shot-anchored L1 and optional temporal filtering | WS2 |
| **L5** | Offsets from the committed crop (HDR10/HLG); sampled source L5 for Dolby Vision inputs | Per-scene offsets for changing aspect ratios | P3 / WS3 |
| **L6** | Container/MediaInfo values with warned fallbacks | Optionally measure MaxCLL/MaxFALL from analysis | P6 |
| **L2/L3/L8** | Neutral L2; no L3/L8 derivation | Experimental open tone-mapping baseline and A/B validation | WS4 |
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
reduction as the histogram. Histogram smoothing no longer reconstructs an average from bin centers;
it applies identical EMA/temporal smoothing to both full-precision mean series with scene resets.
The robust minimum remains an unsmoothed spatial-percentile measurement.

The sidecar minimum is P0.1 by default over the active area after selected denoising. Scene minimum is
the minimum of the already noise-rejected per-frame values, so a real raised-black excursion remains
visible while isolated dark pixels do not dominate. Synthetic tests cover a uniform 0.05-nit floor,
sparse dark contamination, and the `--min-percentile 0` absolute-minimum control. This measured value
is intentionally not wired into RPU generation yet.

### Active area and temporal stability

Multi-position crop probing fixes the former first-frame failure mode. Variable-aspect-ratio inputs
currently use a conservative union so picture is never cut. Per-scene crop application remains a
follow-up because changing the sample area can itself create measurement discontinuities.

Scene cuts already exist, but L1 remains per-frame. Shot aggregation and optional L4-style anchoring
must be compared against reference shot boundaries and checked for pumping around cuts and fades.

### Trims

Neutral L2 remains the safe default. Any non-neutral L2/L8 derivation must begin with a documented open
operator such as ITU-R BT.2390 and remain opt-in unless blinded real-content comparisons show it is at
least as good as neutral output.

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

Remaining validation work is to grow the redistributable corpus and run a small numerical L1
regression automatically. `tools/l1_diff` and `tools/compare_baseline` are currently excluded from the
workspace, so ordinary `cargo test --workspace` does not execute either utility.

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
