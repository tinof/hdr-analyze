# Profile 7 FEL -> Profile 8.1 Research Handoff

> **Status:** Research plan ready; current branch implements pixel-baking fallback only.
> **Updated:** June 4, 2026
> **Crate:** `mkvdolby`
> **Branch reviewed:** `feat/profile7-fel-to-profile81`

---

## Summary

This handoff is the implementation plan for testing the metadata-only FEL approximation hypothesis described in `docs/profile7_fel_to_profile81_preservation.md`.

Current branch state:

- There is a substantial Profile 7 handling path in `mkvdolby`.
- It demuxes BL/EL/RPU, composites BL+EL in software, re-encodes to HDR10 HEVC, then generates a fresh Profile 8.1 RPU.
- That path is a practical pixel-baking fallback, not no-reencode FEL preservation.

Research goal:

> Build an experimental path that keeps the original HDR10 BL pixels untouched, derives Profile 8.1 metadata from the measured FEL contribution, and proves whether that metadata-fit output is useful compared with BL-only and pixel-baked outputs.

---

## Phase 1: Baseline Truth

### Intent

Before fitting metadata, make the current inputs honest and reproducible.

### Required Behavior

- Detect Profile 7 MEL vs FEL using RPU/NLQ inspection, not just `dvhe.07`.
- Keep MEL out of the FEL pixel-baking path unless explicitly requested.
- Treat true FEL as an experimental input with two possible outputs:
  - `*.DV.mkv` from current pixel-baked fallback.
  - `*.P81.metadata-fit.mkv` from the research path if fit gates pass.
- Preserve the current pixel-baking path as the control branch.

### Acceptance Criteria

- A Profile 7 MEL source is reported as MEL and is not mislabeled as FEL.
- A Profile 7 FEL source reports non-zero residual/NLQ status.
- The run records MediaInfo, `dovi_tool info`, input hash, frame count, and exact command lines.

---

## Phase 2: Reference Generation

### Intent

Create comparable outputs for the same source and frame numbers.

### Required Outputs

For each sample:

- BL-only decode or BL-only P8.1 conversion.
- Current pixel-baked fallback output.
- Trusted FEL reference frames.
- Raw BL and reference frame pairs for analysis.

### Reference Rules

- Frame counts must match before analysis proceeds.
- Frame-rate and timebase must be recorded.
- If a trusted FEL reference cannot be generated, mark the sample `needs_reference` and do not claim metadata-fit quality.

### Acceptance Criteria

- Report includes frame alignment status.
- Selected sample frames are reproducible by command.
- Reference and BL sample frames are written into a work directory when `--keep-workdir` is set.

---

## Phase 3: Residual Analysis

### Intent

Determine whether the FEL contribution is representable by Profile 8.1 metadata before trying to generate metadata.

### Required Measurements

- Per-frame luma and chroma residuals.
- Residual heatmaps.
- Spatial-frequency summaries.
- Same-BL-value collision statistics:
  - group pixels by quantized BL YCbCr values,
  - measure whether the FEL target output differs within each group,
  - report collision rate and magnitude.
- Per-scene summaries if RPU scene boundaries are available.

### Decision Gate

Reject metadata-only fitting for a frame or scene if residuals are dominated by spatial/high-frequency differences or same-BL collision rates are high enough that a BL-value transform cannot represent the target.

### Acceptance Criteria

- Each analyzed frame has a report entry with representability verdict.
- Rejected frames/scenes include the reason: `spatial_residual`, `high_collision_rate`, `reference_untrusted`, or `fit_unstable`.

---

## Phase 4: Metadata-Only Prototype

### Intent

Attempt to represent the accepted residual subset as Profile 8.1-compatible RPU metadata.

### Required Behavior

- Fit luma polynomial candidates from BL luma to FEL-reference luma.
- Fit chroma MMR candidates from BL YCbCr to FEL-reference chroma.
- Quantize coefficients to RPU-compatible fixed-point representation.
- Preserve original dynamic metadata where compatible and record anything regenerated or dropped.
- Generate an experimental RPU only for accepted frames/scenes.
- Inject the experimental RPU into the original BL without re-encoding the BL.
- Output using a distinct suffix:

```text
*.P81.metadata-fit.mkv
```

### Interface

Add an experimental command or hidden flag. Suggested command shape:

```bash
mkvdolby fel-fit "/path/to/profile7_fel.mkv" \
  --keep-workdir \
  --sample-frames 0,100,500 \
  --output-report report.json
```

Do not make this the default `mkvdolby` path.

### Acceptance Criteria

- The metadata-fit output uses the original BL video essence.
- The report states exactly which fitted model was used per frame/scene.
- The report includes pre-quantization and post-quantization fit error.
- If fitting fails, the command exits cleanly with a report rather than silently falling back.

---

## Phase 5: Comparison And Decision

### Intent

Decide whether the metadata-only output is useful.

### Required Comparisons

Compare these outputs against the trusted FEL reference:

- BL-only output.
- Metadata-fit output.
- Pixel-baked fallback output.

Use:

- PSNR / SSIM for sanity.
- VMAF or equivalent if the HDR workflow is valid.
- Delta E or ICtCp delta for perceptual color/luminance error.
- Residual heatmaps for spatial artifacts.
- Manual frame review for selected scenes.

### Keep / Reject Rules

Keep metadata-fit as a candidate only if:

- it beats BL-only by objective metrics,
- visual inspection confirms the improvement,
- it does not introduce obvious hue/luma pumping,
- it remains HDR10-compatible for non-DV playback,
- it behaves consistently on at least one real Dolby Vision playback chain.

Reject it if:

- it only improves metrics but looks unstable,
- it is worse than BL-only in important scenes,
- it requires per-pixel behavior that Profile 8.1 metadata cannot carry,
- it depends on untrusted reference frames.

---

## Current Code Anchors

The existing branch already provides useful pieces:

- `mkvdolby/src/fel_composite.rs`
  - `convert_fel_to_hdr10()`
  - `demux_dual_layer()`
  - `composite_bl_el_nlq()`
  - `parse_rpu_params()`
  - `apply_polynomial_reshape()`
  - `apply_mmr_reshape()`
  - `composite_plane()`
- `mkvdolby/src/metadata.rs`
  - `HdrFormat::DolbyVisionFel`
  - `check_hdr_format()`
- `mkvdolby/src/pipeline.rs`
  - `DolbyVisionFel` branch in `convert_file()`

Do not treat the current compositor as a trusted reference until it has been compared against a known-good FEL decode.

---

## Known Risks

- Profile 8.1 metadata cannot store arbitrary residual video samples.
- The current active detection path treats Profile 7 broadly as `DolbyVisionFel`.
- The current chroma/MMR implementation uses simplified 4:2:0 co-siting.
- Existing FEL tests are synthetic and do not prove real-world FEL correctness.
- Professional or hardware reference generation may be needed to avoid circular validation.

---

## Suggested Research-Agent Task

> Investigate whether Dolby Vision Profile 7 FEL residuals in real UHD Blu-ray titles can be approximated by Profile 8.1 RPU metadata without re-encoding. Use primary sources where possible, then open-source tools and reproducible experiments. Determine whether FEL residuals are typically global/parametric or spatial/high-frequency. Compare several known FEL samples by extracting BL, EL, and RPU; generating a trusted FEL reference if possible; fitting polynomial/MMR transforms; and reporting objective/perceptual error. Conclude whether metadata-only approximation is useful, title-dependent, or not feasible.

---

## Source Notes

- [Dolby Vision UHD Blu-ray Authoring Workflow Guide](https://professional.dolby.com/siteassets/pdfs/dolby_vision_uhd_bluray_authoring_workflow.pdf): Profile 7 uses an HDR10-compatible base layer and an enhancement layer carrying residual data and Dolby Vision metadata.
- [Dolby Vision UHD Blu-ray Authoring Workflow Guide](https://professional.dolby.com/siteassets/pdfs/dolby_vision_uhd_bluray_authoring_workflow.pdf): MEL has zero residual; FEL has non-zero residual.
- [Dolby Vision Metadata Levels](https://professionalsupport.dolby.com/s/article/Dolby-Vision-Metadata-Levels): L1/L2/L8/L9/L11 metadata carries analysis, trims, primaries, and other creative/display-management information.
- [Dolby Vision Profiles and Levels](https://professionalsupport.dolby.com/s/article/What-is-Dolby-Vision-Profile?language=en_US): Dolby Vision profiles define layer count and cross-compatibility.

---

## Useful Commands

```bash
# Current quality gate
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --verbose

# Current FEL unit tests
cargo test -p mkvdolby fel_composite -- --nocapture

# Current pixel-baked fallback path
target/release/mkvdolby "/path/to/p7_fel.mkv" --keep-source --fel-preset ultrafast -v
```
