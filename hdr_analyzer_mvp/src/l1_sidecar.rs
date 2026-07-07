use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use madvr_parse::{MadVRFrame, MadVRScene};
use serde::{Deserialize, Serialize};

use crate::cli::PeakDomain;
use crate::crop::CropRect;

pub const L1_SIDECAR_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameL1Measurement {
    pub min_pq: f64,
    pub avg_max_rgb_pq: f64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct L1Sidecar {
    pub version: u32,
    pub min_percentile: f64,
    pub denoise_mode: String,
    pub peak_domain: String,
    pub crop: CropMetadata,
    pub scenes: Vec<SceneL1Metadata>,
    pub frames: FrameL1Metadata,
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
    crop: CropRect,
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
        min_percentile,
        denoise_mode: denoise_mode.to_owned(),
        peak_domain: match peak_domain {
            PeakDomain::MaxRgb => "max-rgb",
            PeakDomain::Luma => "luma",
        }
        .to_owned(),
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
