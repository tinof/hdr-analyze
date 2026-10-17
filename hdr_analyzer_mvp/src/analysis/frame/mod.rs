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

mod peak;

use self::peak::record_cross_quad_diff;
pub(crate) use self::peak::{
    effective_tail_count, high_percentile_pq, low_percentile_pq, robust_peak_pq,
    sigma_from_diff_hist, DIFF_BINS, DIFF_VALUE_BANDS,
};

const PQ_HIST_BINS: usize = 4096;

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

/// Histogram bin of a per-pixel PQ value, in the f32 arithmetic of the CUDA kernel
/// (`load_sample` in kernels.cu), so both backends put every pixel in the same bin.
fn pixel_pq_bin(pq: f32) -> usize {
    ((pq * (PQ_HIST_BINS - 1) as f32 + 0.5) as i32).clamp(0, PQ_HIST_BINS as i32 - 1) as usize
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
