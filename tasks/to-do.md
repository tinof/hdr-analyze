Of course. This is an excellent time to take stock of the project. You've built an incredibly capable tool, and now it's about refining it from a V1.0 to a truly professional-grade utility.

Here is a clear recap of the planned features and improvements we've discussed but have **not yet implemented** in the current codebase.

I'll categorize them from highest to lowest priority based on our discussions.

---

### Priority 1: Core Measurement Accuracy

These are fundamental improvements that will directly increase the quality and correctness of the measurements, bringing them closer to the ground truth.

**1. Weighted Luminance Calculation**
*   **What it is:** Replacing the current `(R+G+B)/3` luminance calculation with the industry-standard weighted formula: `Y = 0.2126*R + 0.7152*G + 0.0722*B`.
*   **Why it's needed:** This is the **single most important pending change.** The current simple average over-represents the brightness of blue and green, leading to inaccurate `avg_pq` and `MaxCLL` values. The weighted formula correctly models human perception of brightness.
*   **Status:** **Planned, not implemented.** This was the primary action item from our last analysis.

**2. Automated Black Bar Detection**
*   **What it is:** An algorithm that automatically detects and excludes the letterbox (top/bottom) or pillarbox (left/right) bars from all calculations.
*   **Why it's needed:** Black bars contain zero brightness information and artificially drag down the Average Picture Level (APL) of a frame. Excluding them is critical for getting an accurate `avg_pq`, which is the foundation of the intelligent optimizer.
*   **Status:** **Planned, not implemented.**

---

### Priority 2: Scene Detection Overhaul

The ground-truth comparison showed that the current `ffmpeg scdet` filter is the weakest link in the analysis pipeline.

**1. `ffmpeg scdet` Sensitivity Tuning**
*   **What it is:** Changing the `scdet` filter's threshold from the default `30` to a more sensitive value like `15`.
*   **Why it's needed:** The current setting is missing many obvious scene changes that the real `madMeasureHDR.exe` detects. This is a low-effort, high-impact fix.
*   **Status:** **Planned, not implemented.**

**2. Native Histogram-Difference Detector (V2.0 Feature)**
*   **What it is:** Implementing a new, optional scene detection algorithm directly in Rust, as discussed. This method would compare the luminance histograms of consecutive frames to detect a change.
*   **Why it's needed:** This would free you from the limitations of `ffmpeg`'s scene detector and perfectly align with the madVR methodology. It offers the highest possible accuracy.
*   **Status:** **Designed, not implemented.**

---

### Priority 3: Performance and Efficiency

This category addresses the valid points raised by the "performance critic" to make the tool faster and more efficient for all users.

**1. Single `ffmpeg` Process**
*   **What it is:** Refactoring the code to use a single `ffmpeg` process for both frame analysis (from `stdout`) and scene detection (from `stderr`) simultaneously.
*   **Why it's needed:** This would reduce the number of times the video file is read from disk from two to one (plus one `ffprobe` call), significantly speeding up the tool's startup and reducing I/O load.
*   **Status:** **Designed, not implemented.**

**2. Optional GPU Hardware Acceleration**
*   **What it is:** Adding a `--hwaccel` flag to allow users with compatible hardware (NVIDIA, AMD, Intel, Apple) to offload the most expensive part of the process—video decoding—to their GPU.
*   **Why it's needed:** This is a power-user feature that would provide a massive performance boost for those who can use it, without sacrificing portability for everyone else.
*   **Status:** **Designed, not implemented.**

---

### Priority 4: Advanced Optimizer and Format Compatibility

These are features that build upon the now-solid foundation to add more intelligence and support.

**1. Scene-Based Optimizer Logic**
*   **What it is:** Using the `scene_avg_pq` that is already being calculated. The optimizer logic would use the APL of the *entire scene* to set a baseline tone mapping strategy, which is then refined frame-by-frame by the rolling average.
*   **Why it's needed:** This better mimics how a human colorist works, ensuring consistency across an entire scene rather than just reacting to recent frames.
*   **Status:** **Partially implemented.** We calculate the data but do not yet use it in the optimizer's decision tree.

**2. Support for Version 6 File Format**
*   **What it is:** Investigating the differences in the `v6` format (which the real `madMeasureHDR.exe` produces) and adding the ability to write it. This likely involves adding fields for other color gamut peak values (`peak_pq_dcip3`, `peak_pq_709`) to the `MadVRFrame` struct and analysis.
*   **Why it's needed:** For maximum compatibility with the latest tools, outputting the most current file version is ideal.
*   **Status:** **Planned, not implemented.**