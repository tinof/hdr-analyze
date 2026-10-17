//! Optional CUDA analysis backend (feature `cuda`).
//!
//! Mirrors `analysis::frame::analyze_native_frame_cropped` on the GPU: one kernel
//! launch per analyzed frame produces the v5 luminance histogram, hue histogram,
//! a 4096-bin PQ histogram in the selected peak domain, true per-pixel luma /
//! max-RGB means, and domain peaks. Only a few KB of results are downloaded per
//! frame. NVDEC frames in the shared primary context are analyzed in place
//! (`analyze_device`); other frames are uploaded from host memory (`analyze`).
//! For the grain-robust (`robust`) peak estimator the kernel also fills the
//! cross-quad difference histogram, and the host applies the same estimator
//! functions as the CPU path.

use anyhow::{anyhow, Result};
use dovi84_composer::Composer;
#[cfg(feature = "cuda")]
use ffmpeg_next::format;
use ffmpeg_next::frame;

#[cfg(feature = "cuda")]
use crate::analysis::frame::{
    effective_tail_count, high_percentile_pq, low_percentile_pq, mean_nits_from_pq_hist,
    robust_peak_pq, sigma_from_diff_hist,
};
use crate::analysis::frame::{AnalyzedFrame, FrameAnalysisOptions};
#[cfg(any(feature = "cuda", test))]
use crate::analysis::frame::{DIFF_BINS, DIFF_VALUE_BANDS};
#[cfg(any(feature = "cuda", test))]
use crate::analysis::histogram::nits_to_pq;
#[cfg(any(feature = "cuda", test))]
use crate::analysis::hlg::{
    dovi84_decoder, dovi84_pq_lut, Dovi84Decoder, MMR_MAX_ORDER, MMR_TERMS,
};
use crate::crop::CropRect;
use crate::ffmpeg_io::TransferFunction;

#[cfg(feature = "cuda")]
const LUMINANCE_BINS: usize = 256;
#[cfg(feature = "cuda")]
const HUE_BINS: usize = 31;
#[cfg(feature = "cuda")]
const PQ_BINS: usize = 4096;
#[cfg(feature = "cuda")]
const PQ_HIST_BASE: usize = LUMINANCE_BINS + HUE_BINS;
#[cfg(feature = "cuda")]
const MAX_LUMA_WORD: usize = PQ_HIST_BASE + PQ_BINS;
#[cfg(feature = "cuda")]
const MAX_RGB_WORD: usize = MAX_LUMA_WORD + 1;
// Result buffer layout; must match kernels.cu. The u64 sums start at u32 word SUMS_WORD,
// one padding word after the counts so they are 8-byte aligned.
#[cfg(any(feature = "cuda", test))]
const SUMS_WORD: usize = 4386;
#[cfg(any(feature = "cuda", test))]
const SUM_WORDS: usize = 3;
/// Cross-quad difference histogram of the grain-robust estimator, after the sums.
#[cfg(any(feature = "cuda", test))]
const DIFF_WORD: usize = SUMS_WORD + 2 * SUM_WORDS;
#[cfg(any(feature = "cuda", test))]
const DIFF_WORDS: usize = DIFF_VALUE_BANDS * DIFF_BINS;
#[cfg(any(feature = "cuda", test))]
const RESULT_WORDS: usize = DIFF_WORD + DIFF_WORDS;
#[cfg(feature = "cuda")]
const _: () = assert!(SUMS_WORD == MAX_RGB_WORD + 2 && SUMS_WORD % 2 == 0);
// kernels.cu hard-codes DIFF_WORD = SUMS_WORD + 6, 16 bands and 64 bins.
#[cfg(any(feature = "cuda", test))]
const _: () = assert!(DIFF_WORD == SUMS_WORD + 6 && DIFF_VALUE_BANDS == 16 && DIFF_BINS == 64);
#[cfg(feature = "cuda")]
const FIXED_POINT_SCALE: f64 = 4_294_967_296.0;
/// Threads per block, a multiple of the warp size (the kernel's warp reductions rely on it).
#[cfg(feature = "cuda")]
const BLOCK_THREADS: u32 = 256;
/// Grid-stride blocks per SM: enough resident warps to hide latency while the per-block
/// shared-histogram clear and flush stay a small fraction of the work.
#[cfg(feature = "cuda")]
const BLOCKS_PER_SM: u32 = 8;

// `dovi_params` layout; must match the DOVI_* defines in kernels.cu.
#[cfg(any(feature = "cuda", test))]
const DOVI_MMR_WORDS: usize = 1 + MMR_MAX_ORDER * MMR_TERMS;
#[cfg(any(feature = "cuda", test))]
const DOVI_MMR_CB: usize = 1024;
#[cfg(any(feature = "cuda", test))]
const DOVI_MMR_CR: usize = DOVI_MMR_CB + DOVI_MMR_WORDS;
#[cfg(any(feature = "cuda", test))]
const DOVI_YCC: usize = DOVI_MMR_CR + DOVI_MMR_WORDS;
#[cfg(any(feature = "cuda", test))]
const DOVI_CHROMA_OFFSET: usize = DOVI_YCC + 6;
#[cfg(any(feature = "cuda", test))]
const DOVI_CHROMA_CLAMP: usize = DOVI_CHROMA_OFFSET + 2;
#[cfg(any(feature = "cuda", test))]
const DOVI_SOURCE_RANGE: usize = DOVI_CHROMA_CLAMP + 2;
#[cfg(any(feature = "cuda", test))]
const DOVI_PARAM_WORDS: usize = DOVI_SOURCE_RANGE + 2;

/// The u64 sums the kernel writes after the counts, read back from the u32 result words.
#[cfg(any(feature = "cuda", test))]
fn result_sums(results: &[u32]) -> [u64; SUM_WORDS] {
    std::array::from_fn(|index| {
        let low = results[SUMS_WORD + 2 * index];
        let high = results[SUMS_WORD + 2 * index + 1];
        if cfg!(target_endian = "little") {
            u64::from(low) | (u64::from(high) << 32)
        } else {
            (u64::from(low) << 32) | u64::from(high)
        }
    })
}

#[cfg(any(feature = "cuda", test))]
/// The kernel's cross-quad difference histogram, in the CPU estimator's layout.
#[cfg(any(feature = "cuda", test))]
fn result_diff_hist(results: &[u32]) -> [[u32; DIFF_BINS]; DIFF_VALUE_BANDS] {
    let mut diff_hist = [[0u32; DIFF_BINS]; DIFF_VALUE_BANDS];
    for (band, counts) in diff_hist.iter_mut().enumerate() {
        let start = DIFF_WORD + band * DIFF_BINS;
        counts.copy_from_slice(&results[start..start + DIFF_BINS]);
    }
    diff_hist
}

#[cfg(any(feature = "cuda", test))]
fn pq_for_code(code: i32, transfer_function: TransferFunction, composer: Composer) -> f64 {
    match transfer_function {
        TransferFunction::Hlg => f64::from(dovi84_pq_lut(composer)[code.clamp(0, 1023) as usize]),
        _ => ((code - 64) as f64) / 876.0,
    }
    .clamp(0.0, 1.0)
}

#[cfg(any(feature = "cuda", test))]
fn build_transfer_lut(transfer_function: TransferFunction, composer: Composer) -> Vec<f32> {
    (0..1024)
        .map(|code| pq_for_code(code, transfer_function, composer) as f32)
        .collect()
}

/// Flatten a shared Profile 8.4 decoder ([`dovi84_decoder`], the struct the CPU path decodes
/// with) into the kernel's `dovi_params` buffer.
#[cfg(any(feature = "cuda", test))]
fn build_dovi84_params(decoder: &Dovi84Decoder) -> Vec<f32> {
    let mut params = Vec::with_capacity(DOVI_PARAM_WORDS);
    params.extend_from_slice(&decoder.luma_term);
    for curve in &decoder.mmr {
        params.push(curve.constant);
        for order in &curve.coef {
            params.extend_from_slice(order);
        }
    }
    params.extend_from_slice(&decoder.ycc_chroma);
    params.extend_from_slice(&decoder.chroma_offset);
    params.extend_from_slice(&decoder.chroma_clamp);
    params.extend_from_slice(&decoder.source_range);
    debug_assert_eq!(params.len(), DOVI_PARAM_WORDS);
    params
}

#[cfg(any(feature = "cuda", test))]
fn build_luminance_bin_lut(transfer_function: TransferFunction, composer: Composer) -> Vec<u16> {
    // Must match the v5 binning constants in analyze_native_frame_cropped exactly.
    let sdr_peak_pq = nits_to_pq(100.0);
    let sdr_step = sdr_peak_pq / 64.0;
    let hdr_step = (1.0 - sdr_peak_pq) / 192.0;
    (0..1024)
        .map(|code| {
            let pq = pq_for_code(code, transfer_function, composer);
            let bin = if pq < sdr_peak_pq {
                (pq / sdr_step).floor() as usize
            } else {
                64 + ((pq - sdr_peak_pq) / hdr_step).floor() as usize
            };
            bin.min(255) as u16
        })
        .collect()
}

#[cfg(feature = "cuda")]
mod backend;

#[cfg(feature = "cuda")]
pub use backend::GpuAnalyzer;

#[cfg(not(feature = "cuda"))]
pub struct GpuAnalyzer;

#[cfg(not(feature = "cuda"))]
impl GpuAnalyzer {
    pub fn new(_transfer_function: TransferFunction, _composer: Composer) -> Result<Self> {
        Err(anyhow!(
            "CUDA analysis is unavailable because this binary was built without --features cuda"
        ))
    }

    pub fn analyze(
        &mut self,
        _frame: &frame::Video,
        _crop_rect: &CropRect,
        _sample_stride: u32,
        _options: &FrameAnalysisOptions<'_>,
    ) -> Result<AnalyzedFrame> {
        Err(anyhow!(
            "CUDA analysis is unavailable because this binary was built without --features cuda"
        ))
    }

    pub fn device_frame_ineligibility(&self, _frame: &frame::Video) -> Option<&'static str> {
        Some("this binary was built without --features cuda")
    }

    pub fn context_faulted(&self) -> bool {
        false
    }

    pub fn analyze_device(
        &mut self,
        frame: &frame::Video,
        crop_rect: &CropRect,
        sample_stride: u32,
        options: &FrameAnalysisOptions<'_>,
    ) -> Result<AnalyzedFrame> {
        self.analyze(frame, crop_rect, sample_stride, options)
    }
}

#[cfg(test)]
mod tests;
