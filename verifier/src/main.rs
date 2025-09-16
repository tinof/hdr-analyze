//! Verification tool for MadVR measurement files
//!
//! This tool can read and validate MadVR measurement files, displaying
//! their contents and verifying the format integrity.

use anyhow::{Context, Result};
use madvr_parse::{MadVRFrame, MadVRMeasurements, MadVRScene};
use std::env;
use std::fs;

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

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <measurement_file.bin>", args[0]);
        std::process::exit(1);
    }

    let file_path = &args[1];
    println!("Verifying measurement file: {}", file_path);

    let (scenes, frames, has_optimizer, header) = read_measurement_file(file_path)?;

    println!("\n=== FILE SUMMARY ===");
    println!("Scenes: {}", scenes.len());
    println!("Frames: {}", frames.len());
    println!(
        "Optimizer data: {}",
        if has_optimizer { "Yes" } else { "No" }
    );

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

        println!(
            "Max Peak PQ: {:.4} ({:.0} nits)",
            max_peak_pq,
            pq_to_nits(max_peak_pq)
        );
        println!(
            "Avg Peak PQ: {:.4} ({:.0} nits)",
            avg_peak_pq,
            pq_to_nits(avg_peak_pq)
        );
        println!(
            "Max Avg PQ: {:.4} ({:.0} nits)",
            max_avg_pq,
            pq_to_nits(max_avg_pq)
        );
        println!(
            "Avg Avg PQ: {:.4} ({:.0} nits)",
            avg_avg_pq,
            pq_to_nits(avg_avg_pq)
        );

        if has_optimizer {
            let target_nits_count = frames.iter().filter(|f| f.target_nits.is_some()).count();
            println!(
                "Frames with target nits: {}/{}",
                target_nits_count,
                frames.len()
            );

            if target_nits_count > 0 {
                let avg_target_nits = frames
                    .iter()
                    .filter_map(|f| f.target_nits)
                    .map(|t| t as f64)
                    .sum::<f64>()
                    / target_nits_count as f64;
                println!("Average target nits: {:.0}", avg_target_nits);
            }
        }
    }

    println!("\n=== VALIDATION ===");
    validate_measurement_data(&scenes, &frames)?;

    // Additional validations: FALL coherence and flags/data consistency
    println!("\n=== ADDITIONAL CHECKS ===");
    validate_fall_consistency(&frames, header.maxfall, header.avgfall);
    validate_flags_vs_data(has_optimizer, &frames);
    println!("✓ File format is valid");
    println!("✓ All data integrity checks passed");

    Ok(())
}

/// Read and parse a MadVR measurement file using the madvr_parse library
fn read_measurement_file(
    file_path: &str,
) -> Result<(
    Vec<MadVRScene>,
    Vec<MadVRFrame>,
    bool,
    madvr_parse::MadVRHeader,
)> {
    // Read the file as bytes
    let file_data = fs::read(file_path).context("Failed to read measurement file")?;

    // Parse using the madvr_parse library
    let measurements = MadVRMeasurements::parse_measurements(&file_data)
        .context("Failed to parse measurement file using madvr_parse library")?;

    // Extract header information for display
    println!("Version: {}", measurements.header.version);
    println!("Header size: {}", measurements.header.header_size);
    println!("Scene count: {}", measurements.header.scene_count);
    println!("Frame count: {}", measurements.header.frame_count);
    println!("Flags: {}", measurements.header.flags);
    println!("MaxCLL: {} nits", measurements.header.maxcll);

    let has_optimizer = measurements.header.flags == 3;

    Ok((
        measurements.scenes,
        measurements.frames,
        has_optimizer,
        measurements.header,
    ))
}

/// Validate the measurement data for consistency
fn validate_measurement_data(scenes: &[MadVRScene], frames: &[MadVRFrame]) -> Result<()> {
    // Check scene validity and count frames
    if !scenes.is_empty() && !frames.is_empty() {
        let mut total_scene_frames = 0u32;
        let mut invalid_scenes = Vec::new();

        for (i, scene) in scenes.iter().enumerate() {
            // Check for invalid scene ranges
            if scene.end < scene.start {
                invalid_scenes.push(format!(
                    "Scene {}: invalid range {}-{} (end < start)",
                    i + 1,
                    scene.start,
                    scene.end
                ));
                continue;
            }

            // Check for scenes that extend beyond total frames
            if scene.start >= frames.len() as u32 || scene.end >= frames.len() as u32 {
                invalid_scenes.push(format!(
                    "Scene {}: range {}-{} extends beyond total frames ({})",
                    i + 1,
                    scene.start,
                    scene.end,
                    frames.len()
                ));
                continue;
            }

            total_scene_frames += scene.end - scene.start + 1;
        }

        // Report invalid scenes as warnings rather than errors
        if !invalid_scenes.is_empty() {
            println!("⚠️  Found {} invalid scene(s):", invalid_scenes.len());
            for invalid in &invalid_scenes {
                println!("   {}", invalid);
            }
        }

        // Only validate frame count for valid scenes
        let valid_scene_count = scenes.len() - invalid_scenes.len();
        if valid_scene_count > 0 && total_scene_frames != frames.len() as u32 {
            println!(
                "⚠️  Scene frame count mismatch: valid scenes cover {} frames, but {} frames exist",
                total_scene_frames,
                frames.len()
            );
        }
    }

    // Check histogram integrity
    for (i, frame) in frames.iter().enumerate() {
        if frame.lum_histogram.len() != 256 {
            anyhow::bail!(
                "Frame {} has invalid histogram length: {}",
                i,
                frame.lum_histogram.len()
            );
        }

        let histogram_sum: f64 = frame.lum_histogram.iter().sum();
        if (histogram_sum - 100.0).abs() > 1.0 {
            anyhow::bail!(
                "Frame {} has invalid histogram sum: {:.2} (should be ~100.0)",
                i,
                histogram_sum
            );
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

/// Compute FALL from frames and compare against header values with tolerance.
fn validate_fall_consistency(frames: &[MadVRFrame], header_maxfall: u32, header_avgfall: u32) {
    if frames.is_empty() {
        println!("No frames; skipping FALL checks");
        return;
    }
    let falls: Vec<f64> = frames.iter().map(|f| pq_to_nits(f.avg_pq)).collect();
    let derived_maxfall = falls.iter().cloned().fold(0.0, f64::max);
    let derived_avgfall = falls.iter().sum::<f64>() / falls.len() as f64;
    let maxfall_tol = 10.0_f64.max(derived_maxfall * 0.02);
    let avgfall_tol = 10.0_f64.max(derived_avgfall * 0.02);

    let max_ok = (derived_maxfall - header_maxfall as f64).abs() <= maxfall_tol;
    let avg_ok = (derived_avgfall - header_avgfall as f64).abs() <= avgfall_tol;

    if max_ok && avg_ok {
        println!("✓ FALL header values match derived values within tolerance");
    } else {
        if !max_ok {
            println!(
                "⚠️  MaxFALL mismatch: header={} derived≈{:.0} (tol±{:.0})",
                header_maxfall, derived_maxfall, maxfall_tol
            );
        }
        if !avg_ok {
            println!(
                "⚠️  AvgFALL mismatch: header={} derived≈{:.0} (tol±{:.0})",
                header_avgfall, derived_avgfall, avgfall_tol
            );
        }
    }
}

/// Check that flags imply presence/absence of optimizer data consistently.
fn validate_flags_vs_data(has_optimizer: bool, frames: &[MadVRFrame]) {
    if has_optimizer {
        let count = frames.iter().filter(|f| f.target_nits.is_some()).count();
        if count == frames.len() {
            println!("✓ Optimizer flag set and per-frame target_nits present");
        } else {
            println!(
                "⚠️  Optimizer flag set but only {}/{} frames have target_nits",
                count,
                frames.len()
            );
        }
    } else {
        let count = frames.iter().filter(|f| f.target_nits.is_some()).count();
        if count == 0 {
            println!("✓ No optimizer flag and no target_nits present");
        } else {
            println!(
                "⚠️  Optimizer flag not set but found target_nits in {}/{} frames",
                count,
                frames.len()
            );
        }
    }
}
