# Research Brief: Lossless-ish FEL Preservation via RPU Modification

## Problem Statement

Dolby Vision Profile 7 files contain dual layers:
- **BL (Base Layer)**: 10-bit HEVC video
- **EL (Enhancement Layer)**: Additional pixel data, either FEL (Full) or MEL (Minimal)

FEL contains per-pixel corrections that push effective precision to 12-bit and include colorist-applied artistic adjustments (brightness curves, shadow lifts, highlight rolls, etc.).

Current conversion practice: Drop FEL entirely, convert to Profile 8.1. This loses the artistic enhancements.

## Proposed Concept

Avoid HEVC re-encoding by capturing FEL's *effective adjustments* as modified RPU metadata.

### Core Observation
- Consumer displays are 10-bit anyway
- FEL data gets composited and dithered down for display
- RPU contains parametric curves (polynomials, pivots, trim values)
- If FEL adjustments are primarily curve-based (not arbitrary spatial corrections), they could theoretically be derived and baked into RPU

### Theoretical Workflow
1. Decode and composite BL + FEL to get "intended" output
2. Analyze difference between raw BL and composited result
3. Derive best-fit parametric curves that approximate the FEL contribution
4. Modify or regenerate RPU with these curves
5. Output as Profile 5 or Profile 8 with original HEVC untouched

## Research Questions

1. **RPU expressiveness**: What transformations can RPU polynomials represent? Are they per-frame, per-scene, global? What are the polynomial degrees and coefficient limits?

2. **FEL content analysis**: For real-world Profile 7 content, how much of the FEL contribution is:
   - Global/curve-based (representable in RPU)
   - Spatially localized (not representable without re-encode)

3. **Existing tooling**: Can dovi_tool, FFmpeg, or other tools decode and composite BL+FEL to analyzable output? What libraries exist for RPU parsing and generation?

4. **Curve fitting feasibility**: Given frame-by-frame BL vs composite comparisons, can we reliably fit RPU-compatible polynomial curves? What error metrics would be acceptable?

5. **Profile 5 vs 8 target**: Profile 5 uses IPTPQc2 signal space (more perceptually uniform). Does this offer better curve-fitting for this purpose, or is Profile 8 equally suitable?

## Known Limitations

- Arbitrary per-pixel spatial corrections (vignettes, localized adjustments) cannot be captured in RPU
- This approach would be "lossy" for spatial FEL data but could preserve 80-90% of artistic intent for curve-based grading
- RPU must be regenerated relative to the specific BL—it describes transformations for that exact base layer

## Potential Tooling Stack

- **dovi_tool**: RPU extraction, injection, some conversion capabilities
- **FFmpeg**: Decoding, potentially with Dolby libraries
- **libplacebo**: HDR tone-mapping, might help with analysis
- **Custom tool needed**: BL vs composite analysis, curve fitting, RPU generation

## Success Criteria

A successful proof-of-concept would:
1. Take a Profile 7 FEL file
2. Output a Profile 5 or 8 file with original HEVC
3. Demonstrate visually comparable output to proper dual-layer playback
4. Quantify what percentage of FEL contribution was captured vs lost

## References to Investigate

- Dolby Vision specification documents (RPU structure, polynomial formats)
- dovi_tool source code and documentation
- Existing discussions on doom9, videohelp forums about FEL analysis
- Any academic papers on HDR metadata generation or curve fitting for video
