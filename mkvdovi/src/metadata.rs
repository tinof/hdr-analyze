use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::external;
use crate::rpu_check::{self, Level5Offsets, RpuFormatKind};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum HdrFormat {
    Hdr10Plus,
    Hdr10WithMeasurements,
    Hdr10Unsupported,
    Hlg,
    DolbyVisionMel,
    DolbyVisionFel,
    DolbyVisionP8,
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
            HdrFormat::DolbyVisionMel => "Dolby Vision Profile 7 MEL",
            HdrFormat::DolbyVisionFel => "Dolby Vision Profile 7 FEL",
            HdrFormat::DolbyVisionP8 => "Dolby Vision Profile 8",
            HdrFormat::Unsupported => "Unsupported",
        }
    }
}

/// Configuration for CM v4.0 metadata generation
#[derive(Debug, Clone)]
pub struct CmV40Config {
    /// Source primary index for L9 (0=P3-D65, 1=BT.709, 2=BT.2020)
    pub source_primary_index: u8,
    /// Content type for L11 (0-4, see ContentType enum)
    pub content_type: u8,
    /// Reference mode flag for L11
    pub reference_mode: bool,
}

impl Default for CmV40Config {
    fn default() -> Self {
        Self {
            source_primary_index: 2, // BT.2020
            content_type: 1,         // Movies
            reference_mode: false,
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

    // Check for Dolby Vision Profile 7 FEL (dual layer)
    // MediaInfo shows "dvhe.07" or "Dolby Vision, Version 1.0, Profile 7"
    let mi_dv_text = match Command::new("mediainfo")
        .args([
            "--Inform=Video;%HDR_Format%/%HDR_Format_Profile%/%HDR_Format_Level%",
            input_file,
        ])
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => String::new(),
    };

    // Detect Dolby Vision before generic HDR10/PQ fallback.
    let mi_codec = match Command::new("mediainfo")
        .args(["--Inform=Video;%CodecID%", input_file])
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => String::new(),
    };
    let dv_probe = format!("{} / {} / {}", mi_hints, mi_dv_text, mi_codec).to_lowercase();

    if dv_probe.contains("dvhe") || dv_probe.contains("dolby vision") {
        if dv_probe.contains("dvhe.08") || dv_probe.contains("profile 8") {
            return HdrFormat::DolbyVisionP8;
        }

        if dv_probe.contains("dvhe.07") || dv_probe.contains("profile 7") {
            return probe_profile7_kind(input_file).unwrap_or(HdrFormat::DolbyVisionFel);
        }
    }

    if let Some(dv_format) = sniff_dolby_vision_rpu(input_file) {
        return dv_format;
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

fn probe_profile7_kind(input_file: &str) -> Option<HdrFormat> {
    let mut temp_dir = std::env::temp_dir();
    temp_dir.push(format!("mkvdovi_dv_probe_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).ok()?;

    let result = rpu_check::extract_rpu_sample(input_file, &temp_dir)
        .ok()
        .and_then(|rpu| rpu_check::classify_rpu_format(&rpu).ok())
        .map(hdr_format_from_rpu);

    let _ = fs::remove_dir_all(&temp_dir);
    result
}

fn sniff_dolby_vision_rpu(input_file: &str) -> Option<HdrFormat> {
    let mut temp_dir = std::env::temp_dir();
    temp_dir.push(format!("mkvdovi_dv_sniff_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).ok()?;
    let rpu_path = temp_dir.join("sniff_RPU.bin");

    let result = if rpu_check::try_extract_rpu_quiet(input_file, &rpu_path, Some(60)) {
        rpu_check::classify_rpu_format(&rpu_path)
            .ok()
            .map(hdr_format_from_rpu)
    } else {
        None
    };

    let _ = fs::remove_dir_all(&temp_dir);
    result
}

fn hdr_format_from_rpu(kind: RpuFormatKind) -> HdrFormat {
    match kind {
        RpuFormatKind::Profile7Mel => HdrFormat::DolbyVisionMel,
        RpuFormatKind::Profile7Fel => HdrFormat::DolbyVisionFel,
        RpuFormatKind::Profile8 => HdrFormat::DolbyVisionP8,
        RpuFormatKind::OtherDolbyVision => HdrFormat::Unsupported,
    }
}

pub fn get_static_metadata(input_file: &str) -> HashMap<String, f64> {
    let mut meta: HashMap<String, f64> = HashMap::new();

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

                    // Parse MasteringDisplay_ColorPrimaries
                    if let Some(mdcp) = track
                        .get("MasteringDisplay_ColorPrimaries")
                        .and_then(|s| s.as_str())
                    {
                        if let Some((gx, gy, bx, by, rx, ry, wpx, wpy)) =
                            parse_mastering_display_color_primaries(mdcp)
                        {
                            meta.insert("md_gx".to_string(), gx as f64);
                            meta.insert("md_gy".to_string(), gy as f64);
                            meta.insert("md_bx".to_string(), bx as f64);
                            meta.insert("md_by".to_string(), by as f64);
                            meta.insert("md_rx".to_string(), rx as f64);
                            meta.insert("md_ry".to_string(), ry as f64);
                            meta.insert("md_wpx".to_string(), wpx as f64);
                            meta.insert("md_wpy".to_string(), wpy as f64);
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

    // Details.txt override (MaxCLL/MaxFALL only; mastering display comes from container metadata)
    if let Some(details_path) = find_details_file(Path::new(input_file)) {
        if let Ok(content) = fs::read_to_string(details_path) {
            let re_cll = Regex::new(r"(?i)MaxCLL\s*:\s*([0-9.,]+)").unwrap();
            let re_fall = Regex::new(r"(?i)MaxFALL\s*:\s*([0-9.,]+)").unwrap();

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

    // Warn for any values that could not be sourced from file metadata, then apply fallbacks.
    // These warnings matter: the display will build its tone-mapping from these values.
    let fallbacks: &[(&str, f64, &str)] = &[
        (
            "max_dml",
            1000.0,
            "mastering display peak luminance (L6 MaxDML)",
        ),
        (
            "min_dml",
            0.005,
            "mastering display min luminance (L6 MinDML)",
        ),
        ("max_cll", 1000.0, "MaxCLL (L6)"),
        ("max_fall", 400.0, "MaxFALL (L6)"),
    ];
    for &(key, default, label) in fallbacks {
        if !meta.contains_key(key) {
            eprintln!(
                "WARNING: {} not found in source metadata; using default {:.4} nits for the Dolby Vision L6 block. \
                 Use mediainfo to verify the source has mastering display / light-level metadata.",
                label, default
            );
            meta.insert(key.to_string(), default);
        }
    }

    meta
}

fn parse_mastering_display_color_primaries(
    mdcp: &str,
) -> Option<(u32, u32, u32, u32, u32, u32, u32, u32)> {
    fn parse_xy(mdcp: &str, label: &str) -> Option<(f64, f64)> {
        // Accept both:
        // - G(x=0.1700, y=0.7970)
        // - G(0.1700,0.7970)
        let re = Regex::new(&format!(
            r"{}\(\s*(?:x=)?([0-9]*\.?[0-9]+)\s*,\s*(?:y=)?([0-9]*\.?[0-9]+)\s*\)",
            regex::escape(label)
        ))
        .ok()?;

        let caps = re.captures(mdcp)?;
        let x = caps.get(1)?.as_str().parse::<f64>().ok()?;
        let y = caps.get(2)?.as_str().parse::<f64>().ok()?;
        Some((x, y))
    }

    fn to_int(v: f64) -> u32 {
        let scaled = (v * 50000.0).round();
        scaled.clamp(0.0, 50000.0) as u32
    }

    let (gx, gy) = parse_xy(mdcp, "G")?;
    let (bx, by) = parse_xy(mdcp, "B")?;
    let (rx, ry) = parse_xy(mdcp, "R")?;
    let (wpx, wpy) = parse_xy(mdcp, "WP")?;

    Some((
        to_int(gx),
        to_int(gy),
        to_int(bx),
        to_int(by),
        to_int(rx),
        to_int(ry),
        to_int(wpx),
        to_int(wpy),
    ))
}

/// Detect source mastering-display primaries from MediaInfo.
/// Warns and returns BT.2020 (index 2) when primaries cannot be determined.
pub fn detect_source_primaries(input_file: &str) -> u8 {
    match get_mediainfo_json(input_file)
        .ok()
        .and_then(|json| detect_source_primaries_from_mediainfo(&json))
    {
        Some(idx) => idx,
        None => {
            eprintln!(
                "WARNING: Source color primaries not detected from MediaInfo; \
                 defaulting to BT.2020 (L9 index 2). \
                 Use --source-primaries 0 to override if content was mastered on P3-D65."
            );
            2
        }
    }
}

fn detect_source_primaries_from_mediainfo(json: &Value) -> Option<u8> {
    let tracks = json
        .get("media")
        .and_then(|m| m.get("track"))
        .and_then(|t| t.as_array())?;

    tracks
        .iter()
        .filter(|track| track.get("@type").and_then(|s| s.as_str()) == Some("Video"))
        .find_map(|track| {
            // L9 describes the mastering display, not the BT.2020 signal container.
            track
                .get("MasteringDisplay_ColorPrimaries")
                .or_else(|| track.get("mastering_display_color_primaries"))
                .and_then(|value| value.as_str())
                .and_then(primary_index_from_label)
                .or_else(|| {
                    track
                        .get("colour_primaries")
                        .or_else(|| track.get("ColorPrimaries"))
                        .and_then(|value| value.as_str())
                        .and_then(primary_index_from_label)
                })
        })
}

fn primary_index_from_label(primaries: &str) -> Option<u8> {
    let primaries = primaries.to_uppercase();
    if primaries.contains("P3") || primaries.contains("DCI") {
        Some(0) // P3-D65
    } else if primaries.contains("709") {
        Some(1) // BT.709
    } else if primaries.contains("2020") {
        Some(2) // BT.2020
    } else {
        None
    }
}

/// Per-scene L1 statistics from the analyzer's `<measurements>.l1.json` sidecar.
#[derive(Debug, Deserialize)]
pub struct L1Sidecar {
    pub version: u32,
    pub scenes: Vec<L1SidecarScene>,
}

#[derive(Debug, Deserialize)]
pub struct L1SidecarScene {
    pub start: u64,
    pub end: u64,
    pub min_pq_12bit: u16,
    /// Retained for sidecar-schema completeness; the RPU average uses the max-RGB mean
    /// (validated against cm v2 shot averages), not the Y-luma mean.
    #[allow(dead_code)]
    pub avg_luma_pq_12bit: u16,
    pub avg_max_rgb_pq_12bit: u16,
    pub max_pq_12bit: u16,
}

/// Load the L1 sidecar written next to a madVR measurements file, if present and valid.
/// Returns `None` when the sidecar is missing, unreadable, or an unsupported version, so
/// callers can fall back to legacy madVR-file generation.
pub fn load_l1_sidecar(measurements_file: &Path) -> Option<L1Sidecar> {
    let mut sidecar_path = measurements_file.as_os_str().to_owned();
    sidecar_path.push(".l1.json");
    let file = File::open(Path::new(&sidecar_path)).ok()?;
    let sidecar: L1Sidecar = serde_json::from_reader(file).ok()?;
    if sidecar.version != 1 || sidecar.scenes.is_empty() {
        return None;
    }
    Some(sidecar)
}

pub fn generate_extra_json(
    output_path: &Path,
    metadata: &HashMap<String, f64>,
    trim_targets: &[u32],
    cm_v40_config: Option<&CmV40Config>,
    level5_offsets: Option<Level5Offsets>,
    l1_sidecar: Option<&L1Sidecar>,
) -> Result<()> {
    let required = |key| {
        metadata
            .get(key)
            .copied()
            .with_context(|| format!("Missing required static metadata value: {key}"))
    };
    let min_dml = required("min_dml")?;
    let max_dml = required("max_dml")?;
    let max_cll = required("max_cll")?;
    let max_fall = required("max_fall")?;

    let mut json_content = json!({
        "profile": "8.1",
        "level6": {
            "max_display_mastering_luminance": max_dml as u32,
            "min_display_mastering_luminance": (min_dml * 10000.0) as u32,
            "max_content_light_level": max_cll as u32,
            "max_frame_average_light_level": max_fall as u32,
        }
    });

    if let Some(offsets) = level5_offsets {
        json_content["level5"] = json!({
            "active_area_left_offset": offsets.left,
            "active_area_right_offset": offsets.right,
            "active_area_top_offset": offsets.top,
            "active_area_bottom_offset": offsets.bottom,
        });
    }

    // Add CM v4.0 specific configuration
    if let Some(cfg) = cm_v40_config {
        json_content["cm_version"] = json!("V40");

        // Build default metadata blocks with L2 trims for each target nit level, plus L9 and L11
        let mut default_blocks = Vec::new();

        // Add L2 trims for each target nit level (100, 600, 1000, etc.)
        // These provide baseline trim values that dovi_tool will use.
        for target in trim_targets {
            let target_pq = nits_to_pq_code(*target);

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

    // Source-honest per-scene L1 from the analyzer sidecar. Explicit shots carry the
    // measured min/avg/max; `l1_avg_pq_cm_version: V29` selects the lower spec floor for
    // `avg_pq` (819 vs the CM v4.0 anchor 1229) so CM v2.9-only displays receive the true
    // scene average instead of a placeholder. dovi_tool still clamps to spec limits.
    if let Some(sidecar) = l1_sidecar {
        let length = sidecar
            .scenes
            .iter()
            .map(|scene| scene.end + 1)
            .max()
            .unwrap_or(0);
        let shots: Vec<Value> = sidecar
            .scenes
            .iter()
            .map(|scene| {
                json!({
                    "start": scene.start,
                    "duration": scene.end - scene.start + 1,
                    "metadata_blocks": [{
                        "Level1": {
                            "min_pq": scene.min_pq_12bit,
                            "max_pq": scene.max_pq_12bit,
                            "avg_pq": scene.avg_max_rgb_pq_12bit,
                        }
                    }],
                })
            })
            .collect();
        json_content["length"] = json!(length);
        json_content["l1_avg_pq_cm_version"] = json!("V29");
        json_content["shots"] = json!(shots);
    }

    let file = File::create(output_path)?;
    serde_json::to_writer_pretty(file, &json_content)?;
    Ok(())
}

fn nits_to_pq_code(nits: u32) -> u32 {
    const MAX_NITS: f64 = 10_000.0;
    const MAX_PQ_CODE: f64 = 4095.0;
    const M1: f64 = 2610.0 / 16384.0;
    const M2: f64 = 2523.0 / 32.0;
    const C1: f64 = 3424.0 / 4096.0;
    const C2: f64 = 2413.0 / 128.0;
    const C3: f64 = 2392.0 / 128.0;

    let normalized_luminance = f64::from(nits.min(MAX_NITS as u32)) / MAX_NITS;
    let luminance_m1 = normalized_luminance.powf(M1);
    let pq = ((C1 + C2 * luminance_m1) / (1.0 + C3 * luminance_m1)).powf(M2);

    (pq * MAX_PQ_CODE).round() as u32
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
    fn cm_v40_default_l11_uses_movies_without_reference_mode() {
        let config = CmV40Config::default();

        assert_eq!(config.content_type, 1);
        assert!(!config.reference_mode);
    }

    #[test]
    fn source_primaries_prefer_display_p3_mastering_display_over_bt2020_container() {
        let mediainfo = json!({
            "media": {
                "track": [{
                    "@type": "Video",
                    "colour_primaries": "BT.2020",
                    "MasteringDisplay_ColorPrimaries": "Display P3"
                }]
            }
        });

        assert_eq!(detect_source_primaries_from_mediainfo(&mediainfo), Some(0));
    }

    #[test]
    fn source_primaries_fall_back_to_container_when_mastering_display_is_absent() {
        let mediainfo = json!({
            "media": {
                "track": [{
                    "@type": "Video",
                    "colour_primaries": "BT.2020"
                }]
            }
        });

        assert_eq!(detect_source_primaries_from_mediainfo(&mediainfo), Some(2));
    }

    #[test]
    fn cm_v40_json_uses_requested_l9_and_l11_values() {
        let output = tempfile::NamedTempFile::new().unwrap();
        let metadata = HashMap::from([
            ("min_dml".to_string(), 0.0001),
            ("max_dml".to_string(), 1000.0),
            ("max_cll".to_string(), 211.0),
            ("max_fall".to_string(), 125.0),
        ]);
        let config = CmV40Config {
            source_primary_index: 0,
            content_type: 1,
            reference_mode: false,
        };

        generate_extra_json(
            output.path(),
            &metadata,
            &[100, 600, 1000],
            Some(&config),
            None,
            None,
        )
        .unwrap();

        let json: Value = serde_json::from_reader(File::open(output.path()).unwrap()).unwrap();
        let blocks = json["default_metadata_blocks"].as_array().unwrap();
        assert!(json.get("target_nits").is_none());
        assert_eq!(blocks[3]["Level9"]["source_primary_index"], 0);
        assert_eq!(blocks[4]["Level11"]["content_type"], 1);
        assert_eq!(blocks[4]["Level11"]["reference_mode_flag"], false);
    }

    #[test]
    fn json_generation_rejects_missing_static_metadata() {
        let output = tempfile::NamedTempFile::new().unwrap();
        let metadata = HashMap::new();

        let error =
            generate_extra_json(output.path(), &metadata, &[], None, None, None).unwrap_err();

        assert!(error.to_string().contains("min_dml"));
    }

    #[test]
    fn l1_sidecar_scenes_become_source_honest_shots() {
        let output = tempfile::NamedTempFile::new().unwrap();
        let metadata = HashMap::from([
            ("min_dml".to_string(), 0.0001),
            ("max_dml".to_string(), 1000.0),
            ("max_cll".to_string(), 997.0),
            ("max_fall".to_string(), 91.0),
        ]);
        let sidecar = L1Sidecar {
            version: 1,
            scenes: vec![
                L1SidecarScene {
                    start: 0,
                    end: 213,
                    min_pq_12bit: 0,
                    avg_luma_pq_12bit: 592,
                    avg_max_rgb_pq_12bit: 614,
                    max_pq_12bit: 2437,
                },
                L1SidecarScene {
                    start: 214,
                    end: 333,
                    min_pq_12bit: 1,
                    avg_luma_pq_12bit: 441,
                    avg_max_rgb_pq_12bit: 462,
                    max_pq_12bit: 3416,
                },
            ],
        };

        generate_extra_json(output.path(), &metadata, &[], None, None, Some(&sidecar)).unwrap();

        let json: Value = serde_json::from_reader(File::open(output.path()).unwrap()).unwrap();
        assert_eq!(json["length"], 334);
        assert_eq!(json["l1_avg_pq_cm_version"], "V29");
        let shots = json["shots"].as_array().unwrap();
        assert_eq!(shots.len(), 2);
        assert_eq!(shots[0]["start"], 0);
        assert_eq!(shots[0]["duration"], 214);
        let l1 = &shots[0]["metadata_blocks"][0]["Level1"];
        assert_eq!(l1["min_pq"], 0);
        assert_eq!(l1["avg_pq"], 614);
        assert_eq!(l1["max_pq"], 2437);
        assert_eq!(shots[1]["metadata_blocks"][0]["Level1"]["avg_pq"], 462);
    }

    #[test]
    fn trim_target_nits_are_converted_to_pq_codes() {
        assert_eq!(nits_to_pq_code(100), 2081);
        assert_eq!(nits_to_pq_code(600), 2851);
        assert_eq!(nits_to_pq_code(1000), 3079);
        assert!(nits_to_pq_code(680) > nits_to_pq_code(600));
        assert!(nits_to_pq_code(680) < nits_to_pq_code(1000));
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
