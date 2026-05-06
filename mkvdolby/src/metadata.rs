use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::external;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum HdrFormat {
    Hdr10Plus,
    Hdr10WithMeasurements,
    Hdr10Unsupported,
    Hlg,
    Unsupported,
}

impl HdrFormat {
    #[allow(dead_code)]
    pub fn name(&self) -> &'static str {
        match self {
            HdrFormat::Hdr10Plus => "HDR10+",
            HdrFormat::Hdr10WithMeasurements => "HDR10 (with measurements)",
            HdrFormat::Hdr10Unsupported => "HDR10 (no measurements)",
            HdrFormat::Hlg => "HLG",
            HdrFormat::Unsupported => "Unsupported",
        }
    }
}

/// Configuration for CM v4.0 metadata generation
#[derive(Debug, Clone)]
pub struct CmV40Config {
    /// Source primary index for L9 (0=P3-D65, 1=BT.709, 2=BT.2020)
    pub source_primary_index: u8,
    /// Content type for L11 (0-6, see ContentType enum)
    pub content_type: u8,
    /// Reference mode flag for L11
    pub reference_mode: bool,
}

impl Default for CmV40Config {
    fn default() -> Self {
        Self {
            source_primary_index: 2, // BT.2020
            content_type: 4,         // Cinema
            reference_mode: true,
        }
    }
}

pub fn get_mediainfo_json(input_file: &str) -> Result<Value> {
    // Basic cache logic could be added using OnceLock or just re-run (fast enough)
    let mut cmd = Command::new("mediainfo");
    cmd.arg("--Output=JSON").arg(input_file);
    let out = external::get_command_output(&mut cmd)?;
    serde_json::from_str(&out).context("Failed to parse mediainfo JSON")
}

pub fn get_ffprobe_json(input_file: &str) -> Result<Value> {
    let mut cmd = Command::new("ffprobe");
    cmd.args([
        "-v",
        "quiet",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-show_frames",
        "-read_intervals",
        "%+#1",
        input_file,
    ]);
    let out = external::get_command_output(&mut cmd)?;
    serde_json::from_str(&out).context("Failed to parse ffprobe JSON")
}

pub fn find_measurements_file(input_file: &Path) -> Option<PathBuf> {
    let dir = input_file.parent().unwrap_or(Path::new("."));
    let stem = input_file.file_stem()?.to_string_lossy();
    let name = input_file.file_name()?.to_string_lossy();

    let candidates = [
        dir.join("measurements.bin"),
        input_file.with_extension("mkv.measurements"), // loose approx
        dir.join(format!("{}.measurements", name)),
        dir.join(format!("{}.measurements", stem)),
        dir.join(format!("{}_measurements.bin", stem)),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Some(candidate.clone());
        }
    }

    // Globbing is strictly needed if pattern matching logic is fuzzy but candidates usually cover it.
    // The python script does glob for exact prefixes.
    // Simplifying for now: exact matches are most common.
    None
}

pub fn find_details_file(input_file: &Path) -> Option<PathBuf> {
    let dir = input_file.parent().unwrap_or(Path::new("."));
    let stem = input_file.file_stem()?.to_string_lossy();

    let candidates = [
        dir.join(format!("{}_mkv_Details.txt", stem)),
        dir.join(format!("{}_Details.txt", stem)),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Some(candidate.clone());
        }
    }
    None
}

pub fn check_hdr_format(input_file: &str) -> HdrFormat {
    let path = Path::new(input_file);

    // 1. MediaInfo checks. Some HLG MKVs expose HLG only as
    // transfer_characteristics_Original while HDR_Format remains empty.
    let mut mi_hints = match Command::new("mediainfo")
        .args([
            "--Inform=Video;%HDR_Format%/%HDR_Format_Compatibility%",
            input_file,
        ])
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => String::new(),
    };

    if let Ok(json) = get_mediainfo_json(input_file) {
        append_mediainfo_video_hints(&json, &mut mi_hints);
    }

    let measurements = find_measurements_file(path).is_some();

    if let Some(format) = classify_hdr_hints(&mi_hints, measurements) {
        return format;
    }

    // 2. Fallback to FFprobe
    // (Simplification: Assuming mediainfo is usually correct or sufficient for now)
    // If MediaInfo failed to detect, check ffprobe color_transfer
    if let Ok(json) = get_ffprobe_json(input_file) {
        // basic checking logic...
        // For brevity in this implementation plan step, relying on MediaInfo is usually 99% there.
        // But let's check streams[0].color_transfer
        if let Some(streams) = json.get("streams").and_then(|v| v.as_array()) {
            for stream in streams {
                if let Some(transfer) = stream.get("color_transfer").and_then(|s| s.as_str()) {
                    if let Some(format) = classify_hdr_hints(transfer, measurements) {
                        return format;
                    }
                }
            }
        }
    }

    HdrFormat::Unsupported
}

fn append_mediainfo_video_hints(json: &Value, hints: &mut String) {
    let Some(tracks) = json
        .get("media")
        .and_then(|m| m.get("track"))
        .and_then(|t| t.as_array())
    else {
        return;
    };

    for track in tracks {
        if track.get("@type").and_then(|s| s.as_str()) != Some("Video") {
            continue;
        }

        let Some(fields) = track.as_object() else {
            continue;
        };

        for (key, value) in fields {
            let key_lower = key.to_ascii_lowercase();
            if !(key_lower.contains("hdr") || key_lower.contains("transfer_characteristics")) {
                continue;
            }

            if let Some(value) = value.as_str() {
                hints.push('\n');
                hints.push_str(value);
            }
        }
    }
}

fn classify_hdr_hints(hints: &str, measurements: bool) -> Option<HdrFormat> {
    let hints = hints.to_uppercase();

    if hints.contains("SMPTE ST 2094 APP 4") || hints.contains("HDR10+") {
        return Some(HdrFormat::Hdr10Plus);
    }
    if hints.contains("HLG") || hints.contains("ARIB") {
        return Some(HdrFormat::Hlg);
    }
    if hints.contains("HDR10")
        || hints.contains("PQ")
        || hints.contains("ST 2084")
        || hints.contains("SMPTE2084")
    {
        return Some(if measurements {
            HdrFormat::Hdr10WithMeasurements
        } else {
            HdrFormat::Hdr10Unsupported
        });
    }

    None
}

pub fn get_static_metadata(input_file: &str) -> HashMap<String, f64> {
    let mut meta = HashMap::new();
    // Default values
    meta.insert("max_dml".to_string(), 1000.0);
    meta.insert("min_dml".to_string(), 0.0050);
    meta.insert("max_cll".to_string(), 1000.0);
    meta.insert("max_fall".to_string(), 400.0);

    // Try MediaInfo
    if let Ok(json) = get_mediainfo_json(input_file) {
        if let Some(tracks) = json
            .get("media")
            .and_then(|m| m.get("track"))
            .and_then(|t| t.as_array())
        {
            for track in tracks {
                if track.get("@type").and_then(|s| s.as_str()) == Some("Video") {
                    // Parse MasteringDisplay_Luminance
                    if let Some(mdl) = track
                        .get("MasteringDisplay_Luminance")
                        .and_then(|s| s.as_str())
                    {
                        let re_max = Regex::new(r"max: ([0-9.]+)").unwrap();
                        let re_min = Regex::new(r"min: ([0-9.]+)").unwrap();

                        if let Some(caps) = re_max.captures(mdl) {
                            if let Ok(v) = caps[1].parse::<f64>() {
                                meta.insert("max_dml".to_string(), v);
                            }
                        }
                        if let Some(caps) = re_min.captures(mdl) {
                            if let Ok(v) = caps[1].parse::<f64>() {
                                meta.insert("min_dml".to_string(), v);
                            }
                        }
                    }
                    // MaxCLL
                    if let Some(val) = track.get("MaxCLL") {
                        if let Some(f) = val.as_f64() {
                            meta.insert("max_cll".to_string(), f);
                        } else if let Some(s) = val.as_str() {
                            let re = Regex::new(r"([0-9.]+)").unwrap();
                            if let Some(caps) = re.captures(s) {
                                if let Ok(v) = caps[1].parse::<f64>() {
                                    meta.insert("max_cll".to_string(), v);
                                }
                            }
                        }
                    }

                    // MaxFALL
                    if let Some(val) = track.get("MaxFALL") {
                        if let Some(f) = val.as_f64() {
                            meta.insert("max_fall".to_string(), f);
                        } else if let Some(s) = val.as_str() {
                            let re = Regex::new(r"([0-9.]+)").unwrap();
                            if let Some(caps) = re.captures(s) {
                                if let Ok(v) = caps[1].parse::<f64>() {
                                    meta.insert("max_fall".to_string(), v);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Details.txt override
    if let Some(details_path) = find_details_file(Path::new(input_file)) {
        if let Ok(content) = std::fs::read_to_string(details_path) {
            // simplified regex parsing for details.txt
            let re_cll = Regex::new(r"(?i)MaxCLL\s*:\s*([0-9.,]+)").unwrap();
            let re_fall = Regex::new(r"(?i)MaxFALL\s*:\s*([0-9.,]+)").unwrap();

            // Very loose parsing, but matching python script is key.
            // Python script logic is detailed (after clipping vs before), we simplify for M1.
            // TODO: Make this robust.
            if let Some(caps) = re_cll.captures(&content) {
                let s = caps[1].replace(',', ".");
                if let Ok(v) = s.parse::<f64>() {
                    meta.insert("max_cll".to_string(), v);
                }
            }
            if let Some(caps) = re_fall.captures(&content) {
                let s = caps[1].replace(',', ".");
                if let Ok(v) = s.parse::<f64>() {
                    meta.insert("max_fall".to_string(), v);
                }
            }
        }
    }

    meta
}

/// Detect source color primaries from MediaInfo
#[allow(dead_code)]
pub fn detect_source_primaries(input_file: &str) -> u8 {
    if let Ok(json) = get_mediainfo_json(input_file) {
        if let Some(tracks) = json
            .get("media")
            .and_then(|m| m.get("track"))
            .and_then(|t| t.as_array())
        {
            for track in tracks {
                if track.get("@type").and_then(|s| s.as_str()) == Some("Video") {
                    // Check colour_primaries field
                    if let Some(primaries) = track
                        .get("colour_primaries")
                        .or_else(|| track.get("ColorPrimaries"))
                        .and_then(|s| s.as_str())
                    {
                        let p = primaries.to_uppercase();
                        if p.contains("P3") || p.contains("DCI") || p.contains("DISPLAY P3") {
                            return 0; // P3-D65
                        }
                        if p.contains("709") {
                            return 1; // BT.709
                        }
                    }
                }
            }
        }
    }
    2 // Default: BT.2020
}

pub fn generate_extra_json(
    output_path: &Path,
    metadata: &HashMap<String, f64>,
    trim_targets: &[u32],
    cm_v40_config: Option<&CmV40Config>,
) -> Result<()> {
    let min_dml = metadata.get("min_dml").unwrap_or(&0.005);
    let max_dml = metadata.get("max_dml").unwrap_or(&1000.0);
    let max_cll = metadata.get("max_cll").unwrap_or(&1000.0);
    let max_fall = metadata.get("max_fall").unwrap_or(&400.0);

    let mut json_content = json!({
        "profile": "8.1",
        "target_nits": trim_targets,
        "level6": {
            "max_display_mastering_luminance": *max_dml as u32,
            "min_display_mastering_luminance": (*min_dml * 10000.0) as u32,
            "max_content_light_level": *max_cll as u32,
            "max_frame_average_light_level": *max_fall as u32,
        }
    });

    // Add CM v4.0 specific configuration
    if let Some(cfg) = cm_v40_config {
        json_content["cm_version"] = json!("V40");

        // Build default metadata blocks with L2 trims for each target nit level, plus L9 and L11
        let mut default_blocks = Vec::new();

        // Add L2 trims for each target nit level (100, 600, 1000, etc.)
        // These provide baseline trim values that dovi_tool will use
        // PQ values: 100 nits ≈ 2081, 600 nits ≈ 2851, 1000 nits ≈ 3079
        let target_pq_map: [(u32, u32); 6] = [
            (100, 2081),
            (300, 2525),
            (600, 2851),
            (1000, 3079),
            (2000, 3388),
            (4000, 3696),
        ];

        for target in trim_targets {
            // Find matching PQ value or interpolate
            let target_pq = target_pq_map
                .iter()
                .find(|(nits, _)| nits == target)
                .map(|(_, pq)| *pq)
                .unwrap_or(3079); // Default to 1000 nits PQ

            default_blocks.push(json!({
                "Level2": {
                    "target_max_pq": target_pq,
                    "trim_slope": 2048,
                    "trim_offset": 2048,
                    "trim_power": 2048,
                    "trim_chroma_weight": 2048,
                    "trim_saturation_gain": 2048,
                    "ms_weight": 2048
                }
            }));
        }

        // Add L9 (source primaries)
        default_blocks.push(json!({
            "Level9": {
                "length": 1,
                "source_primary_index": cfg.source_primary_index
            }
        }));

        // Add L11 (content type)
        default_blocks.push(json!({
            "Level11": {
                "content_type": cfg.content_type,
                "whitepoint": 0,
                "reference_mode_flag": cfg.reference_mode
            }
        }));

        json_content["default_metadata_blocks"] = json!(default_blocks);
    }

    let file = File::create(output_path)?;
    serde_json::to_writer_pretty(file, &json_content)?;
    Ok(())
}

pub fn get_duration_from_mediainfo(input_file: &str) -> Option<f64> {
    if let Ok(json) = get_mediainfo_json(input_file) {
        if let Some(tracks) = json
            .get("media")
            .and_then(|m| m.get("track"))
            .and_then(|t| t.as_array())
        {
            for track in tracks {
                if track.get("@type").and_then(|s| s.as_str()) == Some("Video") {
                    // Duration
                    if let Some(val) = track.get("Duration") {
                        return parse_mediainfo_duration_seconds(val);
                    }
                }
            }
        }
    }
    None
}

fn parse_mediainfo_duration_seconds(value: &Value) -> Option<f64> {
    let duration = value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())?;

    // MediaInfo JSON normally reports seconds. Older comments in this code
    // expected milliseconds, so keep a conservative fallback for obviously
    // millisecond-scale values.
    Some(if duration > 86_400.0 {
        duration / 1000.0
    } else {
        duration
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_hlg_from_original_transfer_characteristics() {
        let hints = "BT.2020 (10-bit)\nHLG / BT.2020 (10-bit)";

        assert_eq!(classify_hdr_hints(hints, false), Some(HdrFormat::Hlg));
    }

    #[test]
    fn classify_bt2020_transfer_alone_as_unknown() {
        let hints = "BT.2020 (10-bit)";

        assert_eq!(classify_hdr_hints(hints, false), None);
    }

    #[test]
    fn classify_pq_as_hdr10_with_measurement_state() {
        assert_eq!(
            classify_hdr_hints("SMPTE ST 2084", true),
            Some(HdrFormat::Hdr10WithMeasurements)
        );
        assert_eq!(
            classify_hdr_hints("SMPTE ST 2084", false),
            Some(HdrFormat::Hdr10Unsupported)
        );
    }

    #[test]
    fn cm_v40_default_source_primaries_are_bt2020_for_dovi_tool() {
        assert_eq!(CmV40Config::default().source_primary_index, 2);
    }

    #[test]
    fn parse_mediainfo_duration_seconds_preserves_seconds() {
        assert_eq!(
            parse_mediainfo_duration_seconds(&json!("3438.032")),
            Some(3438.032)
        );
        assert_eq!(parse_mediainfo_duration_seconds(&json!(15.56)), Some(15.56));
    }

    #[test]
    fn parse_mediainfo_duration_seconds_handles_millisecond_scale_values() {
        assert_eq!(
            parse_mediainfo_duration_seconds(&json!("3438032")),
            Some(3438.032)
        );
    }
}
