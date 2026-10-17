use std::sync::LazyLock;

use anyhow::Result;
use dovi84_composer::Composer;
use ffmpeg_next::frame;
use madvr_parse::MadVRFrame;
use rayon::prelude::*;

use crate::analysis::histogram::{compute_hue_histogram, nits_to_pq, pq_to_nits};
use crate::analysis::hlg::{
    dovi84_decoder, dovi84_pq_lut, downsample_luma, upsample_chroma, ChromaSiting, Dovi84Decoder,
};
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
    /// Where the stream's 4:2:0 chroma samples sit; HLG upsamples the composed chroma there.
    pub chroma_siting: ChromaSiting,
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
/// The spec composer's chroma at chroma resolution (docs/HLG_COMPOSER.md section 9): the MMR
/// of every chroma sample in a rectangle, on the luma down-sampled to that sample.
struct ComposedChroma {
    cx0: usize,
    cy0: usize,
    width: usize,
    values: Vec<[f32; 2]>,
}

impl ComposedChroma {
    /// Chroma samples `cx0..=cx1` by `cy0..=cy1` of a 10-bit 4:2:0 frame. Luma taps outside the
    /// frame repeat its edge; the rectangle never depends on a crop beyond its bounds.
    #[allow(clippy::too_many_arguments)]
    fn new(
        decoder: &Dovi84Decoder,
        luma: impl Fn(usize, usize) -> u16 + Sync,
        chroma: impl Fn(usize, usize) -> (u16, u16) + Sync,
        frame_width: usize,
        frame_height: usize,
        (cx0, cx1): (usize, usize),
        (cy0, cy1): (usize, usize),
    ) -> Self {
        let width = cx1 + 1 - cx0;
        let values = (cy0..=cy1)
            .into_par_iter()
            .flat_map_iter(|cy| {
                let (luma, chroma) = (&luma, &chroma);
                (cx0..=cx1).map(move |cx| {
                    let mmr_luma = downsample_luma(luma, frame_width, frame_height, cx, cy);
                    let (cb, cr) = chroma(cx, cy);
                    decoder.composed_chroma(mmr_luma, &decoder.chroma(cb, cr))
                })
            })
            .collect();
        Self {
            cx0,
            cy0,
            width,
            values,
        }
    }

    fn at(&self, cx: usize, cy: usize) -> [f32; 2] {
        self.values[(cy - self.cy0) * self.width + (cx - self.cx0)]
    }
}

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

    // HLG: the chroma MMR runs once per chroma sample on down-sampled luma (the spec
    // composer's structure), over the crop's chroma samples plus the neighbours the bilinear
    // upsampling of the composed chroma reads, within the frame.
    let frame_width = frame.width() as usize;
    let frame_height = frame.height() as usize;
    let chroma_width = frame_width.div_ceil(2);
    let chroma_height = frame_height.div_ceil(2);
    let read_code =
        |data: &[u8], offset: usize| u16::from_le_bytes([data[offset], data[offset + 1]]) & 0x03FF;
    let composed =
        (options.transfer_function == TransferFunction::Hlg && x_end > x_start && y_end > y_start)
            .then(|| {
                ComposedChroma::new(
                    dovi84,
                    |x, y| read_code(y_plane_data, y * y_stride + x * 2),
                    |cx, cy| {
                        (
                            read_code(u_plane_data, cy * u_stride + cx * 2),
                            read_code(v_plane_data, cy * v_stride + cx * 2),
                        )
                    },
                    frame_width,
                    frame_height,
                    (cx_start, ((x_end - 1) / 2 + 1).min(chroma_width - 1)),
                    (
                        cy_start.saturating_sub(1),
                        ((y_end - 1) / 2 + 1).min(chroma_height - 1),
                    ),
                )
            });

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

                        let max_rgb_pq = match &composed {
                            // Full DV 8.4 decode (luma curve + chroma MMR + RPU matrix) with
                            // the composed chroma upsampled to this pixel, in f32 so the CUDA
                            // kernel reproduces it bit for bit.
                            Some(composed) => {
                                let component = |k: usize| {
                                    upsample_chroma(
                                        |cx, cy| composed.at(cx, cy)[k],
                                        chroma_width,
                                        chroma_height,
                                        options.chroma_siting,
                                        x,
                                        y,
                                    )
                                };
                                f64::from(
                                    dovi84
                                        .max_rgb_pq_composed(y_code, [component(0), component(1)]),
                                )
                            }
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
mod tests;

#[cfg(test)]
mod peak_estimator_tests;
