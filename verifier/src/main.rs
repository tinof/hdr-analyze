//! Verification tool for MadVR measurement files
//!
//! This tool can read and validate MadVR measurement files, displaying
//! their contents and verifying the format integrity.

use anyhow::{Context, Result};
use madvr_parse::{MadVRScene, MadVRFrame};
use std::env;
use std::fs::File;
use std::io::{BufReader, Read};

// --- Constants for PQ Conversion ---
const ST2084_Y_MAX: f64 = 10000.0;
const ST2084_M1: f64 = 2610.0 / 16384.0;
const ST2084_M2: f64 = (2523.0 / 4096.0) * 128.0;
const ST2084_C1: f64 = 3424.0 / 4096.0;
const ST2084_C2: f64 = (2413.0 / 4096.0) * 32.0;
const ST2084_C3: f64 = (2392.0 / 4096.0) * 32.0;

/// Convert PQ value back to nits (inverse PQ function).
fn pq_to_nits(pq: f64) -> f64 {
    if pq <= 0.0 {
        return 0.0;
    }

    let y = ((pq.powf(1.0 / ST2084_M2) - ST2084_C1).max(0.0)
        / (ST2084_C2 - ST2084_C3 * pq.powf(1.0 / ST2084_M2)))
    .powf(1.0 / ST2084_M1);
    y * ST2084_Y_MAX
}

/// Calculate average PQ from histogram data.
fn calculate_avg_pq_from_histogram(histogram: &[f64]) -> f64 {
    let mut weighted_sum = 0.0;
    let mut total_weight = 0.0;

    for (bin_index, &weight) in histogram.iter().enumerate() {
        if weight > 0.0 {
            // Convert bin index back to PQ value
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

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <measurement_file.bin>", args[0]);
        std::process::exit(1);
    }

    let file_path = &args[1];
    println!("Verifying measurement file: {}", file_path);

    let (scenes, frames, has_optimizer) = read_measurement_file(file_path)?;

    println!("\n=== FILE SUMMARY ===");
    println!("Scenes: {}", scenes.len());
    println!("Frames: {}", frames.len());
    println!("Optimizer data: {}", if has_optimizer { "Yes" } else { "No" });

    if !scenes.is_empty() {
        println!("\n=== SCENE ANALYSIS ===");
        for (i, scene) in scenes.iter().enumerate() {
            println!(
                "Scene {}: frames {}-{}, peak {} nits, avg PQ {:.4}",
                i + 1,
                scene.start,
                scene.end,
                scene.peak_nits,
                scene.avg_pq
            );
        }
    }

    if !frames.is_empty() {
        println!("\n=== FRAME STATISTICS ===");
        let max_peak_pq = frames.iter().map(|f| f.peak_pq_2020).fold(0.0f64, f64::max);
        let avg_peak_pq = frames.iter().map(|f| f.peak_pq_2020).sum::<f64>() / frames.len() as f64;
        let max_avg_pq = frames.iter().map(|f| f.avg_pq).fold(0.0f64, f64::max);
        let avg_avg_pq = frames.iter().map(|f| f.avg_pq).sum::<f64>() / frames.len() as f64;

        println!("Max Peak PQ: {:.4} ({:.0} nits)", max_peak_pq, pq_to_nits(max_peak_pq));
        println!("Avg Peak PQ: {:.4} ({:.0} nits)", avg_peak_pq, pq_to_nits(avg_peak_pq));
        println!("Max Avg PQ: {:.4} ({:.0} nits)", max_avg_pq, pq_to_nits(max_avg_pq));
        println!("Avg Avg PQ: {:.4} ({:.0} nits)", avg_avg_pq, pq_to_nits(avg_avg_pq));

        if has_optimizer {
            let target_nits_count = frames.iter().filter(|f| f.target_nits.is_some()).count();
            println!("Frames with target nits: {}/{}", target_nits_count, frames.len());
            
            if target_nits_count > 0 {
                let avg_target_nits = frames
                    .iter()
                    .filter_map(|f| f.target_nits)
                    .map(|t| t as f64)
                    .sum::<f64>() / target_nits_count as f64;
                println!("Average target nits: {:.0}", avg_target_nits);
            }
        }
    }

    println!("\n=== VALIDATION ===");
    validate_measurement_data(&scenes, &frames)?;
    println!("✓ File format is valid");
    println!("✓ All data integrity checks passed");

    Ok(())
}

/// Read and parse a MadVR measurement file
fn read_measurement_file(file_path: &str) -> Result<(Vec<MadVRScene>, Vec<MadVRFrame>, bool)> {
    let file = File::open(file_path).context("Failed to open measurement file")?;
    let mut reader = BufReader::new(file);

    // Read magic code
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).context("Failed to read magic code")?;
    if &magic != b"mvr+" {
        anyhow::bail!("Invalid magic code: expected 'mvr+', got {:?}", magic);
    }

    // Read header
    let mut header = [0u8; 32];
    reader.read_exact(&mut header).context("Failed to read header")?;
    
    let version = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let header_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let scene_count = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    let frame_count = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
    let flags = u32::from_le_bytes([header[16], header[17], header[18], header[19]]);
    let maxcll = u32::from_le_bytes([header[20], header[21], header[22], header[23]]);

    println!("Version: {}", version);
    println!("Header size: {}", header_size);
    println!("Scene count: {}", scene_count);
    println!("Frame count: {}", frame_count);
    println!("Flags: {}", flags);
    println!("MaxCLL: {} nits", maxcll);

    let has_optimizer = flags == 3;

    // Read scenes
    let mut scenes = Vec::new();
    for _ in 0..scene_count {
        let mut scene_data = [0u8; 12];
        reader.read_exact(&mut scene_data).context("Failed to read scene data")?;
        
        let start = u32::from_le_bytes([scene_data[0], scene_data[1], scene_data[2], scene_data[3]]);
        let end = u32::from_le_bytes([scene_data[4], scene_data[5], scene_data[6], scene_data[7]]);
        let peak_nits = u32::from_le_bytes([scene_data[8], scene_data[9], scene_data[10], scene_data[11]]);

        scenes.push(MadVRScene {
            start,
            end,
            peak_nits,
            avg_pq: 0.0, // Not stored in file format
            ..Default::default() // Let the library handle other fields
        });
    }

    // Read frames
    let mut frames = Vec::new();
    for _ in 0..frame_count {
        // Read peak PQ
        let mut peak_data = [0u8; 2];
        reader.read_exact(&mut peak_data).context("Failed to read peak PQ")?;
        let peak_pq_raw = u16::from_le_bytes(peak_data);
        let peak_pq = peak_pq_raw as f64 / 64000.0;

        // Read histogram
        let mut histogram = vec![0f64; 256];
        for bin in &mut histogram {
            let mut hist_data = [0u8; 2];
            reader.read_exact(&mut hist_data).context("Failed to read histogram data")?;
            let hist_raw = u16::from_le_bytes(hist_data);
            *bin = hist_raw as f64 / 640.0;
        }

        // Calculate average PQ from histogram
        let avg_pq = calculate_avg_pq_from_histogram(&histogram);

        frames.push(MadVRFrame {
            peak_pq_2020: peak_pq, // Use the correct field name
            avg_pq,
            lum_histogram: histogram,
            target_nits: None, // Will be read later if optimizer data exists
            ..Default::default() // Let the library handle other fields
        });
    }

    // Read target nits if optimizer data exists
    if has_optimizer {
        for frame in &mut frames {
            let mut target_data = [0u8; 2];
            reader.read_exact(&mut target_data).context("Failed to read target nits")?;
            let target_nits = u16::from_le_bytes(target_data);
            frame.target_nits = Some(target_nits);
        }
    }

    Ok((scenes, frames, has_optimizer))
}

/// Validate the measurement data for consistency
fn validate_measurement_data(scenes: &[MadVRScene], frames: &[MadVRFrame]) -> Result<()> {
    // Check that scenes cover all frames
    if !scenes.is_empty() && !frames.is_empty() {
        let total_scene_frames: u32 = scenes.iter().map(|s| s.end - s.start + 1).sum();
        if total_scene_frames != frames.len() as u32 {
            anyhow::bail!(
                "Scene frame count mismatch: scenes cover {} frames, but {} frames exist",
                total_scene_frames,
                frames.len()
            );
        }
    }

    // Check histogram integrity
    for (i, frame) in frames.iter().enumerate() {
        if frame.lum_histogram.len() != 256 {
            anyhow::bail!("Frame {} has invalid histogram length: {}", i, frame.lum_histogram.len());
        }

        let histogram_sum: f64 = frame.lum_histogram.iter().sum();
        if (histogram_sum - 100.0).abs() > 1.0 {
            anyhow::bail!("Frame {} has invalid histogram sum: {:.2} (should be ~100.0)", i, histogram_sum);
        }
    }

    // Check PQ values are in valid range
    for (i, frame) in frames.iter().enumerate() {
        if frame.peak_pq_2020 < 0.0 || frame.peak_pq_2020 > 1.0 {
            anyhow::bail!("Frame {} has invalid peak PQ: {:.4}", i, frame.peak_pq_2020);
        }
        if frame.avg_pq < 0.0 || frame.avg_pq > 1.0 {
            anyhow::bail!("Frame {} has invalid avg PQ: {:.4}", i, frame.avg_pq);
        }
    }

    Ok(())
}
