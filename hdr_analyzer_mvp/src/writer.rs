use anyhow::{Context, Result};
use madvr_parse::{MadVRFrame, MadVRHeader, MadVRMeasurements, MadVRScene};

use crate::analysis::histogram::pq_to_nits;

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
pub fn write_measurement_file(
    output_path: &str,
    scenes: &[MadVRScene],
    frames: &[MadVRFrame],
    enable_optimizer: bool,
    madvr_version: u32,
    target_peak_nits: Option<u32>,
    header_peak_source: Option<&str>,
) -> Result<()> {
    // 1. Create the Header
    let peaks_nits: Vec<u32> = frames
        .iter()
        .map(|f| pq_to_nits(f.peak_pq_2020).round() as u32)
        .collect();

    let maxcll = match header_peak_source.unwrap_or("max").to_lowercase().as_str() {
        "histogram99" => percentile_u32(&peaks_nits, 0.99),
        "histogram999" => percentile_u32(&peaks_nits, 0.999),
        _ => peaks_nits.into_iter().max().unwrap_or(0),
    };

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
        // For v6, compute per-gamut peaks with simplified gamut-aware approximation
        //
        // APPROXIMATION NOTE:
        // Proper per-gamut peak computation would require:
        // 1. YUV -> RGB (BT.2020) conversion for each pixel
        // 2. RGB color space transformation (BT.2020 -> P3/709) using 3x3 matrices
        // 3. Gamut clipping or tone-mapping for out-of-gamut colors
        // 4. Peak luminance extraction in each target gamut
        //
        // Current simplified approach:
        // - DCI-P3: ~99% of BT.2020 peak (P3 gamut is nearly as wide as 2020 for HDR content)
        // - BT.709: ~95% of BT.2020 peak (709 is significantly smaller, so conservative estimate)
        //
        // This approximation assumes luminance-preserving gamut mapping and is suitable
        // for typical HDR10 content. Future enhancement: full RGB-based gamut conversion.

        let peak_dcip3 = (frame.peak_pq_2020 * 0.99).min(1.0);
        let peak_709 = (frame.peak_pq_2020 * 0.95).min(1.0);

        owned_frames.push(MadVRFrame {
            peak_pq_2020: frame.peak_pq_2020,
            peak_pq_dcip3: if madvr_version >= 6 {
                Some(peak_dcip3)
            } else {
                frame.peak_pq_dcip3
            },
            peak_pq_709: if madvr_version >= 6 {
                Some(peak_709)
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

fn percentile_u32(values: &[u32], p: f64) -> u32 {
    if values.is_empty() {
        return 0;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let p = p.clamp(0.0, 1.0);
    let idx = ((p * (v.len() as f64)) - 1.0).ceil().max(0.0) as usize;
    v[idx.min(v.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::histogram::nits_to_pq;

    #[test]
    fn test_gamut_peak_approximation() {
        // Test that gamut peaks are reasonable approximations
        let bt2020_peak = 0.8_f64; // Example PQ value
        let peak_dcip3 = (bt2020_peak * 0.99).min(1.0);
        let peak_709 = (bt2020_peak * 0.95).min(1.0);

        assert!(peak_dcip3 <= bt2020_peak);
        assert!(peak_709 <= peak_dcip3);
        assert!(peak_dcip3 > 0.7); // Should be close to original
        assert!(peak_709 > 0.7); // Should still be reasonably high
    }

    #[test]
    fn test_compute_falls() {
        // Build three frames with avg_pq corresponding to 100, 200, 300 nits
        fn to_pq(nits: f64) -> f64 {
            nits_to_pq(nits)
        }
        let frames = vec![
            MadVRFrame {
                avg_pq: to_pq(100.0),
                ..Default::default()
            },
            MadVRFrame {
                avg_pq: to_pq(200.0),
                ..Default::default()
            },
            MadVRFrame {
                avg_pq: to_pq(300.0),
                ..Default::default()
            },
        ];
        let (maxfall, avgfall) = compute_falls(&frames);
        assert!(
            (300 - 1..=300 + 1).contains(&maxfall),
            "maxfall ~300, got {}",
            maxfall
        );
        assert!(
            (200 - 1..=200 + 1).contains(&avgfall),
            "avgfall ~200, got {}",
            avgfall
        );
    }

    #[test]
    fn test_percentile_u32() {
        let v = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        assert_eq!(percentile_u32(&v, 0.0), 1);
        assert_eq!(percentile_u32(&v, 0.5), 5);
        assert!(percentile_u32(&v, 0.99) >= 10 - 1);
        assert_eq!(percentile_u32(&[], 0.5), 0);
    }
}
