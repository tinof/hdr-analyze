use anyhow::{Context, Result};
use clap::Parser;
use madvr_parse::{MadVRHeader, MadVRMeasurements, MadVRScene, MadVRFrame};
use std::collections::VecDeque;
use std::io::{BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

// --- Command Line Interface ---
#[derive(Parser)]
#[command(name = "hdr_analyzer_mvp")]
#[command(about = "HDR10 to Dolby Vision converter - Phase 1 MVP")]
struct Cli {
    /// Path to the input video file
    #[arg(short, long)]
    input: String,

    /// Path for the output .bin measurement file
    #[arg(short, long)]
    output: String,

    /// (Phase 3) Enable intelligent optimizer to generate dynamic target nits
    #[arg(long)]
    enable_optimizer: bool,

    /// (Optional) Enable GPU hardware acceleration.
    /// Examples: "cuda" (for NVIDIA), "vaapi" (for Linux/AMD/Intel), "videotoolbox" (for macOS).
    #[arg(long)]
    hwaccel: Option<String>,
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

// --- Formulas ---
/// Convert nits (cd/m²) to PQ (Perceptual Quantizer) value using ST.2084 standard.
///
/// This function implements the ST.2084 EOTF (Electro-Optical Transfer Function)
/// to convert absolute luminance values in nits to PQ code values in the range [0.0, 1.0].
///
/// # Arguments
/// * `nits` - Luminance value in nits (cd/m²), typically in range [0, 10000]
///
/// # Returns
/// PQ value in range [0.0, 1.0] where 1.0 represents 10,000 nits
fn nits_to_pq(nits: u32) -> f64 {
    let y = nits as f64 / ST2084_Y_MAX;
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



/// Main entry point for the HDR analyzer.
///
/// This function orchestrates the complete HDR analysis pipeline:
/// 1. Extracts video information (resolution, frame count)
/// 2. Performs scene detection using ffmpeg
/// 3. Analyzes each frame to generate PQ histograms and peak/average values
/// 4. Optionally runs the advanced optimizer to generate dynamic target nits
/// 5. Writes the results to a madVR-compatible .bin measurement file
///
/// # Returns
/// `Result<()>` - Ok(()) on success, Err on any failure
fn main() -> Result<()> {
    let cli = Cli::parse();

    println!("HDR Analyzer MVP - Starting analysis of: {}", cli.input);

    // Step 1: Get video info using ffprobe
    let (width, height, total_frames) = get_video_info(&cli.input)?;
    println!("Video resolution: {}x{}", width, height);
    if let Some(frames) = total_frames {
        println!("Total frames: {}", frames);
    }

    // Step 2: Consolidated scene detection and frame analysis
    println!("Starting consolidated analysis pipeline...");
    let (mut scenes, mut frames) = run_analysis_pipeline(&cli, width, height, total_frames)?;
    println!("Detected {} scenes and analyzed {} frames", scenes.len(), frames.len());

    // Step 4: Fix scene end frames and compute scene statistics
    fix_scene_end_frames(&mut scenes, frames.len());
    precompute_scene_stats(&mut scenes, &frames);

    // Step 5: Run advanced optimizer if enabled
    if cli.enable_optimizer {
        println!("Running intelligent optimizer pass...");
        run_optimizer_pass(&mut frames);
    }

    // Step 6: Assemble and write the .bin file
    println!("Writing measurement file: {}", cli.output);
    write_measurement_file(&cli.output, &scenes, &frames, cli.enable_optimizer)?;

    println!("Analysis complete!");
    Ok(())
}

/// Get video resolution and frame count using ffprobe.
///
/// This function uses ffprobe to extract essential video metadata needed for analysis.
/// Frame count extraction is optional as some formats don't support it reliably.
///
/// # Arguments
/// * `input_path` - Path to the input video file
///
/// # Returns
/// `Result<(u32, u32, Option<u32>)>` - (width, height, optional_frame_count)
fn get_video_info(input_path: &str) -> Result<(u32, u32, Option<u32>)> {
    // Get resolution
    let resolution_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=s=x:p=0",
            input_path,
        ])
        .output()
        .context("Failed to execute ffprobe - make sure ffmpeg is installed")?;

    if !resolution_output.status.success() {
        anyhow::bail!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&resolution_output.stderr)
        );
    }

    let resolution_str = String::from_utf8(resolution_output.stdout)
        .context("Invalid UTF-8 in ffprobe output")?
        .trim()
        .to_string();

    let parts: Vec<&str> = resolution_str.split('x').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid resolution format: {}", resolution_str);
    }

    let width: u32 = parts[0].parse().context("Invalid width")?;
    let height: u32 = parts[1].parse().context("Invalid height")?;

    // Try to get frame count (this might fail for some formats, so it's optional)
    let frame_count = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_packets",
            "-show_entries",
            "stream=nb_read_packets",
            "-of",
            "csv=p=0",
            input_path,
        ])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()?
                    .trim()
                    .parse::<u32>()
                    .ok()
            } else {
                None
            }
        });

    Ok((width, height, frame_count))
}

/// Consolidated analysis pipeline that performs both scene detection and frame analysis
/// using a single ffmpeg process with multiple output streams.
///
/// This function eliminates the performance bottleneck of running separate ffmpeg processes
/// by using one process that outputs both scene detection metadata (via stderr) and
/// raw frame data (via stdout) simultaneously. Two dedicated threads process these streams
/// concurrently to prevent pipe buffer overflow.
///
/// # Arguments
/// * `cli` - Command line interface containing input path and hardware acceleration settings
/// * `width` - Video width in pixels
/// * `height` - Video height in pixels
/// * `total_frames` - Optional total frame count for progress tracking
///
/// # Returns
/// `Result<(Vec<MadVRScene>, Vec<MadVRFrame>)>` - Tuple of detected scenes and analyzed frames
fn run_analysis_pipeline(
    cli: &Cli,
    width: u32,
    height: u32,
    total_frames: Option<u32>,
) -> Result<(Vec<MadVRScene>, Vec<MadVRFrame>)> {
    println!("Consolidated pipeline in progress (this may take a moment for large files)...");

    // Build ffmpeg command with both scene detection and frame output
    let mut ffmpeg_args = Vec::new();

    // Input arguments with optional hardware acceleration
    if let Some(accel) = &cli.hwaccel {
        println!("Attempting to use hardware acceleration: {}", accel);
        ffmpeg_args.push("-hwaccel".to_string());
        ffmpeg_args.push(accel.clone());

        // Add decoder-specific flags based on acceleration type
        match accel.as_str() {
            "cuda" => {
                ffmpeg_args.push("-c:v".to_string());
                ffmpeg_args.push("hevc_cuvid".to_string());
            }
            "vaapi" => {
                // VAAPI decoders are typically auto-selected
            }
            "videotoolbox" => {
                // VideoToolbox decoders are auto-selected when hwaccel is specified
                // No explicit decoder needed
            }
            _ => {
                println!("Warning: Unknown hardware acceleration type '{}', proceeding with basic hwaccel flag", accel);
            }
        }
    }

    // Standard input
    ffmpeg_args.push("-i".to_string());
    ffmpeg_args.push(cli.input.clone());

    // Scene detection filter with luminance extraction
    ffmpeg_args.push("-vf".to_string());

    // Build filter chain and determine pixel format based on hardware acceleration compatibility
    let (filter_chain, pixel_format, bytes_per_pixel) = if let Some(accel) = &cli.hwaccel {
        match accel.as_str() {
            "videotoolbox" => {
                // VideoToolbox requires explicit format conversion before extractplanes
                ("scdet=threshold=2,metadata=print,format=yuv420p10le,extractplanes=y".to_string(), "gray".to_string(), 1usize)
            }
            "vaapi" => {
                // VAAPI hardware acceleration with luminance extraction
                ("hwdownload,format=yuv420p10le,scdet=threshold=2,metadata=print,extractplanes=y".to_string(), "gray".to_string(), 1usize)
            }
            "cuda" => {
                // CUDA hardware acceleration with luminance extraction
                ("hwdownload,format=yuv420p10le,scdet=threshold=2,metadata=print,extractplanes=y".to_string(), "gray".to_string(), 1usize)
            }
            _ => {
                // Unknown acceleration type, use basic hardware acceleration pipeline
                ("hwdownload,format=yuv420p10le,scdet=threshold=2,metadata=print,extractplanes=y".to_string(), "gray".to_string(), 1usize)
            }
        }
    } else {
        // Software decoding can use extractplanes directly for luminance-only processing
        ("scdet=threshold=2,metadata=print,extractplanes=y".to_string(), "gray".to_string(), 1usize)
    };

    ffmpeg_args.push(filter_chain);

    // Output raw frames to stdout (format depends on hardware acceleration compatibility)
    ffmpeg_args.push("-f".to_string());
    ffmpeg_args.push("rawvideo".to_string());
    ffmpeg_args.push("-pix_fmt".to_string());
    ffmpeg_args.push(pixel_format);
    ffmpeg_args.push("-".to_string()); // stdout

    // Spawn the consolidated ffmpeg process
    let mut child = Command::new("ffmpeg")
        .args(&ffmpeg_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to execute ffmpeg for consolidated analysis")?;

    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take().context("Failed to capture stderr")?;

    // Create channels for inter-thread communication
    let (frame_tx, frame_rx) = mpsc::channel::<MadVRFrame>();
    let (scene_tx, scene_rx) = mpsc::channel::<u32>();

    // Frame processing thread (stdout)
    let frame_thread = {
        let frame_tx = frame_tx.clone();
        let bytes_per_pixel_copy = bytes_per_pixel;
        thread::spawn(move || -> Result<()> {
            let mut stdout_reader = BufReader::new(stdout);
            let frame_size = width as usize * height as usize * bytes_per_pixel_copy; // Bytes per pixel depends on format
            let mut frame_buffer = vec![0u8; frame_size];
            let mut frame_count = 0u32;

            // Progress tracking
            let start_time = Instant::now();
            let mut last_progress_update = Instant::now();
            let progress_update_interval = Duration::from_millis(500);

            println!(
                "Processing frames ({}x{}, {} bytes per frame)...",
                width, height, frame_size
            );
            print!("Initializing frame analysis...");
            std::io::stdout().flush().unwrap_or(());

            loop {
                match stdout_reader.read_exact(&mut frame_buffer) {
                    Ok(()) => {
                        // Process this frame
                        let frame = analyze_single_frame(&frame_buffer, width, height, bytes_per_pixel_copy)?;
                        frame_tx.send(frame).map_err(|_| anyhow::anyhow!("Failed to send frame data"))?;
                        frame_count += 1;

                        // Update progress display periodically
                        let now = Instant::now();
                        if now.duration_since(last_progress_update) >= progress_update_interval
                            || frame_count == 1
                        {
                            last_progress_update = now;

                            // Calculate processing rate (frames per second)
                            let elapsed = now.duration_since(start_time);
                            let fps = if elapsed.as_secs_f64() > 0.0 {
                                frame_count as f64 / elapsed.as_secs_f64()
                            } else {
                                0.0
                            };

                            // Progress display with frame count and processing rate
                            if let Some(total) = total_frames {
                                let progress = (frame_count as f64 / total as f64) * 100.0;
                                print!(
                                    "\rProcessing: {}/{} frames ({:.1}%) at {:.1} fps",
                                    frame_count, total, progress, fps
                                );
                            } else {
                                print!(
                                    "\rProcessing: {} frames at {:.1} fps",
                                    frame_count, fps
                                );
                            }
                            std::io::stdout().flush().unwrap_or(());
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        // End of stream - normal termination
                        break;
                    }
                    Err(e) => {
                        return Err(anyhow::Error::from(e).context("Failed to read frame data"));
                    }
                }
            }

            // Final completion message
            let total_elapsed = start_time.elapsed();
            let final_fps = if total_elapsed.as_secs_f64() > 0.0 {
                frame_count as f64 / total_elapsed.as_secs_f64()
            } else {
                0.0
            };

            println!(
                "\nCompleted processing {} frames in {} ({:.1} fps average)",
                frame_count,
                format_duration(total_elapsed),
                final_fps
            );

            Ok(())
        })
    };

    // Scene detection thread (stderr) - also capture stderr for error reporting
    let (stderr_content_tx, stderr_content_rx) = mpsc::channel::<String>();
    let scene_thread = {
        let scene_tx = scene_tx.clone();
        let stderr_content_tx = stderr_content_tx.clone();
        thread::spawn(move || -> Result<()> {
            let mut stderr_reader = BufReader::new(stderr);
            let mut stderr_content = String::new();
            stderr_reader
                .read_to_string(&mut stderr_content)
                .context("Failed to read ffmpeg stderr")?;

            // Send stderr content for potential error reporting
            stderr_content_tx.send(stderr_content.clone()).unwrap_or(());

            // Parse scene cuts from stderr
            let mut current_frame = 0u32;

            for line in stderr_content.lines() {
                // Look for frame number lines first
                if line.contains("lavfi.scdet.n:") {
                    if let Some(n_start) = line.find("lavfi.scdet.n:") {
                        let n_part = &line[n_start + "lavfi.scdet.n:".len()..];
                        if let Some(frame_num_str) = n_part.split_whitespace().next() {
                            if let Ok(frame_num) = frame_num_str.parse::<u32>() {
                                current_frame = frame_num;
                            }
                        }
                    }
                }
                // Then look for scene detection on the same or nearby lines
                if line.contains("lavfi.scdet.pts_time:") && current_frame > 0 {
                    scene_tx.send(current_frame).map_err(|_| anyhow::anyhow!("Failed to send scene cut data"))?;
                }
            }

            Ok(())
        })
    };

    // Drop the senders to signal completion
    drop(frame_tx);
    drop(scene_tx);

    // Collect results from both threads
    let mut frames = Vec::new();
    let mut scene_cuts = Vec::new();

    // Collect frames
    while let Ok(frame) = frame_rx.recv() {
        frames.push(frame);
    }

    // Collect scene cuts
    while let Ok(scene_cut) = scene_rx.recv() {
        scene_cuts.push(scene_cut);
    }

    // Wait for both threads to complete
    frame_thread.join().map_err(|_| anyhow::anyhow!("Frame processing thread panicked"))??;
    scene_thread.join().map_err(|_| anyhow::anyhow!("Scene detection thread panicked"))??;

    // Wait for ffmpeg to finish
    let status = child.wait().context("Failed to wait for ffmpeg")?;
    if !status.success() {
        // Try to get stderr content for debugging
        let stderr_output = if let Ok(stderr_content) = stderr_content_rx.try_recv() {
            stderr_content
        } else {
            "No stderr information available".to_string()
        };
        
        anyhow::bail!("ffmpeg consolidated analysis failed. Exit status: {}. FFmpeg stderr: {}", status, stderr_output);
    }

    // Convert scene cuts to scenes
    let mut scenes = Vec::new();
    let mut start_frame = 0u32;

    // Sort scene cuts to ensure proper ordering
    scene_cuts.sort_unstable();

    for &cut_frame in &scene_cuts {
        scenes.push(MadVRScene {
            start: start_frame,
            end: cut_frame,
            peak_nits: 0,      // Will be calculated later
            avg_pq: 0.0,       // Will be calculated later
            ..Default::default() // Let the library handle other fields
        });
        start_frame = cut_frame + 1;
    }

    // Add final scene if there are any cuts
    if !scene_cuts.is_empty() {
        scenes.push(MadVRScene {
            start: start_frame,
            end: u32::MAX, // Will be updated with actual frame count
            peak_nits: 0,
            avg_pq: 0.0, // Will be calculated later
            ..Default::default() // Let the library handle other fields
        });
    } else {
        // No scene cuts detected, create single scene
        scenes.push(MadVRScene {
            start: 0,
            end: u32::MAX, // Will be updated with actual frame count
            peak_nits: 0,
            avg_pq: 0.0, // Will be calculated later
            ..Default::default() // Let the library handle other fields
        });
    }

    println!("Scene detection completed: {} scenes detected", scenes.len());

    Ok((scenes, frames))
}





/// Analyze a single frame's data to extract HDR metadata.
///
/// This function processes raw frame data to compute:
/// - Peak PQ value (brightest pixel converted to PQ space)
/// - 256-bin PQ-based luminance histogram
/// - Average PQ value derived from the histogram
///
/// Supports both luminance-only (gray) and RGB24 formats for hardware acceleration compatibility.
///
/// # Arguments
/// * `frame_data` - Raw pixel data (1 byte per pixel for gray, 3 bytes for RGB24)
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
/// * `bytes_per_pixel` - Number of bytes per pixel (1 for gray, 3 for RGB24)
///
/// # Returns
/// `Result<MadVRFrame>` - Analyzed frame data with PQ values and histogram
fn analyze_single_frame(frame_data: &[u8], width: u32, height: u32, bytes_per_pixel: usize) -> Result<MadVRFrame> {
    let pixel_count = (width * height) as usize;
    let mut histogram = vec![0f64; 256];
    let mut max_luminance = 0u8;

    // Process pixels based on format (1 byte for luminance, 3 bytes for RGB)
    if bytes_per_pixel == 1 {
        // Luminance-only processing (gray format)
        for &luminance in frame_data {
            // Find peak luminance
            max_luminance = max_luminance.max(luminance);

            // Convert 8-bit luminance to a linear float (0.0-1.0)
            let linear_lum = luminance as f64 / 255.0;
            // Scale to a nit value (0-10000)
            let pixel_nits = linear_lum * 10000.0;
            // Convert nits to a PQ value (0.0-1.0)
            let pixel_pq = nits_to_pq(pixel_nits as u32);

            // A pixel's PQ value (0.0 to 1.0) maps directly to the 256 bins
            let bin_index = (pixel_pq * 255.0).round() as usize;
            let bin_index = bin_index.min(255); // Clamp to be safe

            histogram[bin_index] += 1.0;
        }
    } else if bytes_per_pixel == 3 {
        // RGB24 processing (fallback for VideoToolbox compatibility)
        for pixel_idx in 0..pixel_count {
            let base_idx = pixel_idx * 3;
            let r = frame_data[base_idx];
            let g = frame_data[base_idx + 1];
            let b = frame_data[base_idx + 2];

            // Find peak brightness (max of any color channel)
            max_luminance = max_luminance.max(r).max(g).max(b);

            // Calculate luminance using Rec. 709/2020 coefficients for perceptual accuracy
            let r_f64 = r as f64;
            let g_f64 = g as f64;
            let b_f64 = b as f64;

            let luminance = (0.2126 * r_f64 + 0.7152 * g_f64 + 0.0722 * b_f64).round() as u8;

            // Convert 8-bit luminance to a linear float (0.0-1.0)
            let linear_lum = luminance as f64 / 255.0;
            // Scale to a nit value (0-10000)
            let pixel_nits = linear_lum * 10000.0;
            // Convert nits to a PQ value (0.0-1.0)
            let pixel_pq = nits_to_pq(pixel_nits as u32);

            // A pixel's PQ value (0.0 to 1.0) maps directly to the 256 bins
            let bin_index = (pixel_pq * 255.0).round() as usize;
            let bin_index = bin_index.min(255); // Clamp to be safe

            histogram[bin_index] += 1.0;
        }
    } else {
        anyhow::bail!("Unsupported pixel format: {} bytes per pixel", bytes_per_pixel);
    }

    // Normalize histogram so sum equals 100.0
    let total_pixels = pixel_count as f64;
    for bin in &mut histogram {
        *bin = (*bin / total_pixels) * 100.0;
    }

    // Calculate peak PQ from the brightest luminance value
    let linear = max_luminance as f64 / 255.0;
    let nits = linear * 10000.0;
    let peak_pq = nits_to_pq(nits as u32);

    // Calculate average PQ from the histogram
    let avg_pq = calculate_avg_pq_from_histogram(&histogram);

    Ok(MadVRFrame {
        peak_pq_2020: peak_pq, // Use the correct field name from madvr_parse
        avg_pq,
        lum_histogram: histogram,
        hue_histogram: Some(vec![0f64; 31]), // Add empty hue histogram for v6 compatibility (31 bins)
        target_nits: None, // Will be set by optimizer if enabled
        ..Default::default() // Let the library handle other fields
    })
}

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
            let max_peak_pq = scene_frames.iter().map(|f| f.peak_pq_2020).fold(0.0f64, f64::max);
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
fn run_optimizer_pass(frames: &mut [MadVRFrame]) {
    const ROLLING_WINDOW_SIZE: usize = 240; // 240 frames as recommended by research

    let mut rolling_avg_queue: VecDeque<f64> = VecDeque::with_capacity(ROLLING_WINDOW_SIZE);
    let total_frames = frames.len();

    println!(
        "Applying advanced optimization heuristics with {}-frame rolling window...",
        ROLLING_WINDOW_SIZE
    );

    for (frame_idx, frame) in frames.iter_mut().enumerate() {
        // Add current frame's avg_pq to rolling window
        rolling_avg_queue.push_back(frame.avg_pq);

        // Remove oldest frame if window is full
        if rolling_avg_queue.len() > ROLLING_WINDOW_SIZE {
            rolling_avg_queue.pop_front();
        }

        // Calculate rolling average
        let rolling_avg_pq: f64 =
            rolling_avg_queue.iter().sum::<f64>() / rolling_avg_queue.len() as f64;

        // Convert peak PQ to nits for decision making
        let peak_nits = pq_to_nits(frame.peak_pq_2020) as u32;

        // Find highlight knee (99th percentile)
        let highlight_knee_nits = find_highlight_knee_nits(&frame.lum_histogram);

        // Convert rolling average PQ back to approximate APL in nits
        let rolling_apl_nits = pq_to_nits(rolling_avg_pq);

        // Apply advanced heuristics
        frame.target_nits = Some(apply_advanced_heuristics(
            peak_nits,
            rolling_apl_nits,
            highlight_knee_nits,
        ));

        // Progress indicator for long videos
        if frame_idx % 1000 == 0 && frame_idx > 0 {
            let progress = (frame_idx as f64 / total_frames as f64) * 100.0;
            print!("\rOptimizer progress: {:.1}%", progress);
            std::io::stdout().flush().unwrap_or(());
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
) -> u16 {
    // Heuristic 1: Hard cap for extreme peaks (prevents flicker and blown-out highlights)
    if peak_nits > 4000 {
        return (highlight_knee_nits.min(4000.0)) as u16;
    }

    // Heuristic 2: Use rolling average to smooth transitions and prevent temporal artifacts
    if rolling_apl_nits < 50.0 {
        // Dark scene - be more aggressive, allow brighter targets to preserve shadow detail
        // But still respect the highlight knee to prevent blown highlights
        let target = peak_nits.clamp(800, 2000); // Minimum 800 nits for dark scenes
        (target as f64).min(highlight_knee_nits * 1.2) as u16 // Allow 20% above knee for dark scenes
    } else if rolling_apl_nits < 150.0 {
        // Medium brightness scene - balanced approach
        let target = peak_nits.clamp(600, 1500);
        (target as f64).min(highlight_knee_nits * 1.1) as u16 // Allow 10% above knee
    } else {
        // Bright scene - be more conservative to prevent blown-out look
        let target = peak_nits.clamp(400, 1000);
        (target as f64).min(highlight_knee_nits) as u16 // Respect the highlight knee strictly
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
///
/// # Returns
/// `Result<()>` - Ok(()) on successful write, Err on failure
fn write_measurement_file(
    output_path: &str,
    scenes: &[MadVRScene],
    frames: &[MadVRFrame],
    enable_optimizer: bool,
) -> Result<()> {
    // 1. Create the Header
    let maxcll = frames.iter()
        .map(|f| pq_to_nits(f.peak_pq_2020) as u32)
        .max()
        .unwrap_or(0);

    let header = MadVRHeader {
        version: 5,
        header_size: 32,
        scene_count: scenes.len() as u32,
        frame_count: frames.len() as u32,
        flags: if enable_optimizer { 3 } else { 2 },
        maxcll,
        ..Default::default() // Let the library handle other default values
    };

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
        owned_frames.push(MadVRFrame {
            peak_pq_2020: frame.peak_pq_2020,
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
    let binary_data = measurements.write_measurements()
        .context("Failed to serialize measurements using madvr_parse library")?;

    // 4. Write the resulting bytes to a file
    std::fs::write(output_path, binary_data)
        .context("Failed to write binary data to output file")?;

    println!("Successfully wrote measurement file.");
    println!("MaxCLL: {} nits", maxcll);

    Ok(())
}
