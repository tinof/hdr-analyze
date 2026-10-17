use super::PQ_HIST_BINS;

pub(crate) const DIFF_VALUE_BANDS: usize = 16;
pub(crate) const DIFF_BINS: usize = 64;
const MIN_GRAIN_SAMPLES: u64 = 500;
const SIGMA_EXACTNESS_GATE: f64 = 0.25 / 4095.0;
/// Reach of a grain tail in sigmas: the expected maximum of ten million Gaussian samples.
/// The top of such a tail falls off by a factor e over at least `sigma / 5.2` codes.
pub(super) const GRAIN_TAIL_SIGMAS: f64 = 5.2;
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
pub(super) const MAX_CODE_GRID_STEP: usize = 9;

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

fn pq_code(pq: f64) -> usize {
    (pq.clamp(0.0, 1.0) * (PQ_HIST_BINS - 1) as f64).round() as usize
}

pub(super) fn record_cross_quad_diff(
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
pub(super) fn normal_upper_quantile(probability: f64) -> f64 {
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
pub(super) fn rank_level(tail: &[u64], top: usize, rank: u64) -> usize {
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
pub(super) fn code_grid_step(pq_hist: &[u64], top: usize, tail: &[u64]) -> usize {
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
