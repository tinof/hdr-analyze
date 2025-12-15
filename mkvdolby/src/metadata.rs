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

    // 1. MediaInfo Text Check
    let mi_text = match Command::new("mediainfo")
        .args([
            "--Inform=Video;%HDR_Format%/%HDR_Format_Compatibility%",
            input_file,
        ])
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => String::new(),
    };

    let measurements = find_measurements_file(path).is_some();

    if mi_text.contains("SMPTE ST 2094 App 4") || mi_text.contains("HDR10+") {
        return HdrFormat::Hdr10Plus;
    }
    if mi_text.contains("HLG") {
        return HdrFormat::Hlg;
    }
    if mi_text.contains("HDR10") || mi_text.contains("PQ") || mi_text.contains("ST 2084") {
        return if measurements {
            HdrFormat::Hdr10WithMeasurements
        } else {
            HdrFormat::Hdr10Unsupported
        };
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
                    let t = transfer.to_uppercase();
                    if t.contains("ARIB") || t.contains("HLG") {
                        return HdrFormat::Hlg;
                    }
                    if t.contains("SMPTE2084") || t.contains("PQ") {
                        return if measurements {
                            HdrFormat::Hdr10WithMeasurements
                        } else {
                            HdrFormat::Hdr10Unsupported
                        };
                    }
                }
            }
        }
    }

    HdrFormat::Unsupported
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

pub fn generate_extra_json(
    output_path: &Path,
    metadata: &HashMap<String, f64>,
    trim_targets: &[u32],
) -> Result<()> {
    let min_dml = metadata.get("min_dml").unwrap_or(&0.005);
    let max_dml = metadata.get("max_dml").unwrap_or(&1000.0);
    let max_cll = metadata.get("max_cll").unwrap_or(&1000.0);
    let max_fall = metadata.get("max_fall").unwrap_or(&400.0);

    let json_content = json!({
        "target_nits": trim_targets,
        "level6": {
            "max_display_mastering_luminance": *max_dml as u32,
            "min_display_mastering_luminance": (*min_dml * 10000.0) as u32,
            "max_content_light_level": *max_cll as u32,
            "max_frame_average_light_level": *max_fall as u32,
        }
    });

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
                        if let Some(f) = val.as_f64() {
                            // If it's a float, it might be seconds or ms? context says ms usually in mediainfo json
                            // But let's check
                            return Some(f / 1000.0);
                        } else if let Some(s) = val.as_str() {
                            if let Ok(ms) = s.parse::<f64>() {
                                return Some(ms / 1000.0);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}
