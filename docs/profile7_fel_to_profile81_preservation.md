# Research Brief: Profile 7 FEL → Profile 8.1 with FEL Preservation

## Executive Summary

This document explores a novel approach to converting Dolby Vision Profile 7 FEL content to Profile 8.1 while preserving the artistic intent of the Full Enhancement Layer—without requiring a full video re-encode.

**Core Insight**: Consumer displays are 10-bit. FEL data gets composited and dithered to 10-bit during playback anyway. If FEL adjustments are primarily curve-based (not arbitrary per-pixel corrections), we may be able to capture them as modified RPU polynomial coefficients and apply them to the unchanged BT.2020 base layer.

---

## 1. Understanding FEL Content

### 1.1 What FEL Contains

The Full Enhancement Layer in Profile 7 consists of:

| Component | Description | Representable in RPU? |
|-----------|-------------|----------------------|
| Additional bit-depth | Pushes effective precision from 10-bit to 12-bit | Partially (dithering preserves perceptual quality) |
| Polynomial reshaping | Luma curve adjustments per scene | **Yes** |
| MMR (Multivariate Multiple Regression) | Chroma corrections | **Yes** (Profile 8.1 supports MMR) |
| Per-pixel residuals (NLQ) | Arbitrary spatial corrections | **No** |
| Colorist trims | Lift/Gamma/Gain, saturation adjustments | **Yes** (L2/L8 metadata) |

### 1.2 Key Observation

The RPU already contains polynomial and MMR mapping data—but in Profile 7 FEL, these coefficients describe how to **process the Enhancement Layer**, not how to directly adjust the Base Layer for display.

When FEL is discarded (current `-m 2` conversion), these reshaping coefficients are removed because they're meaningless without the EL data they reference.

**The hypothesis**: We can derive *new* coefficients that describe the *net effect* of FEL processing on the final image, then apply these to the BL directly.

---

## 2. Theoretical Workflow

### 2.1 High-Level Pipeline

```
┌─────────────────────────────────────────────────────────────────┐
│                     Profile 7 FEL Source                        │
│                    (BL + EL + RPU)                              │
└─────────────────────┬───────────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────────┐
│  Step 1: Decode & Composite                                     │
│  - Decode BL (10-bit BT.2020)                                   │
│  - Decode EL                                                    │
│  - Apply RPU reshaping + NLQ                                    │
│  - Output: "Intended" 12-bit composited frames                  │
└─────────────────────┬───────────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────────┐
│  Step 2: Analyze Differences                                    │
│  - Compare: Raw BL vs Composited output                         │
│  - Per-frame or per-scene analysis                              │
│  - Separate luma vs chroma differences                          │
└─────────────────────┬───────────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────────┐
│  Step 3: Curve Fitting                                          │
│  - Fit polynomial curves to luma differences                    │
│  - Fit MMR coefficients to chroma differences                   │
│  - Constrain to RPU specification limits                        │
└─────────────────────┬───────────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────────┐
│  Step 4: Generate Modified RPU                                  │
│  - Inject derived polynomials into rpu_data_mapping             │
│  - Preserve original L1/L2/L8 dynamic metadata                  │
│  - Output: Enhanced Profile 8.1 RPU                             │
└─────────────────────┬───────────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────────┐
│  Step 5: Inject & Mux                                           │
│  - Inject new RPU into original BL HEVC (no re-encode!)         │
│  - Mux to container                                             │
│  - Output: Profile 8.1 with FEL-derived enhancements            │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 Why This Might Work

1. **No HEVC re-encode**: The base layer pixels remain untouched. We only modify the RPU metadata that tells the display how to process those pixels.

2. **RPU is expressive**: Profile 8.1 RPU supports:
   - Polynomial reshaping (up to 8th order) for luma
   - MMR coefficients for chroma
   - Per-scene or per-frame granularity

3. **Display does the work**: A Dolby Vision display will apply our derived curves in real-time, achieving similar results to what FEL compositing would have produced.

### 2.3 What Gets Lost

- **Arbitrary per-pixel corrections**: If the colorist applied localized adjustments (vignettes, specific region fixes), these cannot be represented in parametric curves
- **Mathematical precision**: 12-bit → 10-bit quantization is inherent, but this is true even in normal FEL playback
- **Non-polynomial relationships**: If FEL adjustments don't fit polynomial/MMR models well, approximation error occurs

---

## 3. Technical Deep Dive

### 3.1 RPU Data Mapping Structure

From dovi_tool/dolby_vision source:

```rust
pub struct RpuDataMapping {
    pub mapping_idc: [u64; 3],           // 0=poly, 1=MMR per channel
    pub mapping_param_pred_flag: [u64; 3],
    pub num_mapping_param_predictors: [u64; 3],
    pub diff_pred_part_idx_mapping_minus1: [u64; 3],
    pub poly_order_minus1: [u64; 3],     // Polynomial degree (0-7 → order 1-8)
    pub poly_coef_int: [Vec<i64>; 3],    // Integer part of coefficients
    pub poly_coef: [Vec<u64>; 3],        // Fractional part of coefficients
    pub mmr_order_minus1: u64,           // MMR order
    pub mmr_constant_int: [i64; 3],
    pub mmr_constant: [u64; 3],
    pub mmr_coef_int: [Vec<Vec<i64>>; 3],
    pub mmr_coef: [Vec<Vec<u64>>; 3],
}
```

**Key fields for our purpose**:
- `poly_order_minus1[0]`: Luma polynomial order (typically 0-2, meaning 1st to 3rd order)
- `poly_coef[0]`: Luma polynomial coefficients
- `mmr_order_minus1`: Chroma MMR order
- `mmr_coef[1], mmr_coef[2]`: Chroma MMR coefficients

### 3.2 Polynomial Reshaping Model

For luma channel, the reshaping polynomial:

```
Y_out = c0 + c1*Y_in + c2*Y_in² + c3*Y_in³ + ... + cn*Y_in^n
```

Where:
- `Y_in` is the input luma value (normalized 0-1 or PQ-encoded)
- `Y_out` is the reshaped output
- `c0...cn` are the polynomial coefficients from RPU

**Fitting approach**: Given pairs of (BL_luma, Composited_luma) for each frame, use least-squares polynomial regression to find coefficients.

### 3.3 MMR (Multivariate Multiple Regression) Model

For chroma channels, MMR uses a more complex model:

```
Cb_out = f(Y, Cb, Cr)
Cr_out = g(Y, Cb, Cr)
```

Where f and g are polynomial functions of all three input channels. This allows chroma corrections that depend on luma (common in color grading).

**MMR coefficient structure** (order 1 example):
```
Cb_out = k0 + k1*Y + k2*Cb + k3*Cr + k4*Y*Cb + k5*Y*Cr + k6*Cb*Cr + k7*Y*Cb*Cr
```

### 3.4 Coefficient Constraints

RPU coefficients have specific bit-depth and range constraints:

| Field | Bits | Range | Notes |
|-------|------|-------|-------|
| poly_coef_int | Signed | Variable | Integer part |
| poly_coef | Unsigned | 0 - 2^precision | Fractional part |
| Effective precision | ~12-16 bits | | Combined int+frac |

The curve fitting algorithm must quantize results to these constraints.

---

## 4. Implementation Plan

### 4.1 Required Components

#### Component 1: FEL Decoder/Compositor

**Purpose**: Produce the "intended" output that combines BL + EL + RPU processing

**Options**:
- libplacebo (open source, supports DV reshaping)
- FFmpeg with libplacebo filter
- Dolby professional tools (if accessible)

**Output**: Frame sequence or video file representing composited result

```bash
# Potential FFmpeg + libplacebo approach
ffmpeg -i profile7_fel.mkv \
  -vf "format=yuv420p16le,libplacebo=..." \
  -f rawvideo composited.yuv
```

#### Component 2: Difference Analyzer

**Purpose**: Compare raw BL frames to composited frames, extract per-scene statistics

**Implementation sketch** (Python):

```python
import numpy as np
from scipy.optimize import curve_fit

def analyze_frame(bl_frame, composited_frame):
    """
    Analyze luma and chroma differences between BL and composited frames.
    Returns statistics for curve fitting.
    """
    # Extract Y, Cb, Cr channels
    bl_y, bl_cb, bl_cr = split_yuv(bl_frame)
    comp_y, comp_cb, comp_cr = split_yuv(composited_frame)
    
    # Collect (input, output) pairs for luma
    luma_pairs = list(zip(bl_y.flatten(), comp_y.flatten()))
    
    # Collect tuples for chroma MMR: (Y, Cb_in, Cr_in) -> (Cb_out, Cr_out)
    chroma_data = collect_chroma_tuples(bl_y, bl_cb, bl_cr, comp_cb, comp_cr)
    
    return luma_pairs, chroma_data

def fit_luma_polynomial(luma_pairs, max_order=3):
    """
    Fit polynomial to luma transformation.
    """
    x = np.array([p[0] for p in luma_pairs])
    y = np.array([p[1] for p in luma_pairs])
    
    # Fit polynomial of specified order
    coefficients = np.polyfit(x, y, max_order)
    
    # Calculate residual error
    y_pred = np.polyval(coefficients, x)
    rmse = np.sqrt(np.mean((y - y_pred) ** 2))
    
    return coefficients, rmse

def fit_mmr_coefficients(chroma_data, order=1):
    """
    Fit MMR model to chroma transformation.
    """
    # Build design matrix for MMR regression
    # ... (complex implementation based on MMR order)
    pass
```

#### Component 3: RPU Generator/Editor

**Purpose**: Create Profile 8.1 RPU with derived coefficients

**Approach**: Extend dovi_tool or create wrapper

```rust
// Conceptual extension to dovi_tool
pub fn create_fel_preserved_rpu(
    original_rpu: &DoviRpu,
    derived_poly_coef: &[f64],
    derived_mmr_coef: &[Vec<f64>],
) -> DoviRpu {
    let mut new_rpu = original_rpu.clone();
    
    // Convert to Profile 8.1 structure
    new_rpu.dovi_profile = 81;
    
    // Inject derived polynomial coefficients
    if let Some(ref mut mapping) = new_rpu.rpu_data_mapping {
        mapping.mapping_idc[0] = 0;  // Use polynomial for luma
        mapping.poly_order_minus1[0] = (derived_poly_coef.len() - 1) as u64;
        mapping.poly_coef[0] = quantize_coefficients(derived_poly_coef);
        
        // Inject MMR for chroma
        mapping.mapping_idc[1] = 1;  // Use MMR for Cb
        mapping.mapping_idc[2] = 1;  // Use MMR for Cr
        mapping.mmr_coef = quantize_mmr_coefficients(derived_mmr_coef);
    }
    
    // Preserve dynamic metadata
    // (L1, L2, L8 already present in original_rpu.vdr_dm_data)
    
    new_rpu
}
```

#### Component 4: Validation Pipeline

**Purpose**: Compare original FEL playback to converted output

```bash
# Generate reference frames from original FEL
ffmpeg -i original_p7_fel.mkv -vf "select=eq(n\,1000)" -frames:v 1 ref_frame.png

# Generate frames from converted P8.1 (needs DV-capable player/decoder)
# Compare using VMAF, SSIM, or perceptual metrics
```

### 4.2 Workflow Script (Conceptual)

```bash
#!/bin/bash
# fel_to_p81_preserved.sh

INPUT="$1"
OUTPUT="$2"

# Step 1: Extract streams
echo "Extracting streams..."
ffmpeg -i "$INPUT" -map 0:v:0 -c copy bl.hevc
ffmpeg -i "$INPUT" -map 0:v:1 -c copy el.hevc 2>/dev/null || echo "Single-track FEL"

# Step 2: Extract original RPU
echo "Extracting RPU..."
dovi_tool extract-rpu "$INPUT" -o original_rpu.bin

# Step 3: Decode composited output
echo "Decoding with FEL compositing..."
ffmpeg -i "$INPUT" \
  -vf "libplacebo=tonemapping=clip" \
  -pix_fmt yuv420p10le \
  -f rawvideo composited.yuv

# Step 4: Decode BL only
echo "Decoding BL only..."
ffmpeg -i bl.hevc \
  -pix_fmt yuv420p10le \
  -f rawvideo bl_only.yuv

# Step 5: Analyze and fit curves (custom tool)
echo "Analyzing FEL contribution..."
fel_analyzer --bl bl_only.yuv \
             --composited composited.yuv \
             --original-rpu original_rpu.bin \
             --output-rpu enhanced_rpu.bin

# Step 6: Inject enhanced RPU into BL
echo "Injecting enhanced RPU..."
dovi_tool inject-rpu -i bl.hevc -r enhanced_rpu.bin -o bl_enhanced.hevc

# Step 7: Mux
echo "Muxing final output..."
ffmpeg -i bl_enhanced.hevc -i "$INPUT" \
  -map 0:v -map 1:a -c copy \
  -movflags +faststart \
  "$OUTPUT"

echo "Done: $OUTPUT"
```

---

## 5. Challenges & Mitigations

### 5.1 FEL Decoding

**Challenge**: No open-source tool fully composites Profile 7 FEL correctly.

**Mitigations**:
- Investigate libplacebo's current FEL support
- Check if mpv with vo=gpu-next can output composited frames
- Consider frame-by-frame extraction from a hardware player via capture card
- Contact dovi_tool maintainer about FEL decoding capabilities

### 5.2 Non-Polynomial Adjustments

**Challenge**: FEL may contain adjustments that don't fit polynomial models.

**Mitigations**:
- Analyze real FEL content to understand typical adjustment patterns
- Use higher-order polynomials if needed (up to 8th order supported)
- Accept some loss for truly arbitrary corrections
- Report per-scene fitting error to identify problematic scenes

### 5.3 Scene Boundary Detection

**Challenge**: Curves may need to change at scene boundaries (matching original L1 shot boundaries).

**Mitigations**:
- Parse original RPU for scene/shot boundaries
- Fit separate curves per scene
- Use existing L1 metadata boundaries as reference

### 5.4 Coefficient Quantization

**Challenge**: Derived floating-point coefficients must be quantized to RPU format.

**Mitigations**:
- Study dovi_tool's existing coefficient quantization
- Validate quantized curves produce acceptable error
- Consider iterative refinement if quantization error is high

---

## 6. Expected Outcomes

### 6.1 Best Case

For content where FEL primarily applies global curve adjustments:
- **80-90%** of FEL artistic intent preserved
- No re-encoding required (fast conversion)
- Visually indistinguishable on consumer displays
- Full HDR10 fallback compatibility

### 6.2 Typical Case

For content with mixed global and localized adjustments:
- **60-80%** of FEL intent preserved via curves
- Some scenes may show minor differences in shadows/highlights
- Still superior to simple FEL discard

### 6.3 Worst Case

For content with heavy per-pixel corrections:
- Polynomial fitting produces high error
- May need to fall back to pixel-baking (full re-encode) for some titles
- Analyzer should flag these cases

---

## 7. Validation Metrics

### 7.1 Objective Metrics

| Metric | Target | Notes |
|--------|--------|-------|
| VMAF | > 95 | Compare FEL playback vs converted P8.1 |
| PSNR | > 40 dB | Basic quality check |
| Delta E (ICtCp) | < 3 | Perceptual color difference |
| Polynomial RMSE | < 0.5% | Curve fitting error |

### 7.2 Subjective Testing

- A/B comparison on reference display
- Focus on: shadow detail, highlight rolloff, skin tones
- Test on target device (Shield Pro) for practical validation

---

## 8. Research Questions for Implementation

1. **libplacebo FEL support**: What's the current state? Can it composite Profile 7 FEL to produce reference output?

2. **Typical FEL patterns**: Analyze 10-20 FEL titles. What percentage of adjustments are curve-based vs spatial?

3. **RPU injection without re-encode**: Confirm dovi_tool's `inject-rpu` preserves frame alignment with modified coefficients.

4. **Display behavior**: How do consumer DV displays (LG OLED, etc.) actually apply RPU reshaping? Is it per-frame or batched?

5. **Coefficient precision**: What's the practical precision limit for RPU polynomials? Is 3rd order sufficient for most content?

---

## 9. Next Steps

1. **Prototype FEL decoder**: Get libplacebo or mpv outputting composited frames
2. **Build analyzer**: Python script to compare frames and fit curves
3. **Test on single title**: Pick a known FEL title, run full pipeline
4. **Measure quality**: Compare to original FEL playback
5. **Iterate**: Refine curve fitting based on results

---

## References

- dovi_tool RpuDataMapping: `dolby_vision/src/rpu/rpu_data_mapping.rs`
- Dolby Vision RPU specification (from patents and reverse engineering)
- libplacebo DV support: https://code.videolan.org/videolan/libplacebo
- Profile 7 structure: https://professionalsupport.dolby.com/s/article/What-is-Dolby-Vision-Profile
