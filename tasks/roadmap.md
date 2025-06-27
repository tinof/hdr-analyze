

### Analysis of the Research Findings

Let's break down the key takeaways from each directive:

1.  **Black Bar Detection:** The recommendation to adapt FFmpeg's `cropdetect` algorithm is perfect. It's a proven, efficient, and well-understood method. The provided pseudocode and default numerical values (`limit=24`, `round=16`) give us a concrete starting point for implementation. **This is a solved problem.**

2.  **Histogram Scene Detection:** The recommendation of **Chi-Squared Distance** is a solid engineering choice, balancing accuracy and performance. The suggested threshold range of `0.2` to `0.5` is an excellent starting point for tuning. This provides a clear path to implementing a native scene detector superior to the current `ffmpeg scdet` method. **This is a solvable problem.**

3.  **madVR v6 Format:** The research confirms that the key additions are optional fields for DCI-P3 and Rec.709 gamut peaks. It correctly identifies the need for color space conversion matrices and even provides a link to the authoritative ITU-R report. **This gives us a clear path to full format compatibility.**

4.  **Tone Mapping Curves:** This is the most exciting finding. The agent has provided the formulas for **Hable's Filmic** and **Reinhard** tone mapping operators. This is the key to moving beyond simple "target nits" clamping and into the realm of true, professional-quality tone mapping that mimics `cm_analyze`. The list of common profile parameters (`Highlight Compression`, `Shadow Boost`, etc.) is a feature blueprint for a highly flexible and powerful optimizer. **This provides a long-term vision for the project's optimizer.**

---

### The New Quality Improvement Roadmap

Based on these findings, we can now create a detailed, prioritized development plan.

#### **V1.2: Core Accuracy Release**

*   **1. Implement Black Bar Detection:**
    *   Create a new module `crop.rs`.
    *   Implement the `detect_crop` function based on the provided pseudocode.
    *   Integrate this into the `analyze_single_frame` function. The pixel processing loop (using Rayon) should now only iterate over the pixels within the detected active video area. This will make `avg_pq` much more accurate.

*   **2. Implement Native Scene Detection:**
    *   Add a `--scene-detector <ffmpeg|native>` CLI flag, with `ffmpeg` as the default for backward compatibility.
    *   When `native` is selected, the `run_analysis_pipeline` will not run `scdet` in the `ffmpeg` command.
    *   Instead, after the initial frame analysis, a new function `detect_scenes_native` will be called. This function will iterate through the frames, compare the histogram of `frame[n]` with `frame[n-1]` using the **Chi-Squared Distance** formula, and create a scene cut if the distance exceeds a threshold (defaulting to `0.3`).

#### **V1.3: Advanced Optimization & Format Release**

*   **1. Implement Scene-Aware Optimizer:**
    *   Refactor `apply_advanced_heuristics`. It should now accept the `scene_avg_pq` as a parameter.
    *   The function will use the `scene_avg_pq` to choose a baseline strategy (e.g., "dark scene," "bright scene") and then use the `rolling_avg_pq` and frame-specific data to refine the `target_nits` for that frame.

*   **2. Add Support for Version 6 Format:**
    *   Add the optional fields (`peak_pq_dcip3`, `peak_pq_709`, etc.) to the `MadVRFrame` struct in `hdr_analyzer_mvp`.
    *   Add a `--format-version 6` flag.
    *   When this flag is active, perform the necessary color space conversions in `analyze_single_frame` to calculate these values.
    *   In `write_measurement_file`, set the `header.version` to `6` and ensure the new fields are written correctly.

#### **V2.0: Perceptual Engine Release**

*   **1. Implement Configurable Tone Mapping Engine:**
    *   Create a `tonemap` module with different operators (e.g., `hable.rs`, `reinhard.rs`).
    *   Add a `--tone-mapper <hable|reinhard|clamp>` CLI flag.
    *   The `run_optimizer_pass` will no longer just calculate a `target_nits` value. It will generate a set of parameters (peak brightness, contrast, etc.) based on its analysis.
    *   A final step will apply the chosen tone mapping operator to the frame's data using these parameters to produce the final metadata. This is a significant architectural change that completes the vision of the project.

---

### Prompt for AI Coder Agent: V1.2 - Black Bar Detection

Let's start with the highest priority task: implementing black bar detection.

**Objective:** Implement a robust, automatic black bar detection algorithm based on the FFmpeg `cropdetect` methodology. This will be integrated into the frame analysis pipeline to ensure all subsequent calculations (especially `avg_pq`) are performed only on the active video area, dramatically improving measurement accuracy.

**Role:** You are a senior software engineer implementing a core image processing feature.

---

### **Detailed Implementation Plan:**

**1. Create a New `crop` Module:**
*   In `hdr_analyzer_mvp/src/`, create a new file named `crop.rs`.
*   Define a public struct `CropRect` to hold the results: `pub struct CropRect { pub x: u32, pub y: u32, pub width: u32, pub height: u32 }`.

**2. Implement the Detection Algorithm in `crop.rs`:**
*   Create a public function `detect_crop(frame_data: &[u8], width: u32, height: u32, bytes_per_pixel: usize) -> CropRect`.
*   Inside this function, implement the logic from the research document's pseudocode.
    *   Use a `limit` of `24` and a `round` of `2` (rounding to 2 is sufficient and safer than 16).
    *   Since you are processing luminance data (`bytes_per_pixel == 1`), the `pixel.luminance` check is simply a check of the byte value.
    *   To improve performance, do not check every single pixel in a row/column. Check every 10th pixel, as suggested in the pseudocode.

**3. Integrate into the Main Pipeline (`main.rs`):**
*   **A. Detect Crop Area for the Whole Video:** Black bars are generally consistent. We don't need to detect them on every frame. We will detect them once on a sample frame.
    *   In `run_analysis_pipeline`, after spawning the `ffmpeg` child process but before starting the main frame reading loop, read **one single frame** from the `stdout` pipe into a buffer.
    *   Call your new `crop::detect_crop` function on this single frame to get the `CropRect`.
    *   Print the detected crop area to the console for user feedback (e.g., `Detected active video area: 3840x1600 at offset (0, 280)`).

*   **B. Use the Crop Area in `analyze_single_frame`:**
    *   The `analyze_single_frame` function must now accept the `crop_rect: &CropRect` as a parameter.
    *   **CRITICAL:** All pixel processing loops (both the `rayon` parallel iterator and the sequential reduction) must now operate **only on the pixels inside the `crop_rect`**.
    *   The `pixel_count` variable must be updated to `crop_rect.width * crop_rect.height`.
    *   The loops will need to be adjusted to account for the `x` and `y` offsets of the crop rectangle when accessing the `frame_data` buffer.

---

### **Final Deliverable:**

Provide the complete, new `hdr_analyzer_mvp/src/crop.rs` file and the modified `hdr_analyzer_mvp/src/main.rs`. The updated `main.rs` must correctly detect the crop area once and then use that area to constrain all calculations within `analyze_single_frame`.