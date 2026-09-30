//! Optional CUDA analysis backend (feature `cuda`).
//!
//! Mirrors `analysis::frame::analyze_native_frame_cropped` on the GPU: one kernel
//! launch per analyzed frame produces the v5 luminance histogram, hue histogram,
//! a 4096-bin PQ histogram in the selected peak domain, true per-pixel luma /
//! max-RGB means, and domain peaks. Only a few KB of results are downloaded per
//! frame. The grain-robust (`robust`) peak estimator needs the cross-quad diff
//! histogram and stays CPU-only; the pipeline never routes it here.

use anyhow::{anyhow, Result};
#[cfg(feature = "cuda")]
use ffmpeg_next::format;
use ffmpeg_next::frame;

#[cfg(feature = "cuda")]
use crate::analysis::frame::{high_percentile_pq, low_percentile_pq};
use crate::analysis::frame::{AnalyzedFrame, FrameAnalysisOptions};
#[cfg(any(feature = "cuda", test))]
use crate::analysis::histogram::nits_to_pq;
#[cfg(any(feature = "cuda", test))]
use crate::analysis::hlg::{dovi84_decoder, dovi84_pq_lut, MMR_MAX_ORDER, MMR_TERMS};
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
#[cfg(feature = "cuda")]
const COUNT_WORDS: usize = MAX_RGB_WORD + 1;
#[cfg(feature = "cuda")]
const SUM_WORDS: usize = 3;
#[cfg(feature = "cuda")]
const FIXED_POINT_SCALE: f64 = 4_294_967_296.0;

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

#[cfg(any(feature = "cuda", test))]
fn pq_for_code(code: i32, transfer_function: TransferFunction) -> f64 {
    match transfer_function {
        TransferFunction::Hlg => f64::from(dovi84_pq_lut()[code.clamp(0, 1023) as usize]),
        _ => ((code - 64) as f64) / 876.0,
    }
    .clamp(0.0, 1.0)
}

#[cfg(any(feature = "cuda", test))]
fn build_transfer_lut(transfer_function: TransferFunction) -> Vec<f32> {
    (0..1024)
        .map(|code| pq_for_code(code, transfer_function) as f32)
        .collect()
}

/// Flatten the shared Profile 8.4 decoder into the kernel's `dovi_params` buffer.
#[cfg(any(feature = "cuda", test))]
fn build_dovi84_params() -> Vec<f32> {
    let decoder = dovi84_decoder();
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
fn build_luminance_bin_lut(transfer_function: TransferFunction) -> Vec<u16> {
    // Must match the v5 binning constants in analyze_native_frame_cropped exactly.
    let sdr_peak_pq = nits_to_pq(100.0);
    let sdr_step = sdr_peak_pq / 64.0;
    let hdr_step = (1.0 - sdr_peak_pq) / 192.0;
    (0..1024)
        .map(|code| {
            let pq = pq_for_code(code, transfer_function);
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
mod backend {
    use std::sync::Arc;

    use cudarc::driver::{
        CudaContext, CudaFunction, CudaSlice, CudaStream, LaunchConfig, PushKernelArg,
    };
    use cudarc::nvrtc::compile_ptx;
    use libloading::Library;
    use madvr_parse::MadVRFrame;

    use super::*;
    use crate::analysis::frame::FramePeakStats;
    use crate::cli::{PeakDomain, PeakEstimator};
    use crate::l1_sidecar::FrameL1Measurement;

    pub struct GpuAnalyzer {
        _cuda_driver: Library,
        _nvrtc: Library,
        stream: Arc<CudaStream>,
        kernel: CudaFunction,
        transfer_lut: CudaSlice<f32>,
        luminance_bin_lut: CudaSlice<u16>,
        dovi_params: CudaSlice<f32>,
        counts: CudaSlice<u32>,
        sums: CudaSlice<u64>,
        y_plane: Option<CudaSlice<u8>>,
        u_plane: Option<CudaSlice<u8>>,
        v_plane: Option<CudaSlice<u8>>,
        y_capacity: usize,
        u_capacity: usize,
        v_capacity: usize,
    }

    impl GpuAnalyzer {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        fn load_first_library(kind: &str, candidates: &[&str]) -> Result<Library> {
            for candidate in candidates {
                // SAFETY: loading the CUDA/NVRTC library only runs its platform loader hooks;
                // cudarc resolves and validates all symbols it actually uses.
                if let Ok(library) = unsafe { Library::new(*candidate) } {
                    return Ok(library);
                }
            }
            Err(anyhow!(
                "{kind} shared library was not found (tried {})",
                candidates.join(", ")
            ))
        }

        #[cfg(target_os = "linux")]
        fn load_cuda_libraries() -> Result<(Library, Library)> {
            let driver = Self::load_first_library("CUDA driver", &["libcuda.so.1", "libcuda.so"])?;
            let nvrtc = Self::load_first_library(
                "CUDA NVRTC",
                &["libnvrtc.so.12", "libnvrtc.so.13", "libnvrtc.so"],
            )?;
            Ok((driver, nvrtc))
        }

        #[cfg(target_os = "windows")]
        fn load_cuda_libraries() -> Result<(Library, Library)> {
            let driver = Self::load_first_library("CUDA driver", &["nvcuda.dll"])?;
            let nvrtc = Self::load_first_library(
                "CUDA NVRTC",
                &[
                    "nvrtc64_120_0.dll",
                    "nvrtc64_130_0.dll",
                    "nvrtc64_131_0.dll",
                    "nvrtc64.dll",
                ],
            )?;
            Ok((driver, nvrtc))
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        fn load_cuda_libraries() -> Result<(Library, Library)> {
            Err(anyhow!(
                "CUDA analysis is supported only on Linux and Windows"
            ))
        }

        pub fn new(transfer_function: TransferFunction) -> Result<Self> {
            // cudarc's dynamic loader panics when a library is wholly absent. Probe with
            // libloading first so `--hwaccel cuda` can reliably fall back to CPU.
            let (cuda_driver, nvrtc) = Self::load_cuda_libraries()?;
            let context = CudaContext::new(0)
                .map_err(|err| anyhow!("failed to open CUDA device 0: {err:?}"))?;
            let stream = context.default_stream();
            let ptx = compile_ptx(include_str!("kernels.cu"))
                .map_err(|err| anyhow!("NVRTC failed to compile HDR analysis kernel: {err}"))?;
            let module = context
                .load_module(ptx)
                .map_err(|err| anyhow!("failed to load CUDA analysis module: {err:?}"))?;
            let kernel = module
                .load_function("analyze_frame")
                .map_err(|err| anyhow!("failed to load analyze_frame kernel: {err:?}"))?;
            let transfer_lut = stream
                .clone_htod(&build_transfer_lut(transfer_function))
                .map_err(|err| anyhow!("failed to upload transfer LUT: {err:?}"))?;
            let luminance_bin_lut = stream
                .clone_htod(&build_luminance_bin_lut(transfer_function))
                .map_err(|err| anyhow!("failed to upload luminance-bin LUT: {err:?}"))?;
            let dovi_params = stream
                .clone_htod(&build_dovi84_params())
                .map_err(|err| anyhow!("failed to upload DV 8.4 decode parameters: {err:?}"))?;
            let counts = stream
                .alloc_zeros::<u32>(COUNT_WORDS)
                .map_err(|err| anyhow!("failed to allocate CUDA count buffer: {err:?}"))?;
            let sums = stream
                .alloc_zeros::<u64>(SUM_WORDS)
                .map_err(|err| anyhow!("failed to allocate CUDA sum buffer: {err:?}"))?;

            Ok(Self {
                _cuda_driver: cuda_driver,
                _nvrtc: nvrtc,
                stream,
                kernel,
                transfer_lut,
                luminance_bin_lut,
                dovi_params,
                counts,
                sums,
                y_plane: None,
                u_plane: None,
                v_plane: None,
                y_capacity: 0,
                u_capacity: 0,
                v_capacity: 0,
            })
        }

        fn upload_plane(
            stream: &Arc<CudaStream>,
            slot: &mut Option<CudaSlice<u8>>,
            capacity: &mut usize,
            data: &[u8],
        ) -> Result<()> {
            if *capacity < data.len() {
                *slot = Some(
                    stream
                        .clone_htod(data)
                        .map_err(|err| anyhow!("failed to allocate CUDA plane buffer: {err:?}"))?,
                );
                *capacity = data.len();
            } else {
                stream
                    .memcpy_htod(data, slot.as_mut().expect("plane buffer must be allocated"))
                    .map_err(|err| anyhow!("failed to upload video plane: {err:?}"))?;
            }
            Ok(())
        }

        pub fn analyze(
            &mut self,
            frame: &frame::Video,
            crop_rect: &CropRect,
            sample_stride: u32,
            options: &FrameAnalysisOptions<'_>,
        ) -> Result<AnalyzedFrame> {
            if options.peak_estimator == PeakEstimator::Robust {
                return Err(anyhow!(
                    "the grain-robust peak estimator is CPU-only (needs the cross-quad diff histogram)"
                ));
            }

            let layout = match frame.format() {
                format::Pixel::YUV420P10LE => 0i32,
                format::Pixel::P010LE => 1i32,
                other => {
                    return Err(anyhow!(
                        "CUDA analysis requires YUV420P10LE or P010LE, got {other:?}"
                    ));
                }
            };

            let y_host = frame.data(0);
            let u_host = frame.data(1);
            let v_host = if layout == 0 { frame.data(2) } else { &[] };
            Self::upload_plane(
                &self.stream,
                &mut self.y_plane,
                &mut self.y_capacity,
                y_host,
            )?;
            Self::upload_plane(
                &self.stream,
                &mut self.u_plane,
                &mut self.u_capacity,
                u_host,
            )?;
            if layout == 0 {
                Self::upload_plane(
                    &self.stream,
                    &mut self.v_plane,
                    &mut self.v_capacity,
                    v_host,
                )?;
            } else if self.v_plane.is_none() {
                self.v_plane =
                    Some(self.stream.alloc_zeros::<u8>(1).map_err(|err| {
                        anyhow!("failed to allocate CUDA sentinel plane: {err:?}")
                    })?);
                self.v_capacity = 1;
            }

            self.stream
                .memset_zeros(&mut self.counts)
                .map_err(|err| anyhow!("failed to clear CUDA count buffer: {err:?}"))?;
            self.stream
                .memset_zeros(&mut self.sums)
                .map_err(|err| anyhow!("failed to clear CUDA sum buffer: {err:?}"))?;

            let stride = sample_stride.max(1) as i32;
            let sample_width = (crop_rect.width as i32 + stride - 1) / stride;
            let sample_height = (crop_rect.height as i32 + stride - 1) / stride;
            let sample_count = sample_width.saturating_mul(sample_height);
            if sample_count <= 0 {
                return Err(anyhow!("crop rectangle produced no samples"));
            }

            let width = frame.width() as i32;
            let height = frame.height() as i32;
            let y_stride = frame.stride(0) as i32;
            let u_stride = frame.stride(1) as i32;
            let v_stride = if layout == 0 {
                frame.stride(2) as i32
            } else {
                0
            };
            let crop_x = crop_rect.x as i32;
            let crop_y = crop_rect.y as i32;
            let crop_width = crop_rect.width as i32;
            let crop_height = crop_rect.height as i32;
            let dovi84_rgb = i32::from(options.transfer_function == TransferFunction::Hlg);
            let peak_is_max_rgb = i32::from(options.peak_domain == PeakDomain::MaxRgb);
            let cfg = LaunchConfig {
                grid_dim: ((sample_count as u32).div_ceil(256), 1, 1),
                block_dim: (256, 1, 1),
                shared_mem_bytes: 0,
            };
            let mut launch = self.stream.launch_builder(&self.kernel);
            launch
                .arg(self.y_plane.as_ref().expect("Y plane uploaded"))
                .arg(self.u_plane.as_ref().expect("U/UV plane uploaded"))
                .arg(self.v_plane.as_ref().expect("V/sentinel plane uploaded"))
                .arg(&self.transfer_lut)
                .arg(&self.luminance_bin_lut)
                .arg(&self.dovi_params)
                .arg(&mut self.counts)
                .arg(&mut self.sums)
                .arg(&width)
                .arg(&height)
                .arg(&y_stride)
                .arg(&u_stride)
                .arg(&v_stride)
                .arg(&crop_x)
                .arg(&crop_y)
                .arg(&crop_width)
                .arg(&crop_height)
                .arg(&stride)
                .arg(&layout)
                .arg(&sample_count)
                .arg(&dovi84_rgb)
                .arg(&peak_is_max_rgb);
            unsafe { launch.launch(cfg) }
                .map_err(|err| anyhow!("CUDA analysis launch failed: {err:?}"))?;

            let counts = self
                .stream
                .clone_dtoh(&self.counts)
                .map_err(|err| anyhow!("failed to download CUDA analysis counts: {err:?}"))?;
            let sums = self
                .stream
                .clone_dtoh(&self.sums)
                .map_err(|err| anyhow!("failed to download CUDA analysis sums: {err:?}"))?;

            let pixel_count = sums[2];
            let mut lum_histogram = vec![0.0f64; LUMINANCE_BINS];
            if pixel_count > 0 {
                for (percent, &count) in lum_histogram.iter_mut().zip(&counts[..LUMINANCE_BINS]) {
                    *percent = (f64::from(count) / pixel_count as f64) * 100.0;
                }
            }

            let hue_counts = &counts[LUMINANCE_BINS..LUMINANCE_BINS + HUE_BINS];
            let hue_total: u64 = hue_counts.iter().map(|&count| u64::from(count)).sum();
            let mut hue_histogram = vec![0.0f64; HUE_BINS];
            if hue_total > 0 {
                for (percent, &count) in hue_histogram.iter_mut().zip(hue_counts) {
                    *percent = (f64::from(count) / hue_total as f64) * 100.0;
                }
            }

            let pq_hist: Vec<u64> = counts[PQ_HIST_BASE..PQ_HIST_BASE + PQ_BINS]
                .iter()
                .map(|&count| u64::from(count))
                .collect();

            let max_luma_pq = f64::from(f32::from_bits(counts[MAX_LUMA_WORD]));
            let max_rgb_pq = f64::from(f32::from_bits(counts[MAX_RGB_WORD]));
            let avg_pq = if pixel_count > 0 {
                (sums[0] as f64 / FIXED_POINT_SCALE / pixel_count as f64).min(1.0)
            } else {
                0.0
            };
            let avg_max_rgb_pq = if pixel_count > 0 {
                (sums[1] as f64 / FIXED_POINT_SCALE / pixel_count as f64).min(1.0)
            } else {
                0.0
            };

            let raw_max_pq = match options.peak_domain {
                PeakDomain::MaxRgb => max_rgb_pq,
                PeakDomain::Luma => max_luma_pq,
            };
            let percentile_pq = high_percentile_pq(&pq_hist, options.peak_percentile);
            let selected_peak_pq = match options.peak_estimator {
                PeakEstimator::Max => raw_max_pq,
                PeakEstimator::Percentile => percentile_pq,
                PeakEstimator::Robust => unreachable!("robust estimator rejected above"),
            };
            let min_pq = low_percentile_pq(&pq_hist, options.min_percentile);

            Ok(AnalyzedFrame {
                frame: MadVRFrame {
                    peak_pq_2020: selected_peak_pq,
                    avg_pq,
                    lum_histogram,
                    hue_histogram: Some(hue_histogram),
                    target_nits: None,
                    ..Default::default()
                },
                l1: FrameL1Measurement {
                    min_pq,
                    avg_max_rgb_pq,
                },
                // Grain statistics (sigma / n_eff / robust correction) need the CPU
                // cross-quad diff histogram; report neutral values on the GPU path.
                peak_stats: FramePeakStats {
                    selected_peak_pq,
                    raw_max_pq,
                    percentile_pq,
                    robust_pq: raw_max_pq,
                    correction_pq: 0.0,
                    sigma_pq: 0.0,
                    n_eff: 0,
                },
            })
        }
    }
}

#[cfg(feature = "cuda")]
pub use backend::GpuAnalyzer;

#[cfg(not(feature = "cuda"))]
pub struct GpuAnalyzer;

#[cfg(not(feature = "cuda"))]
impl GpuAnalyzer {
    pub fn new(_transfer_function: TransferFunction) -> Result<Self> {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pq_lut_matches_limited_range_contract() {
        let lut = build_transfer_lut(TransferFunction::Pq);
        assert_eq!(lut.len(), 1024);
        assert_eq!(lut[0], 0.0);
        assert_eq!(lut[64], 0.0);
        assert!((f64::from(lut[502]) - 0.5).abs() < 1.0e-6);
        assert_eq!(lut[940], 1.0);
        assert_eq!(lut[1023], 1.0);
    }

    #[test]
    fn hlg_lut_is_monotonic_and_peak_limited() {
        let lut = build_transfer_lut(TransferFunction::Hlg);
        // The DV 8.4 curve has a ~3e-7 PQ seam at the 910/911 piece boundary.
        assert!(lut
            .windows(2)
            .all(|pair| f64::from(pair[1]) >= f64::from(pair[0]) - 1.0e-6));
        let source_max = (3079.0_f64 / 4095.0) as f32;
        assert_eq!(lut[940], source_max);
        assert_eq!(lut[1023], source_max);
        assert!(lut.iter().all(|&pq| pq <= source_max));
    }

    #[test]
    fn hlg_transfer_lut_equals_dovi84_lut_bit_exactly() {
        let lut = build_transfer_lut(TransferFunction::Hlg);
        let reference = dovi84_pq_lut();
        assert_eq!(lut.len(), reference.len());
        for (code, (&gpu, &cpu)) in lut.iter().zip(reference.iter()).enumerate() {
            assert_eq!(gpu.to_bits(), cpu.to_bits(), "code {code}");
        }
    }

    /// Line-by-line transcription of `dovi84_max_rgb_pq` in kernels.cu, reading the flat
    /// parameter buffer by the kernel's offsets.
    fn kernel_max_rgb_pq(params: &[f32], y_code: u16, cb_code: u16, cr_code: u16) -> f32 {
        let reshape = |mmr: &[f32], y: f32, u: f32, v: f32| {
            let (yu, yv, uv) = (y * u, y * v, u * v);
            let x = [y, u, v, yu, yv, uv, yu * v];
            let mut s = mmr[0];
            for order in 0..MMR_MAX_ORDER {
                for (term, &value) in x.iter().enumerate() {
                    let mut p = value;
                    if order >= 1 {
                        p = value * value;
                    }
                    if order == 2 {
                        p *= value;
                    }
                    s += mmr[1 + order * MMR_TERMS + term] * p;
                }
            }
            s.max(params[DOVI_CHROMA_CLAMP])
                .min(params[DOVI_CHROMA_CLAMP + 1])
        };
        let (y, u, v) = (
            f32::from(y_code) / 1023.0,
            f32::from(cb_code) / 1023.0,
            f32::from(cr_code) / 1023.0,
        );
        let cb = reshape(&params[DOVI_MMR_CB..], y, u, v) - params[DOVI_CHROMA_OFFSET];
        let cr = reshape(&params[DOVI_MMR_CR..], y, u, v) - params[DOVI_CHROMA_OFFSET + 1];
        let luma = params[usize::from(y_code)];
        let m = &params[DOVI_YCC..];
        let red = luma + m[0] * cb + m[1] * cr;
        let green = luma + m[2] * cb + m[3] * cr;
        let blue = luma + m[4] * cb + m[5] * cr;
        red.max(green)
            .max(blue)
            .max(params[DOVI_SOURCE_RANGE])
            .min(params[DOVI_SOURCE_RANGE + 1])
    }

    #[test]
    fn dovi84_kernel_params_reproduce_the_cpu_decoder_bit_exactly() {
        let params = build_dovi84_params();
        assert_eq!(params.len(), DOVI_PARAM_WORDS);
        let decoder = dovi84_decoder();
        for y in (0..1024_u16).step_by(7) {
            for cb in (0..1024_u16).step_by(31) {
                for cr in (0..1024_u16).step_by(29) {
                    let cpu = decoder.max_rgb_pq(y, &decoder.chroma(cb, cr));
                    let kernel = kernel_max_rgb_pq(&params, y, cb, cr);
                    assert_eq!(cpu.to_bits(), kernel.to_bits(), "codes ({y}, {cb}, {cr})");
                }
            }
        }
    }

    #[test]
    fn luminance_bin_lut_uses_exact_cpu_boundaries() {
        let bins = build_luminance_bin_lut(TransferFunction::Pq);
        let sdr_peak_pq = nits_to_pq(100.0);
        let sdr_step = sdr_peak_pq / 64.0;
        for (code, &bin) in bins.iter().enumerate() {
            let pq = pq_for_code(code as i32, TransferFunction::Pq);
            if pq < sdr_peak_pq {
                assert_eq!(usize::from(bin), (pq / sdr_step).floor() as usize);
            }
        }
    }
}
