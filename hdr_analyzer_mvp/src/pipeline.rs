use std::collections::VecDeque;
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

use ffmpeg_next::{codec, format, frame, software};

use crate::analysis::frame::analyze_native_frame_cropped;
use crate::analysis::histogram::{
    apply_histogram_ema, apply_histogram_temporal_median, select_peak_pq,
};
use crate::analysis::scene::{
    calculate_histogram_difference, convert_scene_cuts_to_scenes, cut_allowed,
};
use crate::cli::{Cli, PeakDomain};
use crate::crop::{detect_crop, is_frame_usable_for_crop, CropRect, CROP_EDGE_TOLERANCE};
use crate::ffmpeg_io::{probe_crop, setup_hardware_decoder, TransferFunction, VideoInfo};
use crate::optimizer::{run_optimizer_pass, OptimizerProfile};
use crate::writer::write_measurement_file;

pub fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}", minutes, seconds)
}

struct CropStabilityMonitor {
    checked_scenes: usize,
    matching_scenes: usize,
    full_frame_scenes: usize,
}

impl CropStabilityMonitor {
    fn new() -> Self {
        Self {
            checked_scenes: 1,
            matching_scenes: 1,
            full_frame_scenes: 0,
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

    fn report(&self) {
        let stability = self.matching_scenes as f64 * 100.0 / self.checked_scenes as f64;
        let other_scenes = self.checked_scenes - self.matching_scenes - self.full_frame_scenes;
        println!(
            "Crop stability: {:.0}% of {} sampled scenes matched the committed crop; {} appeared full-frame; {} used another active area.",
            stability,
            self.checked_scenes,
            self.full_frame_scenes,
            other_scenes
        );
        if self.matching_scenes != self.checked_scenes {
            println!(
                "Variable active area detected; measurements use one conservative stream-level crop. Per-scene crop application is not enabled."
            );
        }
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

    let (mut scenes, mut frames) = run_native_analysis_pipeline(
        cli,
        video_info,
        &mut input_context,
        peak_domain,
        downscale,
        initial_crop,
    )?;

    fix_scene_end_frames(&mut scenes, frames.len());

    // Apply histogram smoothing with scene-aware EMA reset (if enabled)
    if cli.hist_bin_ema_beta > 0.0 || cli.hist_temporal_median > 0 {
        apply_histogram_smoothing_pass(&scenes, &mut frames, cli, peak_domain)?;
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
            let input_path_obj = std::path::Path::new(
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
    peak_domain: PeakDomain,
    downscale: u32,
    initial_crop: Option<CropRect>,
) -> Result<(Vec<MadVRScene>, Vec<MadVRFrame>)> {
    println!("Starting native analysis pipeline...");
    let width = video_info.width;
    let height = video_info.height;
    let total_frames = video_info.total_frames;
    let transfer_function = video_info.transfer_function;

    let video_stream = input_context
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .context("No video stream found")?;
    let video_stream_index = video_stream.index();

    let mut decoder_context = codec::context::Context::from_parameters(video_stream.parameters())
        .context("Failed to create decoder context from stream parameters")?;

    // SAFETY: decoder_context is valid and as_mut_ptr() returns a valid mutable pointer.
    // Setting thread_count to 0 enables FFmpeg's automatic thread count selection,
    // which is a safe operation that only affects the decoder's threading behavior.
    unsafe {
        let ctx = decoder_context.as_mut_ptr();
        (*ctx).thread_count = 0;
    }

    let mut decoder = if let Some(hwaccel) = &cli.hwaccel {
        println!("Attempting to use hardware acceleration: {}", hwaccel);
        setup_hardware_decoder(decoder_context, hwaccel)?
    } else {
        decoder_context
            .decoder()
            .video()
            .context("Failed to create video decoder")?
    };

    let mut scaler: Option<software::scaling::Context> = None;
    let mut target_w = decoder.width();
    let mut target_h = decoder.height();
    if downscale > 1 {
        target_w = (target_w / downscale).max(2) & !1;
        target_h = (target_h / downscale).max(2) & !1;
    }
    let need_scaler = decoder.format() != format::Pixel::YUV420P10LE || downscale > 1;
    if need_scaler {
        scaler = Some(
            software::scaling::Context::get(
                decoder.format(),
                decoder.width(),
                decoder.height(),
                format::Pixel::YUV420P10LE,
                target_w,
                target_h,
                software::scaling::Flags::FAST_BILINEAR,
            )
            .context("Failed to create scaling context")?,
        );
    }

    let mut crop_rect_opt = if cli.no_crop {
        let rect = CropRect::full(target_w, target_h);
        println!(
            "\nCrop disabled: using full frame {}x{}",
            rect.width, rect.height
        );
        Some(rect)
    } else {
        initial_crop
    };
    let mut crop_monitor = crop_rect_opt
        .filter(|_| !cli.no_crop)
        .map(|_| CropStabilityMonitor::new());

    let mut frames = Vec::new();
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

    for (stream, packet) in input_context.packets() {
        if stream.index() == video_stream_index {
            decoder
                .send_packet(&packet)
                .context("Failed to send packet to decoder")?;

            let mut decoded_frame = frame::Video::empty();
            let mut scaled_frame = frame::Video::empty();
            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                // Determine if we should analyze this frame or use cached data
                let should_analyze =
                    frame_count % sample_rate == 0 || last_analyzed_frame.is_none();

                let analyzed_frame = if should_analyze {
                    let analysis_frame = if let Some(ref mut sc) = scaler {
                        sc.run(&decoded_frame, &mut scaled_frame)
                            .context("Failed to scale frame")?;
                        &scaled_frame
                    } else {
                        &decoded_frame
                    };

                    let rect = match crop_rect_opt {
                        Some(rect) => rect,
                        None if is_frame_usable_for_crop(analysis_frame) => {
                            let rect = detect_crop(analysis_frame);
                            println!(
                                "\nFallback active video area: {}x{} at offset ({}, {})",
                                rect.width, rect.height, rect.x, rect.y
                            );
                            crop_rect_opt = Some(rect);
                            crop_monitor = Some(CropStabilityMonitor::new());
                            rect
                        }
                        None => CropRect::full(analysis_frame.width(), analysis_frame.height()),
                    };

                    let analysis_start = if cli.profile_performance {
                        Some(Instant::now())
                    } else {
                        None
                    };
                    let frame_result = analyze_native_frame_cropped(
                        analysis_frame,
                        width,
                        height,
                        &rect,
                        &cli.pre_denoise,
                        transfer_function,
                        cli.hlg_peak_nits,
                        peak_domain,
                    )?;
                    if let Some(start) = analysis_start {
                        analysis_duration += start.elapsed();
                    }

                    // Scene detection on analyzed frames
                    if let Some(ref prev_hist) = previous_histogram {
                        let raw_diff =
                            compute_scene_diff(cli, &frame_result.lum_histogram, prev_hist);
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
                            if let Some(ref mut monitor) = crop_monitor {
                                monitor.record(
                                    rect,
                                    detect_crop(analysis_frame),
                                    analysis_frame.width(),
                                    analysis_frame.height(),
                                );
                            }
                        }
                    }
                    previous_histogram = Some(frame_result.lum_histogram.clone());
                    last_analyzed_frame = Some(copy_frame(&frame_result));
                    frame_result
                } else {
                    // Use cached frame data for skipped frames
                    copy_frame(last_analyzed_frame.as_ref().unwrap())
                };

                frames.push(analyzed_frame);
                frame_count += 1;

                // Update progress display
                pb.set_position(frame_count as u64);
            }
        }
    }

    decoder
        .send_eof()
        .context("Failed to send EOF to decoder")?;
    let mut decoded_frame = frame::Video::empty();
    let mut scaled_frame = frame::Video::empty();
    while decoder.receive_frame(&mut decoded_frame).is_ok() {
        // Determine if we should analyze this frame or use cached data
        let should_analyze = frame_count % sample_rate == 0 || last_analyzed_frame.is_none();

        let analyzed_frame = if should_analyze {
            let analysis_frame = if let Some(ref mut sc) = scaler {
                sc.run(&decoded_frame, &mut scaled_frame)
                    .context("Failed to scale final frame")?;
                &scaled_frame
            } else {
                &decoded_frame
            };

            let rect = match crop_rect_opt {
                Some(rect) => rect,
                None if is_frame_usable_for_crop(analysis_frame) => {
                    let rect = detect_crop(analysis_frame);
                    println!(
                        "\nFallback active video area: {}x{} at offset ({}, {})",
                        rect.width, rect.height, rect.x, rect.y
                    );
                    crop_rect_opt = Some(rect);
                    crop_monitor = Some(CropStabilityMonitor::new());
                    rect
                }
                None => CropRect::full(analysis_frame.width(), analysis_frame.height()),
            };

            let analysis_start = if cli.profile_performance {
                Some(Instant::now())
            } else {
                None
            };
            let frame_result = analyze_native_frame_cropped(
                analysis_frame,
                width,
                height,
                &rect,
                &cli.pre_denoise,
                transfer_function,
                cli.hlg_peak_nits,
                peak_domain,
            )?;
            if let Some(start) = analysis_start {
                analysis_duration += start.elapsed();
            }

            if let Some(ref prev_hist) = previous_histogram {
                let raw_diff = compute_scene_diff(cli, &frame_result.lum_histogram, prev_hist);
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
                    if let Some(ref mut monitor) = crop_monitor {
                        monitor.record(
                            rect,
                            detect_crop(analysis_frame),
                            analysis_frame.width(),
                            analysis_frame.height(),
                        );
                    }
                }
            }
            previous_histogram = Some(frame_result.lum_histogram.clone());
            last_analyzed_frame = Some(copy_frame(&frame_result));
            frame_result
        } else {
            copy_frame(last_analyzed_frame.as_ref().unwrap())
        };

        frames.push(analyzed_frame);
        frame_count += 1;
        pb.set_position(frame_count as u64);
    }

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

    Ok((scenes, frames))
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

fn apply_histogram_smoothing_pass(
    scenes: &[MadVRScene],
    frames: &mut [MadVRFrame],
    cli: &Cli,
    peak_domain: PeakDomain,
) -> Result<()> {
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

    // Process each scene independently to reset EMA at scene boundaries
    for scene in scenes {
        let start_idx = scene.start as usize;
        let end_idx = ((scene.end + 1) as usize).min(frames.len());

        if start_idx >= frames.len() || start_idx >= end_idx {
            continue;
        }

        // Reset EMA state at scene boundary
        let mut ema_state = vec![0.0; 256];
        let mut temporal_history: VecDeque<Vec<f64>> = VecDeque::with_capacity(temporal_window);

        for frame in frames.iter_mut().take(end_idx).skip(start_idx) {
            // Store original peak for reference
            let direct_max_pq = frame.peak_pq_2020;

            // Apply EMA smoothing
            if ema_beta > 0.0 {
                apply_histogram_ema(&mut frame.lum_histogram, &mut ema_state, ema_beta);
            }

            // Apply temporal median if enabled
            if temporal_window > 0 && !temporal_history.is_empty() {
                apply_histogram_temporal_median(
                    &mut frame.lum_histogram,
                    &temporal_history.iter().cloned().collect::<Vec<_>>(),
                );
            }

            // Update temporal history (keep last N-1 frames)
            if temporal_window > 0 {
                temporal_history.push_back(frame.lum_histogram.clone());
                if temporal_history.len() >= temporal_window {
                    temporal_history.pop_front();
                }
            }

            // Recompute peak based on peak_source
            frame.peak_pq_2020 = select_peak_pq(&frame.lum_histogram, direct_max_pq, peak_source);

            // Recompute avg_pq from smoothed histogram using v5 semantics
            let sdr_peak_pq = crate::analysis::histogram::nits_to_pq(100.0);
            let sdr_step = sdr_peak_pq / 64.0;
            let hdr_step = (1.0 - sdr_peak_pq) / 192.0;
            let sdr_mid = sdr_step + (sdr_step / 2.0);
            let hdr_mid = hdr_step + (hdr_step / 2.0);

            let mut avg_pq = 0.0f64;
            for (i, percent) in frame.lum_histogram.iter().enumerate() {
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
            let percent_sum: f64 = frame.lum_histogram.iter().sum();
            if percent_sum > 0.0 {
                avg_pq = (avg_pq * (100.0 / percent_sum)).min(1.0);
            }
            frame.avg_pq = avg_pq.min(1.0);
        }
    }

    println!(
        "Histogram smoothing completed. Peak source: {}",
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
