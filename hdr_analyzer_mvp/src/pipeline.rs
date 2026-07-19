use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use madvr_parse::{MadVRFrame, MadVRScene};

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

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

use ffmpeg_next::{format, frame, software};

use crate::analysis::frame::{analyze_native_frame_cropped, FrameAnalysisOptions, FramePeakStats};
use crate::analysis::gpu::GpuAnalyzer;
use crate::analysis::histogram::{
    apply_histogram_ema, apply_histogram_temporal_median, select_peak_pq,
};
use crate::analysis::scene::{
    calculate_histogram_difference, convert_scene_cuts_to_scenes, cut_allowed,
};
use crate::cli::{Cli, PeakDomain, PeakEstimator};
use crate::crop::{detect_crop, is_frame_usable_for_crop, CropRect, CROP_EDGE_TOLERANCE};
use crate::ffmpeg_io::{
    open_software_decoder, probe_crop, setup_hardware_decoder, transfer_hardware_frame,
    TransferFunction, VideoInfo,
};
use crate::l1_sidecar::{write_l1_sidecar, FrameL1Measurement};
use crate::optimizer::{run_optimizer_pass, OptimizerProfile};
use crate::writer::write_measurement_file;

pub fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}", minutes, seconds)
}

fn peak_estimator_name(estimator: PeakEstimator) -> &'static str {
    match estimator {
        PeakEstimator::Max => "max",
        PeakEstimator::Percentile => "percentile",
        PeakEstimator::Robust => "robust",
    }
}

fn write_frame_stats_csv(path: &Path, stats: &[FramePeakStats]) -> Result<()> {
    let file = File::create(path)
        .with_context(|| format!("Failed to create frame-stats CSV {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    writeln!(
        writer,
        "frame,selected_pq,raw_max_pq,percentile_pq,robust_pq,sigma_pq,correction_pq,n_eff"
    )?;

    for (frame_index, stat) in stats.iter().enumerate() {
        writeln!(
            writer,
            "{frame_index},{:.12},{:.12},{:.12},{:.12},{:.12},{:.12},{}",
            stat.selected_peak_pq,
            stat.raw_max_pq,
            stat.percentile_pq,
            stat.robust_pq,
            stat.sigma_pq,
            stat.correction_pq,
            stat.n_eff
        )?;
    }
    writer
        .flush()
        .with_context(|| format!("Failed to write frame-stats CSV {}", path.display()))
}

struct CropStabilityMonitor {
    checked_scenes: usize,
    matching_scenes: usize,
    full_frame_scenes: usize,
    skipped_scenes: usize,
}

impl CropStabilityMonitor {
    fn new() -> Self {
        Self {
            checked_scenes: 0,
            matching_scenes: 0,
            full_frame_scenes: 0,
            skipped_scenes: 0,
        }
    }

    fn record(
        &mut self,
        committed: CropRect,
        candidate: CropRect,
        frame_width: u32,
        frame_height: u32,
    ) {
        self.checked_scenes += 1;
        if candidate.approximately_matches(committed, CROP_EDGE_TOLERANCE) {
            self.matching_scenes += 1;
        } else if candidate.approximately_matches(
            CropRect::full(frame_width, frame_height),
            CROP_EDGE_TOLERANCE,
        ) {
            self.full_frame_scenes += 1;
        }
    }

    fn record_skipped(&mut self) {
        self.skipped_scenes += 1;
    }

    fn report(&self) {
        if self.checked_scenes == 0 {
            println!("Crop stability: no usable scene cuts were sampled.");
            return;
        }
        let stability = self.matching_scenes as f64 * 100.0 / self.checked_scenes as f64;
        let other_scenes = self.checked_scenes - self.matching_scenes - self.full_frame_scenes;
        println!(
            "Crop stability: {:.0}% of {} sampled scene cuts matched the committed crop; {} appeared full-frame; {} used another active area; {} skipped (low signal).",
            stability,
            self.checked_scenes,
            self.full_frame_scenes,
            other_scenes,
            self.skipped_scenes
        );
        if self.matching_scenes != self.checked_scenes {
            println!(
                "Variable active area detected; measurements use one conservative stream-level crop. Per-scene crop application is not enabled."
            );
        }
    }
}

/// Resolve the crop for the current frame, committing a hardened in-stream fallback
/// on the first usable frame when no probe-committed crop exists yet.
fn resolve_crop_rect(
    crop_rect_opt: &mut Option<CropRect>,
    crop_monitor: &mut Option<CropStabilityMonitor>,
    analysis_frame: &frame::Video,
) -> CropRect {
    match *crop_rect_opt {
        Some(rect) => rect,
        None if is_frame_usable_for_crop(analysis_frame) => {
            let rect = detect_crop(analysis_frame);
            println!(
                "\nFallback active video area: {}x{} at offset ({}, {})",
                rect.width, rect.height, rect.x, rect.y
            );
            *crop_rect_opt = Some(rect);
            *crop_monitor = Some(CropStabilityMonitor::new());
            rect
        }
        None => CropRect::full(analysis_frame.width(), analysis_frame.height()),
    }
}

/// Sample crop stability at an accepted scene cut. Cuts landing on black/low-signal
/// frames are counted as skipped instead of polluting the variable-AR telemetry.
fn sample_scene_cut_crop(
    crop_monitor: &mut Option<CropStabilityMonitor>,
    committed: CropRect,
    analysis_frame: &frame::Video,
) {
    let Some(monitor) = crop_monitor else {
        return;
    };
    if is_frame_usable_for_crop(analysis_frame) {
        monitor.record(
            committed,
            detect_crop(analysis_frame),
            analysis_frame.width(),
            analysis_frame.height(),
        );
    } else {
        monitor.record_skipped();
    }
}

fn effective_downscale(requested: u32) -> u32 {
    match requested {
        1 | 2 | 4 => requested,
        other => {
            eprintln!(
                "Unsupported --downscale value {}. Falling back to 1 (no downscale). Allowed: 1,2,4.",
                other
            );
            1
        }
    }
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

fn scale_rect(rect: CropRect, factor: u32) -> CropRect {
    CropRect {
        x: rect.x * factor,
        y: rect.y * factor,
        width: rect.width * factor,
        height: rect.height * factor,
    }
}

fn shrink_rect(rect: CropRect, factor: u32) -> CropRect {
    CropRect {
        x: rect.x / factor,
        y: rect.y / factor,
        width: (rect.width / factor).max(2),
        height: (rect.height / factor).max(2),
    }
}

/// This function orchestrates the complete HDR analysis pipeline using native FFmpeg
pub fn run(
    cli: &Cli,
    video_info: &VideoInfo,
    mut input_context: format::context::Input,
) -> Result<()> {
    let peak_domain = match video_info.transfer_function {
        TransferFunction::Hlg => {
            if cli.peak_domain == Some(PeakDomain::MaxRgb) {
                eprintln!("Warning: --peak-domain max-rgb is not supported for HLG; using luma.");
            }
            PeakDomain::Luma
        }
        TransferFunction::Pq | TransferFunction::Unknown => {
            cli.peak_domain.unwrap_or(PeakDomain::MaxRgb)
        }
    };

    match video_info.transfer_function {
        TransferFunction::Hlg => {
            println!(
                "Detected HLG transfer function. Using native HLG→PQ conversion (peak {:.0} nits).",
                cli.hlg_peak_nits
            );
        }
        TransferFunction::Unknown => {
            println!(
                "Transfer function unspecified; defaulting to PQ analysis path. Use --hlg-peak-nits if needed."
            );
        }
        TransferFunction::Pq => {}
    }

    println!(
        "Direct peak domain: {}",
        match peak_domain {
            PeakDomain::MaxRgb => "max-rgb",
            PeakDomain::Luma => "luma",
        }
    );
    println!(
        "Peak estimator: {}{}",
        peak_estimator_name(cli.peak_estimator),
        if cli.peak_estimator == PeakEstimator::Percentile {
            format!(" (P{})", cli.peak_percentile)
        } else {
            String::new()
        }
    );

    if cli.scene_metric.to_lowercase() == "hybrid" {
        println!("Scene metric: hybrid (prototype, using histogram-only for now)");
    }

    let downscale = effective_downscale(cli.downscale);
    let input_path = cli
        .input_positional
        .as_deref()
        .or(cli.input_flag.as_deref())
        .context("input path is required")?;
    let initial_crop = if cli.no_crop || cli.crop_probes == 0 {
        None
    } else {
        println!(
            "Probing active video area at {} positions...",
            cli.crop_probes
        );
        match probe_crop(input_path, cli.crop_probes, downscale) {
            Ok(vote) => {
                println!(
                    "Crop probe accepted {}/{} frames across {} crop mode(s); modal support {}/{}.",
                    vote.candidate_count,
                    cli.crop_probes,
                    vote.cluster_count,
                    vote.modal_count,
                    vote.candidate_count
                );
                println!(
                    "Committed active video area: {}x{} at offset ({}, {})",
                    vote.rect.width, vote.rect.height, vote.rect.x, vote.rect.y
                );
                if vote.variable_ar {
                    println!(
                        "Variable active area detected during probing; using the union of observed modes so full-frame picture is preserved."
                    );
                }
                Some(vote.rect)
            }
            Err(error) => {
                eprintln!(
                    "Crop probe unavailable ({error:#}); falling back to in-stream detection."
                );
                None
            }
        }
    };

    let (mut scenes, mut frames, mut l1_measurements, frame_peak_stats, crop) =
        run_native_analysis_pipeline(
            cli,
            video_info,
            &mut input_context,
            downscale,
            initial_crop,
            &FrameAnalysisOptions {
                denoise_mode: &cli.pre_denoise,
                transfer_function: video_info.transfer_function,
                hlg_peak_nits: cli.hlg_peak_nits,
                peak_domain,
                min_percentile: cli.min_percentile,
                peak_estimator: cli.peak_estimator,
                peak_percentile: cli.peak_percentile,
            },
        )?;

    if let Some(path) = &cli.dump_frame_stats {
        write_frame_stats_csv(path, &frame_peak_stats)?;
        println!("Wrote frame peak statistics: {}", path.display());
    }

    fix_scene_end_frames(&mut scenes, frames.len());

    // Apply histogram and both full-precision-average smoothing series with scene-aware resets.
    if cli.hist_bin_ema_beta > 0.0 || cli.hist_temporal_median > 0 {
        apply_histogram_smoothing_pass(
            &scenes,
            &mut frames,
            &mut l1_measurements,
            cli,
            peak_domain,
        )?;
    }

    precompute_scene_stats(&mut scenes, &frames);

    let optimizer_enabled = !cli.disable_optimizer;
    let mut selected_profile: Option<OptimizerProfile> = None;
    if optimizer_enabled {
        println!("Running intelligent optimizer pass...");
        let optimizer_profile = OptimizerProfile::from_name(&cli.optimizer_profile)?;
        run_optimizer_pass(&scenes, &mut frames, &optimizer_profile);
        selected_profile = Some(optimizer_profile);
    }

    // Optional post-optimization target_nits smoothing
    if optimizer_enabled && cli.target_smoother.to_lowercase() == "ema" {
        let alpha = cli.smoother_alpha.clamp(0.0, 1.0);
        let bidirectional = cli.smoother_bidirectional;
        let max_delta = selected_profile
            .map(|p| p.max_delta_per_frame)
            .unwrap_or(200);
        println!(
            "Applying target_nits EMA smoother (alpha={:.3}, bidirectional={})...",
            alpha, bidirectional
        );
        crate::optimizer::apply_target_smoother(
            &scenes,
            &mut frames,
            alpha,
            bidirectional,
            max_delta,
        );
        println!("Target_nits smoothing complete.");
    }

    let output_path = match &cli.output {
        Some(path) => path.clone(),
        None => {
            let input_path_obj = Path::new(
                cli.input_positional
                    .as_ref()
                    .unwrap_or(cli.input_flag.as_ref().unwrap()),
            );
            let stem = input_path_obj
                .file_stem()
                .context("Input file has no filename")?
                .to_str()
                .context("Invalid UTF-8 in filename")?;
            format!("{}_measurements.bin", stem)
        }
    };

    println!("Writing measurement file: {}", output_path);
    write_measurement_file(
        &output_path,
        &scenes,
        &frames,
        optimizer_enabled,
        cli.madvr_version as u32,
        cli.target_peak_nits,
        cli.header_peak_source.as_deref(),
    )?;
    let sidecar_path = write_l1_sidecar(
        Path::new(&output_path),
        &scenes,
        &frames,
        &l1_measurements,
        cli.min_percentile,
        &cli.pre_denoise,
        peak_domain,
        cli.peak_estimator,
        cli.peak_percentile,
        crop,
    )?;
    println!("Wrote L1 measurement sidecar: {}", sidecar_path.display());

    Ok(())
}

fn compute_scene_diff(cli: &Cli, curr_hist: &[f64], prev_hist: &[f64]) -> f64 {
    match cli.scene_metric.to_lowercase().as_str() {
        // Placeholder for future hybrid (histogram + flow). For now, use histogram difference.
        "hybrid" => calculate_histogram_difference(curr_hist, prev_hist),
        _ => calculate_histogram_difference(curr_hist, prev_hist),
    }
}

fn run_native_analysis_pipeline(
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
)> {
    println!("Starting native analysis pipeline...");
    let total_frames = video_info.total_frames;

    let video_stream = input_context
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .context("No video stream found")?;
    let video_stream_index = video_stream.index();
    let codec_parameters = video_stream.parameters();

    let cuda_requested = cli.hwaccel.as_deref() == Some("cuda");
    let gpu_block_reason = if !cuda_requested {
        None
    } else if analysis_options.denoise_mode == "median3" {
        Some("--pre-denoise median3 is CPU-only")
    } else if analysis_options.peak_estimator == PeakEstimator::Robust {
        Some("--peak-estimator robust is CPU-only (needs grain statistics)")
    } else {
        None
    };
    let mut gpu_analyzer = if cuda_requested && gpu_block_reason.is_none() {
        match GpuAnalyzer::new(analysis_options.transfer_function, cli.hlg_peak_nits) {
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
    let mut scene_cuts = Vec::new();
    let mut previous_histogram: Option<Vec<f64>> = None;
    let smoothing_window = cli.scene_smoothing as usize;
    let mut diff_window: VecDeque<f64> = VecDeque::with_capacity(smoothing_window.max(1));
    let mut last_cut_frame: u32 = 0;
    let mut frame_count = 0u32;
    let mut analysis_duration = Duration::ZERO;

    // Frame sampling configuration
    let sample_rate = cli.sample_rate.max(1);
    let mut last_analyzed_frame: Option<MadVRFrame> = None;
    let mut last_l1_measurement: Option<FrameL1Measurement> = None;
    let mut last_peak_stats: Option<FramePeakStats> = None;

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
            let transferred = if decoded_frame.format() == format::Pixel::CUDA {
                Some(transfer_hardware_frame(decoded_frame)?)
            } else {
                None
            };
            let host_frame = transferred.as_ref().unwrap_or(decoded_frame);

            let analysis_start = if cli.profile_performance {
                Some(Instant::now())
            } else {
                None
            };

            let mut gpu_output = None;
            let mut gpu_failed = false;
            if let Some(analyzer) = gpu_analyzer.as_mut() {
                let rect = resolve_crop_rect(&mut crop_rect_opt, &mut crop_monitor, host_frame);
                match analyzer.analyze(host_frame, &rect, downscale, analysis_options) {
                    Ok(result) => gpu_output = Some((result, rect)),
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

            let cut_sample_frame: &frame::Video = if used_gpu || !used_scaled {
                host_frame
            } else {
                &scaled_frame
            };

            // Scene detection on analyzed frames
            if let Some(ref prev_hist) = previous_histogram {
                let raw_diff =
                    compute_scene_diff(cli, &frame_result.frame.lum_histogram, prev_hist);
                let diff_for_threshold = if smoothing_window > 0 {
                    diff_window.push_back(raw_diff);
                    if diff_window.len() > smoothing_window {
                        diff_window.pop_front();
                    }
                    let sum: f64 = diff_window.iter().sum();
                    sum / (diff_window.len() as f64)
                } else {
                    raw_diff
                };

                if diff_for_threshold > cli.scene_threshold
                    && cut_allowed(Some(last_cut_frame), frame_count, cli.min_scene_length)
                {
                    scene_cuts.push(frame_count);
                    last_cut_frame = frame_count;
                    sample_scene_cut_crop(&mut crop_monitor, rect, cut_sample_frame);
                }
            }
            previous_histogram = Some(frame_result.frame.lum_histogram.clone());
            last_analyzed_frame = Some(copy_frame(&frame_result.frame));
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

    if let Some(monitor) = crop_monitor {
        monitor.report();
    }

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

    let crop = crop_rect_opt.unwrap_or_else(|| CropRect::full(target_w, target_h));
    Ok((scenes, frames, l1_measurements, frame_peak_stats, crop))
}

fn fix_scene_end_frames(scenes: &mut [MadVRScene], total_frames: usize) {
    if scenes.is_empty() || total_frames == 0 {
        return;
    }

    let last_frame_idx = (total_frames - 1) as u32;

    for scene in scenes.iter_mut() {
        if scene.end == u32::MAX {
            scene.end = last_frame_idx;
        }

        if scene.end >= total_frames as u32 {
            scene.end = last_frame_idx;
        }

        if scene.start >= total_frames as u32 {
            scene.start = last_frame_idx;
        }

        if scene.start > scene.end {
            scene.start = 0;
            scene.end = last_frame_idx;
        }
    }
}

fn smooth_average(
    current: f64,
    ema_state: &mut Option<f64>,
    history: &mut VecDeque<f64>,
    ema_beta: f64,
    temporal_window: usize,
) -> f64 {
    let mut smoothed = if ema_beta > 0.0 {
        let value = ema_state.map_or(current, |previous| {
            ema_beta * current + (1.0 - ema_beta) * previous
        });
        *ema_state = Some(value);
        value
    } else {
        current
    };

    if temporal_window > 0 {
        history.push_back(smoothed);
        if history.len() > temporal_window {
            history.pop_front();
        }
        let mut values: Vec<f64> = history.iter().copied().collect();
        values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        smoothed = values[values.len() / 2];
    }

    smoothed
}

fn apply_histogram_smoothing_pass(
    scenes: &[MadVRScene],
    frames: &mut [MadVRFrame],
    l1_measurements: &mut [FrameL1Measurement],
    cli: &Cli,
    peak_domain: PeakDomain,
) -> Result<()> {
    if frames.len() != l1_measurements.len() {
        anyhow::bail!(
            "L1 smoothing frame count mismatch: {} frames, {} measurements",
            frames.len(),
            l1_measurements.len()
        );
    }

    println!(
        "Applying histogram smoothing (EMA beta={}, temporal median window={})...",
        cli.hist_bin_ema_beta, cli.hist_temporal_median
    );

    let ema_beta = cli.hist_bin_ema_beta;
    let temporal_window = cli.hist_temporal_median;

    // A max-RGB domain defaults to its direct measurement. Histogram sources
    // remain available as explicit Y-based noise-robustness choices. The legacy
    // profile-dependent default applies only to the luma domain.
    let peak_source = cli.peak_source.as_deref().unwrap_or_else(|| {
        if peak_domain == PeakDomain::MaxRgb
            || cli.optimizer_profile.eq_ignore_ascii_case("conservative")
        {
            "max"
        } else {
            "histogram99"
        }
    });

    // Process each scene independently to reset all smoothing state at boundaries.
    for scene in scenes {
        let start_idx = scene.start as usize;
        let end_idx = ((scene.end + 1) as usize).min(frames.len());

        if start_idx >= frames.len() || start_idx >= end_idx {
            continue;
        }

        let mut ema_state = vec![0.0; 256];
        let mut temporal_history: VecDeque<Vec<f64>> = VecDeque::with_capacity(temporal_window);
        let mut luma_avg_ema_state: Option<f64> = None;
        let mut max_rgb_avg_ema_state: Option<f64> = None;
        let mut luma_avg_history: VecDeque<f64> = VecDeque::with_capacity(temporal_window);
        let mut max_rgb_avg_history: VecDeque<f64> = VecDeque::with_capacity(temporal_window);

        for (frame, l1_measurement) in frames[start_idx..end_idx]
            .iter_mut()
            .zip(l1_measurements[start_idx..end_idx].iter_mut())
        {
            let direct_max_pq = frame.peak_pq_2020;
            let direct_luma_avg_pq = frame.avg_pq;
            let direct_max_rgb_avg_pq = l1_measurement.avg_max_rgb_pq;

            if ema_beta > 0.0 {
                apply_histogram_ema(&mut frame.lum_histogram, &mut ema_state, ema_beta);
            }

            if temporal_window > 0 && !temporal_history.is_empty() {
                apply_histogram_temporal_median(
                    &mut frame.lum_histogram,
                    &temporal_history.iter().cloned().collect::<Vec<_>>(),
                );
            }

            if temporal_window > 0 {
                temporal_history.push_back(frame.lum_histogram.clone());
                if temporal_history.len() >= temporal_window {
                    temporal_history.pop_front();
                }
            }

            frame.peak_pq_2020 = select_peak_pq(&frame.lum_histogram, direct_max_pq, peak_source);

            // Smooth both true per-pixel average domains identically. The first
            // frame of every scene initializes each EMA without zero-state bias.
            frame.avg_pq = smooth_average(
                direct_luma_avg_pq,
                &mut luma_avg_ema_state,
                &mut luma_avg_history,
                ema_beta,
                temporal_window,
            );
            l1_measurement.avg_max_rgb_pq = smooth_average(
                direct_max_rgb_avg_pq,
                &mut max_rgb_avg_ema_state,
                &mut max_rgb_avg_history,
                ema_beta,
                temporal_window,
            );
        }
    }

    println!(
        "Histogram and average smoothing completed. Peak source: {}",
        peak_source
    );
    Ok(())
}

fn precompute_scene_stats(scenes: &mut [MadVRScene], frames: &[MadVRFrame]) {
    println!("Computing scene-based statistics...");

    for scene in scenes.iter_mut() {
        let start_idx = scene.start as usize;
        let end_idx = ((scene.end + 1) as usize).min(frames.len());

        if start_idx < frames.len() && start_idx < end_idx {
            let scene_frames = &frames[start_idx..end_idx];

            let total_avg_pq: f64 = scene_frames.iter().map(|f| f.avg_pq).sum();
            scene.avg_pq = total_avg_pq / scene_frames.len() as f64;

            let max_peak_pq = scene_frames
                .iter()
                .map(|f| f.peak_pq_2020)
                .fold(0.0f64, f64::max);
            scene.peak_nits = crate::analysis::histogram::pq_to_nits(max_peak_pq) as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn average_smoothing_is_identical_across_domains() {
        let mut luma_ema = None;
        let mut max_rgb_ema = None;
        let mut luma_history = VecDeque::new();
        let mut max_rgb_history = VecDeque::new();

        for value in [0.1, 0.4, 0.2, 0.8] {
            let luma = smooth_average(value, &mut luma_ema, &mut luma_history, 0.1, 3);
            let max_rgb = smooth_average(value, &mut max_rgb_ema, &mut max_rgb_history, 0.1, 3);
            assert_eq!(luma, max_rgb);
        }
    }

    #[test]
    fn average_smoothing_has_no_scene_initialization_bias() {
        let mut ema = None;
        let mut history = VecDeque::new();
        assert_eq!(smooth_average(0.75, &mut ema, &mut history, 0.1, 3), 0.75);
    }
}
