# Research Plan: Profile 7 FEL -> Profile 8.1 Metadata-Only Approximation

## Executive Summary

This document keeps the original curiosity alive, but reframes it as a falsifiable research project rather than a presumed conversion strategy.

The question is:

> Can some Dolby Vision Profile 7 FEL contribution be approximated in a Profile 8.1 RPU without re-encoding the HDR10 base layer?

The answer is expected to be content-dependent. A lossless general conversion is not possible because Profile 7 FEL can carry non-zero per-pixel residual video samples, while Profile 8.1 has no enhancement-layer video stream. However, a metadata-only approximation may work when the FEL contribution behaves mostly like a low-dimensional transform of the base-layer image.

The experiment should answer three practical questions:

1. Is metadata-only approximation ever visually useful compared with simply discarding the EL?
2. Can we detect when it will fail before producing a misleading output?
3. If it works, which Profile 8.1 metadata primitives are useful enough to justify implementation?

---

## 1. Theory And Failure Point

### 1.1 What Profile 7 FEL Contains

Profile 7 on UHD Blu-ray uses an HDR10-compatible base layer (BL), an enhancement layer (EL), and Dolby Vision RPU metadata. For MEL, the residual is effectively zero. For FEL, the residual is non-zero.

The relevant components are:

| Component | Description | Metadata-only representable? |
| --- | --- | --- |
| L1/L2/L8/L9/L11 metadata | Analysis, trims, primaries, content hints | Yes, if available and compatible |
| RPU reshaping/composer mapping | Polynomial/MMR-style transforms used during DV processing | Sometimes |
| FEL residual video samples | Per-pixel difference needed to reconstruct closer-to-master output | Not generally |
| Additional precision | 10-bit BL plus residual can reconstruct a higher precision signal | Only approximately |

### 1.2 The Mathematical Test

The metadata-only hypothesis can be written as:

```text
P8.1 output = f(BL_pixel, RPU_metadata)
P7 FEL output = g(BL_pixel, EL_residual_pixel, RPU_metadata)
```

To remove the EL without changing BL pixels, we need to find new metadata such that:

```text
f(BL_pixel, derived_RPU) ~= g(BL_pixel, EL_residual_pixel, original_RPU)
```

This is impossible to do exactly in the general case. If two pixels have the same BL YCbCr values but different FEL-composited output values, a metadata transform that only sees the BL sample and frame/scene metadata cannot reproduce both outputs.

Therefore the first research gate is collision analysis:

```text
same BL value + different FEL target = not exactly representable by metadata alone
```

### 1.3 What May Still Work

The approach may still be useful when the residual is mostly:

- a global or scene-local luma curve,
- a chroma transform correlated with Y/Cb/Cr values,
- a smooth display-mapping difference,
- a small residual whose errors are visually hidden by quantization, compression, or display behavior.

It is expected to fail for:

- spatially localized corrections,
- vignettes or masks,
- object-specific differences,
- grain/texture/high-frequency residuals,
- edge/detail differences,
- residuals that vary by pixel position rather than by pixel value.

---

## 2. Revised Research Workflow

### 2.1 Inputs And Controls

For each sample, generate these outputs:

| Output | Purpose |
| --- | --- |
| BL-only Profile 8.1 conversion | Baseline used by common P7 -> P8.1 workflows that discard EL |
| Current pixel-baked fallback | Control path: composite FEL into pixels, re-encode, generate fresh P8.1 RPU |
| Metadata-fit P8.1 experiment | Research output: original BL pixels plus derived/fitted RPU |
| Trusted FEL reference | Target output for comparison |

The metadata-fit output should use a distinct filename suffix such as:

```text
*.P81.metadata-fit.mkv
```

The current fallback output should remain separate and should not be used to imply that metadata-only preservation succeeded.

### 2.2 Analysis Pipeline

1. Extract BL, EL, and RPU from the Profile 7 source.
2. Decode BL-only frames to a stable raw format.
3. Generate or capture a trusted FEL-composited reference.
4. Align frames exactly and verify matching frame counts.
5. Compute per-frame residuals:
   - luma residual,
   - chroma residual,
   - spatial heatmaps,
   - frequency-domain summaries,
   - same-BL-value output collisions.
6. Fit candidate transforms:
   - luma polynomial by frame or scene,
   - chroma MMR by frame or scene,
   - optional trim-style L2/L8 approximation if feasible.
7. Quantize fitted coefficients to RPU-compatible values.
8. Generate an experimental Profile 8.1 RPU.
9. Inject the experimental RPU into the original BL.
10. Compare BL-only, metadata-fit, and pixel-baked outputs against the FEL reference.

### 2.3 Decision Gates

Do not proceed to metadata generation for a sample if:

- frame alignment is uncertain,
- the trusted FEL reference is not trusted,
- residuals are dominated by high-frequency/spatial content,
- same-BL-value collision rates are high,
- a simple fitted transform produces worse artifacts than BL-only.

Keep the metadata-fit result only if:

- it beats BL-only by objective metrics and visual inspection,
- it does not introduce obvious hue/luma instability,
- it remains HDR10-compatible in fallback playback,
- it behaves consistently on at least one real Dolby Vision display or player chain.

---

## 3. Implementation Shape

### 3.1 New Research Tooling

Implement this as a research/experimental path, not as the default `mkvdovi` conversion path.

Suggested interface:

```bash
mkvdovi fel-fit "/path/to/profile7_fel.mkv" \
  --keep-workdir \
  --sample-frames 0,100,500 \
  --output-report report.json
```

The command should produce:

- extracted BL/EL/RPU paths,
- decoded sample frames,
- residual heatmaps,
- fitted coefficient candidates,
- an experimental RPU if gates pass,
- `*.P81.metadata-fit.mkv`,
- a machine-readable report.

### 3.2 Report Contents

Each run should record:

- input file path and hash,
- MediaInfo summary,
- `dovi_tool info` summary,
- detected FEL/MEL status,
- frame count and frame-rate,
- exact command lines,
- selected frames/scenes,
- fit model used,
- fit error per frame/scene/channel,
- same-BL-value collision statistics,
- spatial-frequency summary,
- objective metric comparison against BL-only and pixel-baked control,
- final decision: `fit_accepted`, `fit_rejected`, or `needs_manual_review`.

### 3.3 Fallback Policy

The current pixel-baking branch remains the practical fallback:

```text
BL + EL + original RPU -> composited HDR10 encode -> fresh P8.1 RPU
```

The metadata-only path is allowed to fail. A clean rejection with evidence is a successful research result.

---

## 4. Revised Expected Outcomes

### Best Case

For a weak-FEL or mostly global residual sample:

- metadata-fit output is visibly closer to FEL reference than BL-only,
- residual heatmaps show mostly smooth low-frequency error,
- fitted luma/chroma transforms pass quantization without large error,
- no re-encode is required.

### Mixed Case

For content with both global and localized residuals:

- metadata-fit improves some scenes but fails others,
- the report identifies which scenes are not metadata-representable,
- output is either scene-gated or rejected.

### Worst Case

For spatial/high-frequency FEL residuals:

- metadata-only fitting is rejected,
- the current pixel-baked fallback is the only branch that can carry the residual into Profile 8.1 output,
- the report explains why metadata-only failed.

The original 80-90% and 60-80% preservation numbers should not be treated as targets. They were hypotheses. The experiment should replace them with measured results.

---

## 5. Validation Metrics

Use metrics to compare three outputs against a trusted FEL reference:

| Metric | Use |
| --- | --- |
| PSNR / SSIM | Basic pixel-level sanity checks |
| VMAF or equivalent | Broad perceptual comparison where tooling is valid for HDR workflow |
| Delta E / ICtCp delta | Color/luminance perceptual error |
| Residual heatmaps | Visual inspection of spatial failure modes |
| Same-BL collision rate | Direct test for metadata representability |
| Fit RMSE per channel | Model quality before RPU quantization |
| Post-quantization RMSE | Model quality after RPU-compatible coefficient encoding |

The final report should compare:

```text
BL-only vs FEL reference
metadata-fit vs FEL reference
pixel-baked fallback vs FEL reference
```

---

## 6. Open Research Questions

1. Can the current in-repo compositor be made accurate enough to serve as the trusted FEL reference, or do we need hardware/professional-tool capture?
2. How often are real FEL residuals mostly smooth/parametric?
3. Which RPU primitives are actually accepted by Profile 8.1 decoders for derived mapping data?
4. Does a fitted RPU behave consistently across Dolby Vision playback devices?
5. Is metadata-fit ever better than simply preserving original RPU while discarding EL?
6. Can we detect bad-fit scenes automatically and safely fall back?

---

## 7. 2026-07-09 MEL/FEL Detection Findings

Profile 7 detection must not treat every `dvhe.07` stream as FEL. A Profile 7 MEL carries an empty enhancement layer, so it should use a metadata-only path: discard the empty EL, convert the RPU to Profile 8.1, and remux. Running the FEL compositor on MEL inputs is unnecessary work and can obscure the simpler repair option.

A deep-inspection pass on a 4K UHD Blu-ray remux TV episode showed a separate metadata failure mode: the source RPU's declared L1 peaks clipped at the mastering ceiling while measured base-layer luminance reached materially higher peaks. On DV-honoring playback chains, that mismatch can produce a visibly dim picture because the player trusts the understated RPU.

| Signal | Observed pattern |
| --- | --- |
| Declared L1 peak ceiling | Hard-clipped near the mastering peak |
| Measured base-layer peak | Reached roughly 2.4x the declared ceiling |
| Affected scenes | Most scenes above the declared ceiling |
| Worst scene mismatch | Declared low peak vs measured peak above 2000 nits |
| Practical fix for MEL | Rebuild RPU from measurements and remux, no re-encode |

This reinforces two requirements for the preservation branch:

- Classify Profile 7 MEL vs FEL from RPU NLQ data, not from `dvhe.07` alone.
- Offer a metadata-rebuild path for MEL and Profile 8 sources where the original RPU looks unreliable, while keeping original L5/L6 context and discarding original L2 trims.

---

## References

- [Dolby Vision UHD Blu-ray Authoring Workflow Guide](https://professional.dolby.com/siteassets/pdfs/dolby_vision_uhd_bluray_authoring_workflow.pdf)
- [Dolby Vision Metadata Levels](https://professionalsupport.dolby.com/s/article/Dolby-Vision-Metadata-Levels)
- [Dolby Vision Profiles and Levels](https://professionalsupport.dolby.com/s/article/What-is-Dolby-Vision-Profile?language=en_US)
- `docs/profile7_fel_developer_handoff.md`
- `mkvdovi/src/fel_composite.rs`
