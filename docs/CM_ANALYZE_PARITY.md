# Toward `cm_analyze` Parity — Gap Analysis & Design

> **Goal:** approach the accuracy and feature set of Dolby Vision's `cm_analyze` (the Content Mapping
> analyzer used in professional DV mastering) with a fully **open-source, research-based** analyzer.
>
> **Honesty statement.** This project does **not** include, reverse-engineer, or redistribute any Dolby
> proprietary code, lookup tables, tone curves, or binary blobs. Everything here is derived from public
> standards (ITU-R, SMPTE, CTA), public documentation, and observable behavior of open tools
> (`dovi_tool`). We say "approach parity," and we treat parity as something **measured**, never asserted.
> Where we cannot match Dolby's exact internal math, we document the approximation and its error.

Related docs: [TECHNICAL_REFERENCE.md](TECHNICAL_REFERENCE.md) ·
[cmv40_upgrade_implementation_plan.md](cmv40_upgrade_implementation_plan.md) · the strategic view lives
in [../ROADMAP.md](../ROADMAP.md).

---

## 1. Primer: what `cm_analyze` produces

Dolby Vision mastering generates a per-frame **RPU** carrying Display Management (DM) metadata, organized
into "levels." Grouped by role:

**Analysis outputs** (derived from the image — this is the core of `cm_analyze`):

- **L1** — per-shot (interpolated per-frame) **min / avg / max** luminance of the **active picture area**,
  in the PQ domain. The single most important analysis output; it drives all downstream tone mapping.
- **L5** — active area (letterbox/pillarbox offsets / aspect ratio).
- **L6** — static ST.2086 mastering-display metadata plus MaxCLL / MaxFALL.

**Trim / creative** (target-display tone-mapping adjustments, normally authored by a colorist or derived
by the tone-mapping operator):

- **L2 / L3** — trims (lift/gain/gamma + saturation/chroma, and L1 offsets) for a specific target display.
- **L8** — CM v4.0 extended trims (the modern equivalent/superset of L2/L3 per target).

**Temporal:**

- **L4** — temporal filtering / shot anchoring that stabilizes L1 over time to prevent flicker/pumping.

**Configuration:**

- **L9** — source / mastering-display primaries.
- **L11** — content type (and reference-mode hint).
- **L254** — CM algorithm version metadata (`dm_version_index`).

The defining value of `cm_analyze` is **(a)** accurate per-shot **L1** over the correct **active area**,
**(b)** **temporal stability** of that L1, and **(c)** trims produced by Dolby's tone-mapping operator.

---

## 2. What hdr-analyze produces today

References are to files read directly from this repo.

| Stage | File | Output |
|-------|------|--------|
| Per-frame analysis | `hdr_analyzer_mvp/src/analysis/frame.rs` (`analyze_native_frame_cropped`) | `peak_pq_2020` (max), `avg_pq`, 256-bin luma histogram, 31-bin hue histogram |
| Peak selection | `analysis/histogram.rs` (`select_peak_pq`) | `max` / `histogram99` / `histogram999` percentile peak |
| Scene detection | `analysis/scene.rs` | chi-square histogram-distance cuts + min-scene-length |
| Optimizer | `optimizer.rs` (`run_optimizer_pass`) | madVR **`target_nits`** (see note) |
| DV metadata | `mkvdovi/src/metadata.rs` (`generate_extra_json`) | neutral L2, L6, L9, L11 |
| RPU assembly | `mkvdovi/src/pipeline.rs` → `dovi_tool generate` | L1 derived from madVR measurements; L5 default; L254 added by `dovi_tool` |

Key facts:

- **L1 max** is computed from the **Y′ (luma) plane only** and selected via a percentile of the 256-bin
  histogram. It is a *luma* statistic, not a *luminance* / max-RGB statistic.
- **L1 avg** is a **mid-bin weighted mean over 256 bins**, with a bin-0 black-bar heuristic and a
  renormalization step (`frame.rs`). It is quantized and not a true full-precision mean.
- **There is no robust L1 `min`.** `dovi_tool generate` derives a min from the madVR histogram, whose
  lowest occupied bins include letterbox black and sensor/grain noise.
- **`target_nits`** (the optimizer's output) is a **madVR playback concept**, orthogonal to Dolby DM
  levels. It is *not* part of the DV RPU and is not a `cm_analyze` output. Useful for madVR/madTPG
  playback; ignore it when reasoning about DV parity.
- **L2 trims are neutral**: `generate_extra_json` writes `trim_slope/offset/power/chroma_weight/
  saturation_gain = 2048` (the neutral midpoint). We intentionally emit no tone-mapping trims; a
  DV-capable display performs its own mapping.
- **Active area**: crop is detected on the **first analyzed frame only** and reused for the whole stream
  (issue [#3](https://github.com/tinof/hdr-analyze/issues/3)); it is **not** emitted as **L5**.

---

## 3. Per-level gap table

| Level | `cm_analyze` | hdr-analyze today | Gap | Parity action | Risk |
|-------|--------------|-------------------|-----|---------------|------|
| **L1 max** | max luminance (image) in PQ over active area | max of Y′ luma, percentile-selected | luma ≠ luminance; max-RGB highlights under/over-stated | compute luminance / max-RGB peak (WS1) | med |
| **L1 avg** | mean PQ of active area | mid-bin weighted mean (256-bin, quantized) | quantization + heuristic bias | true full-precision mean PQ (WS1) | low |
| **L1 min** | min luminance in PQ over active area | none (dovi_tool infers from histogram) | letterbox/noise contamination | robust low-percentile min over active area (WS1) | med |
| **L4** | temporal filtering / shot anchoring | none (optimizer smooths `target_nits` only) | per-frame L1 jitter | shot-anchored L1 + optional L4 (WS2) | med |
| **L5** | active area offsets | dovi_tool defaults | wrong/missing active area | emit L5 from crop (WS3); fix single-frame crop (WS1) | low |
| **L6** | ST.2086 + MaxCLL/MaxFALL | emitted from MediaInfo (warned fallbacks) | minor: measured vs container MaxCLL/FALL | optionally measure MaxCLL/FALL from analysis | low |
| **L2/L3/L8** | tone-mapping trims | **neutral (2048)** | no trim derivation | experimental, opt-in, A/B-gated (WS4) | **high** |
| **L9** | source primaries | auto-detected / `--source-primaries` | — (parity) | maintain | — |
| **L11** | content type | CLI (`--content-type`) | — (parity) | maintain | — |
| **L254** | CM algo version | added by `dovi_tool` | — (parity) | maintain | — |

---

## 4. Accuracy deep-dives (the L1-fidelity gaps that matter)

1. **Luma-only peak vs luminance / max-RGB.** Reading only Y′ misses highlights that are bright in a
   single channel (saturated reds/blues) and slightly mis-weights others. `cm_analyze` works in a
   luminance/ICtCp-like domain. Action: compute max-RGB (or an ICtCp **I**) in the PQ domain from the
   decoded RGB, not Y′. This subsumes the existing "v6 gamut peaks → full RGB" roadmap item — and the
   same RGB decode yields real madVR-v6 per-gamut peaks (`peak_pq_dcip3`/`peak_pq_709`) as a by-product.
   Note those v6 gamut peaks are a **madVR-only** feature and are not consumed by the DV RPU.
2. **Quantized mid-bin avg vs true mean.** A 256-bin histogram + mid-bin centers + renormalization
   introduces a small, content-dependent bias in `avg_pq`. Action: accumulate a true mean of the
   PQ-encoded active-area pixels at full precision (the histogram can remain for distribution/min/peak).
3. **Missing robust min.** A true shadow floor needs the active area (no letterbox) and noise rejection.
   Action: low-percentile (e.g. P1) min over the cropped, optionally denoised plane — never the absolute
   minimum bin.
4. **Single-frame crop / no L5.** A wrong crop corrupts all three L1 stats *and* the (missing) L5. Action:
   multi-frame/stream-level crop probing (issue #3) feeding both analysis and L5 emission.
5. **No shot-anchored L1 / temporal stability.** Dolby groups L1 per shot and filters it temporally.
   We have scene cuts but feed per-frame L1. Action: per-shot aggregation and an optional L4-style filter.

---

## 5. Dependency-ordered workstreams

Each workstream names its **goal**, **approach**, and **validation gate**. Nothing claims parity until
WS0 can measure it.

### WS0 — Validation foundation *(prerequisite for every parity claim)*

- **Goal:** be able to *measure* L1 error against ground truth.
- **Approach:** build a small reference corpus from (a) **real DV masters** — extract reference L1 with
  `dovi_tool extract-rpu` then `dovi_tool export`/`info` to JSON — and (b) **ITU/Dolby test patterns**
  for absolute checks. Extend `tools/compare_baseline` into a frame/shot-aligned diff that reports
  min/avg/max L1 error in PQ. Add a CI smoke comparison on one tiny clip.
- **Gate:** harness produces reproducible per-level error numbers with defined tolerances.

### WS1 — L1 accuracy

- **Goal:** close the per-frame L1 gap (§4.1–§4.4).
- **Approach:** robust min (low-percentile, active-area), true full-precision mean avg, luminance/max-RGB
  peak, and active-area correctness (consume multi-frame crop, issue #3).
- **Gate:** WS0 shows reduced min/avg/max error vs the reference corpus, no regressions on test patterns.

### WS2 — Shot model & temporal stability

- **Goal:** stable, shot-anchored L1 (L4 behavior).
- **Approach:** aggregate L1 per detected shot; optional temporal filter / shot anchor; promote the
  prototype `--scene-metric hybrid` once it beats histogram-only on WS0 shot-boundary metrics.
- **Gate:** reduced per-frame L1 jitter and shot-boundary error vs reference.

### WS3 — L5 active-area emission

- **Goal:** emit correct L5 from detected crop instead of `dovi_tool` defaults.
- **Approach:** map the (multi-frame) crop rectangle to L5 offsets in the RPU generation path.
- **Gate:** L5 offsets match reference active areas within tolerance.

### WS4 — L2/L8 trim derivation *(experimental, opt-in, A/B-gated)*

- **Goal:** optionally derive target-display trims instead of neutral L2.
- **Approach:** start from an **open** tone-mapping operator (ITU-R BT.2390 EETF) as a documented,
  non-proprietary baseline; derive candidate trims from L1 + target display. **Neutral L2 stays the
  default**; any non-neutral output is opt-in and gated behind reference-file A/B comparison, because a
  wrong trim looks worse than no trim.
- **Gate:** blinded A/B on real content shows the derived trim is at least as good as neutral; otherwise
  it does not ship as default.

### WS5 — Interop: Dolby Vision XML export

- **Goal:** export DV metadata XML for DaVinci Resolve / Dolby Metafier.
- **Approach:** serialize L1/L5/L6/L9/L11 (and any L2/L8 from WS4) to the DV XML schema.
- **Gate:** round-trips through `dovi_tool`/Resolve without validation errors; also serves as an
  independent validation/interchange path for WS0.

---

## 6. Validation methodology

- **Relative (parity):** align reference vs generated RPUs by frame/shot, then diff L1 min/avg/max in the
  PQ domain. Report mean/median/max error and percentage within tolerance — never a binary "parity:
  yes/no."
- **Absolute (correctness):** run known test patterns (e.g. ARIB STD-B72 bars, PQ grayscale ramps) where
  the expected PQ value is known a priori, and check our min/avg/max against the spec value.
- **Regression:** WS0's harness runs in CI on a tiny clip so accuracy changes are caught as numbers, not
  vibes.
- **Provenance:** reference DV masters are private; only derived statistics (not the masters) belong in
  the repo/corpus metadata.

---

## 7. Non-goals & risk register

- **No proprietary anything.** No Dolby source, LUTs, exact tone curves, or blobs. If exact parity needs
  Dolby's internal operator, we ship a documented open approximation with its measured error instead.
- **Trims are experimental.** Neutral L2 remains the default; a DV-capable display does its own mapping.
  Shipping bad trims by default is a worse user outcome than neutral output (the high-risk row in §3).
- **Peaks are never silently clamped** (existing policy; advisory warnings only) — this is preserved.
- **Parity is measured.** No doc, README, or release note should claim `cm_analyze` parity is achieved
  without WS0 numbers backing it.

---

## 8. References

- [dovi_tool generator documentation](https://github.com/quietvoid/dovi_tool/blob/main/docs/generator.md)
- [dovi_tool repository](https://github.com/quietvoid/dovi_tool)
- [Dolby Vision Metadata Levels](https://professionalsupport.dolby.com/s/article/Dolby-Vision-Metadata-Levels)
- ITU-R BT.2390 (HDR-TV / EETF tone mapping), BT.2408 (operational practices), BT.2100 (HDR signal: PQ & HLG)
- SMPTE ST.2086 (mastering display metadata), ST.2084 (PQ EOTF), CTA-861.3 (MaxCLL/MaxFALL)
- Internal: [TECHNICAL_REFERENCE.md](TECHNICAL_REFERENCE.md), [cmv40_upgrade_implementation_plan.md](cmv40_upgrade_implementation_plan.md)
