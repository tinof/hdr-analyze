use anyhow::{anyhow, Result};
#[cfg(feature = "cuda")]
use ffmpeg_next::format;
use ffmpeg_next::frame;
use madvr_parse::MadVRFrame;

#[cfg(feature = "cuda")]
use crate::analysis::frame::average_pq_from_v5_histogram;
#[cfg(any(feature = "cuda", test))]
use crate::analysis::frame::v5_histogram_constants;
#[cfg(any(feature = "cuda", test))]
use crate::analysis::histogram::nits_to_pq;
#[cfg(any(feature = "cuda", test))]
use crate::analysis::hlg::hlg_signal_to_nits;
use crate::crop::CropRect;
use crate::ffmpeg_io::TransferFunction;

#[cfg(feature = "cuda")]
const LUMINANCE_BINS: usize = 256;
#[cfg(feature = "cuda")]
const HUE_BINS: usize = 31;
#[cfg(feature = "cuda")]
const RESULT_WORDS: usize = LUMINANCE_BINS + HUE_BINS + 1;
#[cfg(feature = "cuda")]
const PEAK_WORD: usize = RESULT_WORDS - 1;

#[cfg(any(feature = "cuda", test))]
fn pq_for_code(code: i32, transfer_function: TransferFunction, hlg_peak_nits: f64) -> f64 {
    let norm = (((code - 64) as f64) / 876.0).clamp(0.0, 1.0);
    match transfer_function {
        TransferFunction::Hlg => nits_to_pq(hlg_signal_to_nits(norm, hlg_peak_nits)),
        _ => norm,
    }
    .clamp(0.0, 1.0)
}

#[cfg(any(feature = "cuda", test))]
fn build_transfer_lut(transfer_function: TransferFunction, hlg_peak_nits: f64) -> Vec<f32> {
    (0..1024)
        .map(|code| pq_for_code(code, transfer_function, hlg_peak_nits) as f32)
        .collect()
}

#[cfg(any(feature = "cuda", test))]
fn build_luminance_bin_lut(transfer_function: TransferFunction, hlg_peak_nits: f64) -> Vec<u16> {
    let (sdr_peak_pq, sdr_step, hdr_step) = v5_histogram_constants();
    (0..1024)
        .map(|code| {
            let pq = pq_for_code(code, transfer_function, hlg_peak_nits);
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

    use super::*;

    pub struct GpuAnalyzer {
        _cuda_driver: Library,
        _nvrtc: Library,
        stream: Arc<CudaStream>,
        kernel: CudaFunction,
        transfer_lut: CudaSlice<f32>,
        luminance_bin_lut: CudaSlice<u16>,
        results: CudaSlice<u32>,
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

        pub fn new(transfer_function: TransferFunction, hlg_peak_nits: f64) -> Result<Self> {
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
                .clone_htod(&build_transfer_lut(transfer_function, hlg_peak_nits))
                .map_err(|err| anyhow!("failed to upload transfer LUT: {err:?}"))?;
            let luminance_bin_lut = stream
                .clone_htod(&build_luminance_bin_lut(transfer_function, hlg_peak_nits))
                .map_err(|err| anyhow!("failed to upload luminance-bin LUT: {err:?}"))?;
            let results = stream
                .alloc_zeros::<u32>(RESULT_WORDS)
                .map_err(|err| anyhow!("failed to allocate CUDA result buffer: {err:?}"))?;

            Ok(Self {
                _cuda_driver: cuda_driver,
                _nvrtc: nvrtc,
                stream,
                kernel,
                transfer_lut,
                luminance_bin_lut,
                results,
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
        ) -> Result<MadVRFrame> {
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
                .memset_zeros(&mut self.results)
                .map_err(|err| anyhow!("failed to clear CUDA result buffer: {err:?}"))?;

            let stride = sample_stride.max(1) as i32;
            let sample_width = (crop_rect.width as i32 + stride - 1) / stride;
            let sample_height = (crop_rect.height as i32 + stride - 1) / stride;
            let sample_count = sample_width.saturating_mul(sample_height);
            if sample_count <= 0 {
                return Ok(MadVRFrame::default());
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
            let cfg = LaunchConfig {
                grid_dim: (((sample_count as u32).div_ceil(256)), 1, 1),
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
                .arg(&mut self.results)
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
                .arg(&sample_count);
            unsafe { launch.launch(cfg) }
                .map_err(|err| anyhow!("CUDA analysis launch failed: {err:?}"))?;

            let result = self
                .stream
                .clone_dtoh(&self.results)
                .map_err(|err| anyhow!("failed to download CUDA analysis results: {err:?}"))?;

            let lum_total: u64 = result[..LUMINANCE_BINS]
                .iter()
                .map(|&count| u64::from(count))
                .sum();
            let mut lum_histogram = vec![0.0; LUMINANCE_BINS];
            if lum_total > 0 {
                for (percent, &count) in lum_histogram.iter_mut().zip(&result[..LUMINANCE_BINS]) {
                    *percent = (f64::from(count) / lum_total as f64) * 100.0;
                }
            }

            let hue_counts = &result[LUMINANCE_BINS..LUMINANCE_BINS + HUE_BINS];
            let hue_total: u64 = hue_counts.iter().map(|&count| u64::from(count)).sum();
            let mut hue_histogram = vec![0.0; HUE_BINS];
            if hue_total > 0 {
                for (percent, &count) in hue_histogram.iter_mut().zip(hue_counts) {
                    *percent = (f64::from(count) / hue_total as f64) * 100.0;
                }
            }

            Ok(MadVRFrame {
                peak_pq_2020: f32::from_bits(result[PEAK_WORD]) as f64,
                avg_pq: average_pq_from_v5_histogram(&lum_histogram),
                lum_histogram,
                hue_histogram: Some(hue_histogram),
                target_nits: None,
                ..Default::default()
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
    pub fn new(_transfer_function: TransferFunction, _hlg_peak_nits: f64) -> Result<Self> {
        Err(anyhow!(
            "CUDA analysis is unavailable because this binary was built without --features cuda"
        ))
    }

    pub fn analyze(
        &mut self,
        _frame: &frame::Video,
        _crop_rect: &CropRect,
        _sample_stride: u32,
    ) -> Result<MadVRFrame> {
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
        let lut = build_transfer_lut(TransferFunction::Pq, 1000.0);
        assert_eq!(lut.len(), 1024);
        assert_eq!(lut[0], 0.0);
        assert_eq!(lut[64], 0.0);
        assert!((lut[502] - 0.5).abs() < 1.0e-6);
        assert_eq!(lut[940], 1.0);
        assert_eq!(lut[1023], 1.0);
    }

    #[test]
    fn hlg_lut_is_monotonic_and_peak_limited() {
        let lut = build_transfer_lut(TransferFunction::Hlg, 1000.0);
        assert!(lut.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!((lut[940] as f64 - nits_to_pq(1000.0)).abs() < 1.0e-6);
    }

    #[test]
    fn luminance_bin_lut_uses_exact_cpu_boundaries() {
        let bins = build_luminance_bin_lut(TransferFunction::Pq, 1000.0);
        let (_, sdr_step, _) = v5_histogram_constants();
        for (code, &bin) in bins.iter().enumerate() {
            let pq = pq_for_code(code as i32, TransferFunction::Pq, 1000.0);
            if pq < nits_to_pq(100.0) {
                assert_eq!(usize::from(bin), (pq / sdr_step).floor() as usize);
            }
        }
    }
}
