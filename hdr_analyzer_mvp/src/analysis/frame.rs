use std::sync::LazyLock;

use anyhow::Result;
use dovi84_composer::Composer;
use ffmpeg_next::frame;
use madvr_parse::MadVRFrame;
use rayon::prelude::*;

use crate::analysis::histogram::{compute_hue_histogram, nits_to_pq, pq_to_nits};
use crate::analysis::hlg::{dovi84_decoder, dovi84_pq_lut};
use crate::cli::{PeakDomain, PeakEstimator};
use crate::crop::CropRect;
use crate::ffmpeg_io::TransferFunction;
use crate::l1_sidecar::FrameL1Measurement;

const PQ_HIST_BINS: usize = 4096;
pub(crate) const DIFF_VALUE_BANDS: usize = 16;
pub(crate) const DIFF_BINS: usize = 64;
const MIN_GRAIN_SAMPLES: u64 = 500;
const SIGMA_EXACTNESS_GATE: f64 = 0.25 / 4095.0;
/// Reach of a grain tail in sigmas: the expected maximum of ten million Gaussian samples.
/// The top of such a tail falls off by a factor e over at least `sigma / 5.2` codes.
const GRAIN_TAIL_SIGMAS: f64 = 5.2;
/// A top bin is a flat highlight when grain would fill it this rarely (1 in 100).
const FLAT_TOP_ODDS: f64 = 100.0;
/// Largest detached group of pixels that is looked for (a 32x32 highlight).
const DETACHED_MAX_PIXELS: u64 = 1024;
/// Gap test of the detached group: `ln(1 / 8e-7)`, about one false detection in a thousand
/// frames over the `DETACHED_MAX_PIXELS` ranks tested.
const DETACHED_GAP_FACTOR: f64 = 14.0;
/// Ranks (k-th brightest pixel) through which the Gaussian tail is fitted.
const TAIL_FIT_NEAR_RANK: u64 = 64;
const TAIL_FIT_FAR_RANK: u64 = 1024;
const TAIL_FIT_ITERATIONS: usize = 12;
/// Largest spacing in 12-bit codes between neighbouring values of a 10-bit PQ source: one B'
/// chroma step, `1.8814 * 4095 / 896 = 8.6`. A wider empty stretch is never the code grid.
const MAX_CODE_GRID_STEP: usize = 9;

#[derive(Clone, Copy, Debug)]
pub struct FrameAnalysisOptions<'a> {
    pub denoise_mode: &'a str,
    pub transfer_function: TransferFunction,
    /// Profile 8.4 composer HLG is measured through; ignored for PQ.
    pub hlg_composer: Composer,
    pub peak_domain: PeakDomain,
    pub min_percentile: f64,
    pub peak_estimator: PeakEstimator,
    pub peak_percentile: f64,
    /// Keep every analyzed pixel's max-RGB value (`HDR_ANALYZER_DUMP_MAX_RGB`), so a validation
    /// script can compare the decode per pixel; frame statistics can hide errors that cancel.
    pub dump_max_rgb: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FramePeakStats {
    pub selected_peak_pq: f64,
    pub raw_max_pq: f64,
    pub percentile_pq: f64,
    pub robust_pq: f64,
    pub correction_pq: f64,
    pub sigma_pq: f64,
    pub n_eff: u64,
    /// The frame's unrounded max-RGB average (normalized PQ), as `FrameL1Measurement` holds it
    /// before the sidecar rounds it to a 12-bit code; written by `--dump-frame-stats`.
    pub avg_max_rgb_pq: f64,
}

pub struct AnalyzedFrame {
    pub frame: MadVRFrame,
    pub l1: FrameL1Measurement,
    pub peak_stats: FramePeakStats,
    /// With `FrameAnalysisOptions::dump_max_rgb`: the max-RGB value (normalized PQ) of every
    /// pixel of the crop rect, row by row; NaN where a pixel was not analyzed.
    pub max_rgb_pixels: Option<Vec<f32>>,
}

fn mean_or_zero(sum: f64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        sum / count as f64
    }
}

pub(crate) fn low_percentile_pq(histogram: &[u64], percentile: f64) -> f64 {
    let total: u64 = histogram.iter().sum();
    if total == 0 || histogram.len() < 2 {
        return 0.0;
    }

    let ignored_pixels = ((percentile.clamp(0.0, 100.0) / 100.0) * total as f64).floor() as u64;
    let mut cumulative = 0u64;
    for (bin, count) in histogram.iter().enumerate() {
        cumulative += count;
        if cumulative > ignored_pixels {
            return bin as f64 / (histogram.len() - 1) as f64;
        }
    }

    histogram
        .iter()
        .rposition(|count| *count > 0)
        .map_or(0.0, |bin| bin as f64 / (histogram.len() - 1) as f64)
}

pub(crate) fn high_percentile_pq(histogram: &[u64], percentile: f64) -> f64 {
    let total: u64 = histogram.iter().sum();
    if total == 0 || histogram.len() < 2 {
        return 0.0;
    }

    let ignored_pixels =
        (((100.0 - percentile.clamp(0.0, 100.0)) / 100.0) * total as f64).floor() as u64;
    let mut cumulative = 0u64;
    for (bin, count) in histogram.iter().enumerate().rev() {
        cumulative += count;
        if cumulative > ignored_pixels {
            return bin as f64 / (histogram.len() - 1) as f64;
        }
    }

    histogram
        .iter()
        .position(|count| *count > 0)
        .map_or(0.0, |bin| bin as f64 / (histogram.len() - 1) as f64)
}

/// Nits of each 12-bit PQ histogram bin.
static PQ_BIN_NITS: LazyLock<Vec<f64>> = LazyLock::new(|| {
    (0..PQ_HIST_BINS)
        .map(|bin| pq_to_nits(bin as f64 / (PQ_HIST_BINS - 1) as f64))
        .collect()
});

/// Frame-average light level in nits from the 4096-bin PQ histogram: the mean in linear light,
/// which the PQ-domain mean under-reads. CPU and CUDA both build this histogram from integer
/// counts, so the result is identical on either path.
pub(crate) fn mean_nits_from_pq_hist(histogram: &[u64]) -> f64 {
    let total: u64 = histogram.iter().sum();
    if total == 0 {
        return 0.0;
    }
    let sum: f64 = histogram
        .iter()
        .zip(PQ_BIN_NITS.iter())
        .map(|(&count, &nits)| count as f64 * nits)
        .sum();
    sum / total as f64
}

fn pq_code(pq: f64) -> usize {
    (pq.clamp(0.0, 1.0) * (PQ_HIST_BINS - 1) as f64).round() as usize
}

/// Histogram bin of a per-pixel PQ value, in the f32 arithmetic of the CUDA kernel
/// (`load_sample` in kernels.cu), so both backends put every pixel in the same bin.
fn pixel_pq_bin(pq: f32) -> usize {
    ((pq * (PQ_HIST_BINS - 1) as f32 + 0.5) as i32).clamp(0, PQ_HIST_BINS as i32 - 1) as usize
}

fn record_cross_quad_diff(
    diff_hist: &mut [[u32; DIFF_BINS]; DIFF_VALUE_BANDS],
    previous: &mut Option<u16>,
    current_bin: usize,
) {
    let current = current_bin as u16;
    if let Some(previous) = *previous {
        let value_band = (usize::from(current.max(previous)) >> 8).min(DIFF_VALUE_BANDS - 1);
        let difference = usize::from(current.abs_diff(previous)).min(DIFF_BINS - 1);
        diff_hist[value_band][difference] += 1;
    }
    *previous = Some(current);
}

pub(crate) fn sigma_from_diff_hist(
    diff_hist: &[[u32; DIFF_BINS]; DIFF_VALUE_BANDS],
    pq_hist: &[u64],
) -> f64 {
    let peak_band = (pq_code(high_percentile_pq(pq_hist, 99.5)) >> 8).min(DIFF_VALUE_BANDS - 1);

    for band in (0..=peak_band).rev() {
        // Pair differences are assigned by the brighter endpoint. Merge the
        // adjacent lower band so a plateau close to a 256-code boundary does
        // not condition away half of its symmetric grain distribution.
        let lower_band = band.saturating_sub(1);
        let sample_count: u64 = diff_hist[lower_band..=band]
            .iter()
            .flat_map(|histogram| histogram.iter())
            .map(|count| u64::from(*count))
            .sum();
        if sample_count < MIN_GRAIN_SAMPLES {
            continue;
        }

        let median_rank = sample_count.div_ceil(2);
        let mut cumulative = 0u64;
        for difference in 0..DIFF_BINS {
            cumulative += diff_hist[lower_band..=band]
                .iter()
                .map(|histogram| u64::from(histogram[difference]))
                .sum::<u64>();
            if cumulative >= median_rank {
                let sigma_codes = difference as f64 / (std::f64::consts::SQRT_2 * 0.6745);
                return sigma_codes / (PQ_HIST_BINS - 1) as f64;
            }
        }
    }

    0.0
}

pub(crate) fn effective_tail_count(pq_hist: &[u64], raw_max_pq: f64, sigma_pq: f64) -> u64 {
    let threshold = pq_code((raw_max_pq - 2.0 * sigma_pq).max(0.0));
    pq_hist[threshold..].iter().sum()
}

/// Standard normal deviate `z` whose upper-tail probability is `probability`.
///
/// Acklam's rational approximation of the inverse normal distribution (relative error below
/// 1.2e-9), evaluated in f64 on the host, so it is the same on the CPU and CUDA paths.
fn normal_upper_quantile(probability: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_690e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    const CENTRAL_FROM: f64 = 0.024_25;

    let tail = |p: f64| {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    // Work on the lower-tail probability of -z, then mirror.
    let p = probability.clamp(1e-300, 1.0 - 1e-16);
    let lower_quantile = if p < CENTRAL_FROM {
        tail(p)
    } else if p <= 1.0 - CENTRAL_FROM {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        -tail(1.0 - p)
    };
    -lower_quantile
}

/// The pixels that form the top of the PQ histogram, as a centre and a width in 12-bit codes.
struct TopPopulation {
    centre: f64,
    width: f64,
}

/// Bin of the `rank`-th brightest pixel. `tail[d]` is the number of pixels in bins at or above
/// `top - d`.
fn rank_level(tail: &[u64], top: usize, rank: u64) -> usize {
    top - tail.partition_point(|&count| count < rank).min(top)
}

/// Spacing in 12-bit codes of the values the top of the picture can take: the upper quartile
/// of the distances between neighbouring occupied bins, from the `TAIL_FIT_FAR_RANK`-th
/// brightest pixel up, and at most `MAX_CODE_GRID_STEP`.
///
/// A 10-bit source does not reach every 12-bit code. Neutral pixels fall on a grid of about
/// 4.7 codes, and pixels that differ only in one chroma sample on a grid of up to 8.6. Empty
/// bins inside that grid say nothing about the picture, and one grid value collects the
/// pixels of that many codes.
///
/// The limit matters on a narrow top. With four or fewer spacings the upper quartile is the
/// largest of them, which is the gap below a highlight when there is one; without the limit
/// that gap would count as code grid and the highlight as part of the tail.
fn code_grid_step(pq_hist: &[u64], top: usize, tail: &[u64]) -> usize {
    let from = rank_level(tail, top, TAIL_FIT_FAR_RANK);
    let mut spacings = Vec::new();
    let mut previous = None;
    for (bin, &count) in pq_hist.iter().enumerate().take(top + 1).skip(from) {
        if count == 0 {
            continue;
        }
        if let Some(previous) = previous {
            spacings.push(bin - previous);
        }
        previous = Some(bin);
    }
    if spacings.is_empty() {
        return 1;
    }
    spacings.sort_unstable();
    spacings[(3 * spacings.len() / 4).min(spacings.len() - 1)].min(MAX_CODE_GRID_STEP)
}

/// A group of at most `DETACHED_MAX_PIXELS` brightest pixels that stands apart from the rest
/// of the picture: a small highlight, or outliers of the tail.
///
/// Walking down the occupied bins, `above` pixels lie at or above the current bin and the next
/// occupied bin is `gap` codes lower, of which `gap - grid` are empty beyond the code grid. If
/// the picture below falls off exponentially with scale `lambda` (measured between the next
/// bin and the level `3 * above` pixels further down, at least 64), the spacing below the
/// `above`-th brightest pixel is exponential with mean `lambda / above`. A gap of more than
/// `DETACHED_GAP_FACTOR` such means is not part of that tail.
fn detached_group(pq_hist: &[u64], top: usize, tail: &[u64], grid: usize) -> Option<TopPopulation> {
    let total = tail[top];
    let mut bin = top;
    loop {
        let above = tail[top - bin];
        if above > DETACHED_MAX_PIXELS || above >= total {
            return None;
        }
        let mut next = bin - 1;
        while pq_hist[next] == 0 {
            next -= 1;
        }
        let gap = bin - next;
        let window = (4 * above).max(64);
        if gap >= 2 && gap > grid && window < total {
            let empty = (gap - grid) as u64;
            let lambda = (next - rank_level(tail, top, window + 1)) as f64
                / ((window + 1) as f64 / (above + 1) as f64).ln();
            if (above * empty) as f64 > DETACHED_GAP_FACTOR * lambda {
                let weighted = |f: &dyn Fn(f64) -> f64| -> f64 {
                    (bin..=top)
                        .map(|code| f(code as f64) * pq_hist[code] as f64)
                        .sum::<f64>()
                        / above as f64
                };
                let centre = weighted(&|code| code);
                let variance = weighted(&|code| (code - centre) * (code - centre));
                return Some(TopPopulation {
                    centre,
                    width: variance.sqrt(),
                });
            }
        }
        bin = next;
    }
}

/// Gaussian upper tail through the levels of the `TAIL_FIT_NEAR_RANK`-th and
/// `TAIL_FIT_FAR_RANK`-th brightest pixels.
///
/// A Gaussian population of `n` pixels with centre `c` and width `v` has its k-th brightest
/// pixel at `c + v * z(k / n)`. Two ranks give `c` and `v` for a given `n`; `n` is made
/// consistent with the fit (twice the pixels above the centre) by a damped iteration. The
/// width is then reduced by one standard error, so that neither counting noise nor the code
/// grid turns a plateau that is as narrow as its grain into apparent picture spread.
fn fitted_tail(top: usize, tail: &[u64], grid: usize) -> Option<TopPopulation> {
    let total = tail[top];
    if total < 4 * TAIL_FIT_FAR_RANK {
        return None;
    }
    let near_level = rank_level(tail, top, TAIL_FIT_NEAR_RANK) as f64;
    let far_level = rank_level(tail, top, TAIL_FIT_FAR_RANK) as f64;
    if near_level == far_level {
        return Some(TopPopulation {
            centre: near_level,
            width: 0.0,
        });
    }

    let (near_rank, far_rank) = (TAIL_FIT_NEAR_RANK as f64, TAIL_FIT_FAR_RANK as f64);
    let mut population = total as f64;
    let (mut centre, mut width, mut near_z, mut far_z) = (0.0, 0.0, 1.0, 1.0);
    for _ in 0..TAIL_FIT_ITERATIONS {
        near_z = normal_upper_quantile(near_rank / population);
        far_z = normal_upper_quantile(far_rank / population);
        width = (near_level - far_level) / (near_z - far_z);
        centre = near_level - width * near_z;
        let depth = top as f64 - centre;
        let above_centre = if depth >= top as f64 {
            total
        } else {
            tail[depth.floor().max(0.0) as usize]
        };
        let implied = (2.0 * above_centre as f64).clamp(2.0 * far_rank + 2.0, total as f64);
        if (implied - population).abs() < 0.5 {
            break;
        }
        population = (population * implied).sqrt();
    }
    // Sampling: a count k fixes its level to a relative 1 / (z * sqrt(k)) of the width.
    let sampling_error = width
        * (1.0 / (near_rank * near_z.max(0.5).powi(2)) + 1.0 / (far_rank * far_z.max(0.5).powi(2)))
            .sqrt()
        / (near_z - far_z);
    // Grid: each level is rounded to a grid value (uniform, variance grid^2 / 12).
    let grid_error = grid as f64 / (6.0_f64.sqrt() * (near_z - far_z));
    Some(TopPopulation {
        centre,
        width: (width - sampling_error.hypot(grid_error)).max(0.0),
    })
}

/// Grain-robust frame peak: the raw maximum with the part of the upper tail removed that the
/// measured grain accounts for.
///
/// The pixels at the top of the histogram are described by a centre `c` and a width `v`
/// (`TopPopulation`). The observed width contains the grain: `v^2 = picture^2 + sigma^2`.
/// Taking the grain variance out of the width while every pixel keeps its rank gives
///
/// `peak = c + (raw - c) * sqrt(max(0, 1 - sigma^2 / v^2))`
///
/// - `v <= sigma`: the top is as narrow as grain alone makes it (a grainy flat area), and the
///   peak is its centre.
/// - `v >> sigma`: the spread is picture, and the correction falls to second order in sigma.
/// - a flat highlight (`v = 0` at the raw maximum) is kept exactly.
///
/// The correction is limited to `GRAIN_TAIL_SIGMAS * sigma`: grain does not lift the maximum
/// further above the level it sits on, so a larger distance to the centre is picture.
///
/// The population is found in this order:
/// 1. Flat top: the top bin holds more pixels than a grain tail of this sigma puts into one
///    value of the code grid. Such a tail falls off by a factor e over at least `sigma / 5.2`
///    codes, so a pixel of the tail shares the top value with chance
///    `p <= 1 - exp(-5.2 * grid / sigma)`, and `count` pixels do with chance `p^(count - 1)`,
///    required below 1 in `FLAT_TOP_ODDS`. The raw maximum is returned.
/// 2. `detached_group`: a small group of pixels above a gap that the tail below cannot produce.
/// 3. `fitted_tail`: a Gaussian tail through two rank levels.
///
/// No correction is made when sigma is 0 (it is 0 or at least 1.05 codes, so the quarter-code
/// gate only excludes a zero median). The tail fit is skipped on frames with fewer than
/// `4 * TAIL_FIT_FAR_RANK` pixels; a detached group is still looked for. Inputs are integer
/// histogram counts and the arithmetic is host f64, so CPU and CUDA results are identical.
pub(crate) fn robust_peak_pq(pq_hist: &[u64], raw_max_pq: f64, sigma_pq: f64) -> f64 {
    if sigma_pq < SIGMA_EXACTNESS_GATE {
        return raw_max_pq;
    }
    let Some(top) = pq_hist.iter().rposition(|&count| count > 0) else {
        return raw_max_pq;
    };
    let sigma = sigma_pq * (PQ_HIST_BINS - 1) as f64;

    let mut tail = Vec::with_capacity(top + 1);
    let mut cumulative = 0u64;
    for &count in pq_hist[..=top].iter().rev() {
        cumulative += count;
        tail.push(cumulative);
    }
    let grid = code_grid_step(pq_hist, top, &tail);

    let top_count = pq_hist[top];
    // -ln(p) with p = 1 - exp(-5.2 * grid / sigma). `ln_1p` keeps the value when p rounds to 1.
    let shared_top_log_odds = -(-(-GRAIN_TAIL_SIGMAS * grid as f64 / sigma).exp()).ln_1p();
    if top_count >= 2 && (top_count - 1) as f64 * shared_top_log_odds >= FLAT_TOP_ODDS.ln() {
        return raw_max_pq;
    }

    let Some(population) =
        detached_group(pq_hist, top, &tail, grid).or_else(|| fitted_tail(top, &tail, grid))
    else {
        return raw_max_pq;
    };

    let centre = population.centre.min(top as f64);
    let kept = if population.width <= sigma {
        0.0
    } else {
        (1.0 - (sigma / population.width).powi(2)).sqrt()
    };
    let correction_codes = ((top as f64 - centre) * (1.0 - kept)).min(GRAIN_TAIL_SIGMAS * sigma);
    (raw_max_pq - correction_codes / (PQ_HIST_BINS - 1) as f64).clamp(0.0, raw_max_pq)
}

struct FrameAccumulator {
    hist_bins: [f64; 256],
    pq_hist: Box<[u64; PQ_HIST_BINS]>,
    diff_hist: Box<[[u32; DIFF_BINS]; DIFF_VALUE_BANDS]>,
    max_luma_pq: f64,
    max_rgb_pq: f64,
    sum_luma_pq: f64,
    sum_max_rgb_pq: f64,
    pixel_count: u64,
    /// `(x, y, max-RGB)` of every pixel, only with `FrameAnalysisOptions::dump_max_rgb`.
    pixels: Vec<(u32, u32, f32)>,
}

impl FrameAccumulator {
    fn new() -> Self {
        Self {
            hist_bins: [0.0; 256],
            pq_hist: Box::new([0; PQ_HIST_BINS]),
            diff_hist: Box::new([[0; DIFF_BINS]; DIFF_VALUE_BANDS]),
            max_luma_pq: 0.0,
            max_rgb_pq: 0.0,
            sum_luma_pq: 0.0,
            sum_max_rgb_pq: 0.0,
            pixel_count: 0,
            pixels: Vec::new(),
        }
    }

    fn merge(mut self, other: Self) -> Self {
        for (bin, other_bin) in self.hist_bins.iter_mut().zip(other.hist_bins) {
            *bin += other_bin;
        }
        self.max_luma_pq = self.max_luma_pq.max(other.max_luma_pq);
        self.max_rgb_pq = self.max_rgb_pq.max(other.max_rgb_pq);
        self.sum_luma_pq += other.sum_luma_pq;
        self.sum_max_rgb_pq += other.sum_max_rgb_pq;
        self.pixel_count += other.pixel_count;
        self.pixels.extend(other.pixels);
        for (bin, other_bin) in self.pq_hist.iter_mut().zip(other.pq_hist.iter()) {
            *bin += *other_bin;
        }
        for (band, other_band) in self.diff_hist.iter_mut().zip(other.diff_hist.iter()) {
            for (bin, other_bin) in band.iter_mut().zip(other_band.iter()) {
                *bin += *other_bin;
            }
        }
        self
    }
}

/// Apply 3x3 median filter to Y-plane data (in-place on a cloned buffer).
///
/// This reduces noise in the luminance data before histogram computation,
/// improving stability of APL and peak measurements in grainy content.
///
/// # Arguments
/// * `y_data` - Y-plane data (10-bit, 2 bytes per pixel)
/// * `stride` - Row stride in bytes
/// * `crop_rect` - Active area to denoise
///
/// # Returns
/// Denoised Y-plane data (cloned and filtered)
fn apply_median3_denoise(y_data: &[u8], stride: usize, crop_rect: &CropRect) -> Vec<u8> {
    let mut output = y_data.to_vec();
    let x_start = crop_rect.x as usize;
    let y_start = crop_rect.y as usize;
    let x_end = x_start + crop_rect.width as usize;
    let y_end = y_start + crop_rect.height as usize;

    // Process interior pixels (skip borders to avoid edge handling complexity)
    for y in (y_start + 1)..(y_end.saturating_sub(1)) {
        for x in (x_start + 1)..(x_end.saturating_sub(1)) {
            let mut neighbors = Vec::with_capacity(9);

            // Collect 3x3 neighborhood
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let ny = (y as i32 + dy) as usize;
                    let nx = (x as i32 + dx) as usize;
                    let offset = ny * stride + nx * 2;
                    if offset + 1 < y_data.len() {
                        let code =
                            u16::from_le_bytes([y_data[offset], y_data[offset + 1]]) & 0x03FF;
                        neighbors.push(code);
                    }
                }
            }

            // Compute median
            if !neighbors.is_empty() {
                neighbors.sort_unstable();
                let median = neighbors[neighbors.len() / 2];
                let out_offset = y * stride + x * 2;
                if out_offset + 1 < output.len() {
                    let bytes = median.to_le_bytes();
                    output[out_offset] = bytes[0];
                    output[out_offset + 1] = bytes[1];
                }
            }
        }
    }

    output
}

pub fn analyze_native_frame_cropped(
    frame: &frame::Video,
    crop_rect: &CropRect,
    options: &FrameAnalysisOptions<'_>,
) -> Result<AnalyzedFrame> {
    // Y plane data
    let y_plane_data_raw = frame.data(0);
    let y_stride = frame.stride(0);
    let u_plane_data = frame.data(1);
    let v_plane_data = frame.data(2);
    let u_stride = frame.stride(1);
    let v_stride = frame.stride(2);

    // Apply denoising if requested
    let y_plane_data_denoised;
    let y_plane_data = if options.denoise_mode == "median3" {
        y_plane_data_denoised = apply_median3_denoise(y_plane_data_raw, y_stride, crop_rect);
        &y_plane_data_denoised[..]
    } else {
        y_plane_data_raw
    };

    // madVR v5 binning setup
    let sdr_peak_pq = nits_to_pq(100.0);
    let sdr_step = sdr_peak_pq / 64.0;
    let hdr_step = (1.0 - sdr_peak_pq) / 192.0;

    let x_start = crop_rect.x as usize;
    let y_start = crop_rect.y as usize;
    let x_end = x_start + crop_rect.width as usize;
    let y_end = y_start + crop_rect.height as usize;
    let cx_start = x_start / 2;
    let cy_start = y_start / 2;
    let cx_end = x_end.div_ceil(2);
    let cy_end = y_end.div_ceil(2);
    let dovi84 = dovi84_decoder(options.hlg_composer);
    let dovi84_luma_lut = dovi84_pq_lut(options.hlg_composer);

    // Parallel accumulation across 4:2:0 chroma rows. Rayon creates one
    // accumulator per fold partition, so the fine histogram is reused across
    // many rows instead of being allocated once per row.
    let accumulator = (cy_start..cy_end)
        .into_par_iter()
        .fold(FrameAccumulator::new, |mut accumulator, cy| {
            let mut previous_top = None;
            let mut previous_bottom = None;
            for cx in cx_start..cx_end {
                let u_offset = cy.saturating_mul(u_stride) + cx.saturating_mul(2);
                let v_offset = cy.saturating_mul(v_stride) + cx.saturating_mul(2);
                if u_offset + 1 >= u_plane_data.len() || v_offset + 1 >= v_plane_data.len() {
                    continue;
                }

                let cb_code =
                    u16::from_le_bytes([u_plane_data[u_offset], u_plane_data[u_offset + 1]])
                        & 0x03FF;
                let cr_code =
                    u16::from_le_bytes([v_plane_data[v_offset], v_plane_data[v_offset + 1]])
                        & 0x03FF;
                let cb = (f32::from(cb_code) - 512.0) / 896.0;
                let cr = (f32::from(cr_code) - 512.0) / 896.0;
                let dovi84_chroma = match options.transfer_function {
                    TransferFunction::Hlg => Some(dovi84.chroma(cb_code, cr_code)),
                    _ => None,
                };

                for y in [cy * 2, cy * 2 + 1] {
                    if y < y_start || y >= y_end {
                        continue;
                    }
                    for x in [cx * 2, cx * 2 + 1] {
                        if x < x_start || x >= x_end {
                            continue;
                        }

                        let y_offset = y.saturating_mul(y_stride) + x.saturating_mul(2);
                        if y_offset + 1 >= y_plane_data.len() {
                            continue;
                        }
                        let y_code = u16::from_le_bytes([
                            y_plane_data[y_offset],
                            y_plane_data[y_offset + 1],
                        ]) & 0x03FF;
                        let y_signal = (f64::from(y_code) - 64.0) / 876.0;
                        let luma_pq = match options.transfer_function {
                            TransferFunction::Hlg => {
                                f64::from(dovi84_luma_lut[usize::from(y_code).min(1023)])
                            }
                            _ => y_signal,
                        }
                        .clamp(0.0, 1.0);
                        // The luma peak is the kernel's f32 value (its transfer LUT is f32).
                        accumulator.max_luma_pq =
                            accumulator.max_luma_pq.max(f64::from(luma_pq as f32));

                        let max_rgb_pq = match &dovi84_chroma {
                            // Full DV 8.4 decode (luma curve + chroma MMR + RPU matrix),
                            // in f32 so the CUDA kernel reproduces it bit for bit.
                            Some(chroma) => f64::from(dovi84.max_rgb_pq(y_code, chroma)),
                            // f32 with separate multiplies and adds, operation for operation
                            // as in the CUDA kernel, so both backends agree bit for bit.
                            None => {
                                let y_signal = (f32::from(y_code) - 64.0) / 876.0;
                                let red = y_signal + 1.4746 * cr;
                                let blue = y_signal + 1.8814 * cb;
                                let green = (y_signal - 0.2627 * red - 0.0593 * blue) / 0.6780;
                                f64::from(red.max(green.max(blue)).clamp(0.0, 1.0))
                            }
                        };
                        accumulator.max_rgb_pq = accumulator.max_rgb_pq.max(max_rgb_pq);
                        accumulator.sum_luma_pq += luma_pq;
                        accumulator.sum_max_rgb_pq += max_rgb_pq;
                        accumulator.pixel_count += 1;
                        if options.dump_max_rgb {
                            // Exact: every branch above computes max-RGB in f32.
                            accumulator
                                .pixels
                                .push((x as u32, y as u32, max_rgb_pq as f32));
                        }

                        let peak_pq = match options.peak_domain {
                            PeakDomain::MaxRgb => max_rgb_pq,
                            PeakDomain::Luma => luma_pq,
                        };
                        let peak_bin = pixel_pq_bin(peak_pq as f32);
                        accumulator.pq_hist[peak_bin] += 1;

                        // Sample the first valid pixel in each chroma quad. Adjacent samples are
                        // two luma pixels apart and cross a 4:2:0 chroma boundary, so max-RGB grain
                        // is not underestimated by comparing pixels that share Cb/Cr.
                        let sample_x = (cx * 2).max(x_start);
                        if x == sample_x {
                            let previous = if y == cy * 2 {
                                &mut previous_top
                            } else {
                                &mut previous_bottom
                            };
                            record_cross_quad_diff(&mut accumulator.diff_hist, previous, peak_bin);
                        }

                        // Histogram retains its existing Y-based semantics.
                        let bin = if luma_pq < sdr_peak_pq {
                            (luma_pq / sdr_step).floor() as usize
                        } else {
                            64 + ((luma_pq - sdr_peak_pq) / hdr_step).floor() as usize
                        };
                        accumulator.hist_bins[bin.min(255)] += 1.0;
                    }
                }
            }

            accumulator
        })
        .reduce(FrameAccumulator::new, FrameAccumulator::merge);

    let raw_max_pq = match options.peak_domain {
        PeakDomain::MaxRgb => accumulator.max_rgb_pq,
        PeakDomain::Luma => accumulator.max_luma_pq,
    };
    let percentile_pq = high_percentile_pq(accumulator.pq_hist.as_ref(), options.peak_percentile);
    // As on the CUDA path, the grain statistics stay neutral unless the robust estimator is
    // selected, so the other estimators never run this code.
    let (sigma_pq, robust_pq, n_eff) = if options.peak_estimator == PeakEstimator::Robust {
        let sigma_pq =
            sigma_from_diff_hist(accumulator.diff_hist.as_ref(), accumulator.pq_hist.as_ref());
        (
            sigma_pq,
            robust_peak_pq(accumulator.pq_hist.as_ref(), raw_max_pq, sigma_pq),
            effective_tail_count(accumulator.pq_hist.as_ref(), raw_max_pq, sigma_pq),
        )
    } else {
        (0.0, raw_max_pq, 0)
    };
    let selected_peak_pq = match options.peak_estimator {
        PeakEstimator::Max => raw_max_pq,
        PeakEstimator::Percentile => percentile_pq,
        PeakEstimator::Robust => robust_pq,
    };

    let mut histogram: Vec<f64> = accumulator.hist_bins.to_vec();

    // Normalize histogram to percentages (sum ~ 100.0)
    if accumulator.pixel_count > 0 {
        for v in &mut histogram {
            *v = (*v / accumulator.pixel_count as f64) * 100.0;
        }
    }

    let avg_pq = mean_or_zero(accumulator.sum_luma_pq, accumulator.pixel_count).min(1.0);
    let avg_max_rgb_pq = mean_or_zero(accumulator.sum_max_rgb_pq, accumulator.pixel_count).min(1.0);
    let min_pq = low_percentile_pq(accumulator.pq_hist.as_ref(), options.min_percentile);

    // Compute hue histogram from chroma planes
    let hue_histogram = compute_hue_histogram(frame, crop_rect);

    let max_rgb_pixels = options.dump_max_rgb.then(|| {
        let width = crop_rect.width as usize;
        let mut pixels = vec![f32::NAN; width * crop_rect.height as usize];
        for &(x, y, value) in &accumulator.pixels {
            pixels[(y as usize - y_start) * width + (x as usize - x_start)] = value;
        }
        pixels
    });

    Ok(AnalyzedFrame {
        frame: MadVRFrame {
            peak_pq_2020: selected_peak_pq,
            avg_pq,
            lum_histogram: histogram,
            hue_histogram: Some(hue_histogram),
            target_nits: None,
            ..Default::default()
        },
        l1: FrameL1Measurement {
            min_pq,
            avg_luma_pq: avg_pq,
            avg_max_rgb_pq,
            max_rgb_pq: accumulator.max_rgb_pq,
            fall_nits: mean_nits_from_pq_hist(accumulator.pq_hist.as_ref()),
        },
        peak_stats: FramePeakStats {
            selected_peak_pq,
            raw_max_pq,
            percentile_pq,
            robust_pq,
            correction_pq: raw_max_pq - robust_pq,
            sigma_pq,
            n_eff,
            avg_max_rgb_pq,
        },
        max_rgb_pixels,
    })
}

/// Analyze a native FFmpeg frame to extract HDR metadata with correct 10-bit PQ mapping.
///
/// This function processes native FFmpeg frames with direct access to high-bit-depth data,
/// enabling accurate luminance mapping and PQ conversion. The 10-bit luma values (0-1023)
/// directly correspond to the PQ curve for precise measurement.
///
/// # Arguments
/// * `frame` - Native FFmpeg video frame in YUV420P10LE format
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
///
/// # Returns
/// `Result<MadVRFrame>` - Analyzed frame data with accurate PQ values and histogram
#[allow(dead_code)]
pub fn analyze_native_frame(frame: &frame::Video, width: u32, height: u32) -> Result<MadVRFrame> {
    // Get Y-plane data (luminance) from the 10-bit frame
    let y_plane_data = frame.data(0); // Y plane
    let y_stride = frame.stride(0);

    let (hist_bins, max_luma_10bit, sum_luma_pq, processed_pixels) = (0..height)
        .into_par_iter()
        .map(|y| {
            let mut local_hist = [0f64; 256];
            let mut local_max = 0u16;
            let mut local_sum = 0.0f64;
            let mut local_count = 0u64;

            let row_start = (y as usize).saturating_mul(y_stride);
            let row_bytes = width as usize * 2;
            if row_start < y_plane_data.len() {
                let max_len = y_plane_data.len() - row_start;
                let len = row_bytes.min(max_len) & !1;
                if len >= 2 {
                    let row = &y_plane_data[row_start..row_start + len];
                    for px in row.chunks_exact(2) {
                        // Read 10-bit value (little-endian)
                        let luma_10bit = u16::from_le_bytes([px[0], px[1]]) & 0x3FF;

                        if luma_10bit > local_max {
                            local_max = luma_10bit;
                        }

                        // **CORRECT LUMINANCE MAPPING**: 10-bit luma directly corresponds to PQ curve
                        // Normalize 10-bit value to PQ range (0.0-1.0)
                        let pq_value = luma_10bit as f64 / 1023.0;
                        local_sum += pq_value;
                        local_count += 1;

                        // Map PQ value to histogram bin (0-255)
                        let bin_index = (pq_value * 255.0).round() as usize;
                        local_hist[bin_index.min(255)] += 1.0;
                    }
                }
            }

            (local_hist, local_max, local_sum, local_count)
        })
        .reduce(
            || ([0f64; 256], 0u16, 0.0f64, 0u64),
            |mut acc, local| {
                for (acc_bin, local_bin) in acc.0.iter_mut().zip(local.0.iter()) {
                    *acc_bin += *local_bin;
                }
                if local.1 > acc.1 {
                    acc.1 = local.1;
                }
                acc.2 += local.2;
                acc.3 += local.3;
                acc
            },
        );

    let mut histogram: Vec<f64> = hist_bins.to_vec();

    // Normalize histogram so sum equals 100.0
    if processed_pixels > 0 {
        for bin in &mut histogram {
            *bin = (*bin / processed_pixels as f64) * 100.0;
        }
    }

    // Calculate peak PQ from the brightest 10-bit luma value
    let peak_pq = max_luma_10bit as f64 / 1023.0;

    let avg_pq = mean_or_zero(sum_luma_pq, processed_pixels);

    // Compute hue histogram from chroma planes (full frame, no crop)
    let full_frame_crop = CropRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let hue_histogram = compute_hue_histogram(frame, &full_frame_crop);

    Ok(MadVRFrame {
        peak_pq_2020: peak_pq,
        avg_pq,
        lum_histogram: histogram,
        hue_histogram: Some(hue_histogram),
        target_nits: None, // Will be set by optimizer if enabled
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_precision_mean_uses_sum_and_count() {
        assert_eq!(mean_or_zero(1.5, 3), 0.5);
        assert_eq!(mean_or_zero(123.0, 0), 0.0);
    }

    #[test]
    fn low_percentile_rejects_only_the_configured_dark_tail() {
        let mut histogram = [0u64; PQ_HIST_BINS];
        histogram[2] = 1;
        histogram[51] = 999;

        assert_eq!(low_percentile_pq(&histogram, 0.0), 2.0 / 4095.0);
        assert_eq!(low_percentile_pq(&histogram, 0.1), 51.0 / 4095.0);
        assert_eq!(low_percentile_pq(&[0; PQ_HIST_BINS], 0.1), 0.0);
    }

    #[test]
    fn mean_nits_averages_in_linear_light() {
        assert_eq!(mean_nits_from_pq_hist(&[0; PQ_HIST_BINS]), 0.0);

        // 12-bit PQ 2081 is about 100 nits and 3079 about 1000 nits.
        let mut histogram = [0u64; PQ_HIST_BINS];
        histogram[3079] = 10;
        let peak = pq_to_nits(3079.0 / 4095.0);
        assert!((mean_nits_from_pq_hist(&histogram) - peak).abs() < 1e-9);

        histogram[2081] = 30;
        let expected = (10.0 * peak + 30.0 * pq_to_nits(2081.0 / 4095.0)) / 40.0;
        assert!((mean_nits_from_pq_hist(&histogram) - expected).abs() < 1e-9);
        // The PQ-domain mean would give far less than the linear mean.
        let pq_mean = (10.0 * 3079.0 + 30.0 * 2081.0) / 40.0 / 4095.0;
        assert!(pq_to_nits(pq_mean) < 0.6 * expected);
    }
}

#[cfg(test)]
mod peak_estimator_tests {
    use super::*;

    #[test]
    fn high_percentile_mirrors_low_percentile() {
        let mut histogram = [0u64; 100];
        for bin in &mut histogram[10..=89] {
            *bin = 1;
        }
        assert_eq!(low_percentile_pq(&histogram, 10.0), 18.0 / 99.0);
        assert_eq!(high_percentile_pq(&histogram, 90.0), 81.0 / 99.0);
    }

    #[test]
    fn sigma_from_constructed_gaussian_differences_is_within_five_percent() {
        let sigma_codes = 6.0;
        let mut state = 0x4d595df4d0f33173u64;
        let mut histogram = [[0u32; DIFF_BINS]; DIFF_VALUE_BANDS];
        let mut normal = || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let first = state.wrapping_mul(0x2545f4914f6cdd1d);
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let second = state.wrapping_mul(0x2545f4914f6cdd1d);
            let u1 = ((first >> 11) as f64 + 1.0) / ((1u64 << 53) as f64 + 1.0);
            let u2 = ((second >> 11) as f64 + 1.0) / ((1u64 << 53) as f64 + 1.0);
            (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
        };
        for _ in 0..100_000 {
            let difference = ((normal() - normal()).abs() * sigma_codes).round() as usize;
            histogram[12][difference.min(DIFF_BINS - 1)] += 1;
        }
        let mut pq_hist = [0u64; PQ_HIST_BINS];
        pq_hist[12 << 8] = 100_000;

        let measured_codes = sigma_from_diff_hist(&histogram, &pq_hist) * 4095.0;
        assert!((measured_codes - sigma_codes).abs() / sigma_codes < 0.05);
    }

    /// Upper-tail probability of the standard normal distribution (Abramowitz-Stegun 7.1.26).
    fn normal_survival(z: f64) -> f64 {
        let magnitude = z.abs();
        let t = 1.0 / (1.0 + 0.231_641_9 * magnitude);
        let polynomial = t
            * (0.319_381_530
                + t * (-0.356_563_782
                    + t * (1.781_477_937 + t * (-1.821_255_978 + t * 1.330_274_429))));
        let upper =
            polynomial * (-0.5 * magnitude * magnitude).exp() / (2.0 * std::f64::consts::PI).sqrt();
        if z >= 0.0 {
            upper
        } else {
            1.0 - upper
        }
    }

    /// Expected histogram of `count` pixels at `level` with Gaussian grain of `sigma` codes.
    fn grainy_plateau(level: f64, sigma: f64, count: f64) -> [u64; PQ_HIST_BINS] {
        let mut histogram = [0u64; PQ_HIST_BINS];
        for (bin, slot) in histogram.iter_mut().enumerate() {
            let lower = (bin as f64 - 0.5 - level) / sigma;
            let upper = (bin as f64 + 0.5 - level) / sigma;
            *slot = (count * (normal_survival(lower) - normal_survival(upper))).round() as u64;
        }
        histogram
    }

    fn top_bin(histogram: &[u64]) -> usize {
        histogram.iter().rposition(|&count| count > 0).unwrap()
    }

    fn robust_code(histogram: &[u64], sigma: f64) -> f64 {
        let raw = top_bin(histogram) as f64 / 4095.0;
        robust_peak_pq(histogram, raw, sigma / 4095.0) * 4095.0
    }

    #[test]
    fn normal_upper_quantile_matches_reference_values() {
        for (probability, z) in [
            (0.5, 0.0),
            (0.3, 0.524_400_513),
            (0.025, 1.959_963_985),
            (0.001, 3.090_232_306),
            (1.0e-6, 4.753_424_309),
            (0.7, -0.524_400_513),
            (0.999, -3.090_232_306),
        ] {
            let got = normal_upper_quantile(probability);
            assert!(
                (got - z).abs() < 1e-6,
                "Q^-1({probability}) = {got}, expected {z}"
            );
        }
    }

    #[test]
    fn rank_level_is_the_bin_of_the_kth_brightest_pixel() {
        // Bins 10, 8 and 5 hold 2, 3 and 4 pixels; tail[d] counts bins >= 10 - d.
        let tail = [2u64, 2, 5, 5, 5, 9, 9, 9, 9, 9, 9];
        assert_eq!(rank_level(&tail, 10, 1), 10);
        assert_eq!(rank_level(&tail, 10, 2), 10);
        assert_eq!(rank_level(&tail, 10, 3), 8);
        assert_eq!(rank_level(&tail, 10, 5), 8);
        assert_eq!(rank_level(&tail, 10, 6), 5);
        assert_eq!(rank_level(&tail, 10, 9), 5);
        // More pixels than the histogram holds: the bottom bin.
        assert_eq!(rank_level(&tail, 10, 10), 0);
    }

    #[test]
    fn grainy_plateau_is_corrected_to_its_level() {
        // The expected histogram of a plateau has no counting noise, so the fit is tight. Its
        // width is reduced by one sampling error, which leaves the level slightly low or high.
        for (sigma, count) in [(12.0, 1.0e4), (20.0, 1.0e5), (20.0, 4.0e6), (40.0, 1.0e6)] {
            let histogram = grainy_plateau(2000.0, sigma, count);
            let raw_code = top_bin(&histogram) as f64;
            let corrected = robust_code(&histogram, sigma);
            assert!(raw_code > 2000.0 + 2.0 * sigma);
            assert!(
                (corrected - 2000.0).abs() <= 0.5 * sigma + 1.0,
                "sigma {sigma}, {count} pixels: raw {raw_code}, corrected {corrected}"
            );
        }
    }

    /// The same plateau from a 10-bit source: only every 4095/876-th code occurs.
    fn gridded_plateau(level: f64, sigma: f64, count: f64) -> [u64; PQ_HIST_BINS] {
        let step = 4095.0 / 876.0;
        let mut histogram = [0u64; PQ_HIST_BINS];
        for index in -200..=200 {
            let value = level + f64::from(index) * step;
            let lower = (value - 0.5 * step - level) / sigma;
            let upper = (value + 0.5 * step - level) / sigma;
            histogram[value.round() as usize] =
                (count * (normal_survival(lower) - normal_survival(upper))).round() as u64;
        }
        histogram
    }

    #[test]
    fn code_grid_step_follows_the_spacing_of_occupied_bins() {
        let tail_of = |histogram: &[u64], top: usize| -> Vec<u64> {
            histogram[..=top]
                .iter()
                .rev()
                .scan(0u64, |sum, &count| {
                    *sum += count;
                    Some(*sum)
                })
                .collect()
        };
        let dense = grainy_plateau(2000.0, 20.0, 1.0e6);
        let top = top_bin(&dense);
        assert_eq!(code_grid_step(&dense, top, &tail_of(&dense, top)), 1);

        // Spacings alternate between 4 and 5 codes; the upper quartile is 5.
        let gridded = gridded_plateau(2000.0, 20.0, 1.0e6);
        let top = top_bin(&gridded);
        assert_eq!(code_grid_step(&gridded, top, &tail_of(&gridded, top)), 5);
    }

    #[test]
    fn gridded_plateau_is_corrected_not_kept() {
        // The empty bins of the code grid are no gaps in the picture, and several pixels on
        // the top grid value are no flat highlight: the correction must not be skipped. The
        // two fitted levels are rounded to the grid, which moves the fitted width in steps of
        // about one grid step; where that step leaves the width just above sigma, part of the
        // excess stays (the upper bound is one sigma wider than for a dense plateau).
        for (sigma, count) in [(5.0, 6.0e4), (9.0, 6.0e4), (19.0, 6.0e4), (38.0, 1.0e6)] {
            let histogram = gridded_plateau(2000.0, sigma, count);
            let raw_code = top_bin(&histogram) as f64;
            let error = robust_code(&histogram, sigma) - 2000.0;
            assert!(raw_code > 2000.0 + 3.5 * sigma);
            assert!(
                error >= -(0.5 * sigma + 2.0) && error <= 1.5 * sigma,
                "sigma {sigma}, {count} pixels: raw {raw_code}, error {error}"
            );
        }
    }

    #[test]
    fn picture_spread_wider_than_the_grain_is_mostly_kept() {
        // A top that is 10 times wider than the grain is picture: removing the grain variance
        // takes about z * sigma^2 / (2 * width) off the maximum, here under 3 codes.
        let histogram = grainy_plateau(2000.0, 100.0, 4.0e6);
        let raw_code = top_bin(&histogram) as f64;
        let corrected = robust_code(&histogram, 10.0);
        assert!(corrected < raw_code);
        assert!(
            raw_code - corrected < 5.0,
            "raw {raw_code}, corrected {corrected}"
        );
    }

    #[test]
    fn flat_highlights_of_any_size_are_kept() {
        let sigma = 20.0 / 4095.0;
        let raw = 3534.0 / 4095.0;
        for size in [1u64, 4, 64, 1024, 100_000] {
            let mut histogram = grainy_plateau(2000.0, 20.0, 1.0e6);
            histogram[3534] = size;
            assert_eq!(robust_peak_pq(&histogram, raw, sigma), raw, "{size} pixels");
        }
    }

    #[test]
    fn flat_highlight_just_above_a_grainy_plateau_is_kept() {
        // 64 flat pixels half a sigma above the highest grain pixel. The flat-top rule and the
        // gap test both keep them; the tests below separate the two.
        let mut histogram = grainy_plateau(2000.0, 40.0, 1.0e6);
        let highlight = top_bin(&histogram) + 20;
        histogram[highlight] = 64;
        assert_eq!(robust_code(&histogram, 40.0), highlight as f64);
    }

    #[test]
    fn frames_without_measurable_grain_or_with_few_pixels_are_not_corrected() {
        let histogram = grainy_plateau(2000.0, 20.0, 1.0e6);
        let raw = top_bin(&histogram) as f64 / 4095.0;
        assert_eq!(robust_peak_pq(&histogram, raw, 0.2 / 4095.0), raw);
        assert_eq!(robust_peak_pq(&[0; PQ_HIST_BINS], 0.5, 20.0 / 4095.0), 0.5);

        // Too few pixels for the tail fit and no detached group: raw maximum.
        let small = grainy_plateau(2000.0, 20.0, 2000.0);
        let raw = top_bin(&small) as f64 / 4095.0;
        assert_eq!(robust_peak_pq(&small, raw, 20.0 / 4095.0), raw);
    }

    fn assert_code(actual: f64, expected: f64, case: &str) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "{case}: {actual}, expected {expected}"
        );
    }

    #[test]
    fn code_grid_step_is_limited_on_a_narrow_top() {
        // Two occupied values above the 1024th brightest pixel and a highlight: the largest of
        // the two spacings is the gap below the highlight, which is not code grid.
        let mut histogram = gridded_plateau(2000.0, 3.0, 1.0e6);
        histogram[3500] = 1;
        let tail: Vec<u64> = histogram[..=3500]
            .iter()
            .rev()
            .scan(0u64, |sum, &count| {
                *sum += count;
                Some(*sum)
            })
            .collect();
        assert_eq!(code_grid_step(&histogram, 3500, &tail), MAX_CODE_GRID_STEP);
    }

    #[test]
    fn flat_highlight_above_a_narrow_gridded_top_is_kept() {
        // A low-noise 10-bit plateau has few occupied values at its top. One pixel and a 2x2
        // highlight above it must not be read as part of the code grid. `read` is the sigma the
        // analyzer measures on such a plateau (a multiple of 1.048 codes).
        for (sigma, read) in [(3.0, 4.19), (5.0, 5.24), (8.0, 8.38)] {
            for count in [1.0e5, 1.0e6, 8.0e6] {
                for offset in [100, 500, 1500] {
                    for size in [1u64, 4] {
                        let mut histogram = gridded_plateau(2000.0, sigma, count);
                        histogram[2000 + offset] = size;
                        assert_code(
                            robust_code(&histogram, read),
                            (2000 + offset) as f64,
                            &format!("sigma {sigma}, {count} pixels, +{offset}, {size} px"),
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn flat_top_rule_keeps_a_few_flat_pixels_that_no_gap_separates() {
        // Flat pixels 3 codes above the highest grain pixel at sigma 40: the gap is far too
        // small for the gap test, so only the flat-top rule can keep them. It needs 4 pixels
        // here; two are not enough and are corrected as grain (a documented limit).
        let plateau = grainy_plateau(2000.0, 40.0, 1.0e6);
        let highlight = top_bin(&plateau) + 3;
        for size in [4u64, 8] {
            let mut histogram = plateau;
            histogram[highlight] = size;
            assert_code(
                robust_code(&histogram, 40.0),
                highlight as f64,
                &format!("{size} px"),
            );
        }
        let mut histogram = plateau;
        histogram[highlight] = 2;
        assert!(robust_code(&histogram, 40.0) < highlight as f64 - 40.0);
    }

    #[test]
    fn gap_test_decides_for_one_and_two_pixel_highlights() {
        // Sigma 20, dense plateau. Too few pixels for the flat-top rule, so the gap decides:
        // one pixel 100 codes (5 sigma) and two pixels 60 codes above the highest grain pixel
        // are kept. Two pixels 30 codes above it and four pixels 10 codes above it are within
        // the reach of the grain and are corrected as grain (a documented limit); the
        // correction then stops at 5.2 sigma.
        let sigma = 20.0;
        let plateau = grainy_plateau(2000.0, sigma, 1.0e6);
        let grain_top = top_bin(&plateau);
        for (size, offset) in [(1u64, 100), (2, 60), (2, 100)] {
            let mut histogram = plateau;
            histogram[grain_top + offset] = size;
            assert_code(
                robust_code(&histogram, sigma),
                (grain_top + offset) as f64,
                &format!("{size} px at +{offset}"),
            );
        }
        for (size, offset) in [(2u64, 30), (4, 10)] {
            let mut histogram = plateau;
            histogram[grain_top + offset] = size;
            let lowered_by = (grain_top + offset) as f64 - robust_code(&histogram, sigma);
            assert!(
                lowered_by > 4.0 * sigma && lowered_by <= GRAIN_TAIL_SIGMAS * sigma + 1e-6,
                "{size} px at +{offset}: lowered by {lowered_by}"
            );
        }
    }

    #[test]
    fn correction_is_limited_to_the_reach_of_the_grain() {
        // One pixel 60 codes (3 sigma) above the highest grain pixel is not detached, and the
        // fit puts the centre about 150 codes below it. Grain accounts for 5.2 sigma at most.
        let sigma = 20.0;
        let mut histogram = grainy_plateau(2000.0, sigma, 1.0e6);
        let pixel = top_bin(&histogram) + 60;
        histogram[pixel] = 1;
        assert_code(
            robust_code(&histogram, sigma),
            pixel as f64 - GRAIN_TAIL_SIGMAS * sigma,
            "one pixel within the gap limit",
        );
    }

    #[test]
    fn detached_group_with_spread_reads_its_mean() {
        // 61 pixels, one per code, far above the picture: a detached group no wider than
        // sigma reads its mean, not its maximum. Only a single-valued group is kept exactly.
        let mut histogram = grainy_plateau(2000.0, 20.0, 1.0e6);
        for slot in &mut histogram[3500..=3560] {
            *slot = 1;
        }
        assert_code(robust_code(&histogram, 20.0), 3530.0, "ramp of 61 pixels");
    }

    #[test]
    fn grainy_highlight_is_lowered_near_the_grain_and_read_at_its_centre_above_it() {
        // 16 pixels that carry the grain of the picture, placed at the quantiles of a Gaussian.
        // Centred one sigma above the highest grain pixel they are part of the tail and are
        // lowered by more than a sigma below their centre (a documented limit). Centred three
        // sigma above it they form a detached group and read their centre.
        let sigma = 20.0;
        let plateau = grainy_plateau(2000.0, sigma, 1.0e6);
        let grain_top = top_bin(&plateau) as f64;
        let with_highlight = |centre: f64| {
            let mut histogram = plateau;
            for index in 0..16 {
                let z = -normal_upper_quantile((f64::from(index) + 0.5) / 16.0);
                histogram[(centre + sigma * z).round() as usize] += 1;
            }
            histogram
        };
        let near = grain_top + sigma;
        assert!(robust_code(&with_highlight(near), sigma) < near - sigma);
        let clear = grain_top + 3.0 * sigma;
        assert!((robust_code(&with_highlight(clear), sigma) - clear).abs() <= 1.0);
    }

    #[test]
    fn small_frame_is_corrected_only_through_a_detached_group() {
        // 2000 pixels: too few for the tail fit. Three single pixels above a gap still form a
        // detached group, which reads below its maximum.
        let mut histogram = grainy_plateau(2000.0, 20.0, 2000.0);
        for code in [3000, 3030, 3060] {
            histogram[code] = 1;
        }
        let corrected = robust_code(&histogram, 20.0);
        assert!(corrected < 3060.0 && corrected >= 3030.0, "{corrected}");
    }

    #[test]
    fn correction_never_raises_the_peak_and_keeps_the_sub_code_part_of_the_raw_maximum() {
        let histogram = grainy_plateau(2000.0, 20.0, 1.0e6);
        let top = top_bin(&histogram) as f64;
        let raw = (top + 0.3) / 4095.0;
        let on_bin = robust_peak_pq(&histogram, top / 4095.0, 20.0 / 4095.0);
        let off_bin = robust_peak_pq(&histogram, raw, 20.0 / 4095.0);
        assert!(off_bin < raw);
        assert!(((off_bin - on_bin) * 4095.0 - 0.3).abs() < 1e-9);
    }
}
