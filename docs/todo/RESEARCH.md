# Research and Technical Foundations

This document consolidates key research for the `hdr-analyze` project, covering advanced HDR10 analysis techniques and the implementation of native HLG support.

---

## 1. Advanced HDR10 Analysis Techniques

This section summarizes research into state-of-the-art methods for improving the quality and accuracy of HDR10 analysis.

### 1.1. Adaptive Scene Change Detection

Beyond simple histogram differences, modern scene detection relies on multi-metric or learning-based approaches.

-   **Methods**:
    -   **Multi-Metric**: Combining cues like color, luminance, texture, and optical flow can more accurately distinguish between hard cuts and gradual fades.
    -   **Machine Learning**: Deep learning models like **TransNetV2** and **AutoShot** (using 3D CNNs/Transformers) have shown superior performance on standard benchmarks (e.g., ClipShots, BBC datasets). These can be run on downscaled frames for efficiency.
-   **CPU-Friendly Implementation**:
    -   For a CPU-only pipeline, histogram-based methods remain fast and effective. They can be augmented with block-difference algorithms or perceptual hashing.
    -   Optical flow can be approximated using efficient algorithms like OpenCV's Farnebäck.
    -   ML models can be deployed for offline analysis using runtimes like ONNX, which are optimized for CPU inference.

### 1.2. Temporal Tone-Mapping and Metadata Smoothing

To avoid visual artifacts like flicker or "pumping," per-frame metadata (like `target_nits`) must be smoothed over time.

-   **Techniques**:
    -   **Low-Pass Filtering**: An **Exponential Moving Average (EMA)** is a common and effective method for smoothing per-frame luminance metrics.
    -   **Future-Aware Smoothing**: For offline analysis, bidirectional or Finite Impulse Response (FIR) filters can be used. This involves processing a window of past and future frames to make more context-aware decisions, preventing abrupt changes.
    -   **Scene-Aware Resets**: Smoothing filters should be reset at scene boundaries to ensure sharp transitions are respected.

### 1.3. Robustness in PQ Histograms

The Perceptual Quantizer (PQ) transfer function can exaggerate noise, leading to inaccurate peak brightness measurements.

-   **Strategies**:
    -   **Percentile-Based Peaks**: Instead of using the absolute maximum pixel value, calculate the 99th or 99.9th percentile of the luminance distribution. This is a common practice in tools like `hdr10plus_tool` (`histogram99`) and is robust to outliers.
    -   **Histogram Smoothing**: The histogram itself can be smoothed by convolving it with a small Gaussian kernel or by applying a per-bin EMA across frames.
    -   **Pre-Analysis Denoising**: Applying a spatial denoiser (e.g., a median filter or NL-means) to the frame before analysis can stabilize measurements, especially on grainy sources.

---

## 2. Native HLG Support

This section details the research and implementation plan for adding native Hybrid Log-Gamma (HLG) support, eliminating the need for a lossy HLG-to-PQ pre-encode.

### 2.1. HLG to Linear Nits Conversion (Inverse EOTF)

The core of native HLG support is the in-memory conversion of the HLG signal to linear light (nits).

-   **Formula**: The conversion is defined by the BT.2100 standard. A normalized HLG signal `x` (0.0-1.0) is converted to a relative linear light value `L` using a two-part formula:
    -   If `x <= 0.5`, then `L = (x^2) / 3.0`
    -   If `x > 0.5`, then `L = (exp((x - C) / A) + B) / 12.0`
    -   (Constants A, B, C are derived from the standard, e.g., A ≈ 0.1788, B ≈ 0.2847, C ≈ 0.5599).
-   **Absolute Nits**: The relative value `L` is scaled by a peak luminance (e.g., 1000 nits) to get an absolute nit value.
-   **Reference Implementations**: This formula is implemented consistently across open-source projects like the Rust `moxcms` crate and C/C++ libraries like FFmpeg (`libavutil/color_utils.c`).

### 2.2. Linear Nits to PQ Histogram Mapping

Once in the linear domain, the nit values must be binned into the project's existing 256-bin PQ-based histogram.

-   **Formula**: The SMPTE ST-2084 (PQ) standard defines the forward EOTF for converting linear nits `L_c` into a normalized PQ signal `Np` (0.0-1.0):
    -   `Np = ((c1 + c2 * L^n) / (1 + c3 * L^n))^m`
    -   Where `L = L_c / 10000.0` (normalized by PQ's 10,000 nit peak), and `c1, c2, c3, m, n` are constants from the standard.
-   **Workflow**: The full in-memory pipeline per pixel is: `HLG Signal -> Linear Nits -> PQ Signal -> Histogram Bin`.

### 2.3. Validation Strategy

-   **Unit Tests**: Validate the HLG and PQ conversion functions against known value pairs from ITU-R BT.2111-1 and BT.2408 (e.g., HLG signal 0.75 should map to ~203 nits, which maps to a PQ signal of ~0.58).
-   **Test Patterns**: Use official HLG test patterns for end-to-end validation:
    -   **ARIB STD-B72 Color Bars**: Provides patches with precisely defined signal levels.
    -   **Diversified Video Solutions HLG Grayscale Ramp**: Allows for checking the entire HLG curve.

---

## 3. Key Open Source Tools & Datasets

-   **Libraries**:
    -   `dovi_tool`: A key downstream tool used for generating Dolby Vision RPU files from madVR measurements. Serves as a critical validation target.
    -   `madvr_parse`: A Rust library for reading and writing the madVR measurement file format.
    -   `hdr10plus_tool`: A tool for managing HDR10+ metadata, providing a reference for percentile-based peak calculations.
-   **HDR Datasets**:
    -   **LIVE HDR Video Quality Database (UT Austin)**: Provides a collection of high-quality HDR10 clips for testing.
    -   **Netflix Open Content**: Offers several 4K HDR10 demo sequences that serve as realistic test cases.
