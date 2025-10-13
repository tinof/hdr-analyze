use anyhow::Result;
use ffmpeg_next::frame;
use madvr_parse::MadVRFrame;
use rayon::prelude::*;

use crate::analysis::histogram::{compute_hue_histogram, nits_to_pq};
use crate::analysis::hlg::hlg_signal_to_nits;
use crate::crop::CropRect;
use crate::ffmpeg_io::TransferFunction;

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
) -> Result<MadVRFrame> {
    // Y plane data
    let y_plane_data_raw = frame.data(0);
    let y_stride = frame.stride(0);

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

    // Parallel accumulation across rows using per-thread histograms + reduction
    let (hist_bins, max_pq) = (y_start..y_end)
        .into_par_iter()
        .map(|y| {
            let mut local_hist = [0f64; 256];
            let mut local_max = 0.0f64;

            let row_start = y.saturating_mul(y_stride);
            let base = row_start + x_start.saturating_mul(2);
            if base < y_plane_data.len() {
                let want_len = (x_end - x_start).saturating_mul(2);
                let max_len = y_plane_data.len() - base;
                let len = want_len.min(max_len) & !1; // even number of bytes
                if len >= 2 {
                    let row = &y_plane_data[base..base + len];
                    for px in row.chunks_exact(2) {
                        // Read 10-bit limited-range code (0..1023 in 16-bit container)
                        let code10 = u16::from_le_bytes([px[0], px[1]]) & 0x03FF;

                        // Normalize to limited-range [64,940] -> [0,1]
                        let code_i = code10 as i32;
                        let norm = ((code_i - 64) as f64 / 876.0).clamp(0.0, 1.0);

                        let pq = match transfer_function {
                            TransferFunction::Hlg => {
                                let nits = hlg_signal_to_nits(norm, hlg_peak_nits);
                                nits_to_pq(nits)
                            }
                            _ => norm, // PQ/Unknown fall back to normalized PQ proxy
                        }
                        .clamp(0.0, 1.0);
                        if pq > local_max {
                            local_max = pq;
                        }

                        // Map to madVR v5 bins
                        let bin = if pq < sdr_peak_pq {
                            (pq / sdr_step).floor() as usize
                        } else {
                            64 + ((pq - sdr_peak_pq) / hdr_step).floor() as usize
                        };
                        local_hist[bin.min(255)] += 1.0;
                    }
                }
            }

            (local_hist, local_max)
        })
        .reduce(
            || ([0f64; 256], 0.0f64),
            |mut acc, (local_hist, local_max)| {
                for (acc_bin, local_bin) in acc.0.iter_mut().zip(local_hist.iter()) {
                    *acc_bin += *local_bin;
                }
                if local_max > acc.1 {
                    acc.1 = local_max;
                }
                acc
            },
        );

    let mut histogram: Vec<f64> = hist_bins.to_vec();

    // Normalize histogram to percentages (sum ~ 100.0)
    let total_pixels = (crop_rect.width as f64) * (crop_rect.height as f64);
    if total_pixels > 0.0 {
        for v in &mut histogram {
            *v = (*v / total_pixels) * 100.0;
        }
    }

    // Compute avg_pq using mid-bin method similar to madvr_parse
    let sdr_mid = sdr_step + (sdr_step / 2.0);
    let hdr_mid = hdr_step + (hdr_step / 2.0);

    let mut avg_pq = 0.0f64;
    for (i, percent) in histogram.iter().enumerate() {
        // Filter potential black bars at bin 0 per madvr_parse heuristic
        if i == 0 && *percent > 2.0 && *percent < 30.0 {
            continue;
        }
        let pq_value = if i <= 64 {
            (i as f64) * sdr_mid
        } else {
            sdr_peak_pq + (((i - 63) as f64) * hdr_mid)
        };
        avg_pq += pq_value * (*percent / 100.0);
    }
    // Adjust based on sum of histogram bars
    let percent_sum: f64 = histogram.iter().sum();
    if percent_sum > 0.0 {
        avg_pq = (avg_pq * (100.0 / percent_sum)).min(1.0);
    }
    avg_pq = avg_pq.min(1.0);

    // Compute hue histogram from chroma planes
    let hue_histogram = compute_hue_histogram(frame, crop_rect);

    Ok(MadVRFrame {
        peak_pq_2020: max_pq,
        avg_pq,
        lum_histogram: histogram,
        hue_histogram: Some(hue_histogram),
        target_nits: None,
        ..Default::default()
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
    let pixel_count = (width * height) as usize;

    // Get Y-plane data (luminance) from the 10-bit frame
    let y_plane_data = frame.data(0); // Y plane
    let y_stride = frame.stride(0);

    let (hist_bins, max_luma_10bit) = (0..height)
        .into_par_iter()
        .map(|y| {
            let mut local_hist = [0f64; 256];
            let mut local_max = 0u16;

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

                        // Map PQ value to histogram bin (0-255)
                        let bin_index = (pq_value * 255.0).round() as usize;
                        local_hist[bin_index.min(255)] += 1.0;
                    }
                }
            }

            (local_hist, local_max)
        })
        .reduce(
            || ([0f64; 256], 0u16),
            |mut acc, (local_hist, local_max)| {
                for (acc_bin, local_bin) in acc.0.iter_mut().zip(local_hist.iter()) {
                    *acc_bin += *local_bin;
                }
                if local_max > acc.1 {
                    acc.1 = local_max;
                }
                acc
            },
        );

    let mut histogram: Vec<f64> = hist_bins.to_vec();

    // Normalize histogram so sum equals 100.0
    let total_pixels = pixel_count as f64;
    if total_pixels > 0.0 {
        for bin in &mut histogram {
            *bin = (*bin / total_pixels) * 100.0;
        }
    }

    // Calculate peak PQ from the brightest 10-bit luma value
    let peak_pq = max_luma_10bit as f64 / 1023.0;

    // Calculate average PQ from the histogram
    let avg_pq = crate::analysis::histogram::calculate_avg_pq_from_histogram(&histogram);

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
