use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use madvr_parse::{MadVRFrame, MadVRScene};
use serde::{Deserialize, Serialize};

use crate::analysis::histogram::pq_to_nits;
use crate::cli::{PeakDomain, PeakEstimator};
use crate::crop::CropRect;

/// Version 5 adds `source.stream_frames` (coded pictures in the video stream) and
/// `source.leading_skipped_frames` (RASL pictures at the start of an open-GOP cut that no decoder
/// outputs). Measured frame `i` is stream frame `i + leading_skipped_frames` in presentation
/// order, and the scenes cover `stream_frames - leading_skipped_frames` frames; an RPU for the
/// stream needs `leading_skipped_frames` entries in front. Up to version 4 the scenes started at
/// the first decoded frame with no record of skipped pictures.
/// Version 4 changes what the averages are: `avg_luma_pq_12bit` and `avg_max_rgb_pq_12bit`
/// (per frame and per scene) are the unfiltered per-frame means. Up to version 3 each frame mean
/// had passed the histogram EMA / temporal median first, so a scene average leaned toward the
/// scene's first frames (fades and flashes read wrong). The layout is unchanged.
/// Version 4 also carries the optional `light_level` block (content MaxCLL / MaxFALL in nits,
/// CTA-861.3 over the active image area), written for max-RGB runs only. It was added before
/// version 4 reached a release, so readers treat a version 4 sidecar without it as valid.
/// Version 3 adds `analysis.luminance_mapping`: `"pq"`, or for HLG measured through the Dolby
/// Vision Profile 8.4 reconstruction `"dovi84-v2"` (luma curve for luma; luma curve + chroma MMR
/// + RPU matrix for max-RGB) or the earlier `"dovi84-v1"` (luma curve only, max-RGB equal to
/// luma). Both HLG values share the schema; mkvdovi re-analyzes `dovi84-v1` sidecars instead of
/// reusing them, and accept an HLG sidecar only when it names the selected composer exactly. The HLG value names the composer the
/// decode used (`dovi84_composer::Composer::luminance_mapping`): `"dovi84-v2"` for the preset,
/// `"dovi84-bt2100-v1"` for the BT.2100 fit (`--hlg-composer bt2100`), a new value of the same
/// field, so the version stays 4. Version 2 added analyzer/source/analysis
/// provenance and moved `crop` to full-resolution source coordinates (`crop_space: "full"`).
/// Version 1 stored the crop in analysis space.
pub const L1_SIDECAR_VERSION: u32 = 5;

/// Coordinate space of `L1Sidecar::crop` since version 2.
pub const CROP_SPACE_FULL: &str = "full";

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameL1Measurement {
    pub min_pq: f64,
    /// Unfiltered frame mean of Y-luma PQ. `MadVRFrame::avg_pq` starts equal and is then smoothed.
    pub avg_luma_pq: f64,
    /// Unfiltered frame mean of max-RGB PQ.
    pub avg_max_rgb_pq: f64,
    /// Largest max-RGB PQ value in the frame, whatever the peak domain or estimator.
    pub max_rgb_pq: f64,
    /// Frame mean in nits (linear light) of the peak-domain PQ histogram; the frame-average
    /// light level when the peak domain is max-RGB.
    pub fall_nits: f64,
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
    /// Absent when the peak domain is luma (the frame average would not be max-RGB) or the
    /// frames were pre-denoised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light_level: Option<LightLevelMetadata>,
    pub scenes: Vec<SceneL1Metadata>,
    pub frames: FrameL1Metadata,
}

/// Content light levels of the analyzed frames (CTA-861.3 Annex A), over the committed crop:
/// the letterbox bars are outside the active image area and are not averaged in.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct LightLevelMetadata {
    /// Brightest max(R, G, B) sample of any analyzed frame, in nits.
    pub max_cll_nits: u32,
    /// Highest frame average of max(R, G, B) in linear light, in nits.
    pub max_fall_nits: u32,
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
    /// Coded pictures (access units) in the video stream. Added in version 5.
    pub stream_frames: u64,
    /// Undecodable leading (RASL) pictures before the first measured frame. Added in version 5.
    pub leading_skipped_frames: u64,
}

/// Sampling settings the measurements were produced with.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AnalysisMetadata {
    pub downscale: u32,
    pub sample_rate: u32,
    /// True when the CUDA kernel analyzed the whole run (a mid-run CPU fallback reports false).
    pub gpu: bool,
    pub no_crop: bool,
    /// How signal codes were mapped to PQ: `"pq"` (PQ/unspecified input, measured directly)
    /// or the HLG composer's name (`"dovi84-v2"` preset, `"dovi84-bt2100-v1"` BT.2100 fit;
    /// HLG through the DV Profile 8.4 decode). Added in version 3.
    pub luminance_mapping: String,
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
        // The median3 pre-denoise removes the small highlights MaxCLL has to report.
        light_level: match peak_domain {
            PeakDomain::MaxRgb if denoise_mode != "median3" => light_level(measurements),
            _ => None,
        },
        scenes: scene_metadata,
        frames: FrameL1Metadata {
            min_pq_12bit: measurements
                .iter()
                .map(|measurement| pq_to_12bit(measurement.min_pq))
                .collect(),
            avg_luma_pq_12bit: measurements
                .iter()
                .map(|measurement| pq_to_12bit(measurement.avg_luma_pq))
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

/// MaxCLL / MaxFALL over all frames, at least 1 nit (0 means "unknown" in CTA-861.3).
fn light_level(measurements: &[FrameL1Measurement]) -> Option<LightLevelMetadata> {
    if measurements.is_empty() {
        return None;
    }
    let max_cll = measurements
        .iter()
        // Quantized like the L1 values, so CPU and CUDA runs agree on the rounded nits.
        .map(|measurement| pq_to_nits(f64::from(pq_to_12bit(measurement.max_rgb_pq)) / 4095.0))
        .fold(0.0, f64::max);
    let max_fall = measurements
        .iter()
        .map(|measurement| measurement.fall_nits)
        .fold(0.0, f64::max);
    let max_cll_nits = (max_cll.round() as u32).max(1);
    Some(LightLevelMetadata {
        max_cll_nits,
        // The histogram quantizes to 12-bit bins, so a flat frame can average a hair above its peak.
        max_fall_nits: (max_fall.round() as u32).clamp(1, max_cll_nits),
    })
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
    let avg_luma_pq = scene_measurements
        .iter()
        .map(|measurement| measurement.avg_luma_pq)
        .sum::<f64>()
        / scene_measurements.len() as f64;
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
    use dovi84_composer::Composer;

    use super::*;

    #[test]
    fn sidecar_records_provenance_and_full_resolution_crop() {
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
                stream_frames: 4,
                leading_skipped_frames: 2,
            },
            analysis: AnalysisMetadata {
                downscale: 2,
                sample_rate: 1,
                gpu: false,
                no_crop: false,
                luminance_mapping: crate::analysis::hlg::PQ_MAPPING.into(),
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
        assert_eq!(json["version"], 5);
        assert_eq!(json["source"]["stream_frames"], 4);
        assert_eq!(json["source"]["leading_skipped_frames"], 2);
        // The measurements are zero and the (smoothed) madVR frame averages are 0.2 and 0.3:
        // the per-frame sidecar series must come from the measurements.
        assert_eq!(
            json["frames"]["avg_luma_pq_12bit"],
            serde_json::json!([0, 0])
        );
        assert_eq!(json["analysis"]["luminance_mapping"], "pq");
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
    fn hlg_sidecar_round_trips_dovi84_luminance_mapping() {
        for (composer, mapping) in [
            (Composer::Preset, "dovi84-v2"),
            (Composer::Bt2100V1, "dovi84-bt2100-v1"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let output = dir.path().join("hlg.bin");
            let scenes = vec![MadVRScene {
                start: 0,
                end: 0,
                ..Default::default()
            }];
            let frames = vec![MadVRFrame::default()];
            let measurements = vec![FrameL1Measurement::default()];
            let provenance = SidecarProvenance {
                source: SourceMetadata {
                    file_name: "hlg.mkv".into(),
                    size_bytes: 1,
                    width: 1920,
                    height: 1080,
                    transfer_function: "HLG (ARIB STD-B67)".into(),
                    stream_frames: 1,
                    leading_skipped_frames: 0,
                },
                analysis: AnalysisMetadata {
                    downscale: 1,
                    sample_rate: 1,
                    gpu: false,
                    no_crop: true,
                    luminance_mapping: composer.luminance_mapping().into(),
                },
            };
            let path = write_l1_sidecar(
                &output,
                &scenes,
                &frames,
                &measurements,
                0.1,
                "none",
                PeakDomain::Luma,
                PeakEstimator::Max,
                99.9,
                CropRect {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
                &provenance,
            )
            .unwrap();

            let json: serde_json::Value =
                serde_json::from_reader(File::open(&path).unwrap()).unwrap();
            assert_eq!(json["version"], L1_SIDECAR_VERSION);
            assert_eq!(json["analysis"]["luminance_mapping"], mapping);
            // A luma peak domain has no max-RGB frame average, so no light levels are written.
            assert!(json.get("light_level").is_none());
            let parsed: L1Sidecar = serde_json::from_reader(File::open(&path).unwrap()).unwrap();
            assert_eq!(parsed.analysis.luminance_mapping, mapping);
            assert_eq!(
                Composer::from_luminance_mapping(&parsed.analysis.luminance_mapping),
                Some(composer)
            );
        }
    }

    #[test]
    fn light_level_is_the_maximum_over_frames() {
        let frame = |max_rgb_12bit: f64, fall_nits: f64| FrameL1Measurement {
            max_rgb_pq: max_rgb_12bit / 4095.0,
            fall_nits,
            ..Default::default()
        };
        // 12-bit PQ 3079 is 1000.9 nits, 2081 is 100 nits.
        let measurements = [
            frame(2081.0, 40.4),
            frame(3079.0, 12.0),
            frame(2081.0, 80.6),
        ];
        assert_eq!(
            light_level(&measurements),
            Some(LightLevelMetadata {
                max_cll_nits: 1001,
                max_fall_nits: 81,
            })
        );
        // Black content still reports 1 nit, and the average never exceeds the peak.
        assert_eq!(
            light_level(&[frame(0.0, 0.0)]),
            Some(LightLevelMetadata {
                max_cll_nits: 1,
                max_fall_nits: 1,
            })
        );
        assert_eq!(
            light_level(&[frame(2081.0, 100.4)]).unwrap().max_fall_nits,
            100
        );
        assert_eq!(light_level(&[]), None);
    }

    #[test]
    fn sidecar_path_appends_suffix_without_replacing_bin_extension() {
        assert_eq!(
            sidecar_path(Path::new("measurements.bin")),
            PathBuf::from("measurements.bin.l1.json")
        );
    }

    #[test]
    fn scene_stats_use_unfiltered_measurements_not_smoothed_frame_averages() {
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
                avg_luma_pq: 0.25,
                avg_max_rgb_pq: 0.3,
                ..Default::default()
            },
            FrameL1Measurement {
                min_pq: 0.15,
                avg_luma_pq: 0.45,
                avg_max_rgb_pq: 0.5,
                ..Default::default()
            },
        ];

        let metadata = build_scene_metadata(&scene, &frames, &measurements).unwrap();
        assert_eq!(metadata.min_pq_12bit, pq_to_12bit(0.1));
        // `MadVRFrame::avg_pq` (0.2, 0.4) is the smoothed .bin value and must not be used.
        assert_eq!(metadata.avg_luma_pq_12bit, pq_to_12bit(0.35));
        assert_eq!(metadata.avg_max_rgb_pq_12bit, pq_to_12bit(0.4));
        assert_eq!(metadata.max_pq_12bit, pq_to_12bit(0.8));
    }
}
