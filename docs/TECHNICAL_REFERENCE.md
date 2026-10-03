# Technical reference

Section 1 documents how the analyzer is built. Sections 2 to 4 are background and research notes that
informed the design. Some describe options that were never implemented.

---

## 1. Implementation details (as built)

This section documents how the analyzer is actually implemented, moved here from the README to keep
the front page concise.

### 1.1. 10-bit luminance and the PQ-domain histogram

- On the CPU path, frames are converted/scaled to **YUV420P10LE** for consistent 10-bit luminance
  (Y-plane) access. The CUDA path reads P010 (or YUV420P10LE) frames at full resolution with a
  sampling stride and no scaling, directly in GPU memory for NVDEC frames; it produces the same
  histograms, sums and maxima. See [CUDA_PIPELINE.md](CUDA_PIPELINE.md).
- Histogram binning follows the v5 layout:
    -   SDR portion (bins 0-63) and HDR portion (bins 64-255).
    -   The histogram is retained for distribution, scene detection, and madVR compatibility.
- The active per-frame analysis path normalizes HDR10 limited-range codes (nominal 64-940) to
  `[0,1]` before mapping into the v5 histogram bins (aligns with practical HDR10 limited-range
  content).
- Y-luma and max-RGB PQ sums plus the processed-pixel count are accumulated at full precision in the
  same Rayon reduction as the histogram. The Y average feeds scene statistics and MaxFALL in the `.bin`
  after scene-aware smoothing; the L1 sidecar records both domains unfiltered.
- A separate 1024-bin code-level histogram produces the active-area lower-percentile minimum after
  denoising. P0.1 is the default and P0 is the absolute-minimum diagnostic mode.
- Rayon fold accumulators reuse each fine histogram across a worker partition rather than allocating
  one histogram per chroma row.

### 1.2. Scene detection

- Metric: symmetric chi-squared distance between the 256-bin luminance histograms of two frames
  (0 = identical, about 200 = disjoint). CPU and CUDA runs use the same histograms, so their
  cuts are identical.
- Cuts are chosen after the frame loop, when every histogram is known (`analysis/scene.rs`):
  1. **Score.** For each frame, the smallest distance over all pairs of one frame up to 4 before
     and one up to 4 after it. A cut separates every such pair. A flash of up to 3 frames does
     not: the picture comes back, one pair bridges it, and the score drops.
  2. **Local level.** The median frame-to-frame distance of the 12 frames on each side (the
     frame and its direct neighbours left out). Grain and motion raise this level.
  3. **Candidate.** Score above `--scene-threshold` (default 3.0) and above 16 times the local
     level. A fixed threshold alone does not work: on grainy film the frame-to-frame distance
     exceeds any useful fixed value on about half of all frames.
  4. **Selection.** Candidates are accepted strongest first; an accepted cut blocks candidates
     closer than `--min-scene-length` (default 12). A weak candidate just before a real cut
     therefore cannot suppress it.
- With `--sample-rate N` only analyzed frames take part; a cut is reported at the first analyzed
  frame of the new shot, and the look-around and window sizes count analyzed frames.
- Known limit: when the whole picture changes on every frame (a spinning camera, a lightning
  storm), the histogram distance between shots is not larger than inside them and most cuts are
  missed. That needs a second signal (roadmap item E4).
- At a large frame-to-frame distance the crop monitor takes a sample (at most one per minimum
  scene length). This is reporting-only telemetry and not tied to the final cuts.
- `--scene-metric hybrid` is a prototype that currently falls back to histogram distance; optical
  flow fusion remains roadmap work.

### 1.3. Active-area crop detection

- `detect_crop` (`hdr_analyzer_mvp/src/crop.rs`) scans a 10-bit Y-plane for rows/columns with
  ≥10% non-black samples (sampled every 10 px), rounding to even coordinates/dimensions for chroma
  safety.
- By default, a separate decoder probes seven timestamps across 15%-85% of a seekable input,
  rejects black/low-signal frames, clusters candidates with a two-pixel edge tolerance, and commits
  the modal crop before the main analysis pass.
- Multiple observed aspect-ratio modes use their conservative union so picture is not cut. Accepted
  scene cuts provide reporting-only stability telemetry; per-scene crop application remains a
  follow-up because it can introduce measurement discontinuities.
- `--crop-probes 0` uses hardened in-stream fallback detection, while `--no-crop` analyzes the
  full frame.

### 1.4. Direct pixel access

Unlike wrapper tools that parse text logs from external binaries, HDR-Analyze inspects raw 10-bit
pixel data directly in application memory. This avoids inter-process overhead and enables precise,
per-pixel luminance operations that text-based analysis cannot express.

---

# Background and research notes

The sections below summarize research into HDR10 analysis and native HLG support. They record
ideas and references, not current behavior. Current HLG behavior: `mkvdovi` keeps the HLG stream
and writes Dolby Vision Profile 8.4, and the analyzer measures HLG through the 8.4 decode: the luma
reshaping curve for luma, and the full reconstruction (luma curve, chroma MMR curves, RPU
YCbCr-to-RGB matrix) for max-RGB, clamped to the RPU's declared source range (see
[FORMAT_COMPATIBILITY.md](FORMAT_COMPATIBILITY.md)). Section 3 predates that design; its BT.2100
formulas are kept as background. The implemented mapping follows the 8.4 RPU instead of the BT.2100
OOTF, because a Dolby Vision decoder reconstructs PQ through the RPU curves. With the `preset`
composer the two differ (neutral greys decode with a blue tint); the default `bt2100` composer
writes RPU curves fitted to the BT.2100 OOTF, so the decode follows it
([HLG_COMPOSER.md](HLG_COMPOSER.md)).

## 2. Advanced HDR10 Analysis Techniques

This section summarizes research into state-of-the-art methods for improving the quality and accuracy of HDR10 analysis.

### 2.1. Adaptive Scene Change Detection

Beyond simple histogram differences, modern scene detection relies on multi-metric or learning-based approaches.

-   **Methods**:
    -   **Multi-Metric**: Combining cues like color, luminance, texture, and optical flow can more accurately distinguish between hard cuts and gradual fades.
    -   **Machine Learning**: Deep learning models like **TransNetV2** and **AutoShot** (using 3D CNNs/Transformers) have shown superior performance on standard benchmarks (e.g., ClipShots, BBC datasets). These can be run on downscaled frames for efficiency.
-   **CPU-Friendly Implementation**:
    -   For a CPU-only pipeline, histogram-based methods remain fast and effective. They can be augmented with block-difference algorithms or perceptual hashing.
    -   Optical flow can be approximated using efficient algorithms like OpenCV's Farnebäck.
    -   ML models can be deployed for offline analysis using runtimes like ONNX, which are optimized for CPU inference.

### 2.2. Temporal Tone-Mapping and Metadata Smoothing

To avoid visual artifacts like flicker or "pumping," per-frame metadata (like `target_nits`) must be smoothed over time.

-   **Techniques**:
    -   **Low-Pass Filtering**: An **Exponential Moving Average (EMA)** is a common and effective method for smoothing per-frame luminance metrics.
    -   **Future-Aware Smoothing**: For offline analysis, bidirectional or Finite Impulse Response (FIR) filters can be used. This involves processing a window of past and future frames to make more context-aware decisions, preventing abrupt changes.
    -   **Scene-Aware Resets**: Smoothing filters should be reset at scene boundaries to ensure sharp transitions are respected.

### 2.3. Robustness in PQ Histograms

The Perceptual Quantizer (PQ) transfer function can exaggerate noise, leading to inaccurate peak brightness measurements.

-   **Strategies**:
    -   **Percentile-Based Peaks**: Instead of using the absolute maximum pixel value, calculate the 99th or 99.9th percentile of the luminance distribution. This is a common practice in tools like `hdr10plus_tool` (`histogram99`) and is robust to outliers.
    -   **Histogram Smoothing**: The histogram itself can be smoothed by convolving it with a small Gaussian kernel or by applying a per-bin EMA across frames.
    -   **Pre-Analysis Denoising**: Applying a spatial denoiser (e.g., a median filter or NL-means) to the frame before analysis can stabilize measurements, especially on grainy sources.

### 2.4. Grain-robust frame peak (`--peak-estimator robust`)

The raw maximum of a grainy bright area lies four to five sigma above the area's level. The
robust estimator removes that excess without a spatial filter. It is opt-in, and it is computed on
the host from two statistics that the CPU loop and the CUDA kernel fill with integer counts, so
both backends give the same result. With the other estimators it does not run.

**Inputs per frame**

- The 4096-bin PQ histogram of the peak domain (max-RGB or luma) and the raw maximum.
- The cross-quad difference histogram: for pixels two columns apart on a row (they share no
  chroma sample), the absolute difference of their PQ codes, in 16 value bands by 64 bins.
  Sigma is the median absolute difference divided by `sqrt(2) * 0.6745`, read in the band of the
  frame's P99.5 (merged with the band below; the first band down with at least 500 pairs).
  The median is an integer, so sigma is 0 or a multiple of 1.05 codes. No correction is made
  when it is 0.

**Rule.** The pixels at the top of the histogram are described by a centre `c` and a width `v`.
The observed width contains the grain: `v^2 = picture^2 + sigma^2`. Removing the grain variance
while every pixel keeps its rank gives

`peak = c + (raw - c) * sqrt(max(0, 1 - sigma^2 / v^2))`

A top as narrow as the grain (`v <= sigma`) reads its centre. A top much wider than the grain is
picture, and the correction falls to about `sigma^2 / (2 * local scale)`. The correction is
limited to 5.2 sigma, the expected maximum of ten million Gaussian samples: grain does not lift
the maximum further above the level it sits on.

`c` and `v` come from the first of three cases that applies:

1. **Flat top.** The top bin holds more pixels than a grain tail can put on one value. Such a
   tail falls off by a factor e over at least `sigma / 5.2` codes, so a pixel of the tail shares
   the top value with chance `p <= 1 - exp(-5.2 * grid / sigma)`. With `count` pixels in the top
   bin the chance is `p^(count - 1)`; below 1 in 100 the raw maximum is returned. On a dense
   code grid this takes 4 pixels at sigma 40 and 5 at sigma 20; on a 10-bit grid at sigma 19
   it takes about 17.
2. **Detached group.** Walking down the occupied bins, `j` pixels lie above a gap of `g` empty
   codes (beyond the code grid). If the picture below falls off exponentially with scale
   `lambda`, the spacing below the j-th brightest pixel has mean `lambda / j`. When
   `j * g > 14 * lambda` (`14 = ln(1 / 8e-7)`, about one false detection in a thousand frames over
   the 1024 ranks tested) the group is a separate population; `c` and `v` are the mean and the
   standard deviation of its codes. A group with a single value has `v = 0` at the raw maximum
   and is kept exactly. A group with several values that is no wider than sigma reads its mean:
   61 pixels spread evenly over 61 codes read 30 codes below their maximum.
3. **Tail fit.** A Gaussian through the levels of the 64th and the 1024th brightest pixel, with
   the population size made consistent with the fit. The width is reduced by one standard error
   (counting noise and rounding to the code grid). The fit is skipped on frames with fewer than
   4096 pixels; cases 1 and 2 still apply there.

**Code grid.** A 10-bit source does not reach every 12-bit code: neutral pixels fall on a grid of
about 4.7 codes, and pixels that differ in one chroma sample only on a grid of up to 8.6. The
grid step is measured per frame as the upper quartile of the distances between neighbouring
occupied bins above the 1024th brightest pixel, and it is limited to 9 codes. Cases 1 and 2
count only what exceeds it. Without the grid step, a gridded plateau looks like a stack of
detached groups and is not corrected. Without the limit, a top with four or fewer such
distances takes the gap below a highlight for the grid and lowers the highlight to the picture
level.

**Shot peak.** Unchanged: the maximum of the frame peaks.

**Constants.** All constants were set on the 25 development cuts below; none is validated on
other content, and they must stay fixed for any later validation. Changing one at a time on
seven of the cuts shows which of them the results depend on:

- The gap factor 14 sits between two failures: 7 gives back half of the gain on the grainy
  retail cut (+54.4 → +65.2), and 28 lowers two synthetic highlight frames by 95 codes.
- The far fit rank 1024 trades the clean/grain pair against cuts with embedded L1: from 512 to
  4096 the pair bias goes from +14.5 to +35.9 and the largest error on one retail cut from 236
  to 141.
- The largest detached group, 1024 pixels, equals the largest synthetic highlight tested
  (32x32).
- The 5.2 sigma reach is also the limit of the correction. From 4 to 6.5 the largest error of
  one retail cut goes from 166 to 257 and its bias from -39.7 to -48.7; the pairs do not move.
- Flat-top odds (10 to 10^4), the near fit rank (16 to 256) and the group size (256 to 4096)
  move the biases by less than 5 codes, except on one retail cut (up to 7).

**What is kept and what is not**

- Kept exactly (0 codes lost on 12 synthetic segments, on a clean and a grainy clip): flat,
  noise-free highlights of 2x2 to 32x32 pixels and one- and three-frame flashes about 1500
  codes above the picture.
- A highlight close to the grain is not kept. Below the flat-top count, the gap test decides,
  and it needs a gap of about `14 * lambda / j` codes above the highest grain pixel. At sigma
  20 on a plateau of a million pixels, one pixel 100 codes above the highest grain pixel is
  kept and two pixels 30 codes above it are not. In a simulation with sampled grain of sigma
  20 and 40 (a million pixels, 40 frames per case, dense and 10-bit grid), a single flat
  pixel 3 sigma above the highest grain pixel was lowered in 75 to 92% of the frames and 5
  sigma above it in 15 to 32%; a 2x2 was lowered in 0 to 8% at 3 sigma. At sigma 3 to 5 on a
  10-bit grid those distances are within two grid steps, and both were lowered in nearly
  every frame. A lowered highlight reads up to 5.2 sigma low.
- A highlight that carries the grain of the picture is lowered when it lies within the reach
  of the grain below it: 16 pixels centred 1 sigma above the highest grain pixel in 90 to 98%
  of the simulated frames, 3 sigma above it in 8 to 35%. With 256 pixels it was not lowered.
- On a narrow top (four or fewer occupied values above the 1024th brightest pixel) a gap of up
  to 9 codes cannot be told from the code grid, and a group above such a gap is corrected with
  the tail. On 511 frames of the 18 cuts examined the gap below the top bin equals the measured
  grid step (3 to 9 codes) and the frame is corrected, by up to 142 codes.
- A top that sits on a clamp (code 4095, or the HLG decode's upper limit) is treated like any
  other: with too few pixels on the clamp value for the flat-top rule the frame is corrected.
  On one grainy clip that is 90 of the 141 frames whose top is at code 4095 (28 codes on
  average, 56 at most). No test frame reaches the HLG limit.

**Limits (measured on 25 real-content cuts; per-frame figures on the 18 of them with saved
histograms, 23,519 frames)**

- Clean content moves. Sigma reads 5 to 25 codes on clean digital cuts (sensor noise and fine
  texture; on content that sits on the 10-bit luma grid the smallest non-zero sigma is 4.2
  codes, one grid step), and the
  rule removes what that sigma implies: 3 to 21 codes per frame on average, 36 on an HLG cut.
  The 95th percentile of the correction on a clean cut is up to 102 codes and the largest 332.
- The largest correction on any frame is 343 codes, which is the 5.2 sigma limit at the
  largest sigma the difference histogram can report (66 codes). The limit bound on 538 frames.
  Sigma is at that ceiling on 48% of the frames of one grainy clip and on up to 11% of others.
- No content floor is applied. On 4,259 frames the result lies below the frame's 99.99th
  percentile (by up to 177 codes), so more than 0.01% of the pixels are brighter than the
  reported peak. For a large grainy area that is the intended reading; on clean content it is
  not. A floor at that percentile was measured and not adopted: it lowers the frame-to-frame
  variation (to below `max` on the HLG cut), but a grainy plateau that fills the frame then
  reads 3.7 sigma high, which is outside the tolerance of the synthetic grain tests.
- When the fitted width is at or below sigma the whole distance to the centre is removed (up
  to the 5.2 sigma limit). That is the case on 0 to 52% of the fitted frames per cut, and it
  includes clean cuts where sigma comes from texture.
- The square root is steep near `v = sigma`: a width read 10% high keeps about 40% of the
  excess. On simulated gridded plateaus of 57,600 pixels at sigma 19, about 1 frame in 11
  reads more than 0.6 sigma high (up to 2 sigma). The synthetic grain tests use fixed seeds
  and inherit this. A gridded plateau with a true sigma of 3 codes keeps 10 of its 15 codes
  of excess.
- Clean/grain pairs, per frame against the clean twin's raw maximum: bias +102.7 → +23.4,
  +41.0 → +17.0 and +83.3 → +50.5; mean absolute error 108.4 → 60.3, 41.0 → 18.6 and
  85.3 → 70.7. On the first and third pair 27% of the frames read more than 10 codes below
  the clean twin (the largest error of the first pair, 249 codes, is an under-read).
- Frame-to-frame variation inside shots is higher than with the raw maximum on 24 of 25 cuts,
  10% on average and 38% at worst. The distance of the shot peak from the 90th percentile of
  its frame peaks grows on 15 cuts and shrinks on 7.
- One grainy retail cut stays at +54 against its embedded L1 (from +76). On 10 of its 15
  shots the frame that sets the shot peak went through the tail fit, which read the top as
  picture and corrected it by 0 to 23 codes; on 5 it was a flat top or a detached group.
- With a sampling stride (`--downscale` 2 or 4) the pixel counts of the rule refer to
  analysis samples, sigma over-reads and a 2x2 highlight becomes one sample; the analyzer
  prints a warning. Use the estimator at `--downscale 1`.

---

## 3. Native HLG Support

This section details the research and implementation plan for adding native Hybrid Log-Gamma (HLG) support, eliminating the need for a lossy HLG-to-PQ pre-encode.

### 3.1. HLG to Linear Nits Conversion (Inverse EOTF)

The core of native HLG support is the in-memory conversion of the HLG signal to linear light (nits).

-   **Formula**: The conversion is defined by the BT.2100 standard. A normalized HLG signal `x` (0.0-1.0) is converted to a relative linear light value `L` using a two-part formula:
    -   If `x <= 0.5`, then `L = (x^2) / 3.0`
    -   If `x > 0.5`, then `L = (exp((x - C) / A) + B) / 12.0`
    -   (Constants A, B, C are derived from the standard, e.g., A ≈ 0.1788, B ≈ 0.2847, C ≈ 0.5599).
-   **Absolute Nits**: The relative value `L` is scaled by a peak luminance (e.g., 1000 nits) to get an absolute nit value.
-   **Reference Implementations**: This formula is implemented consistently across open-source projects like the Rust `moxcms` crate and C/C++ libraries like FFmpeg (`libavutil/color_utils.c`).

### 3.2. Linear Nits to PQ Histogram Mapping

Once in the linear domain, the nit values must be binned into the project's existing 256-bin PQ-based histogram.

-   **Formula**: The SMPTE ST-2084 (PQ) standard defines the forward EOTF for converting linear nits `L_c` into a normalized PQ signal `Np` (0.0-1.0):
    -   `Np = ((c1 + c2 * L^n) / (1 + c3 * L^n))^m`
    -   Where `L = L_c / 10000.0` (normalized by PQ's 10,000 nit peak), and `c1, c2, c3, m, n` are constants from the standard.
-   **Workflow**: The full in-memory pipeline per pixel is: `HLG Signal -> Linear Nits -> PQ Signal -> Histogram Bin`.

### 3.3. Validation Strategy

-   **Unit Tests**: Validate the HLG and PQ conversion functions against known value pairs from ITU-R BT.2111-1 and BT.2408 (e.g., HLG signal 0.75 should map to ~203 nits, which maps to a PQ signal of ~0.58).
-   **Test Patterns**: Use official HLG test patterns for end-to-end validation:
    -   **ARIB STD-B72 Color Bars**: Provides patches with precisely defined signal levels.
    -   **Diversified Video Solutions HLG Grayscale Ramp**: Allows for checking the entire HLG curve.

---

## 4. Key Open Source Tools & Datasets

-   **Libraries**:
    -   `dovi_tool`: A key downstream tool used for generating Dolby Vision RPU files from madVR measurements. Serves as a critical validation target.
    -   `madvr_parse`: A Rust library for reading and writing the madVR measurement file format.
    -   `hdr10plus_tool`: A tool for managing HDR10+ metadata, providing a reference for percentile-based peak calculations.
-   **HDR Datasets**:
    -   **LIVE HDR Video Quality Database (UT Austin)**: Provides a collection of high-quality HDR10 clips for testing.
    -   **Netflix Open Content**: Offers several 4K HDR10 demo sequences that serve as realistic test cases.
