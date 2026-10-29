use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ffmpeg_next::{codec, format, frame, software};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use madvr_parse::{MadVRFrame, MadVRScene};

use super::{
    crop_to_full_resolution, detect_scene_cuts, max_rgb_dump_dir, resolve_crop_rect,
    sample_scene_cut_crop, scale_rect, shrink_rect, CropStabilityMonitor,
};
use crate::analysis::frame::{analyze_native_frame_cropped, FrameAnalysisOptions, FramePeakStats};
use crate::analysis::gpu::GpuAnalyzer;
use crate::analysis::scene::{
    calculate_histogram_difference, convert_scene_cuts_to_scenes, cut_allowed,
};
use crate::cli::Cli;
use crate::crop::CropRect;
use crate::ffmpeg_io::{
    codec_extradata, open_software_decoder, setup_hardware_decoder, transfer_hardware_frame,
    VideoInfo,
};
use crate::l1_sidecar::FrameL1Measurement;
use crate::leading_pictures::{hevc_framing, LeadingPictureScan};

/// Create a copy of a MadVRFrame (MadVRFrame doesn't implement Clone)
fn copy_frame(frame: &MadVRFrame) -> MadVRFrame {
    MadVRFrame {
        peak_pq_2020: frame.peak_pq_2020,
        peak_pq_dcip3: frame.peak_pq_dcip3,
        peak_pq_709: frame.peak_pq_709,
        lum_histogram: frame.lum_histogram.clone(),
        hue_histogram: frame.hue_histogram.clone(),
        target_nits: frame.target_nits,
        avg_pq: frame.avg_pq,
        target_pq: frame.target_pq,
    }
}

pub fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}", minutes, seconds)
}

/// The frame in host memory: `decoded` itself, or its NVDEC surface downloaded once into
/// `transferred` on first use. Never hand an `AV_PIX_FMT_CUDA` frame to host-side code.
pub(super) fn host_view<'a>(
    decoded: &'a frame::Video,
    transferred: &'a mut Option<frame::Video>,
) -> Result<&'a frame::Video> {
    if decoded.format() != format::Pixel::CUDA {
        return Ok(decoded);
    }
    if transferred.is_none() {
        *transferred = Some(transfer_hardware_frame(decoded)?);
    }
    Ok(transferred.as_ref().expect("transferred above"))
}

/// Convert (and optionally downscale) a decoded frame to YUV420P10LE, creating the
/// scaler lazily from the actual input frame format (hardware decoders only reveal
/// their output format once frames arrive).
fn scale_to_yuv420p10(
    input: &frame::Video,
    scaler: &mut Option<software::scaling::Context>,
    output: &mut frame::Video,
    downscale: u32,
) -> Result<()> {
    let target_width = ((input.width() / downscale.max(1)).max(2)) & !1;
    let target_height = ((input.height() / downscale.max(1)).max(2)) & !1;
    let needs_new = match scaler {
        Some(existing) => {
            existing.input().format != input.format()
                || existing.input().width != input.width()
                || existing.input().height != input.height()
        }
        None => true,
    };
    if needs_new {
        *scaler = Some(
            software::scaling::Context::get(
                input.format(),
                input.width(),
                input.height(),
                format::Pixel::YUV420P10LE,
                target_width,
                target_height,
                software::scaling::Flags::FAST_BILINEAR,
            )
            .context("Failed to create scaling context")?,
        );
        *output = frame::Video::empty();
    }
    scaler
        .as_mut()
        .expect("scaler initialized")
        .run(input, output)
        .context("Failed to convert analysis frame")
}

fn compute_scene_diff(cli: &Cli, curr_hist: &[f64], prev_hist: &[f64]) -> f64 {
    match cli.scene_metric.to_lowercase().as_str() {
        // Placeholder for future hybrid (histogram + flow). For now, use histogram difference.
        "hybrid" => calculate_histogram_difference(curr_hist, prev_hist),
        _ => calculate_histogram_difference(curr_hist, prev_hist),
    }
}

/// One file per analyzed frame, `frame_<index>_x<x>_y<y>_w<w>_h<h>.f32`: the crop rect's
/// max-RGB values (normalized PQ) as little-endian f32, row by row, in the analyzed frame's
/// coordinates.
fn write_max_rgb_dump(
    dir: &Path,
    frame_index: u32,
    rect: &CropRect,
    pixels: Option<&[f32]>,
) -> Result<()> {
    let Some(pixels) = pixels else {
        bail!(
            "HDR_ANALYZER_DUMP_MAX_RGB is set, but this frame's analysis kept no per-pixel values"
        );
    };
    std::fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    let path = dir.join(format!(
        "frame_{frame_index:06}_x{}_y{}_w{}_h{}.f32",
        rect.x, rect.y, rect.width, rect.height
    ));
    let bytes: Vec<u8> = pixels.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(&path, bytes).with_context(|| format!("Failed to write {}", path.display()))
}

#[expect(clippy::too_many_lines, reason = "E13.6: split by phase")]
pub(super) fn run_native_analysis_pipeline(
    cli: &Cli,
    video_info: &VideoInfo,
    input_context: &mut format::context::Input,
    downscale: u32,
    initial_crop: Option<CropRect>,
    analysis_options: &FrameAnalysisOptions<'_>,
) -> Result<(
    Vec<MadVRScene>,
    Vec<MadVRFrame>,
    Vec<FrameL1Measurement>,
    Vec<FramePeakStats>,
    CropRect,
    bool,
    StreamFrames,
)> {
    println!("Starting native analysis pipeline...");
    let total_frames = video_info.total_frames;

    let video_stream = input_context
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .context("No video stream found")?;
    let video_stream_index = video_stream.index();
    let codec_parameters = video_stream.parameters();
    let mut leading_scan = (codec_parameters.id() == codec::Id::HEVC)
        .then(|| LeadingPictureScan::new(hevc_framing(&codec_extradata(&codec_parameters))));
    let mut stream_pictures = 0u64;

    let cuda_requested = cli.hwaccel.as_deref() == Some("cuda");
    let gpu_block_reason = if !cuda_requested {
        None
    } else if analysis_options.denoise_mode == "median3" {
        Some("--pre-denoise median3 is CPU-only")
    } else {
        None
    };
    let mut gpu_analyzer = if cuda_requested && gpu_block_reason.is_none() {
        match GpuAnalyzer::new(
            analysis_options.transfer_function,
            analysis_options.hlg_composer,
        ) {
            Ok(analyzer) => {
                println!(
                    "CUDA analysis active (NVRTC kernel on full-resolution frames, {}x sampling stride)",
                    downscale
                );
                Some(analyzer)
            }
            Err(error) => {
                eprintln!("CUDA analysis unavailable ({error:#}); using CPU analysis");
                None
            }
        }
    } else {
        if let Some(reason) = gpu_block_reason {
            eprintln!("CUDA analysis disabled: {reason}; using CPU analysis");
        }
        None
    };

    let mut decoder = if let Some(hwaccel) = cli.hwaccel.as_deref() {
        println!("Attempting to use hardware acceleration: {hwaccel}");
        setup_hardware_decoder(&codec_parameters, hwaccel)?
    } else {
        open_software_decoder(&codec_parameters)?
    };

    let full_w = decoder.width();
    let full_h = decoder.height();
    let mut target_w = full_w;
    let mut target_h = full_h;
    if downscale > 1 {
        target_w = (target_w / downscale).max(2) & !1;
        target_h = (target_h / downscale).max(2) & !1;
    }

    // The GPU path analyzes full-resolution frames with a sampling stride, so its
    // crop rectangle lives in full-resolution coordinates; the CPU path keeps the
    // downscaled coordinate space of its converted analysis frames.
    let gpu_active_at_start = gpu_analyzer.is_some();
    let mut crop_rect_opt = if cli.no_crop {
        let rect = if gpu_active_at_start {
            CropRect::full(full_w, full_h)
        } else {
            CropRect::full(target_w, target_h)
        };
        println!(
            "\nCrop disabled: using full frame {}x{}",
            rect.width, rect.height
        );
        Some(rect)
    } else if gpu_active_at_start && downscale > 1 {
        initial_crop.map(|rect| scale_rect(rect, downscale))
    } else {
        initial_crop
    };
    let mut crop_monitor = crop_rect_opt
        .filter(|_| !cli.no_crop)
        .map(|_| CropStabilityMonitor::new());

    let mut cpu_scaler: Option<software::scaling::Context> = None;
    let mut scaled_frame = frame::Video::empty();

    let mut frames = Vec::new();
    let mut l1_measurements = Vec::new();
    let mut frame_peak_stats = Vec::new();
    let mut previous_histogram: Option<Vec<f64>> = None;
    let mut last_crop_sample_frame: u32 = 0;
    let mut frame_count = 0u32;
    let mut analysis_duration = Duration::ZERO;

    // Frame sampling configuration
    let sample_rate = cli.sample_rate.max(1);
    let mut last_analyzed_frame: Option<MadVRFrame> = None;
    let mut last_l1_measurement: Option<FrameL1Measurement> = None;
    let mut last_peak_stats: Option<FramePeakStats> = None;

    // In-place analysis of NVDEC frames (see GpuAnalyzer::analyze_device). The environment
    // override forces the host-download path, for parity checks against in-place analysis.
    let mut device_frames_blocked: Option<&'static str> =
        std::env::var_os("HDR_ANALYZER_CUDA_HOST_FRAMES")
            .map(|_| "HDR_ANALYZER_CUDA_HOST_FRAMES is set");
    let mut device_path_reported = false;
    let mut device_frames = 0u64;
    let mut host_downloads = 0u64;

    let start_time = Instant::now();

    // Create progress bar
    let pb = if let Some(total) = total_frames {
        let pb = ProgressBar::new(total as u64);
        pb.set_style(ProgressStyle::default_bar()
            .template("{spinner:.green} {msg} [{elapsed_precise}] [{bar:40.cyan/blue}] {percent}% ({pos}/{len}) {per_sec} ETA: {eta}")
            .unwrap()
            .progress_chars("=>-"));
        pb
    } else {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg} [{elapsed_precise}] {pos} frames {per_sec}")
                .unwrap(),
        );
        pb
    };
    pb.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));
    pb.set_message("Analyzing");

    if sample_rate > 1 {
        eprintln!(
            "Processing with {}x frame sampling (analyzing every {} frame)...",
            sample_rate,
            match sample_rate {
                2 => "2nd",
                3 => "3rd",
                _ => "Nth",
            }
        );
    } else {
        eprintln!("Processing frames with native pipeline...");
    }
    pb.set_position(0); // Show initial progress immediately

    let mut process_decoded = |decoded_frame: &frame::Video| -> Result<()> {
        // Determine if we should analyze this frame or use cached data
        let should_analyze = frame_count % sample_rate == 0 || last_analyzed_frame.is_none();

        let (analyzed_frame, l1_measurement, peak_stats) = if should_analyze {
            // NVDEC frames are downloaded only when the host needs their pixels.
            let mut transferred: Option<frame::Video> = None;

            let analysis_start = if cli.profile_performance {
                Some(Instant::now())
            } else {
                None
            };

            let mut gpu_output = None;
            let mut gpu_failed = false;
            if let Some(analyzer) = gpu_analyzer.as_mut() {
                let in_place = decoded_frame.format() == format::Pixel::CUDA
                    && match device_frames_blocked
                        .or_else(|| analyzer.device_frame_ineligibility(decoded_frame))
                    {
                        None => true,
                        Some(reason) => {
                            if !device_path_reported {
                                println!(
                                    "\nCUDA analysis downloads NVDEC frames to host memory: {reason}"
                                );
                                device_path_reported = true;
                            }
                            false
                        }
                    };
                if in_place && !device_path_reported {
                    println!("\nCUDA analysis on NVDEC device frames (no host round trip)");
                    device_path_reported = true;
                }
                let rect = match crop_rect_opt {
                    Some(rect) => rect,
                    None => resolve_crop_rect(
                        &mut crop_rect_opt,
                        &mut crop_monitor,
                        host_view(decoded_frame, &mut transferred)?,
                    ),
                };
                let result = if in_place {
                    device_frames += 1;
                    match analyzer.analyze_device(decoded_frame, &rect, downscale, analysis_options)
                    {
                        Err(error) if !analyzer.context_faulted() => {
                            // Rejected before any CUDA work, so the context is healthy and the
                            // frame can still be downloaded and analyzed from host memory.
                            eprintln!(
                                "\nNVDEC frame {frame_count} was not analyzed in place ({error:#}); downloading frames to host memory from now on"
                            );
                            device_frames_blocked = Some("in-place analysis was rejected");
                            let host = host_view(decoded_frame, &mut transferred)?;
                            analyzer.analyze(host, &rect, downscale, analysis_options)
                        }
                        other => other,
                    }
                } else {
                    let host = host_view(decoded_frame, &mut transferred)?;
                    analyzer.analyze(host, &rect, downscale, analysis_options)
                };
                match result {
                    Ok(result) => gpu_output = Some((result, rect)),
                    // A failed CUDA call can leave the context the NVDEC decoder shares unusable,
                    // and FFmpeg's CUDA download can then report success for a failed copy. No
                    // later frame can be trusted, so stop instead of falling back.
                    Err(error)
                        if analyzer.context_faulted()
                            && decoded_frame.format() == format::Pixel::CUDA =>
                    {
                        return Err(error.context(format!(
                            "CUDA analysis failed at frame {frame_count}; the NVDEC decoder shares this CUDA context, so the run stops. Rerun with --hwaccel none to analyze on the CPU"
                        )));
                    }
                    Err(error) => {
                        eprintln!(
                            "\nCUDA analysis failed at frame {frame_count} ({error:#}); switching to CPU analysis"
                        );
                        gpu_failed = true;
                    }
                }
            }
            if gpu_failed {
                gpu_analyzer = None;
                if downscale > 1 {
                    // Re-map the committed crop into the CPU path's downscaled space.
                    crop_rect_opt = crop_rect_opt.map(|rect| shrink_rect(rect, downscale));
                }
            }

            let used_gpu = gpu_output.is_some();
            let mut used_scaled = false;
            let (frame_result, rect) = if let Some((result, rect)) = gpu_output {
                (result, rect)
            } else {
                let host_frame = host_view(decoded_frame, &mut transferred)?;
                let needs_scaling =
                    host_frame.format() != format::Pixel::YUV420P10LE || downscale > 1;
                let analysis_frame: &frame::Video = if needs_scaling {
                    scale_to_yuv420p10(host_frame, &mut cpu_scaler, &mut scaled_frame, downscale)?;
                    used_scaled = true;
                    &scaled_frame
                } else {
                    host_frame
                };
                let rect = resolve_crop_rect(&mut crop_rect_opt, &mut crop_monitor, analysis_frame);
                (
                    analyze_native_frame_cropped(analysis_frame, &rect, analysis_options)?,
                    rect,
                )
            };
            if let Some(start) = analysis_start {
                analysis_duration += start.elapsed();
            }

            // Scene boundaries are chosen after the loop (`detect_scene_cuts`). Here a large
            // frame-to-frame distance only marks a likely scene change at which the crop
            // monitor takes a sample. Every final cut exceeds this distance too, but the
            // samples are rate-limited, so they are telemetry, not one per boundary.
            if let Some(ref prev_hist) = previous_histogram {
                let raw_diff =
                    compute_scene_diff(cli, &frame_result.frame.lum_histogram, prev_hist);
                // Without crop monitoring (--no-crop) nothing reads the pixels, so an NVDEC
                // frame is not downloaded.
                if crop_monitor.is_some()
                    && raw_diff > cli.scene_threshold
                    && cut_allowed(
                        Some(last_crop_sample_frame),
                        frame_count,
                        cli.min_scene_length,
                    )
                {
                    last_crop_sample_frame = frame_count;
                    let cut_sample_frame: &frame::Video = if used_gpu || !used_scaled {
                        host_view(decoded_frame, &mut transferred)?
                    } else {
                        &scaled_frame
                    };
                    sample_scene_cut_crop(&mut crop_monitor, rect, cut_sample_frame);
                }
            }
            if transferred.is_some() {
                host_downloads += 1;
            }
            previous_histogram = Some(frame_result.frame.lum_histogram.clone());
            last_analyzed_frame = Some(copy_frame(&frame_result.frame));
            if let Some(dir) = max_rgb_dump_dir() {
                write_max_rgb_dump(
                    &dir,
                    frame_count,
                    &rect,
                    frame_result.max_rgb_pixels.as_deref(),
                )?;
            }
            last_l1_measurement = Some(frame_result.l1);
            last_peak_stats = Some(frame_result.peak_stats);
            (frame_result.frame, frame_result.l1, frame_result.peak_stats)
        } else {
            // Use cached frame data for skipped frames
            (
                copy_frame(last_analyzed_frame.as_ref().unwrap()),
                last_l1_measurement.unwrap(),
                last_peak_stats.unwrap(),
            )
        };

        frames.push(analyzed_frame);
        l1_measurements.push(l1_measurement);
        frame_peak_stats.push(peak_stats);
        frame_count += 1;

        // Update progress display
        pb.set_position(frame_count as u64);
        Ok(())
    };

    for (stream, packet) in input_context.packets() {
        if stream.index() == video_stream_index {
            let data = packet.data().unwrap_or_default();
            // For HEVC only packets with a slice are pictures (not an end-of-sequence packet).
            let picture = match leading_scan.as_mut() {
                Some(scan) => scan.observe_packet(data),
                None => !data.is_empty(),
            };
            stream_pictures += u64::from(picture);
            decoder
                .send_packet(&packet)
                .context("Failed to send packet to decoder")?;

            let mut decoded_frame = frame::Video::empty();
            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                process_decoded(&decoded_frame)?;
            }
        }
    }

    decoder
        .send_eof()
        .context("Failed to send EOF to decoder")?;
    let mut decoded_frame = frame::Video::empty();
    while decoder.receive_frame(&mut decoded_frame).is_ok() {
        process_decoded(&decoded_frame)?;
    }
    drop(process_decoded);

    // Finalize progress display
    pb.finish_with_message("Complete");

    let stream_frames = account_stream_frames(
        stream_pictures,
        u64::from(frame_count),
        leading_scan.as_ref(),
    )?;

    if device_frames > 0 {
        println!(
            "CUDA in-place analysis: {device_frames} NVDEC frames; {host_downloads} downloaded to host memory (crop sampling or fallback)"
        );
    }

    if let Some(monitor) = crop_monitor {
        monitor.report();
    }

    let (scene_cuts, _, _) = detect_scene_cuts(
        &frames,
        cli.sample_rate,
        cli.scene_threshold,
        cli.min_scene_length,
    );
    let scenes = convert_scene_cuts_to_scenes(scene_cuts, frame_count);
    println!(
        "Scene detection completed: {} scenes detected",
        scenes.len()
    );

    if cli.profile_performance {
        let total_elapsed = start_time.elapsed();
        let analysis_secs = analysis_duration.as_secs_f64();
        let analysis_fps = if analysis_secs > 0.0 {
            frame_count as f64 / analysis_secs
        } else {
            0.0
        };
        let decode_duration = total_elapsed.saturating_sub(analysis_duration);
        let decode_secs = decode_duration.as_secs_f64();
        let decode_fps = if decode_secs > 0.0 {
            frame_count as f64 / decode_secs
        } else {
            0.0
        };

        println!("Rayon analysis threads: {}", rayon::current_num_threads());
        println!(
            "Decode & IO wall time: {} ({:.1} fps effective)",
            format_duration(decode_duration),
            decode_fps
        );
        println!(
            "Analysis wall time: {} ({:.1} fps effective)",
            format_duration(analysis_duration),
            analysis_fps
        );
    }

    // The GPU path keeps its crop in full-resolution coordinates; the CPU path (including a
    // mid-run fallback, which shrinks the rect) keeps downscaled analysis coordinates.
    let gpu_active = gpu_analyzer.is_some();
    let crop = if gpu_active {
        crop_rect_opt.unwrap_or_else(|| CropRect::full(full_w, full_h))
    } else {
        crop_to_full_resolution(
            crop_rect_opt.unwrap_or_else(|| CropRect::full(target_w, target_h)),
            downscale,
            (target_w, target_h),
            (full_w, full_h),
        )
    };
    Ok((
        scenes,
        frames,
        l1_measurements,
        frame_peak_stats,
        crop,
        gpu_active,
        stream_frames,
    ))
}

/// Pictures of the video stream against the frames the decoder output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StreamFrames {
    /// Access units (coded pictures) in the video stream.
    pub stream: u64,
    /// RASL pictures at the start that no decoder outputs; measured frame `i` is stream frame
    /// `i + leading_skipped` in presentation order.
    pub leading_skipped: u64,
}

/// Every picture of the stream must be either measured or one of the explained undecodable
/// leading pictures; any other loss would shift per-frame measurements against the video.
pub(super) fn account_stream_frames(
    stream_pictures: u64,
    decoded: u64,
    leading_scan: Option<&LeadingPictureScan>,
) -> Result<StreamFrames> {
    let leading_skipped = leading_scan.map_or(0, LeadingPictureScan::skipped);
    if decoded + leading_skipped != stream_pictures {
        let start = match leading_scan {
            Some(scan) if !scan.starts_with_irap() => {
                " The stream does not start with a random access picture (IDR/CRA/BLA): it was cut in the middle of a GOP."
            }
            _ => "",
        };
        anyhow::bail!(
            "The decoder output {decoded} frames for {stream_pictures} pictures in the video stream, and only {leading_skipped} of the missing pictures are undecodable leading (RASL) pictures at the start.{start} Measurements would not line up with the video frames, so none are written."
        );
    }
    if leading_skipped > 0 {
        println!(
            "The stream starts with {leading_skipped} undecodable leading (RASL) picture(s) (an open-GOP cut at a CRA picture); measured frame 0 is stream frame {leading_skipped}."
        );
    }
    Ok(StreamFrames {
        stream: stream_pictures,
        leading_skipped,
    })
}
