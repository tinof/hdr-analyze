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
mod backend {
    use std::sync::Arc;

    use cudarc::driver::sys::{self, CUctx_flags, CUdevice_attribute, CUresult};
    use cudarc::driver::{
        CudaContext, CudaFunction, CudaSlice, CudaStream, DevicePtr, LaunchConfig, PushKernelArg,
    };
    use cudarc::nvrtc::compile_ptx;
    use ffmpeg_next::ffi;
    use libloading::Library;
    use madvr_parse::MadVRFrame;

    use super::*;
    use crate::analysis::frame::FramePeakStats;
    use crate::analysis::hlg::ChromaSiting;
    use crate::cli::{PeakDomain, PeakEstimator};
    use crate::l1_sidecar::FrameL1Measurement;

    /// Leading fields of FFmpeg's `AVCUDADeviceContext` (hwcontext_cuda.h), unchanged since
    /// FFmpeg 4. Mirrored locally because ffmpeg-sys only binds that header when the CUDA
    /// headers are present at build time.
    #[repr(C)]
    struct CudaDeviceContextHead {
        cuda_ctx: sys::CUcontext,
        stream: sys::CUstream,
    }

    /// Device pointers and pitches of one frame's planes as the kernel reads them.
    struct Planes {
        y: u64,
        u: u64,
        v: u64,
        y_stride: i32,
        u_stride: i32,
        v_stride: i32,
        layout: i32,
        width: i32,
        height: i32,
    }

    fn check(result: CUresult, call: &str) -> Result<()> {
        if result == CUresult::CUDA_SUCCESS {
            Ok(())
        } else {
            Err(anyhow!("{call} failed: {result:?}"))
        }
    }

    /// Make device 0's primary context usable by both cudarc and FFmpeg's NVDEC decoder.
    ///
    /// FFmpeg opens the decoder with `AV_CUDA_USE_PRIMARY_CONTEXT`, which requires the primary
    /// context to carry exactly `CU_CTX_SCHED_BLOCKING_SYNC`, and CUDA only allows setting
    /// the flags while the context is inactive. This must therefore run before cudarc
    /// retains the context, and `GpuAnalyzer::new` must run before the decoder is opened.
    fn prepare_primary_context() -> Result<()> {
        let wanted = CUctx_flags::CU_CTX_SCHED_BLOCKING_SYNC as u32;
        // SAFETY: plain driver API calls on out-parameters owned by this frame; the driver
        // library was loaded and validated by `load_cuda_libraries`.
        unsafe {
            check(sys::cuInit(0), "cuInit")?;
            let mut device = 0;
            check(sys::cuDeviceGet(&mut device, 0), "cuDeviceGet")?;
            let mut flags = 0u32;
            let mut active = 0i32;
            check(
                sys::cuDevicePrimaryCtxGetState(device, &mut flags, &mut active),
                "cuDevicePrimaryCtxGetState",
            )?;
            if flags == wanted {
                return Ok(());
            }
            if active != 0 {
                return Err(anyhow!(
                    "the CUDA primary context is already active with flags {flags:#x}, \
                     which the shared NVDEC decoder cannot use"
                ));
            }
            check(
                sys::cuDevicePrimaryCtxSetFlags_v2(device, wanted),
                "cuDevicePrimaryCtxSetFlags",
            )
        }
    }

    pub struct GpuAnalyzer {
        _cuda_driver: Library,
        _nvrtc: Library,
        stream: Arc<CudaStream>,
        kernel: CudaFunction,
        compose_kernel: CudaFunction,
        transfer_lut: CudaSlice<f32>,
        luminance_bin_lut: CudaSlice<u16>,
        dovi_params: CudaSlice<f32>,
        results: CudaSlice<u32>,
        results_host: Vec<u32>,
        max_blocks: u32,
        faulted: bool,
        y_plane: Option<CudaSlice<u8>>,
        u_plane: Option<CudaSlice<u8>>,
        v_plane: Option<CudaSlice<u8>>,
        y_capacity: usize,
        u_capacity: usize,
        v_capacity: usize,
        /// HLG: composed chroma at chroma resolution, written by `compose_chroma`.
        composed: Option<CudaSlice<f32>>,
        /// `HDR_ANALYZER_DUMP_MAX_RGB`: every sample's max-RGB.
        dump: Option<CudaSlice<f32>>,
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

        /// `composer` selects the Profile 8.4 decode for HLG input; it is ignored for PQ.
        pub fn new(transfer_function: TransferFunction, composer: Composer) -> Result<Self> {
            // cudarc's dynamic loader panics when a library is wholly absent. Probe with
            // libloading first so `--hwaccel cuda` can reliably fall back to CPU.
            let (cuda_driver, nvrtc) = Self::load_cuda_libraries()?;
            prepare_primary_context()?;
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
            let compose_kernel = module
                .load_function("compose_chroma")
                .map_err(|err| anyhow!("failed to load compose_chroma kernel: {err:?}"))?;
            let transfer_lut = stream
                .clone_htod(&build_transfer_lut(transfer_function, composer))
                .map_err(|err| anyhow!("failed to upload transfer LUT: {err:?}"))?;
            let luminance_bin_lut = stream
                .clone_htod(&build_luminance_bin_lut(transfer_function, composer))
                .map_err(|err| anyhow!("failed to upload luminance-bin LUT: {err:?}"))?;
            let dovi_params = stream
                .clone_htod(&build_dovi84_params(dovi84_decoder(composer)))
                .map_err(|err| anyhow!("failed to upload DV 8.4 decode parameters: {err:?}"))?;
            let results = stream
                .alloc_zeros::<u32>(RESULT_WORDS)
                .map_err(|err| anyhow!("failed to allocate CUDA result buffer: {err:?}"))?;
            let sm_count = context
                .attribute(CUdevice_attribute::CU_DEVICE_ATTRIBUTE_MULTIPROCESSOR_COUNT)
                .map_err(|err| anyhow!("failed to query the CUDA SM count: {err:?}"))?;
            let max_blocks = u32::try_from(sm_count).unwrap_or(1).max(1) * BLOCKS_PER_SM;

            Ok(Self {
                _cuda_driver: cuda_driver,
                _nvrtc: nvrtc,
                stream,
                kernel,
                compose_kernel,
                transfer_lut,
                luminance_bin_lut,
                dovi_params,
                results,
                results_host: vec![0; RESULT_WORDS],
                max_blocks,
                faulted: false,
                y_plane: None,
                u_plane: None,
                v_plane: None,
                y_capacity: 0,
                u_capacity: 0,
                v_capacity: 0,
                composed: None,
                dump: None,
            })
        }

        /// A device buffer of at least `len` f32 values, grown (never shrunk) on demand.
        fn ensure_f32(
            stream: &Arc<CudaStream>,
            slot: &mut Option<CudaSlice<f32>>,
            len: usize,
            what: &str,
        ) -> Result<u64> {
            if slot.as_ref().is_none_or(|slice| slice.len() < len) {
                *slot = Some(
                    stream
                        .alloc_zeros::<f32>(len.max(1))
                        .map_err(|err| anyhow!("failed to allocate the CUDA {what}: {err:?}"))?,
                );
            }
            Ok(slot
                .as_ref()
                .expect("buffer was just allocated")
                .device_ptr(stream)
                .0)
        }

        /// Checks that run before any CUDA work, so a refusal leaves the context healthy.
        fn validate(sample_stride: u32, options: &FrameAnalysisOptions<'_>) -> Result<()> {
            if options.dump_max_rgb && sample_stride > 1 {
                return Err(anyhow!(
                    "HDR_ANALYZER_DUMP_MAX_RGB needs --downscale 1 on the CUDA path"
                ));
            }
            Ok(())
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

        fn device_address(slot: &Option<CudaSlice<u8>>, stream: &CudaStream) -> u64 {
            slot.as_ref().map_or(0, |slice| slice.device_ptr(stream).0)
        }

        /// Why an NVDEC frame cannot be analyzed in place, or `None` when it can.
        pub fn device_frame_ineligibility(&self, frame: &frame::Video) -> Option<&'static str> {
            self.device_planes(frame).err()
        }

        /// The plane pointers of an `AV_PIX_FMT_CUDA` frame, plus FFmpeg's CUDA stream.
        ///
        /// Accepted only for a P010 surface in this analyzer's own CUDA context: a frame from
        /// another context (hevc_cuvid, a reinitialized decoder) holds pointers that are not
        /// valid here and goes through the host-download path instead.
        fn device_planes(
            &self,
            frame: &frame::Video,
        ) -> std::result::Result<(Planes, sys::CUstream), &'static str> {
            // SAFETY: `frame` owns a live AVFrame; the pointer is only read below.
            let raw = unsafe { frame.as_ptr() };
            // SAFETY: `raw` is a live AVFrame. For AV_PIX_FMT_CUDA frames FFmpeg guarantees a
            // valid hw_frames_ctx whose AVHWFramesContext and AVHWDeviceContext outlive the
            // frame; every pointer is null-checked before it is dereferenced. The plane
            // pointers are only read as integers, never dereferenced on the host.
            unsafe {
                if (*raw).format != ffi::AVPixelFormat::AV_PIX_FMT_CUDA as i32 {
                    return Err("not a CUDA frame");
                }
                let frames_ref = (*raw).hw_frames_ctx;
                if frames_ref.is_null() || (*frames_ref).data.is_null() {
                    return Err("the frame has no CUDA frames context");
                }
                let frames = (*frames_ref).data as *const ffi::AVHWFramesContext;
                if (*frames).sw_format != ffi::AVPixelFormat::AV_PIX_FMT_P010LE {
                    return Err("the decoder surface is not P010 (8-bit or 12-bit source)");
                }
                let device = (*frames).device_ctx;
                if device.is_null()
                    || (*device).type_ != ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA
                    || (*device).hwctx.is_null()
                {
                    return Err("the frame has no CUDA device context");
                }
                let cuda = (*device).hwctx as *const CudaDeviceContextHead;
                if (*cuda).cuda_ctx != self.stream.context().cu_ctx() {
                    return Err("the decoder runs in a different CUDA context");
                }
                let (width, height) = ((*raw).width, (*raw).height);
                let (y_stride, uv_stride) = ((*raw).linesize[0], (*raw).linesize[1]);
                let (y, uv) = ((*raw).data[0] as u64, (*raw).data[1] as u64);
                if y == 0 || uv == 0 {
                    return Err("the frame is missing a plane");
                }
                // P010: 2 bytes per luma sample, interleaved 2-byte Cb/Cr at half width.
                if width <= 0 || height <= 0 || y_stride < width * 2 || uv_stride < width * 2 {
                    return Err("unexpected P010 plane geometry");
                }
                Ok((
                    Planes {
                        y,
                        u: uv,
                        v: 0,
                        y_stride,
                        u_stride: uv_stride,
                        v_stride: 0,
                        layout: 1,
                        width,
                        height,
                    },
                    (*cuda).stream,
                ))
            }
        }

        /// Analyze an NVDEC frame where the decoder left it, without a host round trip.
        ///
        /// The caller keeps `frame` referenced until this returns; the result download at the
        /// end synchronizes the kernel that reads it.
        pub fn analyze_device(
            &mut self,
            frame: &frame::Video,
            crop_rect: &CropRect,
            sample_stride: u32,
            options: &FrameAnalysisOptions<'_>,
        ) -> Result<AnalyzedFrame> {
            let (planes, producer) = self
                .device_planes(frame)
                .map_err(|reason| anyhow!("NVDEC frame not usable in place: {reason}"))?;
            Self::validate(sample_stride, options)?;
            let sample_count = Self::sample_count(crop_rect, sample_stride)?;
            let result = self.wait_for_producer(producer).and_then(|()| {
                self.launch_and_collect(&planes, crop_rect, sample_stride, sample_count, options)
            });
            self.record_fault(result)
        }

        /// FFmpeg's default producer stream is NULL, the legacy default stream: this analyzer
        /// launches on it too, so the kernel is ordered after NVDEC's surface copy. A non-NULL
        /// producer stream is synchronized explicitly.
        fn wait_for_producer(&self, producer: sys::CUstream) -> Result<()> {
            if producer.is_null() {
                return Ok(());
            }
            self.stream
                .context()
                .bind_to_thread()
                .map_err(|err| anyhow!("failed to bind the CUDA context: {err:?}"))?;
            // SAFETY: `producer` is FFmpeg's live stream in this same context.
            check(
                unsafe { sys::cuStreamSynchronize(producer) },
                "cuStreamSynchronize",
            )
        }

        /// Whether a CUDA call has failed. The NVDEC decoder shares this context, so after a
        /// fault neither its frames nor a download of them can be trusted: FFmpeg's CUDA
        /// transfer can report success after a failed copy.
        pub fn context_faulted(&self) -> bool {
            self.faulted
        }

        /// Every error from the CUDA part of an analysis is a CUDA API failure; validation
        /// that precedes it (options, frame format, crop) returns before this point.
        fn record_fault<T>(&mut self, result: Result<T>) -> Result<T> {
            if result.is_err() {
                self.faulted = true;
            }
            result
        }

        fn sample_count(crop_rect: &CropRect, sample_stride: u32) -> Result<i32> {
            let stride = sample_stride.max(1) as i32;
            let sample_width = (crop_rect.width as i32 + stride - 1) / stride;
            let sample_height = (crop_rect.height as i32 + stride - 1) / stride;
            let sample_count = sample_width.saturating_mul(sample_height);
            if sample_count <= 0 {
                return Err(anyhow!("crop rectangle produced no samples"));
            }
            Ok(sample_count)
        }

        pub fn analyze(
            &mut self,
            frame: &frame::Video,
            crop_rect: &CropRect,
            sample_stride: u32,
            options: &FrameAnalysisOptions<'_>,
        ) -> Result<AnalyzedFrame> {
            let layout = match frame.format() {
                format::Pixel::YUV420P10LE => 0i32,
                format::Pixel::P010LE => 1i32,
                other => {
                    return Err(anyhow!(
                        "CUDA analysis requires YUV420P10LE or P010LE, got {other:?}"
                    ));
                }
            };
            Self::validate(sample_stride, options)?;
            let sample_count = Self::sample_count(crop_rect, sample_stride)?;
            let result = self.upload_host_planes(frame, layout).and_then(|planes| {
                self.launch_and_collect(&planes, crop_rect, sample_stride, sample_count, options)
            });
            self.record_fault(result)
        }

        fn upload_host_planes(&mut self, frame: &frame::Video, layout: i32) -> Result<Planes> {
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
            }

            // Uploads, launch and download all run on `self.stream`, so passing raw addresses
            // (outside cudarc's per-slice event tracking) keeps them ordered.
            Ok(Planes {
                y: Self::device_address(&self.y_plane, &self.stream),
                u: Self::device_address(&self.u_plane, &self.stream),
                // P010 has no separate V plane; the kernel never reads it for layout 1.
                v: if layout == 0 {
                    Self::device_address(&self.v_plane, &self.stream)
                } else {
                    0
                },
                y_stride: frame.stride(0) as i32,
                u_stride: frame.stride(1) as i32,
                v_stride: if layout == 0 {
                    frame.stride(2) as i32
                } else {
                    0
                },
                layout,
                width: frame.width() as i32,
                height: frame.height() as i32,
            })
        }

        /// The CUDA part of an analysis: launch, download, synchronize, then parse on the host.
        fn launch_and_collect(
            &mut self,
            planes: &Planes,
            crop_rect: &CropRect,
            sample_stride: u32,
            sample_count: i32,
            options: &FrameAnalysisOptions<'_>,
        ) -> Result<AnalyzedFrame> {
            let stride = sample_stride.max(1) as i32;
            self.stream
                .memset_zeros(&mut self.results)
                .map_err(|err| anyhow!("failed to clear CUDA result buffer: {err:?}"))?;

            let crop_x = crop_rect.x as i32;
            let crop_y = crop_rect.y as i32;
            let crop_width = crop_rect.width as i32;
            let crop_height = crop_rect.height as i32;
            let dovi84_rgb = i32::from(options.transfer_function == TransferFunction::Hlg);
            let peak_is_max_rgb = i32::from(options.peak_domain == PeakDomain::MaxRgb);
            // The difference histogram costs a second decode for half of the pixels, so it
            // is only gathered for the estimator that needs it.
            let grain_stats = i32::from(options.peak_estimator == PeakEstimator::Robust);

            // HLG: compose the chroma of the crop's chroma samples, plus the neighbours the
            // upsampling reads, within the frame (as frame.rs ComposedChroma).
            let chroma_width = (planes.width + 1) / 2;
            let chroma_height = (planes.height + 1) / 2;
            let cx0 = crop_x / 2;
            let cy0 = (crop_y / 2 - 1).max(0);
            let cx1 = ((crop_x + crop_width - 1) / 2 + 1).min(chroma_width - 1);
            let cy1 = ((crop_y + crop_height - 1) / 2 + 1).min(chroma_height - 1);
            let rect_width = cx1 + 1 - cx0;
            let rect_height = cy1 + 1 - cy0;
            let centred = i32::from(options.chroma_siting == ChromaSiting::Left);
            let composed = if dovi84_rgb != 0 {
                let count = rect_width as usize * rect_height as usize;
                let address = Self::ensure_f32(
                    &self.stream,
                    &mut self.composed,
                    2 * count,
                    "composed-chroma buffer",
                )?;
                let compose_cfg = LaunchConfig {
                    grid_dim: (
                        (count as u32).div_ceil(BLOCK_THREADS).min(self.max_blocks),
                        1,
                        1,
                    ),
                    block_dim: (BLOCK_THREADS, 1, 1),
                    shared_mem_bytes: 0,
                };
                let mut compose = self.stream.launch_builder(&self.compose_kernel);
                compose
                    .arg(&planes.y)
                    .arg(&planes.u)
                    .arg(&planes.v)
                    .arg(&self.dovi_params)
                    .arg(&address)
                    .arg(&planes.width)
                    .arg(&planes.height)
                    .arg(&planes.y_stride)
                    .arg(&planes.u_stride)
                    .arg(&planes.v_stride)
                    .arg(&planes.layout)
                    .arg(&cx0)
                    .arg(&cy0)
                    .arg(&rect_width)
                    .arg(&rect_height);
                // Same stream as analyze_frame below, so the launches are ordered.
                unsafe { compose.launch(compose_cfg) }
                    .map_err(|err| anyhow!("CUDA chroma composition launch failed: {err:?}"))?;
                address
            } else {
                0u64
            };
            let dump = if options.dump_max_rgb {
                Self::ensure_f32(
                    &self.stream,
                    &mut self.dump,
                    sample_count as usize,
                    "max-RGB dump buffer",
                )?
            } else {
                0u64
            };

            let cfg = LaunchConfig {
                grid_dim: (
                    (sample_count as u32)
                        .div_ceil(BLOCK_THREADS)
                        .min(self.max_blocks),
                    1,
                    1,
                ),
                block_dim: (BLOCK_THREADS, 1, 1),
                shared_mem_bytes: 0,
            };
            let mut launch = self.stream.launch_builder(&self.kernel);
            launch
                .arg(&planes.y)
                .arg(&planes.u)
                .arg(&planes.v)
                .arg(&self.transfer_lut)
                .arg(&self.luminance_bin_lut)
                .arg(&self.dovi_params)
                .arg(&composed)
                .arg(&mut self.results)
                .arg(&dump)
                .arg(&planes.width)
                .arg(&planes.height)
                .arg(&planes.y_stride)
                .arg(&planes.u_stride)
                .arg(&planes.v_stride)
                .arg(&crop_x)
                .arg(&crop_y)
                .arg(&crop_width)
                .arg(&crop_height)
                .arg(&stride)
                .arg(&planes.layout)
                .arg(&sample_count)
                .arg(&dovi84_rgb)
                .arg(&peak_is_max_rgb)
                .arg(&grain_stats)
                .arg(&cx0)
                .arg(&cy0)
                .arg(&rect_width)
                .arg(&chroma_width)
                .arg(&chroma_height)
                .arg(&centred);
            unsafe { launch.launch(cfg) }
                .map_err(|err| anyhow!("CUDA analysis launch failed: {err:?}"))?;

            self.stream
                .memcpy_dtoh(&self.results, &mut self.results_host)
                .map_err(|err| anyhow!("failed to download CUDA analysis results: {err:?}"))?;
            // With stride 1 (`validate`) the samples are the crop's pixels, row by row.
            let max_rgb_pixels = match (options.dump_max_rgb, self.dump.as_ref()) {
                (true, Some(buffer)) => {
                    let mut pixels = vec![0.0f32; buffer.len()];
                    self.stream
                        .memcpy_dtoh(buffer, &mut pixels)
                        .map_err(|err| anyhow!("failed to download the max-RGB dump: {err:?}"))?;
                    pixels.truncate(sample_count as usize);
                    Some(pixels)
                }
                _ => None,
            };
            // A copy into pageable memory normally completes before returning, but CUDA does
            // not promise it; the results, and the caller's frame, are only safe after this.
            self.stream
                .synchronize()
                .map_err(|err| anyhow!("CUDA analysis did not complete: {err:?}"))?;
            let counts = &self.results_host[..SUMS_WORD];
            let sums = result_sums(&self.results_host);

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
            // Without the difference histogram the grain statistics stay neutral.
            let (sigma_pq, robust_pq, n_eff) = if grain_stats != 0 {
                let diff_hist = result_diff_hist(&self.results_host);
                let sigma_pq = sigma_from_diff_hist(&diff_hist, &pq_hist);
                (
                    sigma_pq,
                    robust_peak_pq(&pq_hist, raw_max_pq, sigma_pq),
                    effective_tail_count(&pq_hist, raw_max_pq, sigma_pq),
                )
            } else {
                (0.0, raw_max_pq, 0)
            };
            let selected_peak_pq = match options.peak_estimator {
                PeakEstimator::Max => raw_max_pq,
                PeakEstimator::Percentile => percentile_pq,
                PeakEstimator::Robust => robust_pq,
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
                    avg_luma_pq: avg_pq,
                    avg_max_rgb_pq,
                    max_rgb_pq,
                    fall_nits: mean_nits_from_pq_hist(&pq_hist),
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
    }
}

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
mod tests {
    use super::*;
    use crate::analysis::hlg::{downsample_luma, upsample_chroma, ChromaSiting};

    #[test]
    fn pq_lut_matches_limited_range_contract() {
        let lut = build_transfer_lut(TransferFunction::Pq, Composer::Preset);
        assert_eq!(lut.len(), 1024);
        assert_eq!(lut[0], 0.0);
        assert_eq!(lut[64], 0.0);
        assert!((f64::from(lut[502]) - 0.5).abs() < 1.0e-6);
        assert_eq!(lut[940], 1.0);
        assert_eq!(lut[1023], 1.0);
    }

    #[test]
    fn hlg_lut_is_monotonic_and_peak_limited() {
        let lut = build_transfer_lut(TransferFunction::Hlg, Composer::Preset);
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
        for composer in Composer::ALL {
            let lut = build_transfer_lut(TransferFunction::Hlg, composer);
            let reference = dovi84_pq_lut(composer);
            assert_eq!(lut.len(), reference.len());
            for (code, (&gpu, &cpu)) in lut.iter().zip(reference.iter()).enumerate() {
                assert_eq!(gpu.to_bits(), cpu.to_bits(), "{composer:?} code {code}");
            }
        }
    }

    /// Line-by-line transcription of `dovi84_reshape_chroma` as `compose_chroma` in
    /// kernels.cu calls it, reading the flat parameter buffer by the kernel's offsets.
    fn kernel_composed(params: &[f32], mmr_luma: u16, cb_code: u16, cr_code: u16) -> [f32; 2] {
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
            f32::from(mmr_luma) / 1023.0,
            f32::from(cb_code) / 1023.0,
            f32::from(cr_code) / 1023.0,
        );
        [
            reshape(&params[DOVI_MMR_CB..], y, u, v),
            reshape(&params[DOVI_MMR_CR..], y, u, v),
        ]
    }

    /// Transcription of `dovi84_max_rgb_composed` in kernels.cu.
    fn kernel_max_rgb_composed(params: &[f32], y_code: u16, composed: [f32; 2]) -> f32 {
        let cb = composed[0] - params[DOVI_CHROMA_OFFSET];
        let cr = composed[1] - params[DOVI_CHROMA_OFFSET + 1];
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

    /// Transcription of `downsample_luma` in kernels.cu (signed arithmetic, as there).
    fn kernel_downsample(luma: &[u16], width: i32, height: i32, cx: i32, cy: i32) -> u16 {
        let code = |x: i32, y: i32| u32::from(luma[(y * width + x) as usize]);
        let mut x = 2 * cx;
        let left = (x - 1).max(0);
        let right = (x + 1).min(width - 1);
        x = x.min(width - 1);
        let rows: [u32; 2] = std::array::from_fn(|k| {
            let y = (2 * cy + k as i32).min(height - 1);
            (code(left, y) + 2 * code(x, y) + code(right, y) + 2) >> 2
        });
        ((rows[0] + rows[1] + 1) >> 1) as u16
    }

    /// Transcription of `chroma_taps` in kernels.cu.
    fn kernel_taps(pos: i32, len: i32, centred: bool) -> (i32, f32, i32, f32) {
        let j = (pos / 2).min(len - 1);
        let last = len - 1;
        match (centred, pos & 1) {
            (false, 0) => (j, 1.0, j, 0.0),
            (false, _) => (j, 0.5, (j + 1).min(last), 0.5),
            (true, 0) => (j, 0.75, (j - 1).max(0), 0.25),
            (true, _) => (j, 0.75, (j + 1).min(last), 0.25),
        }
    }

    /// Transcription of `upsample_chroma` in kernels.cu, on the interleaved composed buffer.
    #[allow(clippy::too_many_arguments)]
    fn kernel_upsample(
        composed: &[f32],
        component: i32,
        (cx0, cy0, rect_width): (i32, i32, i32),
        chroma_width: i32,
        chroma_height: i32,
        centred: bool,
        x: i32,
        y: i32,
    ) -> f32 {
        let (c0, cw0, c1, cw1) = kernel_taps(x, chroma_width, false);
        let (r0, rw0, r1, rw1) = kernel_taps(y, chroma_height, centred);
        let at = |cx: i32, cy: i32| {
            composed[(2 * ((cy - cy0) * rect_width + (cx - cx0)) + component) as usize]
        };
        let row0 = cw0 * at(c0, r0) + cw1 * at(c1, r0);
        let row1 = cw0 * at(c0, r1) + cw1 * at(c1, r1);
        rw0 * row0 + rw1 * row1
    }

    #[test]
    fn dovi84_kernel_params_reproduce_the_cpu_decoder_bit_exactly() {
        for composer in Composer::ALL {
            let decoder = dovi84_decoder(composer);
            let params = build_dovi84_params(decoder);
            assert_eq!(params.len(), DOVI_PARAM_WORDS);
            for mmr_luma in (0..1024_u16).step_by(7) {
                for cb in (0..1024_u16).step_by(31) {
                    for cr in (0..1024_u16).step_by(29) {
                        let cpu = decoder.composed_chroma(mmr_luma, &decoder.chroma(cb, cr));
                        let kernel = kernel_composed(&params, mmr_luma, cb, cr);
                        assert_eq!(
                            cpu.map(f32::to_bits),
                            kernel.map(f32::to_bits),
                            "{composer:?} codes ({mmr_luma}, {cb}, {cr})"
                        );
                        // The pixel's own luma differs from the MMR's in general.
                        let y = 1023 - mmr_luma;
                        assert_eq!(
                            decoder.max_rgb_pq_composed(y, cpu).to_bits(),
                            kernel_max_rgb_composed(&params, y, kernel).to_bits(),
                            "{composer:?} pixel {y} with composed chroma of ({mmr_luma}, {cb}, {cr})"
                        );
                    }
                }
            }
        }
    }

    /// The kernels' spatial decode (pre-pass rectangle, down-sampling, taps, upsampling) equals
    /// the CPU's building blocks pixel for pixel, on even and odd frames, both chroma
    /// locations and crops at and away from the frame edge.
    #[test]
    fn dovi84_kernel_spatial_decode_matches_the_cpu() {
        for composer in Composer::ALL {
            let decoder = dovi84_decoder(composer);
            let params = build_dovi84_params(decoder);
            for (width, height) in [(10_i32, 6_i32), (9, 5)] {
                let (chroma_width, chroma_height) = ((width + 1) / 2, (height + 1) / 2);
                let mut state = 0x2545_f491_u32;
                let mut code = |range: u32| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    (state % range) as u16
                };
                let luma: Vec<u16> = (0..width * height).map(|_| 64 + code(877)).collect();
                let cb: Vec<u16> = (0..chroma_width * chroma_height)
                    .map(|_| 64 + code(897))
                    .collect();
                let cr: Vec<u16> = (0..chroma_width * chroma_height)
                    .map(|_| 64 + code(897))
                    .collect();
                for siting in [ChromaSiting::Left, ChromaSiting::TopLeft] {
                    let centred = siting == ChromaSiting::Left;
                    for (crop_x, crop_y, crop_width, crop_height) in
                        [(0, 0, width, height), (2, 2, width - 4, height - 3)]
                    {
                        // As launch_and_collect.
                        let cx0 = crop_x / 2;
                        let cy0 = (crop_y / 2 - 1).max(0);
                        let cx1 = ((crop_x + crop_width - 1) / 2 + 1).min(chroma_width - 1);
                        let cy1 = ((crop_y + crop_height - 1) / 2 + 1).min(chroma_height - 1);
                        let rect_width = cx1 + 1 - cx0;
                        let mut composed = Vec::new();
                        for cy in cy0..=cy1 {
                            for cx in cx0..=cx1 {
                                let index = (cy * chroma_width + cx) as usize;
                                let mmr_luma = kernel_downsample(&luma, width, height, cx, cy);
                                composed.extend(kernel_composed(
                                    &params, mmr_luma, cb[index], cr[index],
                                ));
                            }
                        }
                        let cpu_composed = |cx: usize, cy: usize| {
                            let mmr_luma = downsample_luma(
                                |x, y| luma[y * width as usize + x],
                                width as usize,
                                height as usize,
                                cx,
                                cy,
                            );
                            let index = cy * chroma_width as usize + cx;
                            decoder.composed_chroma(mmr_luma, &decoder.chroma(cb[index], cr[index]))
                        };
                        for y in crop_y..crop_y + crop_height {
                            for x in crop_x..crop_x + crop_width {
                                let pixel = luma[(y * width + x) as usize];
                                let cpu_chroma: [f32; 2] = std::array::from_fn(|k| {
                                    upsample_chroma(
                                        |cx, cy| cpu_composed(cx, cy)[k],
                                        chroma_width as usize,
                                        chroma_height as usize,
                                        siting,
                                        x as usize,
                                        y as usize,
                                    )
                                });
                                let kernel_chroma: [f32; 2] = std::array::from_fn(|k| {
                                    kernel_upsample(
                                        &composed,
                                        k as i32,
                                        (cx0, cy0, rect_width),
                                        chroma_width,
                                        chroma_height,
                                        centred,
                                        x,
                                        y,
                                    )
                                });
                                assert_eq!(
                                    decoder.max_rgb_pq_composed(pixel, cpu_chroma).to_bits(),
                                    kernel_max_rgb_composed(&params, pixel, kernel_chroma).to_bits(),
                                    "{composer:?} {width}x{height} {siting:?} crop ({crop_x}, {crop_y}) pixel ({x}, {y})"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn result_diff_hist_reads_the_words_after_the_sums() {
        let mut results = vec![0u32; RESULT_WORDS];
        // kernels.cu: results[DIFF_WORD + band * DIFF_BINS + difference]
        results[DIFF_WORD] = 7;
        results[DIFF_WORD + 3 * DIFF_BINS + 5] = 11;
        results[RESULT_WORDS - 1] = 13;
        let diff_hist = result_diff_hist(&results);
        assert_eq!(diff_hist[0][0], 7);
        assert_eq!(diff_hist[3][5], 11);
        assert_eq!(diff_hist[DIFF_VALUE_BANDS - 1][DIFF_BINS - 1], 13);
        let total: u32 = diff_hist.iter().flatten().sum();
        assert_eq!(total, 31);
        assert_eq!(DIFF_WORD, 4392);
        assert_eq!(RESULT_WORDS, 5416);
    }

    #[test]
    fn result_sums_read_the_kernels_u64_words() {
        let mut results = vec![0u32; RESULT_WORDS];
        let expected = [u64::MAX - 5, 1u64 << 32, 8_294_400];
        for (index, value) in expected.iter().enumerate() {
            let bytes = value.to_ne_bytes();
            let offset = SUMS_WORD + 2 * index;
            results[offset] = u32::from_ne_bytes(bytes[..4].try_into().unwrap());
            results[offset + 1] = u32::from_ne_bytes(bytes[4..].try_into().unwrap());
        }
        assert_eq!(result_sums(&results), expected);
    }

    #[test]
    fn f32_fixed_point_matches_the_former_f64_conversion() {
        // The kernel now computes truncate(x * 2^32) in f32; the scale is a power of two,
        // so the product is exact and equals the old f64 path for every x in [0, 1].
        let check = |x: f32| {
            let old = (f64::from(x) * 4_294_967_296.0) as u64;
            let new = (x * 4_294_967_296.0_f32) as u64;
            assert_eq!(old, new, "x = {x:e}");
        };
        for x in [0.0, f32::MIN_POSITIVE, 1.0e-30, 0.5, 0.999_999_94, 1.0] {
            check(x);
        }
        for lut in [
            build_transfer_lut(TransferFunction::Pq, Composer::Preset),
            build_transfer_lut(TransferFunction::Hlg, Composer::Preset),
            build_transfer_lut(TransferFunction::Hlg, Composer::Bt2100V1),
        ] {
            lut.into_iter().for_each(check);
        }
    }

    #[test]
    fn luminance_bin_lut_uses_exact_cpu_boundaries() {
        let bins = build_luminance_bin_lut(TransferFunction::Pq, Composer::Preset);
        let sdr_peak_pq = nits_to_pq(100.0);
        let sdr_step = sdr_peak_pq / 64.0;
        for (code, &bin) in bins.iter().enumerate() {
            let pq = pq_for_code(code as i32, TransferFunction::Pq, Composer::Preset);
            if pq < sdr_peak_pq {
                assert_eq!(usize::from(bin), (pq / sdr_step).floor() as usize);
            }
        }
    }
}
