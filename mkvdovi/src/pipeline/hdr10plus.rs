use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::cli::PeakSource;
use crate::external::{run_command_with_progress, run_command_with_spinner};
use crate::progress;
use crate::resume;

#[derive(Debug, PartialEq)]
pub(super) struct Hdr10PlusPeakStats {
    pub(super) max_peak_nits: f64,
    pub(super) outlier_scene_count: usize,
}

pub(super) fn inspect_hdr10plus_scene_peaks(
    metadata_path: &Path,
    peak_source: PeakSource,
    outlier_threshold_nits: f64,
) -> Result<Option<Hdr10PlusPeakStats>> {
    let metadata: Value = serde_json::from_reader(
        File::open(metadata_path).context("Failed to open extracted HDR10+ metadata JSON")?,
    )
    .context("Failed to parse extracted HDR10+ metadata JSON")?;
    hdr10plus_scene_peak_stats(&metadata, peak_source, outlier_threshold_nits)
}

pub(super) fn hdr10plus_scene_peak_stats(
    metadata: &Value,
    peak_source: PeakSource,
    outlier_threshold_nits: f64,
) -> Result<Option<Hdr10PlusPeakStats>> {
    let scene_info = metadata
        .get("SceneInfo")
        .and_then(Value::as_array)
        .context("HDR10+ metadata is missing SceneInfo")?;
    let first_frame_indices = metadata
        .pointer("/SceneInfoSummary/SceneFirstFrameIndex")
        .and_then(Value::as_array)
        .context("HDR10+ metadata is missing SceneInfoSummary.SceneFirstFrameIndex")?;
    let first_frame_offset = first_frame_indices
        .first()
        .and_then(Value::as_u64)
        .context("HDR10+ metadata has no scene first-frame indices")?;

    let mut outlier_scenes = HashSet::new();
    let mut max_peak_nits = 0.0_f64;
    for scene_index in first_frame_indices {
        let source_index = scene_index
            .as_u64()
            .context("HDR10+ scene first-frame index is not an integer")?;
        let relative_index = source_index
            .checked_sub(first_frame_offset)
            .context("HDR10+ scene first-frame index precedes the first scene")?;
        let scene = scene_info
            .get(relative_index as usize)
            .context("HDR10+ scene first-frame index is out of range")?;
        let peak_nits = hdr10plus_peak_nits(scene, peak_source)
            .context("HDR10+ scene is missing peak-brightness metadata")?;

        if peak_nits > outlier_threshold_nits {
            outlier_scenes.insert(source_index);
            max_peak_nits = max_peak_nits.max(peak_nits);
        }
    }

    if outlier_scenes.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Hdr10PlusPeakStats {
            max_peak_nits,
            outlier_scene_count: outlier_scenes.len(),
        }))
    }
}

pub(super) fn hdr10plus_peak_nits(scene: &Value, peak_source: PeakSource) -> Option<f64> {
    let luminance = scene.get("LuminanceParameters")?;
    let tenths_of_a_nit = match peak_source {
        PeakSource::Histogram => luminance
            .pointer("/LuminanceDistributions/DistributionValues")?
            .as_array()?
            .iter()
            .filter_map(Value::as_u64)
            .max()? as f64,
        PeakSource::Histogram99 => luminance
            .pointer("/LuminanceDistributions/DistributionValues")?
            .as_array()?
            .last()?
            .as_u64()? as f64,
        PeakSource::MaxScl => luminance
            .get("MaxScl")?
            .as_array()?
            .iter()
            .filter_map(Value::as_u64)
            .max()? as f64,
        PeakSource::MaxSclLuminance => {
            let max_scl = luminance.get("MaxScl")?.as_array()?;
            let [r, g, b] = max_scl.as_slice() else {
                return None;
            };
            (0.2627 * r.as_u64()? as f64)
                + (0.678 * g.as_u64()? as f64)
                + (0.0593 * b.as_u64()? as f64)
        }
    };

    Some(tenths_of_a_nit / 10.0)
}

pub(super) fn extract_hdr10plus_metadata(
    input: &str,
    temp_dir: &Path,
    resume: bool,
    stall_secs: u64,
) -> Result<Option<PathBuf>> {
    let hevc = temp_dir.join("video.hevc");
    if resume && resume::is_complete(&hevc) {
        progress::print_info("Reusing extracted HEVC stream from a previous run.");
    } else {
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            input,
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-f",
            "hevc",
            "-y",
            hevc.to_str().unwrap(),
        ]);

        let total = fs::metadata(input).ok().map(|m| m.len());
        if !run_command_with_progress(
            &mut cmd,
            &temp_dir.join("ffmpeg_extract_hdr10p.log"),
            "Extracting HEVC stream",
            &hevc,
            total,
            stall_secs,
        )? {
            return Ok(None);
        }
        resume::mark_done(&hevc)?;
    }

    let json_out = temp_dir.join("hdr10plus_metadata.json");
    if resume && resume::is_complete(&json_out) {
        progress::print_info("Reusing extracted HDR10+ metadata from a previous run.");
        return Ok(Some(json_out));
    }
    let mut tool = Command::new("hdr10plus_tool");
    tool.args([
        "extract",
        "-i",
        hevc.to_str().unwrap(),
        "-o",
        json_out.to_str().unwrap(),
    ]);

    if run_command_with_spinner(
        &mut tool,
        &temp_dir.join("hdr10plus_tool.log"),
        "Extracting HDR10+ metadata",
    )? && json_out.exists()
        && fs::metadata(&json_out)?.len() > 0
    {
        resume::mark_done(&json_out)?;
        return Ok(Some(json_out));
    }
    Ok(None)
}
