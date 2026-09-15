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

/// Existing measurements files for `input_file`, most specific first. The analyzer's own output
/// (`<stem>_measurements.bin`) leads and the shared `measurements.bin` comes last, so a stale
/// file from another title cannot shadow the valid one.
pub fn measurements_candidates(input_file: &Path) -> Vec<PathBuf> {
    let dir = input_file.parent().unwrap_or(Path::new("."));
    let (Some(stem), Some(name)) = (input_file.file_stem(), input_file.file_name()) else {
        return Vec::new();
    };
    let stem = stem.to_string_lossy();
    let name = name.to_string_lossy();

    let candidates = [
        dir.join(format!("{stem}_measurements.bin")),
        dir.join(format!("{stem}.measurements")),
        dir.join(format!("{name}.measurements")),
        input_file.with_extension("mkv.measurements"),
        dir.join("measurements.bin"),
    ];

    let mut found: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        if candidate.exists() && !found.contains(&candidate) {
            found.push(candidate);
        }
    }
    found
}

/// The most specific existing measurements file for `input_file`.
pub fn find_measurements_file(input_file: &Path) -> Option<PathBuf> {
    measurements_candidates(input_file).into_iter().next()
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
/// Versions 1 and 2 are accepted; version 2 adds provenance and a full-resolution crop.
#[derive(Debug, Default, Deserialize)]
pub struct L1Sidecar {
    pub version: u32,
    pub scenes: Vec<L1SidecarScene>,
    #[serde(default)]
    pub analyzer_version: Option<String>,
    #[serde(default)]
    pub source: Option<L1SidecarSource>,
    #[serde(default)]
    pub analysis: Option<L1SidecarAnalysis>,
    #[serde(default)]
    pub crop_space: Option<String>,
    #[serde(default)]
    pub crop: Option<L1SidecarCrop>,
    #[serde(default)]
    pub frames: Option<L1SidecarFrames>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct L1SidecarSource {
    pub file_name: String,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct L1SidecarAnalysis {
    pub downscale: u32,
    pub sample_rate: u32,
    pub gpu: bool,
    pub no_crop: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct L1SidecarCrop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Default, Deserialize)]
pub struct L1SidecarFrames {
    pub min_pq_12bit: Vec<u16>,
}

impl L1Sidecar {
    /// Number of frames the sidecar covers (last scene end + 1).
    pub fn frame_count(&self) -> u64 {
        self.scenes
            .iter()
            .map(|scene| scene.end + 1)
            .max()
            .unwrap_or(0)
    }

    /// One-line provenance summary for progress output.
    pub fn provenance_summary(&self) -> String {
        let mut parts = vec![format!("sidecar v{}", self.version)];
        if let Some(version) = &self.analyzer_version {
            parts.push(format!("analyzer {version}"));
        }
        if let Some(analysis) = &self.analysis {
            parts.push(format!(
                "downscale {}, sample-rate {}{}",
                analysis.downscale,
                analysis.sample_rate,
                if analysis.gpu { ", GPU" } else { "" }
            ));
        }
        if let Some(crop) = self.crop {
            parts.push(format!(
                "crop {}x{}+{}+{}",
                crop.width, crop.height, crop.x, crop.y
            ));
        }
        parts.push(format!("{} frames", self.frame_count()));
        parts.join(", ")
    }
}

/// L5 active-area offsets derived from the analyzer's committed crop. Requires a version 2
/// sidecar (full-resolution crop plus source dimensions). Returns `None` for a full-frame crop
/// (dovi_tool's zero default already describes it) or when crop detection was disabled.
pub fn level5_from_sidecar(sidecar: &L1Sidecar) -> Option<Level5Offsets> {
    if sidecar.crop_space.as_deref() != Some("full") {
        return None;
    }
    if sidecar
        .analysis
        .as_ref()
        .is_some_and(|analysis| analysis.no_crop)
    {
        return None;
    }
    let source = sidecar.source.as_ref()?;
    let crop = sidecar.crop?;
    let right_edge = crop.x.checked_add(crop.width)?;
    let bottom_edge = crop.y.checked_add(crop.height)?;
    if right_edge > source.width || bottom_edge > source.height {
        return None;
    }
    let clamp = |value: u32| u16::try_from(value).unwrap_or(u16::MAX);
    let offsets = Level5Offsets {
        left: clamp(crop.x),
        right: clamp(source.width - right_edge),
        top: clamp(crop.y),
        bottom: clamp(source.height - bottom_edge),
    };
    (offsets != Level5Offsets::default()).then_some(offsets)
}

/// Why a sidecar cannot be used for source-honest L1.
#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error("no L1 sidecar at {0}")]
    Missing(PathBuf),
    #[error("L1 sidecar {path} is unreadable: {reason}")]
    Unreadable { path: PathBuf, reason: String },
    #[error("unsupported L1 sidecar version {0}")]
    UnsupportedVersion(u32),
    #[error("L1 sidecar is invalid: {0}")]
    Invalid(String),
}

/// What the caller knows about the input the sidecar must describe. Leave fields `None` for a
/// sidecar that was just produced (the analyzer run is authoritative), or when the analyzed file
/// is an intermediate rather than the input.
#[derive(Debug, Default)]
pub struct SidecarExpectation {
    pub file_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub frames: Option<u64>,
}

impl SidecarExpectation {
    /// Expect the sidecar to describe `input` exactly (identity + optional frame count).
    pub fn for_input(input: &Path, frames: Option<u64>) -> Self {
        Self {
            file_name: input
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
            size_bytes: fs::metadata(input).ok().map(|meta| meta.len()),
            frames,
        }
    }
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

/// Path of the L1 sidecar written next to a madVR measurements file.
pub fn l1_sidecar_path(measurements_file: &Path) -> PathBuf {
    let mut sidecar_path = measurements_file.as_os_str().to_owned();
    sidecar_path.push(".l1.json");
    PathBuf::from(sidecar_path)
}

/// Load and validate the L1 sidecar next to a measurements file. Every failure is reported
/// so callers can re-run analysis instead of silently using optimizer-derived L1. On success
/// the sidecar comes back with advisory warnings the caller should print.
pub fn load_l1_sidecar(
    measurements_file: &Path,
    expect: &SidecarExpectation,
) -> std::result::Result<(L1Sidecar, Vec<String>), SidecarError> {
    let path = l1_sidecar_path(measurements_file);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(SidecarError::Missing(path));
        }
        Err(error) => {
            return Err(SidecarError::Unreadable {
                path,
                reason: error.to_string(),
            });
        }
    };
    let sidecar: L1Sidecar =
        serde_json::from_reader(std::io::BufReader::new(file)).map_err(|error| {
            SidecarError::Unreadable {
                path: path.clone(),
                reason: error.to_string(),
            }
        })?;
    let advisories = validate_l1_sidecar(&sidecar, expect)?;
    Ok((sidecar, advisories))
}

/// Largest difference between the sidecar's frame count and MediaInfo's input count that still
/// reuses the sidecar. MediaInfo estimates `FrameCount` from duration when an MKV has no
/// statistics tags, so small differences are expected. `--verify` still compares the RPU with the
/// muxed output exactly, which catches real truncation.
pub fn frame_count_tolerance(expected: u64) -> u64 {
    (expected / 1000).max(2)
}

/// Most scenes listed by name in the min <= avg <= max advisory.
const MAX_ORDERING_ADVISORY_SCENES: usize = 5;

/// One advisory for scenes outside min <= avg <= max. The analyzer's average is always the
/// max-RGB mean, while the peak follows --peak-domain and --peak-estimator, so a luma peak domain
/// or a percentile/robust estimator can legitimately put the average above the peak. dovi_tool
/// clamps L1 to spec limits, so this is not a reason to discard measurements.
fn ordering_advisories(sidecar: &L1Sidecar) -> Vec<String> {
    let violations: Vec<String> = sidecar
        .scenes
        .iter()
        .enumerate()
        .filter_map(|(index, scene)| {
            if scene.avg_max_rgb_pq_12bit > scene.max_pq_12bit {
                Some(format!(
                    "scene {index}: avg {} exceeds max {}",
                    scene.avg_max_rgb_pq_12bit, scene.max_pq_12bit
                ))
            } else if scene.min_pq_12bit > scene.avg_max_rgb_pq_12bit {
                Some(format!(
                    "scene {index}: min {} exceeds avg {}",
                    scene.min_pq_12bit, scene.avg_max_rgb_pq_12bit
                ))
            } else {
                None
            }
        })
        .collect();
    if violations.is_empty() {
        return Vec::new();
    }
    let shown = violations[..violations.len().min(MAX_ORDERING_ADVISORY_SCENES)].join("; ");
    let hidden = violations
        .len()
        .saturating_sub(MAX_ORDERING_ADVISORY_SCENES);
    let more = if hidden > 0 {
        format!("; and {hidden} more")
    } else {
        String::new()
    };
    vec![format!(
        "L1 sidecar scenes are outside min <= avg <= max ({shown}{more}). This is expected with a luma peak domain or a percentile/robust peak estimator; dovi_tool clamps these values."
    )]
}

/// Structural and identity checks for a parsed sidecar. Violations that would make the RPU
/// wrong are errors; conditions the conversion tolerates come back as advisory warnings.
pub fn validate_l1_sidecar(
    sidecar: &L1Sidecar,
    expect: &SidecarExpectation,
) -> std::result::Result<Vec<String>, SidecarError> {
    if !matches!(sidecar.version, 1 | 2) {
        return Err(SidecarError::UnsupportedVersion(sidecar.version));
    }
    let invalid = |reason: String| Err(SidecarError::Invalid(reason));
    let Some(first) = sidecar.scenes.first() else {
        return invalid("no scenes".into());
    };
    if first.start != 0 {
        return invalid(format!("first scene starts at frame {}", first.start));
    }
    for (index, scene) in sidecar.scenes.iter().enumerate() {
        if scene.end < scene.start {
            return invalid(format!("scene {index} ends before it starts"));
        }
        if let Some(previous) = index.checked_sub(1).map(|i| &sidecar.scenes[i]) {
            if scene.start != previous.end + 1 {
                return invalid(format!(
                    "scene {index} starts at frame {} but the previous scene ended at {}",
                    scene.start, previous.end
                ));
            }
        }
    }
    let mut advisories = ordering_advisories(sidecar);
    let covered = sidecar.frame_count();
    if let Some(frames) = &sidecar.frames {
        if frames.min_pq_12bit.len() as u64 != covered {
            return invalid(format!(
                "scenes cover {covered} frames but per-frame data has {}",
                frames.min_pq_12bit.len()
            ));
        }
    }
    if let Some(expected) = expect.frames {
        let difference = expected.abs_diff(covered);
        let tolerance = frame_count_tolerance(expected);
        if difference > tolerance {
            return invalid(format!(
                "covers {covered} frames but the input video has {expected}"
            ));
        }
        if difference > 0 {
            advisories.push(format!(
                "L1 sidecar covers {covered} frames but MediaInfo reports {expected} for the input; within the {tolerance}-frame tolerance, because MediaInfo estimates the count from duration when the MKV has no statistics tags."
            ));
        }
    }
    if let Some(source) = &sidecar.source {
        let name_differs = expect
            .file_name
            .as_ref()
            .is_some_and(|name| *name != source.file_name);
        let size_differs = expect
            .size_bytes
            .is_some_and(|size| size != source.size_bytes);
        if name_differs || size_differs {
            return invalid(format!(
                "produced for '{}' ({} bytes), not this input",
                source.file_name, source.size_bytes
            ));
        }
    }
    Ok(advisories)
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

/// Video-track frame count reported by MediaInfo (`FrameCount` on the first Video track).
pub fn get_frame_count(input_file: &str) -> Option<u64> {
    let json = get_mediainfo_json(input_file).ok()?;
    video_track_frame_count(&json)
}

fn video_track_frame_count(json: &Value) -> Option<u64> {
    json.pointer("/media/track")?
        .as_array()?
        .iter()
        .find(|track| track.get("@type").and_then(Value::as_str) == Some("Video"))
        .and_then(|track| track.get("FrameCount"))
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str()?.trim().parse::<u64>().ok())
        })
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
            ..Default::default()
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

    fn scene(start: u64, end: u64, min: u16, avg: u16, max: u16) -> L1SidecarScene {
        L1SidecarScene {
            start,
            end,
            min_pq_12bit: min,
            avg_luma_pq_12bit: avg,
            avg_max_rgb_pq_12bit: avg,
            max_pq_12bit: max,
        }
    }

    fn v2_sidecar_json() -> Value {
        json!({
            "version": 2,
            "analyzer_version": "0.3.0 (+cuda)",
            "source": {"file_name": "input.mkv", "size_bytes": 42, "width": 3840, "height": 2160,
                       "transfer_function": "PQ (SMPTE 2084)"},
            "analysis": {"downscale": 1, "sample_rate": 1, "gpu": true, "no_crop": false},
            "crop_space": "full",
            "crop": {"x": 0, "y": 280, "width": 3840, "height": 1600},
            "scenes": [
                {"start": 0, "end": 9, "min_pq_12bit": 1, "avg_luma_pq_12bit": 500,
                 "avg_max_rgb_pq_12bit": 520, "max_pq_12bit": 2400},
                {"start": 10, "end": 19, "min_pq_12bit": 2, "avg_luma_pq_12bit": 600,
                 "avg_max_rgb_pq_12bit": 610, "max_pq_12bit": 2500}
            ],
            "frames": {"min_pq_12bit": vec![0; 20]}
        })
    }

    #[test]
    fn v1_sidecar_without_provenance_still_validates() {
        let sidecar: L1Sidecar = serde_json::from_value(json!({
            "version": 1,
            "scenes": [{"start": 0, "end": 4, "min_pq_12bit": 0, "avg_luma_pq_12bit": 10,
                        "avg_max_rgb_pq_12bit": 12, "max_pq_12bit": 100}]
        }))
        .unwrap();
        assert!(validate_l1_sidecar(&sidecar, &SidecarExpectation::default()).is_ok());
        assert!(sidecar.source.is_none());
    }

    #[test]
    fn v2_sidecar_matching_the_input_validates() {
        let sidecar: L1Sidecar = serde_json::from_value(v2_sidecar_json()).unwrap();
        let expect = SidecarExpectation {
            file_name: Some("input.mkv".into()),
            size_bytes: Some(42),
            frames: Some(20),
        };
        assert!(validate_l1_sidecar(&sidecar, &expect).unwrap().is_empty());
        assert!(sidecar
            .provenance_summary()
            .contains("crop 3840x1600+0+280"));
    }

    #[test]
    fn sidecar_for_a_different_input_is_rejected() {
        let sidecar: L1Sidecar = serde_json::from_value(v2_sidecar_json()).unwrap();
        let expect = SidecarExpectation {
            file_name: Some("input.mkv".into()),
            size_bytes: Some(43),
            frames: None,
        };
        assert!(matches!(
            validate_l1_sidecar(&sidecar, &expect),
            Err(SidecarError::Invalid(reason)) if reason.contains("not this input")
        ));
    }

    #[test]
    fn sidecar_frame_count_mismatch_is_rejected() {
        let sidecar: L1Sidecar = serde_json::from_value(v2_sidecar_json()).unwrap();
        let expect = SidecarExpectation {
            frames: Some(40),
            ..Default::default()
        };
        assert!(matches!(
            validate_l1_sidecar(&sidecar, &expect),
            Err(SidecarError::Invalid(_))
        ));
    }

    #[test]
    fn structurally_broken_sidecars_are_rejected() {
        let cases = [
            vec![scene(0, 9, 1, 5, 40), scene(11, 19, 1, 5, 40)], // gap
            vec![scene(5, 9, 1, 5, 40)],                          // does not start at 0
            vec![],                                               // empty
        ];
        for scenes in cases {
            let sidecar = L1Sidecar {
                version: 2,
                scenes,
                ..Default::default()
            };
            assert!(matches!(
                validate_l1_sidecar(&sidecar, &SidecarExpectation::default()),
                Err(SidecarError::Invalid(_))
            ));
        }
        let future = L1Sidecar {
            version: 3,
            scenes: vec![scene(0, 1, 0, 1, 2)],
            ..Default::default()
        };
        assert!(matches!(
            validate_l1_sidecar(&future, &SidecarExpectation::default()),
            Err(SidecarError::UnsupportedVersion(3))
        ));
    }

    #[test]
    fn avg_above_max_is_an_advisory_not_an_error() {
        let sidecar = L1Sidecar {
            version: 2,
            scenes: vec![scene(0, 9, 1, 3000, 2900), scene(10, 19, 1, 5, 40)],
            ..Default::default()
        };
        let advisories = validate_l1_sidecar(&sidecar, &SidecarExpectation::default()).unwrap();
        assert_eq!(advisories.len(), 1);
        assert!(advisories[0].contains("scene 0: avg 3000 exceeds max 2900"));
        assert!(!advisories[0].contains("scene 1"));
    }

    #[test]
    fn ordering_advisory_lists_at_most_five_scenes() {
        let scenes = (0..8)
            .map(|index| scene(index * 10, index * 10 + 9, 1, 50, 40))
            .collect();
        let sidecar = L1Sidecar {
            version: 2,
            scenes,
            ..Default::default()
        };
        let advisories = validate_l1_sidecar(&sidecar, &SidecarExpectation::default()).unwrap();
        assert_eq!(advisories.len(), 1);
        assert!(advisories[0].contains("scene 4:"));
        assert!(!advisories[0].contains("scene 5:"));
        assert!(advisories[0].contains("and 3 more"));
    }

    #[test]
    fn freshly_written_sidecar_with_avg_above_max_loads() {
        let dir = tempfile::tempdir().unwrap();
        let measurements = dir.path().join("title_measurements.bin");
        fs::write(&measurements, b"").unwrap();
        let mut sidecar = v2_sidecar_json();
        sidecar["scenes"][1]["avg_max_rgb_pq_12bit"] = json!(3000);
        sidecar["scenes"][1]["max_pq_12bit"] = json!(2900);
        fs::write(
            l1_sidecar_path(&measurements),
            serde_json::to_vec(&sidecar).unwrap(),
        )
        .unwrap();
        let (loaded, advisories) =
            load_l1_sidecar(&measurements, &SidecarExpectation::default()).unwrap();
        assert_eq!(loaded.frame_count(), 20);
        assert_eq!(advisories.len(), 1);
    }

    #[test]
    fn sidecar_frame_count_within_tolerance_validates_with_advisory() {
        let sidecar: L1Sidecar = serde_json::from_value(v2_sidecar_json()).unwrap();
        for frames in [18, 21, 22] {
            let expect = SidecarExpectation {
                frames: Some(frames),
                ..Default::default()
            };
            let advisories = validate_l1_sidecar(&sidecar, &expect).unwrap();
            assert_eq!(advisories.len(), 1, "{frames} frames");
            assert!(advisories[0].contains(&format!("MediaInfo reports {frames}")));
        }
        assert_eq!(frame_count_tolerance(20), 2);
        assert_eq!(frame_count_tolerance(143_562), 143);
    }

    #[test]
    fn measurements_candidates_prefer_the_analyzer_output() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("title.mkv");
        let shared = dir.path().join("measurements.bin");
        let own = dir.path().join("title_measurements.bin");
        let named = dir.path().join("title.mkv.measurements");

        assert!(measurements_candidates(&input).is_empty());
        fs::write(&shared, b"stale").unwrap();
        assert_eq!(measurements_candidates(&input), vec![shared.clone()]);

        fs::write(&own, b"valid").unwrap();
        fs::write(&named, b"other").unwrap();
        assert_eq!(
            measurements_candidates(&input),
            vec![own.clone(), named, shared]
        );
        assert_eq!(find_measurements_file(&input), Some(own));
    }

    #[test]
    fn letterbox_crop_becomes_level5_offsets() {
        let sidecar: L1Sidecar = serde_json::from_value(v2_sidecar_json()).unwrap();
        assert_eq!(
            level5_from_sidecar(&sidecar),
            Some(Level5Offsets {
                left: 0,
                right: 0,
                top: 280,
                bottom: 280,
            })
        );
    }

    #[test]
    fn full_frame_or_legacy_crop_emits_no_level5() {
        let mut full = v2_sidecar_json();
        full["crop"] = json!({"x": 0, "y": 0, "width": 3840, "height": 2160});
        let sidecar: L1Sidecar = serde_json::from_value(full).unwrap();
        assert_eq!(level5_from_sidecar(&sidecar), None);

        let mut no_crop = v2_sidecar_json();
        no_crop["analysis"]["no_crop"] = json!(true);
        let sidecar: L1Sidecar = serde_json::from_value(no_crop).unwrap();
        assert_eq!(level5_from_sidecar(&sidecar), None);

        let mut v1 = v2_sidecar_json();
        v1["version"] = json!(1);
        v1.as_object_mut().unwrap().remove("crop_space");
        let sidecar: L1Sidecar = serde_json::from_value(v1).unwrap();
        assert_eq!(level5_from_sidecar(&sidecar), None);
    }

    #[test]
    fn missing_sidecar_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_l1_sidecar(&dir.path().join("m.bin"), &SidecarExpectation::default());
        assert!(matches!(result, Err(SidecarError::Missing(_))));
    }

    #[test]
    fn mediainfo_video_frame_count_is_parsed() {
        let json = json!({"media": {"track": [
            {"@type": "General", "FrameCount": "999"},
            {"@type": "Video", "FrameCount": "2908"}
        ]}});
        assert_eq!(video_track_frame_count(&json), Some(2908));
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
