# Cargo.toml

```toml
[package]
name = "hdr_analyzer_mvp"
version = "0.1.0"
edition = "2021"
authors = ["HDR Analyzer Team"]
description = "Phase 1 MVP for HDR10 to Dolby Vision converter - HDR video analysis tool"
license = "MIT"

[[bin]]
name = "hdr_analyzer_mvp"
path = "src/main.rs"

[dependencies]
clap = { version = "4.4", features = ["derive"] }
byteorder = "1.5"
anyhow = "1.0"

```

# HDR10-LG-Cymatic-Jazz.mkv

This is a binary file of the type: Binary

# improved_measurements.bin

This is a binary file of the type: Binary

# longer_measurements.bin

This is a binary file of the type: Binary

# longer_test_video.mp4

This is a binary file of the type: Binary

# output_measurements.bin

This is a binary file of the type: Binary

# README.md

```md
# HDR Analyzer MVP

A command-line tool for analyzing HDR video files and generating measurement files compatible with the `madvr_parse` library format. This tool serves as the Phase 1 MVP for an open-source HDR10 to Dolby Vision converter.

## Features

- **Scene Detection**: Automatically detects scene cuts using ffmpeg's scene detection filter
- **Per-Frame Analysis**: Analyzes each frame for peak brightness and luminance distribution
- **Binary Output**: Generates `.bin` measurement files in madvr format (version 5)
- **HDR Metrics**: Calculates peak PQ values and luminance histograms for each frame
- **Enhanced Progress Reporting**: Visual progress bar with ETA, processing rate, and completion percentage

## Prerequisites

- **Rust**: Install from [rustup.rs](https://rustup.rs/)
- **FFmpeg**: Must be installed and available in your system's PATH
  - On macOS: `brew install ffmpeg`
  - On Ubuntu/Debian: `sudo apt install ffmpeg`
  - On Windows: Download from [ffmpeg.org](https://ffmpeg.org/download.html)

## Installation

1. Clone or download this repository
2. Build the project:
   \`\`\`bash
   cargo build --release
   \`\`\`

   The build should complete without any warnings or errors.

## Usage

\`\`\`bash
cargo run -- -i <input_video> -o <output_file.bin>
\`\`\`

Or if you've built the release version:

\`\`\`bash
./target/release/hdr_analyzer_mvp -i <input_video> -o <output_file.bin>
\`\`\`

### Arguments

- `-i, --input <PATH>`: Path to the input HDR video file
- `-o, --output <PATH>`: Path for the output `.bin` measurement file

### Example

\`\`\`bash
cargo run -- -i sample_hdr_video.mkv -o measurements.bin
\`\`\`

## Output Format

The tool generates a binary measurement file compatible with the `madvr_parse` library format (version 5). The file contains:

- **Header**: Version info, scene/frame counts, and metadata
- **Scene Data**: Scene boundaries and peak nits per scene
- **Frame Data**: Per-frame peak PQ values and 256-bin luminance histograms

## Technical Details

### Scene Detection
- Uses ffmpeg's `scdet` filter with a threshold of 30
- Automatically segments the video into scenes based on content changes

### Frame Analysis
- Decodes video to raw RGB24 format via ffmpeg pipe
- Calculates peak brightness per frame (maximum RGB component)
- Generates 256-bin luminance histograms
- Converts brightness values to PQ (Perceptual Quantizer) format

### PQ Conversion
Uses the ST.2084 (SMPTE-2084) standard for PQ conversion:
- Supports up to 10,000 nits peak brightness
- Accurate perceptual quantization for HDR content

### Progress Reporting
Enhanced real-time progress display during frame analysis:
- **Visual Progress Bar**: ASCII progress bar showing completion status
- **Percentage Complete**: Accurate percentage based on total frame count
- **Processing Rate**: Real-time frames per second (fps) counter
- **ETA**: Estimated time remaining for completion
- **Single-line Updates**: Clean terminal output using carriage return

Example progress display:
\`\`\`
Processing frames: [=========>    ] 67% (1340/2000) | 12.5 fps | ETA: 00:52
\`\`\`

## Limitations (MVP)

- Simplified luminance calculation (RGB average)
- Basic histogram binning strategy
- No custom per-frame target nits support
- Limited error recovery for malformed input

## Dependencies

- `clap`: Command-line argument parsing
- `byteorder`: Binary data serialization
- `anyhow`: Error handling

## License

MIT License - see LICENSE file for details.

## Contributing

This is an MVP implementation. Future enhancements may include:
- More sophisticated luminance calculations
- Advanced histogram analysis
- Support for additional HDR formats
- Performance optimizations for large files

```

# src/main.rs

```rs
use anyhow::{Context, Result};
use byteorder::{LittleEndian, WriteBytesExt};
use clap::Parser;
use std::collections::VecDeque;
use std::io::{BufReader, BufWriter, Read, Write};
use std::process::{Command, Stdio};
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
}

// --- Data Structures ---
#[derive(Debug, Default, Clone)]
struct MvpScene {
    start: u32,
    end: u32,
    peak_nits: u32,
    scene_avg_pq: f64, // Average Picture Level for the entire scene
}

#[derive(Debug, Default)]
struct MvpFrame {
    peak_pq: f64,
    avg_pq: f64,
    lum_histogram: Vec<f64>, // Should have 256 elements
    target_nits: Option<u16>,
}

// --- Constants for PQ Conversion ---
const ST2084_Y_MAX: f64 = 10000.0;
const ST2084_M1: f64 = 2610.0 / 16384.0;
const ST2084_M2: f64 = (2523.0 / 4096.0) * 128.0;
const ST2084_C1: f64 = 3424.0 / 4096.0;
const ST2084_C2: f64 = (2413.0 / 4096.0) * 32.0;
const ST2084_C3: f64 = (2392.0 / 4096.0) * 32.0;

// --- Formulas ---
fn nits_to_pq(nits: u32) -> f64 {
    let y = nits as f64 / ST2084_Y_MAX;
    ((ST2084_C1 + ST2084_C2 * y.powf(ST2084_M1)) / (1.0 + ST2084_C3 * y.powf(ST2084_M1)))
        .powf(ST2084_M2)
}

/// Calculate average PQ from histogram data
/// The histogram represents PQ values directly, with each bin corresponding to a PQ range
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

/// Convert PQ value back to nits (inverse PQ function)
fn pq_to_nits(pq: f64) -> f64 {
    if pq <= 0.0 {
        return 0.0;
    }

    let y = ((pq.powf(1.0 / ST2084_M2) - ST2084_C1).max(0.0) / (ST2084_C2 - ST2084_C3 * pq.powf(1.0 / ST2084_M2))).powf(1.0 / ST2084_M1);
    y * ST2084_Y_MAX
}

/// Find the 99th percentile (highlight knee) from the luminance histogram
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

/// Format duration as MM:SS
fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}", minutes, seconds)
}

/// Create a progress bar string
fn create_progress_bar(current: u32, total: Option<u32>, width: usize) -> String {
    let (percentage, filled_width) = if let Some(total) = total {
        if total > 0 {
            let pct = (current as f64 / total as f64 * 100.0).min(100.0);
            let filled = ((current as f64 / total as f64) * width as f64) as usize;
            (pct, filled.min(width))
        } else {
            (0.0, 0)
        }
    } else {
        // Unknown total - show a spinning indicator
        let spin_pos = ((current / 10) as usize) % width;
        return format!("[{}=>{}] Processing...",
                      " ".repeat(spin_pos),
                      " ".repeat(width.saturating_sub(spin_pos + 2)));
    };

    let filled = "=".repeat(filled_width);
    let arrow = if filled_width < width { ">" } else { "" };
    let empty = " ".repeat(width.saturating_sub(filled_width).saturating_sub(if arrow.is_empty() { 0 } else { 1 }));

    format!("[{}{}{}] {:3.0}%", filled, arrow, empty, percentage)
}



fn main() -> Result<()> {
    let cli = Cli::parse();

    println!("HDR Analyzer MVP - Starting analysis of: {}", cli.input);

    // Step 1: Get video info using ffprobe
    let (width, height, total_frames) = get_video_info(&cli.input)?;
    println!("Video resolution: {}x{}", width, height);
    if let Some(frames) = total_frames {
        println!("Total frames: {}", frames);
    }

    // Step 2: Scene detection via ffmpeg
    println!("Performing scene detection...");
    let mut scenes = detect_scenes(&cli.input)?;
    println!("Detected {} scenes", scenes.len());

    // Step 3: Per-frame analysis via ffmpeg pipe
    println!("Starting per-frame analysis...");
    let mut frames = analyze_frames(&cli.input, width, height, total_frames)?;
    println!("Analyzed {} frames", frames.len());

    // Step 4: Pre-compute scene statistics
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

/// Get video resolution and frame count using ffprobe
fn get_video_info(input_path: &str) -> Result<(u32, u32, Option<u32>)> {
    // Get resolution
    let resolution_output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height",
            "-of", "csv=s=x:p=0",
            input_path
        ])
        .output()
        .context("Failed to execute ffprobe - make sure ffmpeg is installed")?;

    if !resolution_output.status.success() {
        anyhow::bail!("ffprobe failed: {}", String::from_utf8_lossy(&resolution_output.stderr));
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
            "-v", "error",
            "-select_streams", "v:0",
            "-count_packets",
            "-show_entries", "stream=nb_read_packets",
            "-of", "csv=p=0",
            input_path
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

/// Detect scene cuts using ffmpeg (optimized for speed)
fn detect_scenes(input_path: &str) -> Result<Vec<MvpScene>> {
    println!("Scene detection in progress (this may take a moment for large files)...");

    // Use lower resolution for scene detection to speed up processing
    // Scene cuts don't require full resolution analysis
    let mut child = Command::new("ffmpeg")
        .args([
            "-i", input_path,
            "-vf", "scale=640:360,scdet=threshold=30,metadata=print",
            "-f", "null",
            "-"
        ])
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("Failed to execute ffmpeg for scene detection")?;

    let stderr = child.stderr.take().context("Failed to capture stderr")?;
    let mut stderr_reader = BufReader::new(stderr);
    let mut stderr_content = String::new();
    stderr_reader.read_to_string(&mut stderr_content)
        .context("Failed to read ffmpeg stderr")?;

    let status = child.wait().context("Failed to wait for ffmpeg")?;
    if !status.success() {
        anyhow::bail!("ffmpeg scene detection failed");
    }

    // Parse scene cuts from stderr - simplified approach for MVP
    let mut scene_cuts = Vec::new();
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
            scene_cuts.push(current_frame);
        }
    }

    // Convert scene cuts to scenes
    let mut scenes = Vec::new();
    let mut start_frame = 0u32;

    for &cut_frame in &scene_cuts {
        scenes.push(MvpScene {
            start: start_frame,
            end: cut_frame,
            peak_nits: 0, // Will be calculated later
            scene_avg_pq: 0.0, // Will be calculated later
        });
        start_frame = cut_frame + 1;
    }

    // Add final scene if there are any cuts
    if !scene_cuts.is_empty() {
        scenes.push(MvpScene {
            start: start_frame,
            end: u32::MAX, // Will be updated with actual frame count
            peak_nits: 0,
            scene_avg_pq: 0.0, // Will be calculated later
        });
    } else {
        // No scene cuts detected, create single scene
        scenes.push(MvpScene {
            start: 0,
            end: u32::MAX, // Will be updated with actual frame count
            peak_nits: 0,
            scene_avg_pq: 0.0, // Will be calculated later
        });
    }

    Ok(scenes)
}

/// Analyze frames using ffmpeg pipe
fn analyze_frames(input_path: &str, width: u32, height: u32, total_frames: Option<u32>) -> Result<Vec<MvpFrame>> {
    let mut child = Command::new("ffmpeg")
        .args([
            "-i", input_path,
            "-f", "rawvideo",
            "-pix_fmt", "rgb24",
            "-"
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to execute ffmpeg for frame analysis")?;

    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let mut stdout_reader = std::io::BufReader::new(stdout);

    let frame_size = (width * height * 3) as usize; // RGB24 = 3 bytes per pixel
    let mut frame_buffer = vec![0u8; frame_size];
    let mut frames = Vec::new();
    let mut frame_count = 0u32;

    // Progress tracking variables
    let start_time = Instant::now();
    let mut last_progress_update = Instant::now();
    let progress_update_interval = Duration::from_millis(500); // Update every 500ms

    println!("Processing frames ({}x{}, {} bytes per frame)...", width, height, frame_size);
    print!("Initializing frame analysis...");
    std::io::stdout().flush().unwrap_or(());

    loop {
        match stdout_reader.read_exact(&mut frame_buffer) {
            Ok(()) => {
                // Process this frame
                let frame = analyze_single_frame(&frame_buffer, width, height)?;
                frames.push(frame);
                frame_count += 1;

                // Update progress display periodically
                let now = Instant::now();
                if now.duration_since(last_progress_update) >= progress_update_interval || frame_count == 1 {
                    last_progress_update = now;

                    // Calculate processing rate (frames per second)
                    let elapsed = now.duration_since(start_time);
                    let fps = if elapsed.as_secs_f64() > 0.0 {
                        frame_count as f64 / elapsed.as_secs_f64()
                    } else {
                        0.0
                    };

                    // Create progress display
                    let progress_bar = create_progress_bar(frame_count, total_frames, 20);

                    let display = if let Some(total) = total_frames {
                        // Calculate ETA
                        let eta_str = if fps > 0.0 && frame_count < total {
                            let remaining_frames = total.saturating_sub(frame_count);
                            let eta_seconds = remaining_frames as f64 / fps;
                            format!("ETA: {}", format_duration(Duration::from_secs_f64(eta_seconds)))
                        } else {
                            "ETA: --:--".to_string()
                        };

                        format!("\rProcessing frames: {} ({}/{}) | {:.1} fps | {}",
                               progress_bar, frame_count, total, fps, eta_str)
                    } else {
                        format!("\rProcessing frames: {} ({}) | {:.1} fps | Analyzing...",
                               progress_bar, frame_count, fps)
                    };

                    print!("{}", display);
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

    // Wait for ffmpeg to finish
    let status = child.wait().context("Failed to wait for ffmpeg")?;
    if !status.success() {
        anyhow::bail!("ffmpeg frame analysis failed");
    }

    // Final completion message on a new line
    let total_elapsed = start_time.elapsed();
    let final_fps = if total_elapsed.as_secs_f64() > 0.0 {
        frame_count as f64 / total_elapsed.as_secs_f64()
    } else {
        0.0
    };

    println!("\nCompleted processing {} frames in {} ({:.1} fps average)",
             frame_count, format_duration(total_elapsed), final_fps);
    Ok(frames)
}

/// Analyze a single frame's RGB data
fn analyze_single_frame(frame_data: &[u8], width: u32, height: u32) -> Result<MvpFrame> {
    let pixel_count = (width * height) as usize;
    let mut histogram = vec![0f64; 256];
    let mut max_byte = 0u8;

    // Process each pixel (3 bytes: RGB)
    for pixel_idx in 0..pixel_count {
        let base_idx = pixel_idx * 3;
        let r = frame_data[base_idx];
        let g = frame_data[base_idx + 1];
        let b = frame_data[base_idx + 2];

        // Find peak brightness (max of any color channel)
        max_byte = max_byte.max(r).max(g).max(b);

        // Calculate luminance (simple average for MVP)
        let luminance = ((r as u32 + g as u32 + b as u32) / 3) as u8;

        // NEW PQ-BASED HISTOGRAM LOGIC:
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

    // Normalize histogram so sum equals 100.0
    let total_pixels = pixel_count as f64;
    for bin in &mut histogram {
        *bin = (*bin / total_pixels) * 100.0;
    }

    // Calculate peak PQ
    let linear = max_byte as f64 / 255.0;
    let nits = linear * 10000.0;
    let peak_pq = nits_to_pq(nits as u32);

    // Calculate average PQ from the histogram
    let avg_pq = calculate_avg_pq_from_histogram(&histogram);

    Ok(MvpFrame {
        peak_pq,
        avg_pq,
        lum_histogram: histogram,
        target_nits: None, // Will be set by optimizer if enabled
    })
}

/// Pre-compute scene-based statistics
fn precompute_scene_stats(scenes: &mut Vec<MvpScene>, frames: &[MvpFrame]) {
    println!("Computing scene-based statistics...");

    for scene in scenes.iter_mut() {
        let start_idx = scene.start as usize;
        let end_idx = ((scene.end + 1) as usize).min(frames.len());

        if start_idx < frames.len() && start_idx < end_idx {
            let scene_frames = &frames[start_idx..end_idx];

            // Calculate average PQ for the entire scene
            let total_avg_pq: f64 = scene_frames.iter().map(|f| f.avg_pq).sum();
            scene.scene_avg_pq = total_avg_pq / scene_frames.len() as f64;
        }
    }
}

/// Advanced optimizer with rolling averages and scene-aware heuristics
fn run_optimizer_pass(frames: &mut Vec<MvpFrame>) {
    const ROLLING_WINDOW_SIZE: usize = 240; // 240 frames as recommended by research

    let mut rolling_avg_queue: VecDeque<f64> = VecDeque::with_capacity(ROLLING_WINDOW_SIZE);
    let total_frames = frames.len();

    println!("Applying advanced optimization heuristics with {}-frame rolling window...", ROLLING_WINDOW_SIZE);

    for (frame_idx, frame) in frames.iter_mut().enumerate() {
        // Add current frame's avg_pq to rolling window
        rolling_avg_queue.push_back(frame.avg_pq);

        // Remove oldest frame if window is full
        if rolling_avg_queue.len() > ROLLING_WINDOW_SIZE {
            rolling_avg_queue.pop_front();
        }

        // Calculate rolling average
        let rolling_avg_pq: f64 = rolling_avg_queue.iter().sum::<f64>() / rolling_avg_queue.len() as f64;

        // Convert peak PQ to nits for decision making
        let peak_nits = pq_to_nits(frame.peak_pq) as u32;

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

/// Apply advanced optimization heuristics
fn apply_advanced_heuristics(peak_nits: u32, rolling_apl_nits: f64, highlight_knee_nits: f64) -> u16 {
    // Heuristic 1: Hard cap for extreme peaks (prevents flicker and blown-out highlights)
    if peak_nits > 4000 {
        return (highlight_knee_nits.min(4000.0)) as u16;
    }

    // Heuristic 2: Use rolling average to smooth transitions and prevent temporal artifacts
    if rolling_apl_nits < 50.0 {
        // Dark scene - be more aggressive, allow brighter targets to preserve shadow detail
        // But still respect the highlight knee to prevent blown highlights
        let target = peak_nits.min(2000).max(800); // Minimum 800 nits for dark scenes
        (target as f64).min(highlight_knee_nits * 1.2) as u16 // Allow 20% above knee for dark scenes
    } else if rolling_apl_nits < 150.0 {
        // Medium brightness scene - balanced approach
        let target = peak_nits.min(1500).max(600);
        (target as f64).min(highlight_knee_nits * 1.1) as u16 // Allow 10% above knee
    } else {
        // Bright scene - be more conservative to prevent blown-out look
        let target = peak_nits.min(1000).max(400);
        (target as f64).min(highlight_knee_nits) as u16 // Respect the highlight knee strictly
    }
}

/// Write the measurement file in madvr format
fn write_measurement_file(output_path: &str, scenes: &[MvpScene], frames: &[MvpFrame], enable_optimizer: bool) -> Result<()> {
    let file = std::fs::File::create(output_path)
        .context("Failed to create output file")?;
    let mut writer = BufWriter::new(file);

    // Update scenes with actual frame count and peak nits
    let mut updated_scenes = scenes.to_vec();
    let frame_count = frames.len() as u32;

    // Update the last scene's end frame
    if let Some(last_scene) = updated_scenes.last_mut() {
        if last_scene.end == u32::MAX {
            last_scene.end = frame_count.saturating_sub(1);
        }
    }

    // Calculate peak nits for each scene
    for scene in &mut updated_scenes {
        let start_idx = scene.start as usize;
        let end_idx = (scene.end as usize + 1).min(frames.len());

        if start_idx < frames.len() && start_idx < end_idx {
            let scene_frames = &frames[start_idx..end_idx];
            let max_peak_pq = scene_frames.iter()
                .map(|f| f.peak_pq)
                .fold(0.0f64, f64::max);

            // Convert PQ back to nits for storage
            // This is a simplified reverse conversion for MVP
            let nits = (max_peak_pq * ST2084_Y_MAX) as u32;
            scene.peak_nits = nits.min(10000);
        }
    }

    // Calculate maxcll (maximum content light level)
    let maxcll = frames.iter()
        .map(|f| (f.peak_pq * ST2084_Y_MAX) as u32)
        .max()
        .unwrap_or(0)
        .min(10000);

    // Determine flags based on optimizer usage
    let flags = if enable_optimizer { 3 } else { 2 };

    // Write magic code
    writer.write_all(b"mvr+").context("Failed to write magic code")?;

    // Write header
    writer.write_u32::<LittleEndian>(5).context("Failed to write version")?; // version
    writer.write_u32::<LittleEndian>(32).context("Failed to write header_size")?; // header_size
    writer.write_u32::<LittleEndian>(updated_scenes.len() as u32).context("Failed to write scene_count")?;
    writer.write_u32::<LittleEndian>(frame_count).context("Failed to write frame_count")?;
    writer.write_u32::<LittleEndian>(flags).context("Failed to write flags")?;
    writer.write_u32::<LittleEndian>(maxcll).context("Failed to write maxcll")?;
    writer.write_u32::<LittleEndian>(0).context("Failed to write maxfall")?; // maxfall (MVP: 0)
    writer.write_u32::<LittleEndian>(0).context("Failed to write avgfall")?; // avgfall (MVP: 0)

    // Write scenes block
    // First: scene starts
    for scene in &updated_scenes {
        writer.write_u32::<LittleEndian>(scene.start).context("Failed to write scene start")?;
    }

    // Second: scene ends + 1
    for scene in &updated_scenes {
        writer.write_u32::<LittleEndian>(scene.end + 1).context("Failed to write scene end")?;
    }

    // Third: scene peak nits
    for scene in &updated_scenes {
        writer.write_u32::<LittleEndian>(scene.peak_nits).context("Failed to write scene peak nits")?;
    }

    // Write frames block
    for frame in frames {
        // Write peak_pq_2020 as u16
        let peak_pq_2020 = (frame.peak_pq * 64000.0).round() as u16;
        writer.write_u16::<LittleEndian>(peak_pq_2020).context("Failed to write peak_pq_2020")?;

        // Write histogram (256 u16 values)
        for &hist_value in &frame.lum_histogram {
            let hist_u16 = (hist_value * 640.0).round() as u16;
            writer.write_u16::<LittleEndian>(hist_u16).context("Failed to write histogram value")?;
        }
    }

    // (Phase 3) Write the custom per-frame target nits block if optimizer was enabled
    if enable_optimizer {
        println!("Writing custom target nits block...");
        for frame in frames {
            // If a frame has a target, write it. Otherwise, write 0 as a default.
            let target_nits = frame.target_nits.unwrap_or(0);
            writer.write_u16::<LittleEndian>(target_nits).context("Failed to write target_nits")?;
        }
    }

    writer.flush().context("Failed to flush output file")?;

    println!("Successfully wrote measurement file with {} scenes and {} frames",
             updated_scenes.len(), frame_count);
    println!("MaxCLL: {} nits", maxcll);

    Ok(())
}

```

# test_optimized.bin

This is a binary file of the type: Binary

# test_tool.sh

```sh
#!/bin/bash

# Test script for HDR Analyzer MVP
# This script demonstrates how to use the tool with a sample video

echo "HDR Analyzer MVP Test Script"
echo "============================"

# Check if ffmpeg is available
if ! command -v ffmpeg &> /dev/null; then
    echo "Error: ffmpeg is not installed or not in PATH"
    echo "Please install ffmpeg first:"
    echo "  macOS: brew install ffmpeg"
    echo "  Ubuntu/Debian: sudo apt install ffmpeg"
    exit 1
fi

# Build the tool
echo "Building HDR Analyzer MVP..."
cargo build --release

if [ $? -ne 0 ]; then
    echo "Error: Failed to build the tool"
    exit 1
fi

echo "Build successful!"

# Check if a test video file is provided
if [ $# -eq 0 ]; then
    echo ""
    echo "Usage: $0 <input_video_file>"
    echo ""
    echo "Example:"
    echo "  $0 sample_hdr_video.mkv"
    echo ""
    echo "The tool will create a measurement file named 'output_measurements.bin'"
    echo ""
    echo "To create a simple test video with ffmpeg:"
    echo "  ffmpeg -f lavfi -i testsrc2=duration=10:size=1920x1080:rate=24 -pix_fmt yuv420p test_video.mp4"
    exit 1
fi

INPUT_FILE="$1"
OUTPUT_FILE="output_measurements.bin"

# Check if input file exists
if [ ! -f "$INPUT_FILE" ]; then
    echo "Error: Input file '$INPUT_FILE' does not exist"
    exit 1
fi

echo ""
echo "Input file: $INPUT_FILE"
echo "Output file: $OUTPUT_FILE"
echo ""

# Run the tool
echo "Running HDR Analyzer MVP..."
./target/release/hdr_analyzer_mvp -i "$INPUT_FILE" -o "$OUTPUT_FILE"

if [ $? -eq 0 ]; then
    echo ""
    echo "Analysis completed successfully!"
    echo "Output file: $OUTPUT_FILE"
    
    if [ -f "$OUTPUT_FILE" ]; then
        echo "File size: $(ls -lh "$OUTPUT_FILE" | awk '{print $5}')"
        echo "File type: $(file "$OUTPUT_FILE")"
    fi
else
    echo ""
    echo "Error: Analysis failed"
    exit 1
fi

```

# test_video.mp4

This is a binary file of the type: Binary

