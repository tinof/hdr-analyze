use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use madvr_parse::{MadVRFrame, MadVRScene};
use serde::{Deserialize, Serialize};

use crate::cli::{PeakDomain, PeakEstimator};
use crate::crop::CropRect;

/// Version 2 added analyzer/source/analysis provenance and moved `crop` to full-resolution
/// source coordinates (`crop_space: "full"`). Version 1 stored the crop in analysis space.
pub const L1_SIDECAR_VERSION: u32 = 2;

/// Coordinate space of `L1Sidecar::crop` since version 2.
pub const CROP_SPACE_FULL: &str = "full";

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameL1Measurement {
    pub min_pq: f64,
    pub avg_max_rgb_pq: f64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct L1Sidecar {
    pub version: u32,
    /// `hdr_analyzer_mvp --version` string, including `(+cuda)` for GPU-capable builds.
    pub analyzer_version: String,
    pub source: SourceMetadata,
    pub analysis: AnalysisMetadata,
    pub crop_space: String,
    pub min_percentile: f64,
    pub denoise_mode: String,
    pub peak_domain: String,
    pub peak_estimator: String,
    pub peak_percentile: f64,
    pub crop: CropMetadata,
    pub scenes: Vec<SceneL1Metadata>,
    pub frames: FrameL1Metadata,
}

/// Identity of the analyzed input, so consumers can reject a sidecar written for another file.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SourceMetadata {
    /// File name only (no directory).
    pub file_name: String,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    pub transfer_function: String,
}

/// Sampling settings the measurements were produced with.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AnalysisMetadata {
    pub downscale: u32,
    pub sample_rate: u32,
    /// True when the CUDA kernel analyzed the whole run (a mid-run CPU fallback reports false).
    pub gpu: bool,
    pub no_crop: bool,
}

/// Provenance recorded alongside the L1 statistics.
#[derive(Clone, Debug)]
pub struct SidecarProvenance {
    pub source: SourceMetadata,
    pub analysis: AnalysisMetadata,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CropMetadata {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SceneL1Metadata {
    pub start: u32,
    pub end: u32,
    pub min_pq_12bit: u16,
    pub avg_luma_pq_12bit: u16,
    pub avg_max_rgb_pq_12bit: u16,
    pub max_pq_12bit: u16,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FrameL1Metadata {
    pub min_pq_12bit: Vec<u16>,
    pub avg_luma_pq_12bit: Vec<u16>,
    pub avg_max_rgb_pq_12bit: Vec<u16>,
}

pub fn sidecar_path(output_path: &Path) -> PathBuf {
    let mut path: OsString = output_path.as_os_str().to_owned();
    path.push(".l1.json");
    PathBuf::from(path)
}

pub fn write_l1_sidecar(
    output_path: &Path,
    scenes: &[MadVRScene],
    frames: &[MadVRFrame],
    measurements: &[FrameL1Measurement],
    min_percentile: f64,
    denoise_mode: &str,
    peak_domain: PeakDomain,
    peak_estimator: PeakEstimator,
    peak_percentile: f64,
    crop: CropRect,
    provenance: &SidecarProvenance,
) -> Result<PathBuf> {
    if frames.len() != measurements.len() {
        anyhow::bail!(
            "L1 sidecar frame count mismatch: {} frames, {} measurements",
            frames.len(),
            measurements.len()
        );
    }

    let scene_metadata = scenes
        .iter()
        .filter_map(|scene| build_scene_metadata(scene, frames, measurements))
        .collect();
    let sidecar = L1Sidecar {
        version: L1_SIDECAR_VERSION,
        analyzer_version: crate::cli::VERSION.to_owned(),
        source: provenance.source.clone(),
        analysis: provenance.analysis.clone(),
        crop_space: CROP_SPACE_FULL.to_owned(),
        min_percentile,
        denoise_mode: denoise_mode.to_owned(),
        peak_domain: match peak_domain {
            PeakDomain::MaxRgb => "max-rgb",
            PeakDomain::Luma => "luma",
        }
        .to_owned(),
        peak_estimator: match peak_estimator {
            PeakEstimator::Max => "max",
            PeakEstimator::Percentile => "percentile",
            PeakEstimator::Robust => "robust",
        }
        .to_owned(),
        peak_percentile,
        crop: CropMetadata {
            x: crop.x,
            y: crop.y,
            width: crop.width,
            height: crop.height,
        },
        scenes: scene_metadata,
        frames: FrameL1Metadata {
            min_pq_12bit: measurements
                .iter()
                .map(|measurement| pq_to_12bit(measurement.min_pq))
                .collect(),
            avg_luma_pq_12bit: frames
                .iter()
                .map(|frame| pq_to_12bit(frame.avg_pq))
                .collect(),
            avg_max_rgb_pq_12bit: measurements
                .iter()
                .map(|measurement| pq_to_12bit(measurement.avg_max_rgb_pq))
                .collect(),
        },
    };

    let path = sidecar_path(output_path);
    let file = File::create(&path)
        .with_context(|| format!("Failed to create L1 sidecar {}", path.display()))?;
    serde_json::to_writer_pretty(file, &sidecar)
        .with_context(|| format!("Failed to serialize L1 sidecar {}", path.display()))?;
    Ok(path)
}

fn build_scene_metadata(
    scene: &MadVRScene,
    frames: &[MadVRFrame],
    measurements: &[FrameL1Measurement],
) -> Option<SceneL1Metadata> {
    let start = scene.start as usize;
    let end = ((scene.end + 1) as usize).min(frames.len());
    if start >= end || start >= measurements.len() {
        return None;
    }

    let scene_frames = &frames[start..end];
    let scene_measurements = &measurements[start..end.min(measurements.len())];
    if scene_measurements.is_empty() {
        return None;
    }

    let min_pq = scene_measurements
        .iter()
        .map(|measurement| measurement.min_pq)
        .fold(1.0, f64::min);
    let avg_luma_pq =
        scene_frames.iter().map(|frame| frame.avg_pq).sum::<f64>() / scene_frames.len() as f64;
    let avg_max_rgb_pq = scene_measurements
        .iter()
        .map(|measurement| measurement.avg_max_rgb_pq)
        .sum::<f64>()
        / scene_measurements.len() as f64;
    let max_pq = scene_frames
        .iter()
        .map(|frame| frame.peak_pq_2020)
        .fold(0.0, f64::max);

    Some(SceneL1Metadata {
        start: scene.start,
        end: scene.end,
        min_pq_12bit: pq_to_12bit(min_pq),
        avg_luma_pq_12bit: pq_to_12bit(avg_luma_pq),
        avg_max_rgb_pq_12bit: pq_to_12bit(avg_max_rgb_pq),
        max_pq_12bit: pq_to_12bit(max_pq),
    })
}

fn pq_to_12bit(pq: f64) -> u16 {
    (pq.clamp(0.0, 1.0) * 4095.0).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_sidecar_records_provenance_and_full_resolution_crop() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("m.bin");
        let scenes = vec![MadVRScene {
            start: 0,
            end: 1,
            ..Default::default()
        }];
        let frames = vec![
            MadVRFrame {
                avg_pq: 0.2,
                peak_pq_2020: 0.6,
                ..Default::default()
            },
            MadVRFrame {
                avg_pq: 0.3,
                peak_pq_2020: 0.7,
                ..Default::default()
            },
        ];
        let measurements = vec![FrameL1Measurement::default(); 2];
        let provenance = SidecarProvenance {
            source: SourceMetadata {
                file_name: "input.mkv".into(),
                size_bytes: 1234,
                width: 3840,
                height: 2160,
                transfer_function: "PQ (SMPTE 2084)".into(),
            },
            analysis: AnalysisMetadata {
                downscale: 2,
                sample_rate: 1,
                gpu: false,
                no_crop: false,
            },
        };
        let path = write_l1_sidecar(
            &output,
            &scenes,
            &frames,
            &measurements,
            0.1,
            "none",
            PeakDomain::MaxRgb,
            PeakEstimator::Max,
            99.9,
            CropRect {
                x: 0,
                y: 280,
                width: 3840,
                height: 1600,
            },
            &provenance,
        )
        .unwrap();

        let json: serde_json::Value = serde_json::from_reader(File::open(path).unwrap()).unwrap();
        assert_eq!(json["version"], 2);
        assert_eq!(json["crop_space"], "full");
        assert_eq!(json["crop"]["y"], 280);
        assert_eq!(json["source"]["size_bytes"], 1234);
        assert_eq!(json["analysis"]["downscale"], 2);
        assert!(json["analyzer_version"]
            .as_str()
            .unwrap()
            .starts_with(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn sidecar_path_appends_suffix_without_replacing_bin_extension() {
        assert_eq!(
            sidecar_path(Path::new("measurements.bin")),
            PathBuf::from("measurements.bin.l1.json")
        );
    }

    #[test]
    fn scene_min_uses_minimum_of_robust_frame_measurements() {
        let scene = MadVRScene {
            start: 0,
            end: 1,
            ..Default::default()
        };
        let frames = vec![
            MadVRFrame {
                avg_pq: 0.2,
                peak_pq_2020: 0.8,
                ..Default::default()
            },
            MadVRFrame {
                avg_pq: 0.4,
                peak_pq_2020: 0.7,
                ..Default::default()
            },
        ];
        let measurements = vec![
            FrameL1Measurement {
                min_pq: 0.1,
                avg_max_rgb_pq: 0.3,
            },
            FrameL1Measurement {
                min_pq: 0.15,
                avg_max_rgb_pq: 0.5,
            },
        ];

        let metadata = build_scene_metadata(&scene, &frames, &measurements).unwrap();
        assert_eq!(metadata.min_pq_12bit, pq_to_12bit(0.1));
        assert_eq!(metadata.avg_luma_pq_12bit, pq_to_12bit(0.3));
        assert_eq!(metadata.avg_max_rgb_pq_12bit, pq_to_12bit(0.4));
        assert_eq!(metadata.max_pq_12bit, pq_to_12bit(0.8));
    }
}
