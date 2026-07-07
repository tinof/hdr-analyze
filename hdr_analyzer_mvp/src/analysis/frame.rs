use anyhow::Result;
use ffmpeg_next::frame;
use madvr_parse::MadVRFrame;
use rayon::prelude::*;

use crate::analysis::histogram::{compute_hue_histogram, nits_to_pq};
use crate::analysis::hlg::hlg_signal_to_nits;
use crate::cli::PeakDomain;
use crate::crop::CropRect;
use crate::ffmpeg_io::TransferFunction;
use crate::l1_sidecar::FrameL1Measurement;

pub struct AnalyzedFrame {
    pub frame: MadVRFrame,
    pub l1: FrameL1Measurement,
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

struct FrameAccumulator {
    hist_bins: [f64; 256],
    min_hist_bins: Box<[u64; 1024]>,
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
            min_hist_bins: Box::new([0; 1024]),
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
        for (bin, other_bin) in self
            .min_hist_bins
            .iter_mut()
            .zip(other.min_hist_bins.iter())
        {
            *bin += *other_bin;
        }
        self.max_luma_pq = self.max_luma_pq.max(other.max_luma_pq);
        self.max_rgb_pq = self.max_rgb_pq.max(other.max_rgb_pq);
        self.sum_luma_pq += other.sum_luma_pq;
        self.sum_max_rgb_pq += other.sum_max_rgb_pq;
        self.pixel_count += other.pixel_count;
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
    _width: u32,
    _height: u32,
    crop_rect: &CropRect,
    denoise_mode: &str,
    transfer_function: TransferFunction,
    hlg_peak_nits: f64,
    peak_domain: PeakDomain,
    min_percentile: f64,
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
    let y_plane_data = if denoise_mode == "median3" {
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
                        let luma_pq = match transfer_function {
                            TransferFunction::Hlg => {
                                let nits =
                                    hlg_signal_to_nits(y_signal.clamp(0.0, 1.0), hlg_peak_nits);
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
                        let max_rgb_pq = match transfer_function {
                            TransferFunction::Hlg => luma_pq,
                            _ => rgb_peak,
                        };
                        accumulator.max_rgb_pq = accumulator.max_rgb_pq.max(max_rgb_pq);
                        accumulator.sum_luma_pq += luma_pq;
                        accumulator.sum_max_rgb_pq += max_rgb_pq;
                        accumulator.pixel_count += 1;

                        let min_pq = match peak_domain {
                            PeakDomain::MaxRgb => max_rgb_pq,
                            PeakDomain::Luma => luma_pq,
                        };
                        let min_bin = (min_pq * 1023.0).round() as usize;
                        accumulator.min_hist_bins[min_bin.min(1023)] += 1;

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

    let max_pq = match peak_domain {
        PeakDomain::MaxRgb => accumulator.max_rgb_pq,
        PeakDomain::Luma => accumulator.max_luma_pq,
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
    let min_pq = low_percentile_pq(accumulator.min_hist_bins.as_ref(), min_percentile);

    // Compute hue histogram from chroma planes
    let hue_histogram = compute_hue_histogram(frame, crop_rect);

    Ok(AnalyzedFrame {
        frame: MadVRFrame {
            peak_pq_2020: max_pq,
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
        let mut histogram = [0u64; 1024];
        histogram[2] = 1;
        histogram[51] = 999;

        assert_eq!(low_percentile_pq(&histogram, 0.0), 2.0 / 1023.0);
        assert_eq!(low_percentile_pq(&histogram, 0.1), 51.0 / 1023.0);
        assert_eq!(low_percentile_pq(&[0; 1024], 0.1), 0.0);
    }
}
