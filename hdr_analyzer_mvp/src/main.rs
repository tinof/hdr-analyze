use anyhow::{Context, Result};
use clap::Parser;
use madvr_parse::{MadVRFrame, MadVRHeader, MadVRMeasurements, MadVRScene};
use std::collections::VecDeque;
use std::io::Write;
use std::time::{Duration, Instant};

// Native FFmpeg imports
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{codec, format, frame, media, software};

mod crop;
use crop::CropRect;

// --- Command Line Interface ---
#[derive(Parser)]
#[command(name = "hdr_analyzer_mvp")]
#[command(about = "HDR10 to Dolby Vision converter - Phase 1 MVP")]
struct Cli {
    /// Path to the input video file
    input: String,

    /// Path for the output .bin measurement file (optional - auto-generates from input filename if not provided)
    #[arg(short, long)]
    output: Option<String>,

    /// (Phase 3) Enable intelligent optimizer to generate dynamic target nits
    #[arg(long)]
    enable_optimizer: bool,

    /// (Optional) Enable GPU hardware acceleration.
    /// Examples: "cuda" (for NVIDIA), "vaapi" (for Linux/AMD/Intel), "videotoolbox" (for macOS).
    #[arg(long)]
    hwaccel: Option<String>,

    /// madVR measurement file version to write (5 or 6). Default: 5
    #[arg(long, default_value_t = 5)]
    madvr_version: u8,

    /// Scene detection threshold (distance metric). Default: 0.3
    #[arg(long, default_value_t = 0.3)]
    scene_threshold: f64,

    /// Minimum scene length in frames. Cuts closer than this are dropped. Default: 24
    #[arg(long, default_value_t = 24)]
    min_scene_length: u32,

    /// Optional smoothing window (in frames) over the scene-change metric. 0 disables smoothing.
    #[arg(long, default_value_t = 0)]
    scene_smoothing: u32,

    /// Optional override for header.target_peak_nits (used for v6). If omitted, defaults to computed maxCLL.
    #[arg(long)]
    target_peak_nits: Option<u32>,

    /// Downscale factor for analysis to improve throughput (1=full, 2=half, 4=quarter)
    /// Only affects internal analysis resolution. Output statistics remain comparable.
    #[arg(long, default_value_t = 1)]
    downscale: u32,

    /// Disable active-area crop detection (analyze full frame). Useful for diagnostics/validation.
    #[arg(long)]
    no_crop: bool,
}

// --- Data Structures ---
// Using official MadVRScene and MadVRFrame structs from madvr_parse crate

// --- Constants for PQ Conversion ---
const ST2084_Y_MAX: f64 = 10000.0;
const ST2084_M1: f64 = 2610.0 / 16384.0;
const ST2084_M2: f64 = (2523.0 / 4096.0) * 128.0;
const ST2084_C1: f64 = 3424.0 / 4096.0;
const ST2084_C2: f64 = (2413.0 / 4096.0) * 32.0;
const ST2084_C3: f64 = (2392.0 / 4096.0) * 32.0;

/* --- Formulas --- */
#[inline]
fn nits_to_pq(nits: f64) -> f64 {
    let y = (nits / ST2084_Y_MAX).max(0.0);
    ((ST2084_C1 + ST2084_C2 * y.powf(ST2084_M1)) / (1.0 + ST2084_C3 * y.powf(ST2084_M1)))
        .powf(ST2084_M2)
}

/// Calculate average PQ from histogram data.
///
/// The histogram represents PQ values directly, with each bin corresponding to a PQ range.
/// This function computes a weighted average where each bin's contribution is proportional
/// to the percentage of pixels it contains.
///
/// # Arguments
/// * `histogram` - Array of 256 values representing pixel percentages for each PQ bin
///
/// # Returns
/// Weighted average PQ value in range [0.0, 1.0]
#[allow(dead_code)]
fn calculate_avg_pq_from_histogram(histogram: &[f64]) -> f64 {
    let mut weighted_sum = 0.0;
    let mut total_weight = 0.0;

    for (bin_index, &weight) in histogram.iter().enumerate() {
        if weight > 0.0 {
            // Convert bin index back to PQ value
            // Each bin represents a PQ range from 0.0 to 1.0
            let pq_value = (bin_index as f64) / 255.0;

            weighted_sum += pq_value * weight;
            total_weight += weight;
        }
    }

    if total_weight > 0.0 {
        weighted_sum / total_weight
    } else {
        0.0
    }
}

/// Convert PQ value back to nits (inverse PQ function).
///
/// This function implements the inverse ST.2084 EOTF to convert PQ code values
/// back to absolute luminance values in nits.
///
/// # Arguments
/// * `pq` - PQ value in range [0.0, 1.0]
///
/// # Returns
/// Luminance value in nits (cd/m²)
fn pq_to_nits(pq: f64) -> f64 {
    if pq <= 0.0 {
        return 0.0;
    }

    let y = ((pq.powf(1.0 / ST2084_M2) - ST2084_C1).max(0.0)
        / (ST2084_C2 - ST2084_C3 * pq.powf(1.0 / ST2084_M2)))
    .powf(1.0 / ST2084_M1);
    y * ST2084_Y_MAX
}

/// Find the 99th percentile (highlight knee) from the luminance histogram.
///
/// This function identifies the luminance level below which 99% of pixels fall,
/// which is useful for determining appropriate tone mapping targets while
/// preserving highlight detail.
///
/// # Arguments
/// * `histogram` - Array of 256 values representing pixel percentages for each PQ bin
///
/// # Returns
/// Luminance value in nits representing the 99th percentile
fn find_highlight_knee_nits(histogram: &[f64]) -> f64 {
    let mut cumulative_percentage = 0.0;

    // Start from the highest bin and work backwards
    for (bin_index, &percentage) in histogram.iter().enumerate().rev() {
        cumulative_percentage += percentage;

        // When we reach 1% (99th percentile), this is our highlight knee
        if cumulative_percentage >= 1.0 {
            // Convert bin index back to approximate nits value
            let pq_value = (bin_index as f64) / 255.0;
            return pq_to_nits(pq_value);
        }
    }

    // Fallback if no significant highlights found
    1000.0
}

// create_luminance_to_bin_lut function removed - no longer needed in native pipeline

/// Format duration as MM:SS for user-friendly display.
///
/// # Arguments
/// * `duration` - Duration to format
///
/// # Returns
/// String in format "MM:SS" (e.g., "03:45" for 3 minutes 45 seconds)
fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}", minutes, seconds)
}

/// Main entry point for the HDR analyzer with native FFmpeg pipeline.
///
/// This function orchestrates the complete HDR analysis pipeline using native FFmpeg:
/// 1. Initializes native video processing with ffmpeg-next
/// 2. Performs direct scene detection and frame analysis with accurate 10-bit PQ mapping
/// 3. Generates PQ histograms and peak/average values from native frame data
/// 4. Optionally runs the advanced optimizer to generate dynamic target nits
/// 5. Writes the results to a madVR-compatible .bin measurement file
///
/// # Returns
/// `Result<()>` - Ok(()) on success, Err on any failure
fn main() -> Result<()> {
    let cli = Cli::parse();

    // Auto-generate output filename if not provided
    let output_path = match &cli.output {
        Some(path) => path.clone(),
        None => {
            let input_path = std::path::Path::new(&cli.input);
            let stem = input_path
                .file_stem()
                .context("Input file has no filename")?
                .to_str()
                .context("Invalid UTF-8 in filename")?;
            format!("{}_measurements.bin", stem)
        }
    };

    println!(
        "HDR Analyzer MVP (Native Pipeline) - Starting analysis of: {}",
        cli.input
    );

    // Step 1: Get video info using native FFmpeg
    let (width, height, total_frames, input_context) = get_native_video_info(&cli.input)?;
    println!("Video resolution: {}x{}", width, height);
    if let Some(frames) = total_frames {
        println!("Total frames: {}", frames);
    }

    // Step 2: Native scene detection and frame analysis
    println!("Starting native analysis pipeline...");
    let (mut scenes, mut frames) =
        run_native_analysis_pipeline(&cli, width, height, total_frames, input_context)?;
    println!(
        "Detected {} scenes and analyzed {} frames",
        scenes.len(),
        frames.len()
    );

    // Step 3: Fix scene end frames and compute scene statistics
    fix_scene_end_frames(&mut scenes, frames.len());
    precompute_scene_stats(&mut scenes, &frames);

    // Step 4: Run advanced optimizer if enabled
    if cli.enable_optimizer {
        println!("Running intelligent optimizer pass...");
        run_optimizer_pass(&scenes, &mut frames);
    }

    // Step 5: Assemble and write the .bin file
    println!("Writing measurement file: {}", output_path);
    write_measurement_file(
        &output_path,
        &scenes,
        &frames,
        cli.enable_optimizer,
        cli.madvr_version as u32,
        cli.target_peak_nits,
    )?;

    println!("Native analysis complete!");
    Ok(())
}

/// Native video information extraction using ffmpeg-next.
///
/// This function replaces the external ffprobe process with native FFmpeg library calls
/// to extract essential video metadata needed for analysis.
///
/// # Arguments
/// * `input_path` - Path to the input video file
///
/// # Returns
/// `Result<(u32, u32, Option<u32>, format::context::Input)>` - (width, height, optional_frame_count, input_context)
fn get_native_video_info(
    input_path: &str,
) -> Result<(u32, u32, Option<u32>, format::context::Input)> {
    // Initialize FFmpeg
    ffmpeg::init().context("Failed to initialize FFmpeg")?;

    // Open input file
    let input_context = format::input(input_path).context("Failed to open input video file")?;

    // Find the best video stream
    let video_stream = input_context
        .streams()
        .best(media::Type::Video)
        .context("No video stream found in input file")?;

    let video_params = video_stream.parameters();

    // Extract width and height from video parameters
    let (width, height) = match video_params.medium() {
        media::Type::Video => {
            // Get decoder context to access width/height
            let decoder_context = codec::context::Context::from_parameters(video_params)
                .context("Failed to create decoder context")?;
            let decoder = decoder_context
                .decoder()
                .video()
                .context("Failed to create video decoder")?;
            (decoder.width(), decoder.height())
        }
        _ => return Err(anyhow::anyhow!("Stream is not a video stream")),
    };

    // Try to estimate frame count from duration and frame rate
    let frame_count = if video_stream.duration() != ffmpeg::ffi::AV_NOPTS_VALUE {
        let duration = video_stream.duration();
        let time_base = video_stream.time_base();
        let avg_frame_rate = video_stream.avg_frame_rate();

        if avg_frame_rate.numerator() > 0 && avg_frame_rate.denominator() > 0 {
            let duration_seconds = (duration as f64) * f64::from(time_base);
            let fps = avg_frame_rate.numerator() as f64 / avg_frame_rate.denominator() as f64;
            Some((duration_seconds * fps) as u32)
        } else {
            None
        }
    } else {
        None
    };

    println!("Native video info: {}x{}", width, height);
    if let Some(frames) = frame_count {
        println!("Estimated frames: {}", frames);
    }

    Ok((width, height, frame_count, input_context))
}

/// Native analysis pipeline using ffmpeg-next for direct video processing.
///
/// This function replaces the external ffmpeg process with native FFmpeg library calls
/// to perform both scene detection and frame analysis. It provides direct access to
/// high-bit-depth video data for accurate luminance mapping and histogram-based scene detection.
///
/// # Arguments
/// * `cli` - Command line interface containing input path and hardware acceleration settings
/// * `width` - Video width in pixels
/// * `height` - Video height in pixels
/// * `total_frames` - Optional total frame count for progress tracking
/// * `input_context` - Native FFmpeg input context
///
/// # Returns
/// `Result<(Vec<MadVRScene>, Vec<MadVRFrame>)>` - Tuple of detected scenes and analyzed frames
fn run_native_analysis_pipeline(
    cli: &Cli,
    width: u32,
    height: u32,
    total_frames: Option<u32>,
    mut input_context: format::context::Input,
) -> Result<(Vec<MadVRScene>, Vec<MadVRFrame>)> {
    println!("Starting native analysis pipeline...");

    // Find the best video stream
    let video_stream = input_context
        .streams()
        .best(media::Type::Video)
        .context("No video stream found")?;
    let video_stream_index = video_stream.index();

    // Set up decoder with hardware acceleration if requested
    let mut decoder_context = codec::context::Context::from_parameters(video_stream.parameters())
        .context("Failed to create decoder context from stream parameters")?;

    // Enable FFmpeg multi-threading (auto thread count). Set before opening the decoder.
    unsafe {
        let ctx = decoder_context.as_mut_ptr();
        // 0 means auto (FFmpeg chooses based on cores and codec)
        (*ctx).thread_count = 0;
        // Leave thread_type to codec default (HEVC typically uses frame threading).
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

    // Prepare optional scaler for consistent 10-bit format analysis and optional downscaling
    let mut scaler: Option<software::scaling::Context> = None;
    // Validate downscale factor
    let downscale = match cli.downscale {
        1 | 2 | 4 => cli.downscale,
        other => {
            eprintln!(
                "Unsupported --downscale value {}. Falling back to 1 (no downscale). Allowed: 1,2,4.",
                other
            );
            1
        }
    };
    // Ensure target dims remain >=2 and even when downscaling
    let mut target_w = decoder.width();
    let mut target_h = decoder.height();
    if downscale > 1 {
        target_w = (target_w / downscale).max(2) & !1;
        target_h = (target_h / downscale).max(2) & !1;
    }
    // Only build a scaler when needed: pixfmt mismatch or downscale requested
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

    // Initialize analysis data structures
    let mut frames = Vec::new();
    let mut scene_cuts = Vec::new();
    let mut previous_histogram: Option<Vec<f64>> = None;
    // Scene smoothing state
    let smoothing_window = cli.scene_smoothing as usize;
    let mut diff_window: VecDeque<f64> = VecDeque::with_capacity(smoothing_window.max(1));
    // Min scene length guard state
    let mut last_cut_frame: u32 = 0; // start of video acts as implicit cut
    let mut frame_count = 0u32;
    let mut crop_rect_opt: Option<CropRect> = None;

    // Progress tracking
    let start_time = Instant::now();
    let mut last_progress_update = Instant::now();
    let progress_update_interval = Duration::from_millis(500);

    println!("Processing frames with native pipeline...");
    print!("Initializing frame analysis...");
    std::io::stdout().flush().unwrap_or(());

    // Main processing loop
    for (stream, packet) in input_context.packets() {
        if stream.index() == video_stream_index {
            decoder
                .send_packet(&packet)
                .context("Failed to send packet to decoder")?;

            // Receive and process decoded frames
            let mut decoded_frame = frame::Video::empty();
            let mut scaled_frame = frame::Video::empty(); // reused buffer
            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                // Convert only if needed, else analyze decoded_frame directly
                let analysis_frame = if let Some(ref mut sc) = scaler {
                    sc.run(&decoded_frame, &mut scaled_frame)
                        .context("Failed to scale frame")?;
                    &scaled_frame
                } else {
                    &decoded_frame
                };

                // Analyze the native frame within active area (detect crop once unless disabled)
                if crop_rect_opt.is_none() {
                    if cli.no_crop {
                        let rect = CropRect::full(analysis_frame.width(), analysis_frame.height());
                        println!(
                            "\nCrop disabled: using full frame {}x{}",
                            rect.width, rect.height
                        );
                        crop_rect_opt = Some(rect);
                    } else {
                        let rect = crop::detect_crop(analysis_frame);
                        println!(
                            "\nDetected active video area: {}x{} at offset ({}, {})",
                            rect.width, rect.height, rect.x, rect.y
                        );
                        crop_rect_opt = Some(rect);
                    }
                }
                let rect = crop_rect_opt.as_ref().unwrap();
                let analyzed_frame =
                    analyze_native_frame_cropped(analysis_frame, width, height, rect)?;

                // Scene detection using histogram comparison
                if let Some(ref prev_hist) = previous_histogram {
                    let raw_diff =
                        calculate_histogram_difference(&analyzed_frame.lum_histogram, prev_hist);
                    // Apply optional smoothing over the diff signal
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
                    }
                }
                previous_histogram = Some(analyzed_frame.lum_histogram.clone());

                frames.push(analyzed_frame);
                frame_count += 1;

                // Update progress display periodically
                let now = Instant::now();
                if now.duration_since(last_progress_update) >= progress_update_interval
                    || frame_count == 1
                {
                    last_progress_update = now;

                    let elapsed = now.duration_since(start_time);
                    let fps = if elapsed.as_secs_f64() > 0.0 {
                        frame_count as f64 / elapsed.as_secs_f64()
                    } else {
                        0.0
                    };

                    if let Some(total) = total_frames {
                        let progress = (frame_count as f64 / total as f64) * 100.0;
                        print!(
                            "\rProcessing: {}/{} frames ({:.1}%) at {:.1} fps",
                            frame_count, total, progress, fps
                        );
                    } else {
                        print!("\rProcessing: {} frames at {:.1} fps", frame_count, fps);
                    }
                    std::io::stdout().flush().unwrap_or(());
                }
            }
        }
    }

    // Send EOF to decoder and process remaining frames
    decoder
        .send_eof()
        .context("Failed to send EOF to decoder")?;
    let mut decoded_frame = frame::Video::empty();
    let mut scaled_frame = frame::Video::empty();
    while decoder.receive_frame(&mut decoded_frame).is_ok() {
        let analysis_frame = if let Some(ref mut sc) = scaler {
            sc.run(&decoded_frame, &mut scaled_frame)
                .context("Failed to scale final frame")?;
            &scaled_frame
        } else {
            &decoded_frame
        };

        // Analyze the native frame within active area (reuse crop)
        if crop_rect_opt.is_none() {
            if cli.no_crop {
                let rect = CropRect::full(analysis_frame.width(), analysis_frame.height());
                println!(
                    "\nCrop disabled: using full frame {}x{}",
                    rect.width, rect.height
                );
                crop_rect_opt = Some(rect);
            } else {
                let rect = crop::detect_crop(analysis_frame);
                println!(
                    "\nDetected active video area: {}x{} at offset ({}, {})",
                    rect.width, rect.height, rect.x, rect.y
                );
                crop_rect_opt = Some(rect);
            }
        }
        let rect = crop_rect_opt.as_ref().unwrap();
        let analyzed_frame = analyze_native_frame_cropped(analysis_frame, width, height, rect)?;

        if let Some(ref prev_hist) = previous_histogram {
            let raw_diff =
                calculate_histogram_difference(&analyzed_frame.lum_histogram, prev_hist);
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
            }
        }

        frames.push(analyzed_frame);
        frame_count += 1;
    }

    // Final completion message
    let total_elapsed = start_time.elapsed();
    let final_fps = if total_elapsed.as_secs_f64() > 0.0 {
        frame_count as f64 / total_elapsed.as_secs_f64()
    } else {
        0.0
    };

    println!(
        "\nCompleted native processing {} frames in {} ({:.1} fps average)",
        frame_count,
        format_duration(total_elapsed),
        final_fps
    );

    // Convert scene cuts to scenes
    let scenes = convert_scene_cuts_to_scenes(scene_cuts, frame_count);
    println!(
        "Scene detection completed: {} scenes detected",
        scenes.len()
    );

    Ok((scenes, frames))
}

/// Set up hardware-accelerated decoder based on the specified acceleration type.
///
/// # Arguments
/// * `decoder_context` - The decoder context to configure
/// * `hwaccel` - Hardware acceleration type ("cuda", "vaapi", "videotoolbox")
///
/// # Returns
/// `Result<codec::decoder::Video>` - Configured hardware decoder
fn setup_hardware_decoder(
    decoder_context: codec::context::Context,
    hwaccel: &str,
) -> Result<codec::decoder::Video> {
    match hwaccel {
        "cuda" => {
            // Try to find CUDA-specific decoder
            if let Some(cuda_decoder) = codec::decoder::find_by_name("hevc_cuvid") {
                let mut context = codec::context::Context::new_with_codec(cuda_decoder);
                // Copy parameters from the original context
                unsafe {
                    (*context.as_mut_ptr()).width = (*decoder_context.as_ptr()).width;
                    (*context.as_mut_ptr()).height = (*decoder_context.as_ptr()).height;
                    (*context.as_mut_ptr()).pix_fmt = (*decoder_context.as_ptr()).pix_fmt;
                }
                context
                    .decoder()
                    .video()
                    .context("Failed to create CUDA hardware decoder")
            } else {
                println!("CUDA decoder not available, falling back to software decoder");
                decoder_context
                    .decoder()
                    .video()
                    .context("Failed to create fallback software decoder")
            }
        }
        "vaapi" | "videotoolbox" => {
            // For VAAPI and VideoToolbox, we'll use software decoding for now
            // as hardware acceleration setup is more complex and requires device contexts
            println!(
                "Hardware acceleration {} requested, using software decoder for now",
                hwaccel
            );
            decoder_context
                .decoder()
                .video()
                .context("Failed to create software decoder")
        }
        _ => {
            println!(
                "Unknown hardware acceleration type '{}', using software decoder",
                hwaccel
            );
            decoder_context
                .decoder()
                .video()
                .context("Failed to create software decoder")
        }
    }
}

fn analyze_native_frame_cropped(
    frame: &frame::Video,
    _width: u32,
    _height: u32,
    crop_rect: &CropRect,
) -> Result<MadVRFrame> {
    let mut histogram = vec![0f64; 256];
    let mut max_pq = 0.0f64;

    // Y plane data
    let y_plane_data = frame.data(0);
    let y_stride = frame.stride(0);

    // madVR v5 binning setup
    let sdr_peak_pq = nits_to_pq(100.0);
    let sdr_step = sdr_peak_pq / 64.0;
    let hdr_step = (1.0 - sdr_peak_pq) / 192.0;

    let x_start = crop_rect.x as usize;
    let y_start = crop_rect.y as usize;
    let x_end = x_start + crop_rect.width as usize;
    let y_end = y_start + crop_rect.height as usize;

    // Process by rows to reduce bounds checks and improve cache locality
    for y in y_start..y_end {
        let row_start = y.saturating_mul(y_stride);
        // Compute byte range for cropped row
        let base = row_start + x_start.saturating_mul(2);
        if base >= y_plane_data.len() {
            continue;
        }
        let want_len = (x_end - x_start).saturating_mul(2);
        let max_len = y_plane_data.len() - base;
        let len = want_len.min(max_len);
        let row = &y_plane_data[base..base + len];

        for px in row.chunks_exact(2) {
            // Read 10-bit limited-range code (0..1023 in 16-bit container)
            let code10 = u16::from_le_bytes([px[0], px[1]]) & 0x03FF;

            // Normalize to limited-range [64,940] -> [0,1]
            let code_i = code10 as i32;
            let norm = ((code_i - 64) as f64 / 876.0).clamp(0.0, 1.0);

            let pq = norm; // Approximate PQ code from Y' (HDR10 pipeline)
            if pq > max_pq {
                max_pq = pq;
            }

            // Map to madVR v5 bins
            let bin = if pq < sdr_peak_pq {
                (pq / sdr_step).floor() as usize
            } else {
                64 + ((pq - sdr_peak_pq) / hdr_step).floor() as usize
            };
            histogram[bin.min(255)] += 1.0;
        }
    }

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

    Ok(MadVRFrame {
        peak_pq_2020: max_pq,
        avg_pq,
        lum_histogram: histogram,
        hue_histogram: Some(vec![0.0; 31]),
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
fn analyze_native_frame(frame: &frame::Video, width: u32, height: u32) -> Result<MadVRFrame> {
    let pixel_count = (width * height) as usize;
    let mut histogram = vec![0f64; 256];
    let mut max_luma_10bit = 0u16;

    // Get Y-plane data (luminance) from the 10-bit frame
    let y_plane_data = frame.data(0); // Y plane
    let y_stride = frame.stride(0);

    // Process 10-bit luminance data
    // In YUV420P10LE, each luma sample is 2 bytes (little-endian 10-bit value)
    for y in 0..height {
        let row_start = (y as usize) * y_stride;
        for x in 0..(width as usize) {
            let pixel_offset = row_start + (x * 2); // 2 bytes per 10-bit pixel

            if pixel_offset + 1 < y_plane_data.len() {
                // Read 10-bit value (little-endian)
                let luma_10bit = u16::from_le_bytes([
                    y_plane_data[pixel_offset],
                    y_plane_data[pixel_offset + 1],
                ]) & 0x3FF; // Mask to 10 bits (0-1023)

                max_luma_10bit = max_luma_10bit.max(luma_10bit);

                // **CORRECT LUMINANCE MAPPING**: 10-bit luma directly corresponds to PQ curve
                // Normalize 10-bit value to PQ range (0.0-1.0)
                let pq_value = luma_10bit as f64 / 1023.0;

                // Map PQ value to histogram bin (0-255)
                let bin_index = (pq_value * 255.0).round() as usize;
                let bin_index = bin_index.min(255);

                histogram[bin_index] += 1.0;
            }
        }
    }

    // Normalize histogram so sum equals 100.0
    let total_pixels = pixel_count as f64;
    for bin in &mut histogram {
        *bin = (*bin / total_pixels) * 100.0;
    }

    // Calculate peak PQ from the brightest 10-bit luma value
    let peak_pq = max_luma_10bit as f64 / 1023.0;

    // Calculate average PQ from the histogram
    let avg_pq = calculate_avg_pq_from_histogram(&histogram);

    Ok(MadVRFrame {
        peak_pq_2020: peak_pq,
        avg_pq,
        lum_histogram: histogram,
        hue_histogram: Some(vec![0f64; 31]), // Add empty hue histogram for v6 compatibility
        target_nits: None,                   // Will be set by optimizer if enabled
        ..Default::default()
    })
}

/// Calculate histogram difference using Sum of Absolute Differences for scene detection.
///
/// # Arguments
/// * `hist1` - First histogram
/// * `hist2` - Second histogram
///
/// # Returns
/// Difference score (higher values indicate more significant changes)
fn calculate_histogram_difference(hist1: &[f64], hist2: &[f64]) -> f64 {
    // Chi-squared distance (symmetric form) with small epsilon to avoid div-by-zero
    let mut dist = 0.0f64;
    let len = hist1.len().min(hist2.len());
    for i in 0..len {
        let a = hist1[i];
        let b = hist2[i];
        let denom = a + b + 1e-6;
        let diff = a - b;
        dist += (diff * diff) / denom;
    }
    dist
}

/// Decide whether a candidate cut is allowed given the last accepted cut and minimum scene length.
fn cut_allowed(last_cut: Option<u32>, candidate_frame: u32, min_scene_len: u32) -> bool {
    match last_cut {
        None => candidate_frame >= min_scene_len,
        Some(prev) => candidate_frame.saturating_sub(prev) >= min_scene_len,
    }
}

/// Convert scene cuts to MadVRScene structures.
///
/// # Arguments
/// * `scene_cuts` - Vector of frame indices where scene cuts occur
/// * `total_frames` - Total number of frames processed
///
/// # Returns
/// Vector of MadVRScene structures
fn convert_scene_cuts_to_scenes(mut scene_cuts: Vec<u32>, total_frames: u32) -> Vec<MadVRScene> {
    let mut scenes = Vec::new();
    let mut start_frame = 0u32;

    // Sort scene cuts to ensure proper ordering
    scene_cuts.sort_unstable();

    for &cut_frame in &scene_cuts {
        scenes.push(MadVRScene {
            start: start_frame,
            end: cut_frame.saturating_sub(1),
            peak_nits: 0, // Will be calculated later
            avg_pq: 0.0,  // Will be calculated later
            ..Default::default()
        });
        start_frame = cut_frame;
    }

    // Add final scene
    if !scene_cuts.is_empty() {
        scenes.push(MadVRScene {
            start: start_frame,
            end: total_frames.saturating_sub(1), // Use actual last frame index
            peak_nits: 0,
            avg_pq: 0.0,
            ..Default::default()
        });
    } else {
        // No scene cuts detected, create single scene
        scenes.push(MadVRScene {
            start: 0,
            end: total_frames.saturating_sub(1),
            peak_nits: 0,
            avg_pq: 0.0,
            ..Default::default()
        });
    }

    scenes
}

// OLD EXTERNAL FFMPEG PIPELINE REMOVED - Now using native ffmpeg-next pipeline

// OLD analyze_single_frame FUNCTION REMOVED - Now using analyze_native_frame with direct 10-bit processing

/// Fix scene end frames after we know the actual frame count.
///
/// This function updates scene end frames that were set to u32::MAX during
/// scene detection to use the actual last frame index. It also validates
/// that all scene ranges are within bounds.
///
/// # Arguments
/// * `scenes` - Mutable slice of scene data to fix
/// * `total_frames` - Total number of frames in the video
fn fix_scene_end_frames(scenes: &mut [MadVRScene], total_frames: usize) {
    if scenes.is_empty() || total_frames == 0 {
        return;
    }

    let last_frame_idx = (total_frames - 1) as u32;

    for scene in scenes.iter_mut() {
        // Fix scenes that have u32::MAX as end frame
        if scene.end == u32::MAX {
            scene.end = last_frame_idx;
        }

        // Ensure scene end doesn't exceed total frames (frame indices are 0-based)
        if scene.end >= total_frames as u32 {
            scene.end = last_frame_idx;
        }

        // Ensure scene start is valid
        if scene.start >= total_frames as u32 {
            scene.start = last_frame_idx;
        }

        // Ensure start <= end
        if scene.start > scene.end {
            // If start > end, this is likely corrupted data
            // Set both to a safe range
            scene.start = 0;
            scene.end = last_frame_idx;
        }
    }
}

/// Pre-compute scene-based statistics for optimization.
///
/// This function calculates aggregate statistics for each detected scene,
/// including the average PQ value across all frames in the scene. These
/// statistics are used by the optimizer to make scene-aware decisions.
///
/// # Arguments
/// * `scenes` - Mutable slice of scene data to update
/// * `frames` - Frame analysis data to aggregate
fn precompute_scene_stats(scenes: &mut [MadVRScene], frames: &[MadVRFrame]) {
    println!("Computing scene-based statistics...");

    for scene in scenes.iter_mut() {
        let start_idx = scene.start as usize;
        let end_idx = ((scene.end + 1) as usize).min(frames.len());

        if start_idx < frames.len() && start_idx < end_idx {
            let scene_frames = &frames[start_idx..end_idx];

            // Calculate average PQ for the entire scene
            let total_avg_pq: f64 = scene_frames.iter().map(|f| f.avg_pq).sum();
            scene.avg_pq = total_avg_pq / scene_frames.len() as f64;

            // Calculate peak nits for the scene
            let max_peak_pq = scene_frames
                .iter()
                .map(|f| f.peak_pq_2020)
                .fold(0.0f64, f64::max);
            scene.peak_nits = pq_to_nits(max_peak_pq) as u32;
        }
    }
}

/// Advanced optimizer with rolling averages and scene-aware heuristics.
///
/// This function implements the core optimization algorithm that generates
/// dynamic target nits for each frame. It uses:
/// - 240-frame rolling average for temporal smoothing
/// - 99th percentile highlight knee detection
/// - Scene-aware heuristics based on average picture level
/// - Peak brightness analysis for tone mapping decisions
///
/// The optimizer aims to preserve artistic intent while ensuring smooth
/// transitions and preventing blown highlights or crushed shadows.
///
/// # Arguments
/// * `frames` - Mutable slice of frame data to optimize
fn run_optimizer_pass(scenes: &[MadVRScene], frames: &mut [MadVRFrame]) {
    const ROLLING_WINDOW_SIZE: usize = 240; // 240 frames as recommended by research

    let total_frames = frames.len();
    println!(
        "Applying advanced optimization heuristics with {}-frame rolling window (scene-aware)...",
        ROLLING_WINDOW_SIZE
    );

    let mut processed = 0usize;
    let mut prev_target: Option<u16> = None;
    const MAX_DELTA_PER_FRAME: u16 = 200; // limit how fast target_nits can change frame-to-frame

    for scene in scenes {
        let start = scene.start as usize;
        let end = ((scene.end + 1) as usize).min(frames.len());
        if start >= end { continue; }

        // Reset smoothing at scene boundary to avoid cross-scene lag
        let mut rolling_avg_queue: VecDeque<f64> = VecDeque::with_capacity(ROLLING_WINDOW_SIZE);

        let scene_avg_apl_nits = pq_to_nits(scene.avg_pq);

        for idx in start..end {
            let frame = &mut frames[idx];

            // Add current frame's avg_pq to rolling window
            rolling_avg_queue.push_back(frame.avg_pq);
            if rolling_avg_queue.len() > ROLLING_WINDOW_SIZE {
                rolling_avg_queue.pop_front();
            }

            // Rolling average PQ blended with scene average to be truly scene-aware
            let rolling_avg_pq = rolling_avg_queue.iter().sum::<f64>() / rolling_avg_queue.len() as f64;
            let rolling_apl_nits = pq_to_nits(rolling_avg_pq);
            let blended_apl_nits = 0.6 * rolling_apl_nits + 0.4 * scene_avg_apl_nits;

            // Convert peak PQ to nits for decision making
            let peak_nits = pq_to_nits(frame.peak_pq_2020) as u32;
            // Find highlight knee (99th percentile)
            let highlight_knee_nits = find_highlight_knee_nits(&frame.lum_histogram);

            // Apply heuristics with scene-aware APL
            let raw_target = apply_advanced_heuristics(
                peak_nits,
                blended_apl_nits,
                highlight_knee_nits,
                scene_avg_apl_nits,
            );

            // Apply delta limiting for temporal smoothness
            let final_target = apply_delta_limit(prev_target, raw_target, MAX_DELTA_PER_FRAME);
            frame.target_nits = Some(final_target);
            prev_target = Some(final_target);

            processed += 1;
            if processed % 1000 == 0 {
                let progress = (processed as f64 / total_frames as f64) * 100.0;
                print!("\rOptimizer progress: {:.1}%", progress);
                std::io::stdout().flush().unwrap_or(());
            }
        }
    }

    println!("\rOptimizer completed: {} frames processed", total_frames);
}

/// Apply advanced optimization heuristics to determine target nits.
///
/// This function implements the core tone mapping logic using multiple
/// heuristics to determine the optimal target nits for a frame:
///
/// 1. Hard cap for extreme peaks (>4000 nits) to prevent flicker
/// 2. Scene-aware processing based on rolling average APL:
///    - Dark scenes: More aggressive, preserve shadow detail
///    - Medium scenes: Balanced approach
///    - Bright scenes: Conservative to prevent blown highlights
/// 3. Highlight knee respect to preserve detail in bright areas
///
/// # Arguments
/// * `peak_nits` - Peak brightness of the current frame
/// * `rolling_apl_nits` - Rolling average picture level in nits
/// * `highlight_knee_nits` - 99th percentile brightness level
///
/// # Returns
/// Target nits value for tone mapping (as u16)
fn apply_advanced_heuristics(
    peak_nits: u32,
    rolling_apl_nits: f64,
    highlight_knee_nits: f64,
    scene_avg_apl_nits: f64,
) -> u16 {
    // Heuristic 1: Hard cap for extreme peaks (prevents flicker and blown-out highlights)
    if peak_nits > 4000 {
        return (highlight_knee_nits.min(4000.0)) as u16;
    }

    // Heuristic 2: Use rolling average to smooth transitions and prevent temporal artifacts
    // Blend rolling with scene average to stabilize classification
    let apl_ref = 0.7 * rolling_apl_nits + 0.3 * scene_avg_apl_nits;
    if apl_ref < 50.0 {
        // Dark scene - be more aggressive, allow brighter targets to preserve shadow detail
        // But still respect the highlight knee to prevent blown highlights
        let target = peak_nits.clamp(800, 2000); // Minimum 800 nits for dark scenes
        (target as f64).min(highlight_knee_nits * 1.2) as u16 // Allow 20% above knee for dark scenes
    } else if apl_ref < 150.0 {
        // Medium brightness scene - balanced approach
        let target = peak_nits.clamp(600, 1500);
        (target as f64).min(highlight_knee_nits * 1.1) as u16 // Allow 10% above knee
    } else {
        // Bright scene - be more conservative to prevent blown-out look
        let target = peak_nits.clamp(400, 1000);
        (target as f64).min(highlight_knee_nits) as u16 // Respect the highlight knee strictly
    }
}

/// Limit frame-to-frame change of target_nits to reduce flicker.
fn apply_delta_limit(prev: Option<u16>, target: u16, max_delta: u16) -> u16 {
    if let Some(p) = prev {
        if target > p {
            p.saturating_add(max_delta).min(target)
        } else if target < p {
            p.saturating_sub(max_delta).max(target)
        } else {
            target
        }
    } else {
        target
    }
}

/// Write the measurement file in madVR format.
///
/// This function generates a binary measurement file compatible with madVR
/// and other Dolby Vision processing tools. The file format includes:
/// - Header with version, scene count, frame count, and flags
/// - Scene data (start/end frames and peak nits)
/// - Per-frame data (peak PQ and 256-bin histograms)
/// - Optional target nits block when optimizer is enabled
///
/// The binary format uses little-endian encoding and follows the madVR
/// specification for measurement files.
///
/// # Arguments
/// * `output_path` - Path where the .bin file will be written
/// * `scenes` - Scene analysis data
/// * `frames` - Frame analysis data
/// * `enable_optimizer` - Whether optimizer data should be included
/// * `madvr_version` - madVR measurement version to write (5 or 6)
/// * `target_peak_nits` - Optional override for header.target_peak_nits (v6)
///
/// # Returns
/// `Result<()>` - Ok(()) on successful write, Err on failure
fn write_measurement_file(
    output_path: &str,
    scenes: &[MadVRScene],
    frames: &[MadVRFrame],
    enable_optimizer: bool,
    madvr_version: u32,
    target_peak_nits: Option<u32>,
) -> Result<()> {
    // 1. Create the Header
    let maxcll = frames
        .iter()
        .map(|f| pq_to_nits(f.peak_pq_2020) as u32)
        .max()
        .unwrap_or(0);

    // Compute FALL metrics from per-frame avg PQ
    let (maxfall, avgfall) = compute_falls(frames);

    let header_size = if madvr_version >= 6 { 36 } else { 32 };

    let mut header = MadVRHeader {
        version: madvr_version,
        header_size,
        scene_count: scenes.len() as u32,
        frame_count: frames.len() as u32,
        flags: if enable_optimizer { 3 } else { 2 },
        maxcll,
        maxfall,
        avgfall,
        ..Default::default() // Let the library handle other default values
    };

    if madvr_version >= 6 {
        header.target_peak_nits = target_peak_nits.unwrap_or(maxcll);
    }

    // 2. Create the top-level Measurements object
    // We need to create new vectors with the data since the structs don't implement Clone
    let mut owned_scenes = Vec::new();
    for scene in scenes {
        owned_scenes.push(MadVRScene {
            start: scene.start,
            end: scene.end,
            peak_nits: scene.peak_nits,
            avg_pq: scene.avg_pq,
            ..Default::default()
        });
    }

    let mut owned_frames = Vec::new();
    for frame in frames {
        // For v6, madVR expects per-gamut peaks; duplicate 2020 peak until proper computation is added
        owned_frames.push(MadVRFrame {
            peak_pq_2020: frame.peak_pq_2020,
            peak_pq_dcip3: if madvr_version >= 6 {
                Some(frame.peak_pq_2020)
            } else {
                frame.peak_pq_dcip3
            },
            peak_pq_709: if madvr_version >= 6 {
                Some(frame.peak_pq_2020)
            } else {
                frame.peak_pq_709
            },
            avg_pq: frame.avg_pq,
            lum_histogram: frame.lum_histogram.clone(),
            hue_histogram: frame.hue_histogram.clone(),
            target_nits: frame.target_nits,
            ..Default::default()
        });
    }

    let measurements = MadVRMeasurements {
        header,
        scenes: owned_scenes,
        frames: owned_frames,
    };

    // 3. Let the library do all the hard work!
    println!("Serializing measurement data using madvr_parse library...");
    let binary_data = measurements
        .write_measurements()
        .context("Failed to serialize measurements using madvr_parse library")?;

    // 4. Write the resulting bytes to a file
    std::fs::write(output_path, binary_data)
        .context("Failed to write binary data to output file")?;

    println!("Successfully wrote measurement file.");
    println!("MaxCLL: {} nits", maxcll);

    Ok(())
}

/// Compute MaxFALL and AvgFALL from frames' avg_pq values.
fn compute_falls(frames: &[MadVRFrame]) -> (u32, u32) {
    if frames.is_empty() {
        return (0, 0);
    }
    let falls_nits: Vec<f64> = frames.iter().map(|f| pq_to_nits(f.avg_pq)).collect();
    let maxfall = falls_nits.iter().cloned().fold(0.0, f64::max).round() as u32;
    let avgfall = (falls_nits.iter().sum::<f64>() / falls_nits.len() as f64).round() as u32;
    (maxfall, avgfall)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cut_allowed_min_len() {
        // First cut at frame 10 not allowed if min len 24
        assert_eq!(cut_allowed(Some(0), 10, 24), false);
        // Cut at frame 24 allowed
        assert_eq!(cut_allowed(Some(0), 24, 24), true);
        // Subsequent cut needs another 24 frames
        assert_eq!(cut_allowed(Some(24), 40, 24), false);
        assert_eq!(cut_allowed(Some(24), 48, 24), true);
    }

    #[test]
    fn test_compute_falls() {
        // Build three frames with avg_pq corresponding to 100, 200, 300 nits
        fn to_pq(nits: f64) -> f64 { nits_to_pq(nits) }
        let frames = vec![
            MadVRFrame { avg_pq: to_pq(100.0), ..Default::default() },
            MadVRFrame { avg_pq: to_pq(200.0), ..Default::default() },
            MadVRFrame { avg_pq: to_pq(300.0), ..Default::default() },
        ];
        let (maxfall, avgfall) = compute_falls(&frames);
        assert!(maxfall >= 300 - 1 && maxfall <= 300 + 1, "maxfall ~300, got {}", maxfall);
        assert!(avgfall >= 200 - 1 && avgfall <= 200 + 1, "avgfall ~200, got {}", avgfall);
    }

    #[test]
    fn test_histogram_diff_smoothing_behaves() {
        // A simple increasing sequence; smoothing should be lower than last value for window>1
        let diffs = [0.1, 0.2, 0.5, 1.0];
        let mut dq: VecDeque<f64> = VecDeque::with_capacity(3);
        let mut smoothed = Vec::new();
        for d in diffs { dq.push_back(d); if dq.len() > 3 { dq.pop_front(); } smoothed.push(dq.iter().sum::<f64>() / dq.len() as f64) }
        assert!(smoothed[3] < 1.0, "smoothed value should be below last raw value");
        assert!(smoothed[0] - 0.1_f64).abs() < 1e-9;
    }

    #[test]
    fn test_apply_delta_limit() {
        // No previous: pass-through
        assert_eq!(apply_delta_limit(None, 800, 200), 800);
        // Limit positive jump
        assert_eq!(apply_delta_limit(Some(600), 1000, 200), 800);
        // Limit negative jump
        assert_eq!(apply_delta_limit(Some(900), 400, 200), 700);
        // Within delta: unchanged
        assert_eq!(apply_delta_limit(Some(700), 820, 200), 820);
    }
}
