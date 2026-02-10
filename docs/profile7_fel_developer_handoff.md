# Profile 7 FEL → Profile 8.1 Conversion — Developer Handoff

> **Status:** Work-in-progress — core pipeline functional, critical gaps remain  
> **Date:** February 2026  
> **Crate:** `mkvdolby`

---

## Table of Contents

1. [Project Overview](#project-overview)
2. [Research & Findings](#research--findings)
3. [Implementation Summary](#implementation-summary)
4. [Files Created / Modified](#files-created--modified)
5. [Testing & Verification](#testing--verification)
6. [Known Issues & Critical Gaps](#known-issues--critical-gaps)
7. [What Needs To Be Done Next](#what-needs-to-be-done-next)
8. [Key Technical Context](#key-technical-context)
9. [Files to Read First](#files-to-read-first)
10. [Build & Run Commands](#build--run-commands)

---

## Project Overview

This feature adds support for converting **Dolby Vision Profile 7 FEL (Full Enhancement Layer)** content to **Profile 8.1** by compositing the BL+EL layers together, re-encoding to HDR10, and generating fresh DV RPU metadata.

Profile 7 FEL is a dual-layer format where the Enhancement Layer (EL) contains per-pixel NLQ (Non-Linear Quantization) residuals that must be added to the Base Layer (BL) to reconstruct the original HDR image. Without FEL decoding, viewers see a degraded picture (e.g., solid red or washed-out colors).

---

## Research & Findings

### Test Material

- **File:** `/mnt/storage/downloads/complete/DolbyVisionProfile7-testfiles/FEL TEST ST DL P7 CMV4.0 4000nits V4-MKV.mkv`
- **Description:** P7 FEL test pattern — shows pure red on non-FEL devices and "THIS DEVICE CAN DECODE FEL" text on FEL-compatible devices
- **Duration:** ~2 minutes, 122 frames (5-second clip used for fast iteration)

### Key Discovery: `dovi_tool` Cannot Composite FEL NLQ

All `dovi_tool` conversion modes (1, 2, and 5) fail for this file — all produce red output. The reason: the FEL data is encoded entirely as **NLQ per-pixel residuals**, not parametric curves. `dovi_tool`'s mode 2 applies parametric polynomial/MMR curves but does not apply the NLQ pixel-level corrections.

Furthermore, `dovi_tool info -f N` does **not** expose NLQ data in its JSON output — the `rpu_data_nlq` field is simply absent. This forced us to use the `dolby_vision` Rust crate directly for RPU parsing.

### NLQ Compositing Algorithm

The NLQ LinearDeadzone compositing formula was reverse-engineered from quietvoid's [`vs-nlq`](https://github.com/quietvoid/vs-nlq) VapourSynth plugin — the only known open-source implementation.

**Formula (per pixel):**
```
offset = nlq_offset  (fixed-point, vdr_bit_depth precision)
hdr_bit_depth = 12 (always for DV output)
shift = hdr_bit_depth - el_bit_depth

el_shifted = el_sample << shift
dead_zone = (vdr_in_max - vdr_in_min) * linear_deadzone_slope + linear_deadzone_threshold
range = vdr_in_max - vdr_in_min

if el_shifted < dead_zone:
    residual = 0
else:
    residual = (el_shifted - dead_zone) * range / (2^hdr_bit_depth - dead_zone)

if el_shifted > 0:
    output = BL_reshaped + offset + residual
else:
    output = BL_reshaped + offset - residual
```

### Planning Docs vs Implementation

The original planning docs (`docs/profile7_fel_to_profile81_preservation.md` and `docs/fel_to_rpu_research_brief.md`) proposed a no-re-encode approach using curve-fitting (capturing NLQ residuals as parametric L1 trim metadata). We implemented the **"Section 6.3 fallback"** (full re-encode) instead, because NLQ per-pixel residuals fundamentally cannot be captured as parametric curves without significant quality loss.

---

## Implementation Summary

### Architecture

```
Input MKV (P7 FEL)
    │
    ├─ Extract HEVC stream (ffmpeg)
    ├─ Demux BL+EL (dovi_tool demux)
    │
    ├─ Decode BL (ffmpeg pipe → raw YUV)
    ├─ Decode EL (ffmpeg pipe → raw YUV)
    ├─ Parse RPU (dolby_vision crate → NLQ params per frame)
    │
    ├─ Composite Loop (per frame):
    │   ├─ Read BL frame (3840×2160)
    │   ├─ Read EL frame (1920×1080 → upscale to 3840×2160 via Lanczos)
    │   ├─ [TODO] Apply polynomial reshaping to BL luma
    │   ├─ [TODO] Apply MMR reshaping to BL chroma
    │   ├─ Add NLQ residual (LinearDeadzone formula)
    │   └─ Write composited frame
    │
    ├─ Re-encode to HEVC HDR10 (x265/nvenc/videotoolbox)
    └─ Return composited MKV path
         │
         └─ Standard mkvdolby pipeline continues:
              ├─ Generate measurements (hdr_analyzer_mvp)
              ├─ Generate DV RPU (dovi_tool generate)
              ├─ Inject RPU (dovi_tool inject)
              └─ Mux final MKV (mkvmerge, audio/subs from original)
```

### Pipeline Integration

The FEL conversion hooks into `mkvdolby`'s existing pipeline at the format-detection stage:

1. **Detection:** `metadata.rs` checks for `dvhe.07` codec ID via MediaInfo → returns `HdrFormat::DolbyVisionFel`
2. **Conversion:** `pipeline.rs` matches on `DolbyVisionFel`, calls `fel_composite::convert_fel_to_hdr10()`
3. **Handoff:** The composited MKV (clean HDR10) is passed back to the standard pipeline as `bl_source_file`, and `hdr_type` is changed to `Hdr10WithMeasurements`
4. **Audio/Subs:** `mkvmerge` pulls audio and subtitle tracks from the **original** input file (not the composited MKV)

---

## Files Created / Modified

### New Files

| File | Lines | Purpose |
|------|-------|---------|
| `mkvdolby/src/fel_composite.rs` | ~968 | Core FEL compositing engine |

**`fel_composite.rs` contains:**
- `NlqParams` struct — per-frame NLQ parameters (offsets, slopes, thresholds, min/max)
- `parse_rpu_nlq_params()` — extracts NLQ parameters from `DoviRpu` objects via `dolby_vision` crate
- `composite_plane()` — per-plane NLQ LinearDeadzone compositing
- `composite_bl_el_nlq()` — orchestrates BL+EL frame decoding, EL upsampling, NLQ compositing
- `reencode_composited()` — re-encodes composited YUV to HEVC with HDR10 static metadata SEIs
- `convert_fel_to_hdr10()` — top-level entry point (extract → demux → composite → encode → mux)
- 3 unit tests (identity composite, residual composite, number extraction)

### Modified Files

| File | Changes | Purpose |
|------|---------|---------|
| `mkvdolby/Cargo.toml` | +1 line | Added `dolby_vision = "3.3"` dependency |
| `mkvdolby/src/main.rs` | +1 line | Added `mod fel_composite;` module registration |
| `mkvdolby/src/cli.rs` | +8 lines | Added `--fel-crf` (u8, default 18) and `--fel-preset` (String, default "medium") CLI args |
| `mkvdolby/src/metadata.rs` | +31 lines | Added `DolbyVisionFel` variant to `HdrFormat` enum; P7 FEL auto-detection via `dvhe.07` |
| `mkvdolby/src/pipeline.rs` | +24 lines | Added `DolbyVisionFel` match branch calling `convert_fel_to_hdr10()`; made `hdr_type` mutable |
| `.gitignore` | +9 lines | Added entries for intermediate files and tool artifacts |

---

## Testing & Verification

### Automated Tests

All 7 workspace tests pass (3 new in `fel_composite.rs`):

```
test fel_composite::tests::test_composite_plane_identity ... ok
test fel_composite::tests::test_composite_plane_residual ... ok
test fel_composite::tests::test_extract_number ... ok
```

### Manual Tests — VERIFIED ✅

| Test | Result |
|------|--------|
| 5-second clip conversion (122 frames, ultrafast preset) | ✅ ~25 seconds |
| Full 2-minute test file (medium preset) | ✅ ~20 minutes |
| Output plays correctly on Nvidia Shield Pro + LG OLED C9 | ✅ FEL text visible |
| Output MediaInfo shows `dvhe.08.06` (Profile 8.1) | ✅ |
| Clippy clean | ✅ |
| `cargo fmt` clean | ✅ |

### Output File

```
FEL TEST ST DL P7 CMV4.0 4000nits V4-MKV.DV.mkv
Size: 2.57 MiB
Codec: HEVC dvhe.08.06 (Dolby Vision Profile 8.1)
```

---

## Known Issues & Critical Gaps

### 🔴 CRITICAL

#### 1. Polynomial/MMR Reshaping NOT Implemented Before NLQ

**Impact:** Real movie content will produce incorrect colors/luminance.

Per the Dolby Vision specification, the compositing pipeline is:
1. Apply **polynomial reshaping** to BL luma (channel 0)
2. Apply **MMR reshaping** to BL chroma (channels 1, 2)
3. **Then** add NLQ residual

Our `composite_plane()` function (line ~491) does `let mut h = bl_16;` and adds NLQ directly — **no reshaping is applied**.

This worked on the test file only because it had **identity polynomial mapping** (`poly_coef = [0, 1]` → `Y = X`). Real movie FEL content will likely have non-identity polynomials.

**Relevant structs from `dolby_vision` crate (v3.3.2):**

```rust
// rpu_data_mapping.rs
RpuDataMapping {
    curves: [DoviReshapingCurve; 3],  // per Y, Cb, Cr
    // ...
}

DoviReshapingCurve {
    pivots: Vec<u16>,
    mapping_idc: DoviMappingMethod,
    polynomial: Option<DoviPolynomialCurve>,
    mmr: Option<DoviMMRCurve>,
}

DoviPolynomialCurve {
    poly_order_minus1: Vec<u64>,         // per piece
    poly_coef_int: Vec<ArrayVec<[i64; 3]>>,   // integer parts
    poly_coef: Vec<ArrayVec<[u64; 3]>>,        // fractional parts
}

DoviMMRCurve {
    mmr_order_minus1: Vec<u8>,
    mmr_constant_int: Vec<i64>,
    mmr_constant: Vec<u64>,
    mmr_coef_int: Vec<ArrayVec<[i64; MMR_MAX_COEFFS]>>,
    mmr_coef: Vec<ArrayVec<[u64; MMR_MAX_COEFFS]>>,  // MMR_MAX_COEFFS = 7
}
```

**Formulas:**
- **Polynomial (luma):** `Y_out = c0 + c1 × Y_in + c2 × Y_in²` (piecewise between pivots)
- **MMR (chroma):** `Cb_out = k0 + k1×Y + k2×Cb + k3×Cr + k4×Y×Cb + k5×Y×Cr + k6×Cb×Cr` (+ higher orders)
- **Fixed-point:** `fp_coef = (coef_int << coeff_log2_denom) + coef_frac`

#### 2. Multi-TB Intermediate YUV Written to Disk

**Impact:** A 2-hour 4K movie would produce a ~4 TB `composited.yuv` file.

Currently `composite_bl_el_nlq()` writes raw YUV to a file, then `reencode_composited()` reads it back. The fix is to pipe composited output directly to the ffmpeg encoder's stdin (BL and EL decoding already uses pipes).

### 🟡 MEDIUM

#### 3. `el_spatial_resampling_filter_flag` Not Checked

The code always upscales EL from 1920×1080 → 3840×2160 unconditionally. It should check `rpu.header.el_spatial_resampling_filter_flag` and skip the scale filter if EL is already full resolution.

#### 4. Mastering Display Color Primaries Hardcoded

`reencode_composited()` (line ~689) hardcodes chromaticity coordinates:
```
G(8500,39850) B(6550,2300) R(35400,14600)
```

Luminance values (MaxCLL/MaxFALL) ARE correctly read from the source, but chromaticity coordinates are not extracted from the input. Additionally, the CUDA and VideoToolbox encoder paths don't set HDR10 static metadata SEIs at all.

#### 5. EL vs BL Dimension Comparison Missing

Should compare actual EL and BL decoded dimensions and skip the upscale filter when they already match, rather than assuming EL is always half-resolution.

---

## What Needs To Be Done Next

### Priority 1: Implement Polynomial/MMR Reshaping

1. Add reshaping curve data to `NlqParams` struct (or create a new `ReshapingParams` struct)
2. In `parse_rpu_nlq_params()`, extract `mapping.curves[ch]` polynomial/MMR data per frame per channel
3. In `composite_plane()`, **before** adding NLQ residual:
   - For **luma (channel 0):** find which piecewise segment the BL value falls in (using pivots), evaluate `c0 + c1×x + c2×x²`
   - For **chroma (channels 1, 2):** evaluate MMR formula `k0 + k1×Y + k2×Cb + k3×Cr + ...` using all 3 input channels
4. Handle fixed-point arithmetic: `fp_coef = (coef_int << coeff_log2_denom) + coef_frac`

### Priority 2: Stream Composited Output to Encoder

1. Spawn the ffmpeg encoder with stdin pipe before the compositing loop
2. Feed composited frames directly to encoder stdin in the loop
3. Remove the intermediate file write/read
4. This eliminates the ~4 TB intermediate file for feature-length content

### Priority 3: Fix Medium Issues

1. Check `rpu.header.el_spatial_resampling_filter_flag` before upscaling
2. Extract mastering display primaries from source MediaInfo
3. Compare EL/BL dimensions instead of assuming half-resolution
4. Add HDR10 SEI metadata to CUDA/VideoToolbox encoder paths

---

## Key Technical Context

| Item | Value |
|------|-------|
| `dolby_vision` crate version | 3.3.2 (already a dependency) |
| `coeff_log2_denom` | 23 (fixed-point precision for coefficient math) |
| BL bit depth | 10 |
| EL bit depth | 10 |
| VDR output bit depth | 12 |
| RPU parsing | `dolby_vision::rpu::utils::parse_rpu_file()` → `Vec<DoviRpu>` |
| Each `DoviRpu` fields | `.header`, `.rpu_data_mapping`, `.vdr_dm_data` |
| NLQ compositing | Correctly implemented — only the preceding reshaping step is missing |

---

## Files to Read First

| Priority | File | Reason |
|----------|------|--------|
| 1 | `mkvdolby/src/fel_composite.rs` | Main implementation file to modify |
| 2 | `dolby_vision-3.3.2/src/rpu/rpu_data_mapping.rs` | Crate structs for reshaping curves (551 lines) |
| 3 | `docs/profile7_fel_to_profile81_preservation.md` | Original planning doc for full context |
| 4 | `mkvdolby/src/pipeline.rs` | Integration point |
| 5 | `mkvdolby/src/metadata.rs` | Format detection logic |

---

## Build & Run Commands

```bash
# Build
cargo build --release -p mkvdolby

# Run all tests
cargo test --workspace

# Run FEL-specific tests
cargo test -p mkvdolby -- fel_composite

# Lint
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# Convert a P7 FEL file (quick test)
target/release/mkvdolby "/path/to/p7_fel.mkv" --keep-source --fel-preset ultrafast -v

# Convert with custom CRF
target/release/mkvdolby "/path/to/p7_fel.mkv" --keep-source --fel-crf 16 --fel-preset medium
```

### CLI Arguments for FEL

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `--fel-crf` | `u8` | `18` | CRF quality for re-encoding (lower = higher quality) |
| `--fel-preset` | `String` | `"medium"` | x265 encoding preset (ultrafast → veryslow) |
