# Plan: Native HLG Support for hdr-analyze

## 1. Objective

To enhance `hdr-analyze` with native support for Hybrid Log-Gamma (HLG) video content. This will create a faster, more accurate, and truly lossless analysis workflow, eliminating the need for the current workaround of re-encoding HLG to PQ (HDR10) before analysis.

**Benefits:**
- **Lossless Workflow:** Analysis is performed on the original, untouched source data, preserving maximum fidelity.
- **Increased Speed:** The time-consuming `ffmpeg` re-encoding step is completely removed from the pipeline.
- **Improved Accuracy:** By avoiding a lossy re-encode, the analysis results (peak brightness, APL, etc.) will be more precise, free from any compression artifacts or shifts introduced during the intermediate conversion.

---

## 2. Current Workflow (For Context)

The `mkvdolby` script currently uses a valid but suboptimal workaround:

1.  **Detect HLG:** Uses `mediainfo` to identify the HLG transfer function.
2.  **Re-encode to PQ:** Uses `ffmpeg` and `libx265` to perform a lossy conversion of the HLG video into a temporary HDR10 (PQ) file.
3.  **Analyze PQ:** Runs `hdr-analyze` on the temporary PQ file.
4.  **Generate RPU:** Uses the generated measurements to create Dolby Vision metadata.

This plan aims to make steps 2 and 3 obsolete by integrating the logic directly into `hdr-analyze`.

---

## 3. Proposed Native HLG Workflow

The core idea is to modify `hdr-analyze` to understand HLG natively, performing the necessary conversions in-memory on a per-frame basis.

1.  **Detect HLG Stream:** The tool will inspect the video stream's metadata (e.g., via `ffmpeg-next`) to identify the transfer function as HLG (`arib-std-b67`).
2.  **Decode Frame:** The HLG frame is decoded into an in-memory buffer, just as with PQ content.
3.  **Apply HLG Inverse EOTF:** For each pixel in the analysis area, apply the HLG inverse EOTF formula to convert the non-linear signal value into a linear light value (nits). This is the crucial step that replaces the `zscale` filter from `ffmpeg`.
4.  **Perform Analysis:** All existing analysis logic (peak nits, average picture level) will run on these linear light values.
5.  **Map to PQ Histogram:** The linear light values are then mapped into the existing 256-bin PQ-based histogram structure required by the madVR measurement format. This involves converting the linear nit value back into a PQ signal value and finding the corresponding bin.

---

## 4. Implementation Steps

This can be broken down into the following engineering tasks:

-   **[x] Step 1: HLG Detection (`ffmpeg_io.rs`)**
    -   Extend the video stream information gathering to extract the transfer function characteristic.
    -   Store this characteristic (e.g., in an `enum TransferFunction { PQ, HLG, Unknown }`).
    -   Pass this information to the main pipeline.

-   **[x] Step 2: Create HLG Analysis Module (`analysis/hlg.rs`)**
    -   Create a new module to contain HLG-specific logic.
    -   Implement the HLG inverse EOTF function. This function will take a 10-bit pixel value and the system gamma (peak display brightness, typically 1000 nits for this context) and return a linear nit value.
    -   Implement a function to map a linear nit value to a PQ-based histogram bin.

-   **[x] Step 3: Integrate into the Pipeline (`pipeline.rs`, `analysis/frame.rs`)**
    -   In the main processing loop, add a conditional check for the transfer function.
    -   If `TransferFunction::HLG`, call the new HLG analysis path.
    -   If `TransferFunction::PQ`, use the existing analysis path.
    -   The output of both paths should be the same: a populated 256-bin PQ histogram and other frame statistics.

-   **[x] Step 4: Add CLI Control and Logging**
    -   Added `--hlg-peak-nits <nits>` to override assumed peak display brightness (default 1000).
    -   CLI now logs when the native HLG path is activated and reminds users about PQ fallback.

---

## 5. Key Mathematical Formulas & Implementation References

The research has confirmed the exact formulas and provided permissively licensed Rust code as a reference.

-   **HLG Inverse EOTF (Signal to Nits):**
    -   Reference: `moxcms` Rust crate (BSD-3-Clause OR Apache-2.0).
    -   The function converts a normalized HLG signal `x` (0.0-1.0) to a linear light value `L` (relative to a peak of 12.0, assuming a system gamma of 1.2). This value is then scaled by the display's peak luminance `Lw` (default 1000 nits).
    -   **Rust Implementation:**
        ```rust
        // Constants from moxcms, derived from BT.2100
        const A: f64 = 0.17883277;
        const B: f64 = 0.28466892;
        const C: f64 = 0.55991073;

        fn hlg_to_linear(hlg_signal: f64) -> f64 {
            if hlg_signal <= 0.5 {
                (hlg_signal * hlg_signal) / 3.0
            } else {
                ((hlg_signal - C) / A).exp().mul_add(1.0, B) / 12.0
            }
        }

        // Usage:
        // let normalized_10bit_value = pixel_value as f64 / 1023.0;
        // let relative_luminance = hlg_to_linear(normalized_10bit_value);
        // let nits = relative_luminance * 1000.0; // Assuming 1000 nit peak
        ```

-   **PQ EOTF (Linear Nits to PQ Signal for Histogram):**
    -   Reference: FFmpeg `libavutil/color_utils.c`.
    -   This function converts an absolute linear luminance value `L_c` (in nits) to a normalized PQ signal value `Np` (0.0-1.0), which can then be mapped to a histogram bin.
    -   **Rust Implementation:**
        ```rust
        // Constants from SMPTE ST-2084
        const C1: f64 = 0.8359375;   // 3424/4096
        const C2: f64 = 18.8515625;  // 2413/4096 * 32
        const C3: f64 = 18.6875;     // 2392/4096 * 32
        const M: f64 = 78.84375;    // 128/4096 * 2523
        const N: f64 = 0.1593017578125; // 2610/16384

        fn linear_to_pq(nits: f64) -> f64 {
            let l = nits / 10000.0; // Normalize by PQ's 10,000 nit peak
            let ln = l.powf(N);
            let np = ((C1 + C2 * ln) / (1.0 + C3 * ln)).powf(M);
            np
        }
        ```

---

## 6. Validation and Testing

The research provides concrete test patterns and a clear validation strategy.

1.  **Unit Testing with Known Values:**
    -   Use the formulas to create unit tests. For example, verify that HLG signal 0.75 (721 in 10-bit narrow) maps to ~203 nits, which in turn maps to a PQ signal of ~0.58.
    -   Reference: ITU-R BT.2111-1 and BT.2408 for known HLG/PQ mapping values.

2.  **Reference Comparison (Integration Test):**
    -   Process a set of HLG test clips using the current `mkvdolby` re-encoding method.
    -   Process the same clips using the new native HLG path.
    -   Compare the generated `_measurements.bin` files. The results should be very close, with any differences attributable to the removal of the lossy compression step.

3.  **Test Pattern Validation:**
    -   Use the following open-source test patterns to validate the implementation against ground truth.
    -   **ARIB STD-B72 Color Bars:**
        -   **Source:** [KCAM's ARIB STD-B72 Generator (MIT)](https://github.com/k-kuro/ARIB-STD-B72-Color-Bar-Generator)
        -   **Method:** Generate the YUV file and encode it to HEVC with HLG flags. Analyze the resulting file and verify that the measured nit values for the color patches match the official specification.
    -   **HLG Grayscale Ramp:**
        -   **Source:** [Diversified Video Solutions HLG Demo](https://www.diversifiedvideosolutions.com/dvs_uhd_hdr-10_and_hlg_test_patterns.php) (look for `UHD_HLG-HDR_Grayscale_Demo.zip`)
        -   **Method:** Analyze the grayscale ramp and ensure the measured luminance increases correctly according to the HLG curve.

4.  **Downstream Tooling:**
    -   Ensure that `dovi_tool` can successfully process the measurements generated from the native HLG path to create a valid Dolby Vision RPU.
