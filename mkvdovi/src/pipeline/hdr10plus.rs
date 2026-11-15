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

use super::hdr10plus_coverage::COVERAGE_FILE;

/// The Annex B stream the HDR10+ path extracts into the temp directory.
pub(super) const HDR10PLUS_HEVC: &str = "video.hevc";
/// The log of `hdr10plus_tool extract` in the temp directory.
pub(super) const HDR10PLUS_TOOL_LOG: &str = "hdr10plus_tool.log";
/// The log of the `--skip-reorder` retry, kept apart so that the first log keeps the failure.
pub(super) const HDR10PLUS_SKIP_REORDER_LOG: &str = "hdr10plus_tool_skip_reorder.log";
/// Written next to the JSON when the `--skip-reorder` retry produced it, so that a resumed run
/// that reuses the JSON warns again.
pub(super) const HDR10PLUS_SKIP_REORDER_MARKER: &str = "hdr10plus_metadata.skip_reorder";
const SKIP_REORDER_WARNING: &str = "HDR10+ metadata extracted with --skip-reorder: it keeps the \
     order of the bitstream and is not reordered to display order.";

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
    let hevc = temp_dir.join(HDR10PLUS_HEVC);
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
        // A new stream needs a new payload count.
        resume::invalidate(&temp_dir.join(COVERAGE_FILE))?;
    }

    let json_out = temp_dir.join("hdr10plus_metadata.json");
    if resume && resume::is_complete(&json_out) {
        progress::print_info("Reusing extracted HDR10+ metadata from a previous run.");
        if temp_dir.join(HDR10PLUS_SKIP_REORDER_MARKER).exists() {
            progress::print_warn(SKIP_REORDER_WARNING);
        }
        return Ok(Some(json_out));
    }
    let log = temp_dir.join(HDR10PLUS_TOOL_LOG);
    if run_hdr10plus_extract(&hevc, &json_out, &log, false)? {
        resume::mark_done(&json_out)?;
        return Ok(Some(json_out));
    }
    // P5.1: `--skip-reorder` is hdr10plus_tool's workaround for misauthored HEVC files. It runs
    // only after the normal extraction failed, and a JSON it writes is not reordered.
    progress::print_warn(&format!(
        "hdr10plus_tool extract failed (see {}); retrying with --skip-reorder.",
        log.display()
    ));
    let retry_log = temp_dir.join(HDR10PLUS_SKIP_REORDER_LOG);
    if run_hdr10plus_extract(&hevc, &json_out, &retry_log, true)? {
        let marker = temp_dir.join(HDR10PLUS_SKIP_REORDER_MARKER);
        fs::write(&marker, b"").with_context(|| format!("Failed to write {}", marker.display()))?;
        progress::print_warn(SKIP_REORDER_WARNING);
        resume::mark_done(&json_out)?;
        return Ok(Some(json_out));
    }
    progress::print_warn(&format!(
        "The --skip-reorder retry failed too (see {}).",
        retry_log.display()
    ));
    Ok(None)
}

/// One `hdr10plus_tool extract` run; true when it wrote a non-empty JSON. A JSON left by an
/// earlier run is removed and unsealed first, with its `--skip-reorder` marker, so that only
/// this run can pass and only the retry leaves a marker.
fn run_hdr10plus_extract(
    hevc: &Path,
    json_out: &Path,
    log: &Path,
    skip_reorder: bool,
) -> Result<bool> {
    resume::invalidate(json_out)?;
    let marker = json_out.with_file_name(HDR10PLUS_SKIP_REORDER_MARKER);
    if marker.exists() {
        fs::remove_file(&marker)
            .with_context(|| format!("Failed to remove {}", marker.display()))?;
    }
    if json_out.exists() {
        fs::remove_file(json_out)
            .with_context(|| format!("Failed to remove {}", json_out.display()))?;
    }
    let mut tool = Command::new("hdr10plus_tool");
    tool.arg("extract");
    if skip_reorder {
        tool.arg("--skip-reorder");
    }
    tool.arg("-i").arg(hevc).arg("-o").arg(json_out);
    let message = if skip_reorder {
        "Extracting HDR10+ metadata with --skip-reorder"
    } else {
        "Extracting HDR10+ metadata"
    };
    Ok(run_command_with_spinner(&mut tool, log, message)?
        && fs::metadata(json_out).is_ok_and(|meta| meta.len() > 0))
}
