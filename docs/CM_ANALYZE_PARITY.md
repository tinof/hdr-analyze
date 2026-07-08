# Toward `cm_analyze` Parity — Technical Gap Analysis

> **Goal:** approach the accuracy and feature set of Dolby Vision's `cm_analyze` with a fully
> open-source, research-based analyzer. This project does not include, reverse-engineer, or
> redistribute proprietary Dolby code, lookup tables, tone curves, or binary blobs. Parity is measured,
> never asserted.

This document explains the technical gaps and validation method. Status and prioritization live only
in the [roadmap](../ROADMAP.md); current conversion usage lives in
[DOLBY_VISION.md](DOLBY_VISION.md).

## What `cm_analyze` produces

Dolby Vision mastering carries Display Management metadata in an RPU. The relevant levels are:

- **L1** — min / avg / max luminance of the active picture area, organized and stabilized by shot.
- **L5** — active-area letterbox/pillarbox offsets.
- **L6** — ST.2086 mastering-display metadata plus MaxCLL/MaxFALL.
- **L2/L3/L8** — target-display creative trims and offsets.
- **L4** — temporal filtering / shot anchoring used to stabilize L1.
- **L9** — source/mastering-display primaries.
- **L11** — content type and reference-mode hint.
- **L254** — Content Mapping algorithm version metadata.

The central parity problem is accurate, temporally stable L1 over the correct active area. Creative
trims are a separate, higher-risk tone-mapping problem.

## Current implementation

| Stage | Implementation | Current output |
|-------|----------------|----------------|
| Per-frame analysis | `hdr_analyzer_mvp/src/analysis/frame.rs` | Direct peak, true Y/max-RGB means, robust 1024-bin minimum, 256-bin luma histogram, 31-bin hue histogram |
| Peak selection | `analysis/histogram.rs` | Direct max or Y-based P99/P99.9 peak |
| Active area | `crop.rs`, `ffmpeg_io.rs`, `pipeline.rs` | Multi-position crop probe with low-signal rejection, tolerance clustering, and conservative variable-AR union |
| Scene detection | `analysis/scene.rs` | Histogram-distance cuts and minimum scene length |
| Optimizer | `optimizer.rs` | madVR `target_nits`; this is not itself a Dolby metadata level |
| DV configuration | `mkvdovi/src/metadata.rs` | Neutral L2, L6, L9, L11 |
| RPU assembly | `mkvdovi/src/pipeline.rs` | `dovi_tool generate`; L1 inferred from source data/measurements, default L5, L254 from `dovi_tool` |

Key facts:

- PQ direct peaks default to limited-range BT.2020 NCL **max-RGB**. `--peak-domain luma` retains the
  legacy Y′ peak; HLG remains luma-based. Histogram percentile sources and APL remain Y-based.
- `avg_pq` is a full-precision per-pixel Y-luma mean over the active area. The sidecar also records a
  measured max-RGB mean so validation can compare domains without changing the RPU definition.
- The analyzer measures a robust active-area minimum from a 1024-bin code-level histogram after
  selected denoising. It defaults to P0.1; `--min-percentile 0` selects the absolute minimum.
- `madvr_parse::MadVRFrame` has no minimum field, and `dovi_tool` hardcodes `min_pq = 0` for madVR
  generator input; it does **not** infer L1 minimum from the histogram. The measured minimum is
  therefore emitted only in `<output>.l1.json` pending explicit RPU wiring.
- Seven seek-based crop probes are used by default across 15%–85% of seekable inputs. Black/low-signal
  frames are rejected, candidates are clustered within two pixels, and multiple aspect-ratio modes
  use their union. Scene cuts are monitored but do not change the committed crop.
- The committed crop affects measurements but is not passed through as L5 metadata.
- L2 trims are neutral (`2048`). L9 detection prefers mastering-display primaries, then container
  primaries, and warns before falling back to BT.2020.
- With `dovi_tool generate --use-custom-targets` and optimizer targets present, the generated
  per-frame L1 maximum follows optimizer `target_pq`, not the analyzer's measured peak. Untangling
  custom-target frame edits from source-honest L1 delivery remains P0 work.

## Per-level gap table

| Level | Current state | Remaining gap | Roadmap |
|-------|---------------|---------------|---------|
| **L1 max** | PQ max-RGB direct peak measured and scored; optional Y′/percentile sources | Chroma resampling difference, target-gamut transforms, and HLG max-RGB | P2 / WS1 |
| **L1 avg** | True Y-luma mean delivered through scene measurements; Y and max-RGB means also recorded in the sidecar | Decide from validation whether the RPU average domain should change | WS1 |
| **L1 min** | Noise-rejected active-area minimum measured and emitted in the sidecar | Wire measured minimum into the RPU without conflicting with custom-target frame edits | WS1 / P0 |
| **L4** | None; optimizer smooths madVR `target_nits` only | Shot-anchored L1 and optional temporal filtering | WS2 |
| **L5** | `dovi_tool` default | Emit offsets from the committed crop and validate variable-AR policy | P3 / WS3 |
| **L6** | Container/MediaInfo values with warned fallbacks | Optionally measure MaxCLL/MaxFALL from analysis | P6 |
| **L2/L3/L8** | Neutral L2; no L3/L8 derivation | Experimental open tone-mapping baseline and A/B validation | WS4 |
| **L9** | Auto-detected with CLI override | Maintain and expand inconsistent-source diagnostics | P5 / P6 |
| **L11/L254** | Emitted | Maintain validation coverage | — |
| **XML** | No export | Resolve/Metafier-compatible metadata interchange | WS5 |

## Accuracy gaps

### Max-RGB peak

The analyzer decodes limited-range BT.2020 NCL and tracks the maximum R′/G′/B′ PQ signal alongside
Y′. This closed the large saturated-highlight definition gap. Against the identical demuxed base layer,
the published run measured a +12.8-code median difference from `cm_analyze`; nearest-neighbor 4:2:0
chroma sharing versus spline resampling is the leading explanation. See [VALIDATION.md](VALIDATION.md).

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
