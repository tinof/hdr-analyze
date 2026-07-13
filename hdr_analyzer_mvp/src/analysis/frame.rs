use anyhow::Result;
use ffmpeg_next::frame;
use madvr_parse::MadVRFrame;
use rayon::prelude::*;

use crate::analysis::histogram::{compute_hue_histogram, nits_to_pq};
use crate::analysis::hlg::hlg_signal_to_nits;
use crate::cli::{PeakDomain, PeakEstimator};
use crate::crop::CropRect;
use crate::ffmpeg_io::TransferFunction;
use crate::l1_sidecar::FrameL1Measurement;

const PQ_HIST_BINS: usize = 4096;
const DIFF_VALUE_BANDS: usize = 16;
const DIFF_BINS: usize = 64;
const MIN_GRAIN_SAMPLES: u64 = 500;
const ROBUST_FLOOR_PERCENTILE: f64 = 99.99;
const ROBUST_FLOOR_Z: f64 = 3.719_016_485_455_709;
const SIGMA_EXACTNESS_GATE: f64 = 0.25 / 4095.0;

#[derive(Clone, Copy, Debug)]
pub struct FrameAnalysisOptions<'a> {
    pub denoise_mode: &'a str,
    pub transfer_function: TransferFunction,
    pub hlg_peak_nits: f64,
    pub peak_domain: PeakDomain,
    pub min_percentile: f64,
    pub peak_estimator: PeakEstimator,
    pub peak_percentile: f64,
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
}

pub struct AnalyzedFrame {
    pub frame: MadVRFrame,
    pub l1: FrameL1Measurement,
    pub peak_stats: FramePeakStats,
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

fn pq_code(pq: f64) -> usize {
    (pq.clamp(0.0, 1.0) * (PQ_HIST_BINS - 1) as f64).round() as usize
}

fn record_cross_quad_diff(
    diff_hist: &mut [[u32; DIFF_BINS]; DIFF_VALUE_BANDS],
    previous: &mut Option<u16>,
    current_pq: f64,
) {
    let current = pq_code(current_pq) as u16;
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

fn effective_tail_count(pq_hist: &[u64], raw_max_pq: f64, sigma_pq: f64) -> u64 {
    let threshold = pq_code((raw_max_pq - 2.0 * sigma_pq).max(0.0));
    pq_hist[threshold..].iter().sum()
}

fn expected_gaussian_max(sample_count: u64) -> f64 {
    if sample_count <= 1 {
        return 0.0;
    }
    let log_n = (sample_count as f64).ln();
    let leading = (2.0 * log_n).sqrt();
    leading - (log_n.ln() + (4.0 * std::f64::consts::PI).ln()) / (2.0 * leading)
}

fn normal_survival(z: f64) -> f64 {
    // Abramowitz-Stegun 7.1.26, expressed as a stable upper-tail
    // approximation for the modest positive z used by the two-sigma window.
    let z = z.max(0.0);
    let t = 1.0 / (1.0 + 0.231_641_9 * z);
    let polynomial = t
        * (0.319_381_530
            + t * (-0.356_563_782
                + t * (1.781_477_937 + t * (-1.821_255_978 + t * 1.330_274_429))));
    polynomial * (-0.5 * z * z).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

fn inferred_support_count(tail_count: u64, total_count: u64) -> u64 {
    if tail_count <= 1 {
        return tail_count;
    }

    let predicted_tail = |support: u64| {
        let threshold_z = (expected_gaussian_max(support) - 2.0).max(0.0);
        support as f64 * normal_survival(threshold_z)
    };
    let mut low = tail_count.max(2);
    let mut high = total_count.max(low);
    while low < high {
        let mid = low + (high - low) / 2;
        if predicted_tail(mid) < tail_count as f64 {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

/// Correct a raw extreme only when measurable grain exists.
///
/// The constants are intentionally calibration-scoped. The two-sigma tail window
/// provides enough observations to infer the size of the underlying bright
/// support, rather than incorrectly treating the truncated tail itself as that
/// support. The four-sigma cap bounds the correction for unusual distributions.
/// P99.99 remains a content guard, but is shifted down by the Gaussian quantile
/// implied by the measured sigma; an unadjusted percentile would retain about
/// 3.72 sigma of bias on a uniformly grainy plateau. The 500-sample band floor
/// and quarter-code exactness gate prevent correction without measurable grain.
/// Reference CSVs are reserved for the final one-shot score only.
pub(crate) fn robust_peak_pq(pq_hist: &[u64], raw_max_pq: f64, sigma_pq: f64) -> f64 {
    if sigma_pq < SIGMA_EXACTNESS_GATE {
        return raw_max_pq;
    }

    let tail_count = effective_tail_count(pq_hist, raw_max_pq, sigma_pq);
    if tail_count <= 1 {
        return raw_max_pq;
    }

    let total_count = pq_hist.iter().sum();
    let support_count = inferred_support_count(tail_count, total_count);
    let correction = (sigma_pq * expected_gaussian_max(support_count)).min(4.0 * sigma_pq);
    let percentile = high_percentile_pq(pq_hist, ROBUST_FLOOR_PERCENTILE);
    let content_floor = (percentile - ROBUST_FLOOR_Z * sigma_pq).max(0.0);
    (raw_max_pq - correction).clamp(content_floor.min(raw_max_pq), raw_max_pq)
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
                let cb = (f64::from(cb_code) - 512.0) / 896.0;
                let cr = (f64::from(cr_code) - 512.0) / 896.0;

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
                                let nits = hlg_signal_to_nits(
                                    y_signal.clamp(0.0, 1.0),
                                    options.hlg_peak_nits,
                                );
                                nits_to_pq(nits)
                            }
                            _ => y_signal,
                        }
                        .clamp(0.0, 1.0);
                        accumulator.max_luma_pq = accumulator.max_luma_pq.max(luma_pq);

                        let red = y_signal + 1.4746 * cr;
                        let blue = y_signal + 1.8814 * cb;
                        let green = (y_signal - 0.2627 * red - 0.0593 * blue) / 0.6780;
                        let rgb_peak = red.max(green).max(blue).clamp(0.0, 1.0);
                        let max_rgb_pq = match options.transfer_function {
                            TransferFunction::Hlg => luma_pq,
                            _ => rgb_peak,
                        };
                        accumulator.max_rgb_pq = accumulator.max_rgb_pq.max(max_rgb_pq);
                        accumulator.sum_luma_pq += luma_pq;
                        accumulator.sum_max_rgb_pq += max_rgb_pq;
                        accumulator.pixel_count += 1;

                        let peak_pq = match options.peak_domain {
                            PeakDomain::MaxRgb => max_rgb_pq,
                            PeakDomain::Luma => luma_pq,
                        };
                        accumulator.pq_hist[pq_code(peak_pq)] += 1;

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
                            record_cross_quad_diff(&mut accumulator.diff_hist, previous, peak_pq);
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
    let sigma_pq =
        sigma_from_diff_hist(accumulator.diff_hist.as_ref(), accumulator.pq_hist.as_ref());
    let robust_pq = robust_peak_pq(accumulator.pq_hist.as_ref(), raw_max_pq, sigma_pq);
    let n_eff = effective_tail_count(accumulator.pq_hist.as_ref(), raw_max_pq, sigma_pq);
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
            avg_max_rgb_pq,
        },
        peak_stats: FramePeakStats {
            selected_peak_pq,
            raw_max_pq,
            percentile_pq,
            robust_pq,
            correction_pq: raw_max_pq - robust_pq,
            sigma_pq,
            n_eff,
        },
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

    #[test]
    fn robust_peak_corrects_plateau_but_not_single_spike() {
        let sigma = 3.0 / 4095.0;
        let mut plateau = [0u64; PQ_HIST_BINS];
        plateau[3500] = 10_000;
        plateau[3501..3520].fill(100);
        plateau[3520] = 1;
        let raw = 3520.0 / 4095.0;
        let corrected = robust_peak_pq(&plateau, raw, sigma);
        assert!(corrected < raw);
        let noise_adjusted_floor =
            high_percentile_pq(&plateau, ROBUST_FLOOR_PERCENTILE) - ROBUST_FLOOR_Z * sigma;
        assert!(corrected >= noise_adjusted_floor);

        let mut spike = [0u64; PQ_HIST_BINS];
        spike[3500] = 10_000;
        spike[3520] = 1;
        assert_eq!(robust_peak_pq(&spike, raw, sigma), raw);
    }

    #[test]
    fn robust_peak_respects_noise_adjusted_content_floor() {
        let sigma = 20.0 / 4095.0;
        let mut histogram = [0u64; PQ_HIST_BINS];
        histogram[3000] = 1_000_000;
        histogram[3010..=3100].fill(10);
        let raw = 3100.0 / 4095.0;
        let floor =
            high_percentile_pq(&histogram, ROBUST_FLOOR_PERCENTILE) - ROBUST_FLOOR_Z * sigma;
        assert!(robust_peak_pq(&histogram, raw, sigma) >= floor);
    }

    #[test]
    fn support_inference_recovers_population_from_truncated_tail() {
        let support = 57_600;
        let tail = (support as f64
            * normal_survival((expected_gaussian_max(support) - 2.0).max(0.0)))
        .round() as u64;
        let inferred = inferred_support_count(tail, support);
        assert!((inferred as f64 - support as f64).abs() / (support as f64) < 0.01);
    }
}
