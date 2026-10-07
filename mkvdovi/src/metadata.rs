use anyhow::{Context, Result};
use dolby_vision::rpu::profiles::{profile84::Profile84, DoviProfile};
use dolby_vision::utils::nits_to_pq_12_bit;
use dovi84_composer::Composer;
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

/// True when MediaInfo/ffprobe transfer hints name HLG (ARIB STD-B67).
fn hints_indicate_hlg(hints: &str) -> bool {
    let hints = hints.to_uppercase();
    hints.contains("HLG") || hints.contains("ARIB")
}

/// True when the input's video track uses (or declares compatibility with) the HLG transfer,
/// e.g. the base layer of a Dolby Vision Profile 8.4 file. MediaInfo first, ffprobe fallback.
/// Transfer tag of the first decoded video frame as ffprobe reports it. It reflects the HEVC
/// VUI plus the alternative transfer characteristics SEI, not the MKV colour element.
pub fn first_frame_color_transfer(input_file: &str) -> Option<String> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-read_intervals",
            "%+#1",
            "-show_entries",
            "frame=color_transfer",
            "-of",
            "default=nw=1:nk=1",
            input_file,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// True for an HLG input whose HEVC bitstream does not signal HLG itself, neither in the VUI nor
/// through the alternative transfer SEI (for example HLG tagged only in the MKV colour element).
/// mkvmerge derives the Dolby Vision compatibility ID from the bitstream, so such a stream needs
/// its VUI rewritten to keep ID 4 (HLG). Returns false when ffprobe is unavailable.
pub fn hlg_bitstream_lacks_transfer(input_file: &str) -> bool {
    first_frame_color_transfer(input_file).is_some_and(|transfer| transfer != "arib-std-b67")
}

pub fn has_hlg_transfer(input_file: &str) -> bool {
    let mut hints = String::new();
    if let Ok(json) = get_mediainfo_json(input_file) {
        append_mediainfo_video_hints(&json, &mut hints);
    }
    if hints_indicate_hlg(&hints) {
        return true;
    }
    get_ffprobe_json(input_file).is_ok_and(|json| {
        json.get("streams")
            .and_then(Value::as_array)
            .is_some_and(|streams| {
                streams.iter().any(|stream| {
                    stream
                        .get("color_transfer")
                        .and_then(Value::as_str)
                        .is_some_and(hints_indicate_hlg)
                })
            })
    })
}

/// A colour field a Dolby Vision Profile 8.4 RPU fixes for its HLG base layer: the RPU's
/// `ycc_to_rgb` is limited-range BT.2020 non-constant-luminance, and the base layer is copied
/// bit-exact, so an input tagged otherwise decodes wrong on every Dolby Vision display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColourField {
    Range,
    Matrix,
    Primaries,
}

impl ColourField {
    const ALL: [ColourField; 3] = [
        ColourField::Range,
        ColourField::Matrix,
        ColourField::Primaries,
    ];

    fn label(self) -> &'static str {
        match self {
            ColourField::Range => "range",
            ColourField::Matrix => "matrix",
            ColourField::Primaries => "primaries",
        }
    }

    /// What Profile 8.4 requires, and what an untagged field is assumed to be.
    fn requirement(self) -> &'static str {
        match self {
            ColourField::Range => "limited range",
            ColourField::Matrix => "BT.2020 non-constant-luminance matrix",
            ColourField::Primaries => "BT.2020 primaries",
        }
    }

    /// MediaInfo keys: the effective value (container or stream) and, when the two differ, the
    /// stream's own value.
    fn mediainfo_keys(self) -> [&'static str; 2] {
        match self {
            ColourField::Range => ["colour_range", "colour_range_Original"],
            ColourField::Matrix => ["matrix_coefficients", "matrix_coefficients_Original"],
            ColourField::Primaries => ["colour_primaries", "colour_primaries_Original"],
        }
    }

    fn ffprobe_key(self) -> &'static str {
        match self {
            ColourField::Range => "color_range",
            ColourField::Matrix => "color_space",
            ColourField::Primaries => "color_primaries",
        }
    }

    /// `Some(true)` for a value Profile 8.4 can describe, `Some(false)` for any other named value
    /// (including ones this list does not know, so they are shown rather than let through), and
    /// `None` for an untagged one. Accepts MediaInfo and ffprobe spellings.
    fn conforms(self, value: &str) -> Option<bool> {
        let value = value.trim().to_ascii_lowercase();
        if matches!(value.as_str(), "" | "unknown" | "unspecified") {
            return None;
        }
        let accepted: &[&str] = match self {
            ColourField::Range => &["limited", "tv", "mpeg"],
            ColourField::Matrix => &["bt.2020 non-constant", "bt2020nc", "bt2020_ncl"],
            ColourField::Primaries => &["bt.2020", "bt2020"],
        };
        Some(accepted.contains(&value.as_str()))
    }
}

/// First track (MediaInfo) or stream (ffprobe) whose type field names video.
fn first_video_entry<'a>(
    json: &'a Value,
    list: &[&str],
    type_key: &str,
    video: &str,
) -> Option<&'a Value> {
    let mut entries = json;
    for key in list {
        entries = entries.get(key)?;
    }
    entries
        .as_array()?
        .iter()
        .find(|entry| entry.get(type_key).and_then(Value::as_str) == Some(video))
}

/// The HLG input contract of Profile 8.4 (docs/HLG_COMPOSER.md section 11), from MediaInfo JSON
/// (`--Output=JSON`) and ffprobe JSON (`-show_streams`). Each field is resolved on its own from
/// every source, so a partial MediaInfo result does not keep ffprobe from filling the rest. Any
/// source naming a non-conforming value refuses, and the error lists every source and value of
/// that field, so conflicting tags are reported, not resolved. Untagged fields are accepted under
/// the Profile 8.4 assumption; the `Ok` value holds the warning that says so.
fn hlg_colour_contract(
    mediainfo: Option<&Value>,
    ffprobe: Option<&Value>,
) -> std::result::Result<Vec<String>, String> {
    let mediainfo_video =
        mediainfo.and_then(|json| first_video_entry(json, &["media", "track"], "@type", "Video"));
    let ffprobe_video =
        ffprobe.and_then(|json| first_video_entry(json, &["streams"], "codec_type", "video"));

    let mut refusals = Vec::new();
    let mut untagged = Vec::new();
    for field in ColourField::ALL {
        let mut observed: Vec<(String, String)> = Vec::new();
        for key in field.mediainfo_keys() {
            if let Some(value) = mediainfo_video.and_then(|track| track.get(key)?.as_str()) {
                observed.push((format!("MediaInfo {key}"), value.to_owned()));
            }
        }
        let key = field.ffprobe_key();
        if let Some(value) = ffprobe_video.and_then(|stream| stream.get(key)?.as_str()) {
            observed.push((format!("ffprobe {key}"), value.to_owned()));
        }

        let verdicts: Vec<Option<bool>> = observed
            .iter()
            .map(|(_, value)| field.conforms(value))
            .collect();
        if verdicts.contains(&Some(false)) {
            let listing: Vec<String> = observed
                .iter()
                .map(|(source, value)| format!("{source} = {value}"))
                .collect();
            refusals.push(format!("{}: {}", field.label(), listing.join(", ")));
        } else if !verdicts.contains(&Some(true)) {
            untagged.push(field);
        }
    }

    let requirements: Vec<&str> = ColourField::ALL
        .iter()
        .map(|field| field.requirement())
        .collect();
    if !refusals.is_empty() {
        return Err(format!(
            "HLG input is tagged with colorimetry a Dolby Vision Profile 8.4 RPU cannot describe ({}; the base layer is copied unchanged). {}.",
            requirements.join(", "),
            refusals.join("; ")
        ));
    }
    if untagged.is_empty() {
        return Ok(Vec::new());
    }
    let labels: Vec<&str> = untagged.iter().map(|field| field.label()).collect();
    let assumed: Vec<&str> = untagged.iter().map(|field| field.requirement()).collect();
    Ok(vec![format!(
        "HLG input does not tag its colour {}; assuming {}, the only colorimetry a Profile 8.4 RPU describes. Check the source if it looks wrong.",
        labels.join(", "),
        assumed.join(", ")
    )])
}

/// Check an HLG input against the Profile 8.4 input contract before any extraction (see
/// [`hlg_colour_contract`]). One MediaInfo and one ffprobe call; a tool that fails contributes
/// nothing, and fields no source tags are accepted with a warning.
pub fn check_hlg_colour_contract(input_file: &str) -> std::result::Result<Vec<String>, String> {
    let mediainfo = get_mediainfo_json(input_file).ok();
    let ffprobe = get_ffprobe_json(input_file).ok();
    hlg_colour_contract(mediainfo.as_ref(), ffprobe.as_ref())
}

fn classify_hdr_hints(hints: &str, measurements: bool) -> Option<HdrFormat> {
    let hints = hints.to_uppercase();

    if hints.contains("SMPTE ST 2094 APP 4") || hints.contains("HDR10+") {
        return Some(HdrFormat::Hdr10Plus);
    }
    if hints_indicate_hlg(&hints) {
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

/// Store a source-stated MaxCLL / MaxFALL. Zero means "unknown" in CTA-861.3, so it is not a
/// stated value and never replaces one.
fn insert_light_level(meta: &mut HashMap<String, f64>, key: &str, value: f64) {
    if value > 0.0 {
        meta.insert(key.to_string(), value);
    }
}

/// Static HDR metadata the source states (MediaInfo, then a Details.txt override for
/// MaxCLL/MaxFALL), without defaults for what is missing.
pub fn read_static_metadata(input_file: &str) -> HashMap<String, f64> {
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
                            insert_light_level(&mut meta, "max_cll", f);
                        } else if let Some(s) = val.as_str() {
                            let re = Regex::new(r"([0-9.]+)").unwrap();
                            if let Some(caps) = re.captures(s) {
                                if let Ok(v) = caps[1].parse::<f64>() {
                                    insert_light_level(&mut meta, "max_cll", v);
                                }
                            }
                        }
                    }

                    // MaxFALL
                    if let Some(val) = track.get("MaxFALL") {
                        if let Some(f) = val.as_f64() {
                            insert_light_level(&mut meta, "max_fall", f);
                        } else if let Some(s) = val.as_str() {
                            let re = Regex::new(r"([0-9.]+)").unwrap();
                            if let Some(caps) = re.captures(s) {
                                if let Ok(v) = caps[1].parse::<f64>() {
                                    insert_light_level(&mut meta, "max_fall", v);
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
                    insert_light_level(&mut meta, "max_cll", v);
                }
            }
            if let Some(caps) = re_fall.captures(&content) {
                let s = caps[1].replace(',', ".");
                if let Ok(v) = s.parse::<f64>() {
                    insert_light_level(&mut meta, "max_fall", v);
                }
            }
        }
    }

    meta
}

/// Warn for each of `keys` that could not be sourced from file metadata, then apply its fallback.
/// These warnings matter: the display will build its tone-mapping from these values.
pub fn apply_static_defaults(meta: &mut HashMap<String, f64>, keys: &[&str], hlg_source: bool) {
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
        if keys.contains(&key) && !meta.contains_key(key) {
            if hlg_source {
                crate::progress::print_info(&format!(
                    "{label} not in source metadata (usual for HLG); using default {default:.4} nits for L6."
                ));
                meta.insert(key.to_string(), default);
                continue;
            }
            eprintln!(
                "WARNING: {} not found in source metadata; using default {:.4} nits for the Dolby Vision L6 block. \
                 Use mediainfo to verify the source has mastering display / light-level metadata.",
                label, default
            );
            meta.insert(key.to_string(), default);
        }
    }
}

/// Fill a MaxCLL / MaxFALL the source does not state: the analyzer's measured value when the
/// sidecar has a usable one, the default otherwise. Source-stated values are never changed.
/// A filled value is adjusted so MaxFALL <= MaxCLL holds against a source-stated counterpart.
/// Returns the info lines to print.
pub fn resolve_light_levels(
    meta: &mut HashMap<String, f64>,
    sidecar: Option<&L1Sidecar>,
    hlg_source: bool,
) -> Vec<String> {
    let source_cll = meta.contains_key("max_cll");
    let source_fall = meta.contains_key("max_fall");
    if source_cll && source_fall {
        return Vec::new();
    }

    let mut messages = Vec::new();
    let measured = match sidecar.map(L1Sidecar::measured_light_levels) {
        Some(Ok(measured)) => measured,
        Some(Err(reason)) => {
            messages.push(format!(
                "No measured MaxCLL/MaxFALL for L6: {reason}. Using defaults."
            ));
            MeasuredLightLevels::default()
        }
        None => MeasuredLightLevels::default(),
    };
    if let Some(note) = measured.note {
        messages.push(note);
    }
    for (key, label, value) in [
        ("max_cll", "MaxCLL", measured.max_cll),
        ("max_fall", "MaxFALL", measured.max_fall),
    ] {
        if meta.contains_key(key) {
            continue;
        }
        if let Some(nits) = value {
            // The top of the Profile 8.4 range (PQ code 3079) is 1000.9 nits and rounds to 1001,
            // one above the 1000-nit mastering peak the RPU declares.
            let nits = if hlg_source { nits.min(1000) } else { nits };
            messages.push(format!(
                "{label} not in source metadata; using the measured {nits} nits for L6."
            ));
            meta.insert(key.to_string(), f64::from(nits));
        }
    }
    apply_static_defaults(meta, &["max_cll", "max_fall"], hlg_source);

    let (cll, fall) = (meta["max_cll"], meta["max_fall"]);
    if fall > cll {
        if source_cll {
            messages.push(format!(
                "MaxFALL lowered to the source MaxCLL ({cll:.0} nits) for L6."
            ));
            meta.insert("max_fall".to_string(), cll);
        } else {
            messages.push(format!("MaxCLL raised to MaxFALL ({fall:.0} nits) for L6."));
            meta.insert("max_cll".to_string(), fall);
        }
    }
    messages
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
/// Versions 1 to 5 are accepted; version 2 adds provenance and a full-resolution crop,
/// version 3 adds `analysis.luminance_mapping` (`"pq"` or a [`Composer::luminance_mapping`] name),
/// version 4 stores unfiltered averages (same layout), and version 5 records the stream's
/// picture count and the undecodable leading pictures before the first measured frame.
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
    /// Content MaxCLL / MaxFALL measured by the analyzer (max-RGB runs of newer version 4
    /// analyzers only).
    #[serde(default)]
    pub light_level: Option<L1SidecarLightLevel>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct L1SidecarLightLevel {
    pub max_cll_nits: u32,
    pub max_fall_nits: u32,
}

/// The sidecar light levels that are safe to put into L6.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MeasuredLightLevels {
    pub max_cll: Option<u32>,
    pub max_fall: Option<u32>,
    /// Why one of the two is withheld.
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct L1SidecarSource {
    pub file_name: String,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    /// Transfer function the analyzer detected, e.g. `"HLG (ARIB STD-B67)"` (version 2+).
    #[serde(default)]
    pub transfer_function: Option<String>,
    /// Coded pictures in the video stream (version 5+).
    #[serde(default)]
    pub stream_frames: Option<u64>,
    /// RASL pictures at the start of an open-GOP cut that no decoder outputs (version 5+).
    /// Measured frame `i` is stream frame `i + leading_skipped_frames`.
    #[serde(default)]
    pub leading_skipped_frames: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct L1SidecarAnalysis {
    pub downscale: u32,
    pub sample_rate: u32,
    pub gpu: bool,
    pub no_crop: bool,
    /// How signal codes became PQ luminance: `"pq"`, or for HLG the
    /// [`Composer::luminance_mapping`] of the Profile 8.4 composer measured through (version 3+).
    #[serde(default)]
    pub luminance_mapping: Option<String>,
}

/// Newest sidecar version this build reads.
pub const L1_SIDECAR_MAX_VERSION: u32 = 5;

/// From this version on the sidecar accounts for every picture of the video stream, including
/// undecodable leading pictures at the start.
pub const L1_SIDECAR_STREAM_FRAMES_VERSION: u32 = 5;

/// From this version on the sidecar averages are unfiltered frame means; older sidecars are
/// reused with an advisory warning.
pub const L1_SIDECAR_UNFILTERED_AVERAGES_VERSION: u32 = 4;

/// Sidecar `analysis.luminance_mapping` of pre-release builds: luma curve only (max-RGB equal to
/// luma), which under-measures saturated highlights, so it is re-analyzed rather than reused.
/// Every current HLG mapping is max-RGB through the full Dolby Vision Profile 8.4 reconstruction
/// (luma curve + chroma MMR + RPU matrix) of one composer, named by
/// [`Composer::luminance_mapping`].
const LUMA_ONLY_DOVI84_MAPPING: &str = "dovi84-v1";

/// Any Dolby Vision 8.4 HLG mapping revision; none of them is valid L1 for a PQ input.
fn is_dovi84_family(mapping: Option<&str>) -> bool {
    mapping.is_some_and(|mapping| mapping.starts_with("dovi84-"))
}

impl L1Sidecar {
    /// The version 3 luminance mapping, when present.
    pub fn luminance_mapping(&self) -> Option<&str> {
        self.analysis
            .as_ref()
            .and_then(|analysis| analysis.luminance_mapping.as_deref())
    }

    /// True when the sidecar says it was measured from an HLG source.
    fn source_is_hlg(&self) -> bool {
        self.source
            .as_ref()
            .and_then(|source| source.transfer_function.as_deref())
            .is_some_and(|transfer| transfer.to_uppercase().contains("HLG"))
    }
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
    /// MaxCLL / MaxFALL usable for L6, or why the sidecar has none. Both need every frame
    /// analyzed (a skipped frame can hide a flash). MaxCLL also needs every pixel: a sampling
    /// stride can miss a small highlight, while a frame average is sound at any stride.
    pub fn measured_light_levels(&self) -> std::result::Result<MeasuredLightLevels, String> {
        let Some(levels) = self.light_level else {
            return Err(
                "the measurements carry no content light levels (older analyzer or luma peak domain; delete them to re-analyze)"
                    .into(),
            );
        };
        let (cll, fall) = (levels.max_cll_nits, levels.max_fall_nits);
        if !(1..=10_000).contains(&cll) || !(1..=10_000).contains(&fall) || fall > cll {
            return Err(format!(
                "the sidecar light levels are invalid (MaxCLL {cll}, MaxFALL {fall})"
            ));
        }
        let Some(analysis) = &self.analysis else {
            return Err("the sidecar does not record its sampling".into());
        };
        if analysis.sample_rate != 1 {
            return Err(format!(
                "the analysis skipped frames (sample-rate {})",
                analysis.sample_rate
            ));
        }
        if analysis.downscale != 1 {
            return Ok(MeasuredLightLevels {
                max_cll: None,
                max_fall: Some(fall),
                note: Some(format!(
                    "MaxCLL is not taken from the measurements: analysis at downscale {} can miss small highlights (--analysis-quality accurate measures it).",
                    analysis.downscale
                )),
            });
        }
        Ok(MeasuredLightLevels {
            max_cll: Some(cll),
            max_fall: Some(fall),
            note: None,
        })
    }

    /// Number of frames the sidecar covers (last scene end + 1).
    pub fn frame_count(&self) -> u64 {
        self.scenes
            .iter()
            .map(|scene| scene.end + 1)
            .max()
            .unwrap_or(0)
    }

    /// Undecodable leading pictures in front of the first measured frame (0 before version 5).
    /// The RPU needs this many extra entries at the start: `dovi_tool inject-rpu` gives RPU `n`
    /// to the picture with presentation number `n`, and these pictures come first.
    pub fn leading_frames(&self) -> u64 {
        self.source
            .as_ref()
            .and_then(|source| source.leading_skipped_frames)
            .unwrap_or(0)
    }

    /// Pictures of the video stream the sidecar describes: the measured frames plus the
    /// leading pictures in front of them. This is the length of the RPU.
    pub fn stream_frame_count(&self) -> u64 {
        self.frame_count() + self.leading_frames()
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
        let leading = self.leading_frames();
        if leading > 0 {
            parts.push(format!(
                "{leading} undecodable leading pictures before them"
            ));
        }
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
    #[error("L1 sidecar {0} (re-analysis required)")]
    LuminanceMappingMismatch(String),
}

/// What the caller knows about the input the sidecar must describe. Leave fields `None` for a
/// sidecar that was just produced (the analyzer run is authoritative), or when the analyzed file
/// is an intermediate rather than the input.
#[derive(Debug, Default)]
pub struct SidecarExpectation {
    pub file_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub frames: Option<u64>,
    /// `Some(composer)` when the input is HLG: the sidecar must be version 3+, measured from an
    /// HLG source through that Dolby Vision 8.4 composer (exact name). When `None`, a Dolby
    /// Vision 8.4 mapped sidecar is rejected.
    pub hlg_composer: Option<Composer>,
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
            hlg_composer: None,
        }
    }

    /// Set the composer an HLG input is converted with, `None` for other inputs (see
    /// [`SidecarExpectation::hlg_composer`]).
    pub fn with_hlg_composer(mut self, hlg_composer: Option<Composer>) -> Self {
        self.hlg_composer = hlg_composer;
        self
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
    if !matches!(sidecar.version, 1..=L1_SIDECAR_MAX_VERSION) {
        return Err(SidecarError::UnsupportedVersion(sidecar.version));
    }
    check_luminance_mapping(sidecar, expect.hlg_composer)?;
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
    if sidecar.version < L1_SIDECAR_UNFILTERED_AVERAGES_VERSION {
        advisories.push(format!(
            "L1 sidecar v{} stores averages that were smoothed over time, so the L1 average of a scene that changes (a fade, a flash) can read wrong. To re-measure, delete the measurements file and run again with an hdr_analyzer_mvp from this version.",
            sidecar.version
        ));
    }
    let covered = sidecar.frame_count();
    if let Some(frames) = &sidecar.frames {
        if frames.min_pq_12bit.len() as u64 != covered {
            return invalid(format!(
                "scenes cover {covered} frames but per-frame data has {}",
                frames.min_pq_12bit.len()
            ));
        }
    }
    let accounted = sidecar.version >= L1_SIDECAR_STREAM_FRAMES_VERSION;
    if accounted {
        let source = sidecar.source.as_ref();
        let (Some(stream), Some(leading)) = (
            source.and_then(|source| source.stream_frames),
            source.and_then(|source| source.leading_skipped_frames),
        ) else {
            return invalid(format!(
                "v{} does not record the stream's picture count",
                sidecar.version
            ));
        };
        if covered + leading != stream {
            return invalid(format!(
                "covers {covered} frames after {leading} leading pictures, but the stream has {stream}"
            ));
        }
    }
    if let Some(expected) = expect.frames {
        let covered = sidecar.stream_frame_count();
        let difference = expected.abs_diff(covered);
        // Before version 5 a sidecar does not say whether the decoder skipped pictures at the
        // start, and a skip shifts every scene against the video. Only an exact match proves
        // that nothing was skipped.
        if !accounted && difference > 0 {
            return invalid(format!(
                "v{} covers {covered} frames but the input video has {expected}, and it predates the record of undecodable leading pictures (sidecar v{L1_SIDECAR_STREAM_FRAMES_VERSION})",
                sidecar.version
            ));
        }
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

/// HLG measurements are only valid when taken through the Dolby Vision 8.4 reconstruction of the
/// composer the RPU will carry (sidecar v3+, exact composer name), and no 8.4 mapping is valid
/// for a PQ input.
fn check_luminance_mapping(
    sidecar: &L1Sidecar,
    hlg_composer: Option<Composer>,
) -> std::result::Result<(), SidecarError> {
    let mapping = sidecar.luminance_mapping();
    if let Some(composer) = hlg_composer {
        let expected = composer.luminance_mapping();
        if mapping == Some(LUMA_ONLY_DOVI84_MAPPING) {
            return Err(SidecarError::LuminanceMappingMismatch(
                "uses the pre-release luma-only Dolby Vision 8.4 mapping (dovi84-v1), which under-measures saturated highlights"
                    .into(),
            ));
        }
        if let Some(legacy) = mapping.and_then(Composer::from_legacy_luminance_mapping) {
            return Err(SidecarError::LuminanceMappingMismatch(format!(
                "was measured with the {} composer's pre-spec 4:2:0 HLG decode ({}), which reshapes chroma per pixel instead of at chroma resolution; this run needs {expected}",
                legacy.cli_name(),
                mapping.unwrap_or("none")
            )));
        }
        let measured_with = mapping.and_then(Composer::from_luminance_mapping);
        if sidecar.version >= 3 && measured_with.is_none() && is_dovi84_family(mapping) {
            return Err(SidecarError::LuminanceMappingMismatch(format!(
                "names a Dolby Vision 8.4 HLG mapping this mkvdovi does not know ({}); it may come from a newer analyzer",
                mapping.unwrap_or("none")
            )));
        }
        if sidecar.version < 3 || measured_with.is_none() {
            return Err(SidecarError::LuminanceMappingMismatch(format!(
                "v{} (mapping {}) was measured without the Dolby Vision 8.4 HLG mapping",
                sidecar.version,
                mapping.unwrap_or("none")
            )));
        }
        if let Some(other) = measured_with.filter(|&other| other != composer) {
            return Err(SidecarError::LuminanceMappingMismatch(format!(
                "was measured through the {} HLG composer ({}), but this run writes --hlg-composer {} ({expected})",
                other.cli_name(),
                other.luminance_mapping(),
                composer.cli_name()
            )));
        }
        if !sidecar.source_is_hlg() {
            return Err(SidecarError::LuminanceMappingMismatch(
                "does not record an HLG source transfer function".into(),
            ));
        }
    } else if is_dovi84_family(mapping) {
        return Err(SidecarError::LuminanceMappingMismatch(
            "was measured through the Dolby Vision 8.4 HLG mapping, but the input is not HLG"
                .into(),
        ));
    }
    Ok(())
}

/// Dolby Vision profile for the generated RPU. HLG input becomes Profile 8.4 (the HLG base layer
/// is kept bit-exact) and needs L1 measured through the 8.4 reconstruction of `hlg_composer`, the
/// composer the RPU will carry; everything else is 8.1.
pub fn dv_profile_for(
    hdr_type: HdrFormat,
    sidecar: Option<&L1Sidecar>,
    hlg_composer: Composer,
) -> Result<&'static str> {
    let mapping = sidecar.and_then(L1Sidecar::luminance_mapping);
    let expected = hlg_composer.luminance_mapping();
    match (hdr_type, sidecar) {
        (HdrFormat::Hlg, None) => {
            anyhow::bail!("HLG input needs measured L1 (an analyzer sidecar) for Profile 8.4")
        }
        (HdrFormat::Hlg, Some(_)) if mapping != Some(expected) => anyhow::bail!(
            "HLG input with --hlg-composer {} needs L1 measured through the Dolby Vision 8.4 mapping {expected}, but the sidecar mapping is {}",
            hlg_composer.cli_name(),
            mapping.unwrap_or("none")
        ),
        (HdrFormat::Hlg, Some(_)) => Ok("8.4"),
        (_, Some(_)) if is_dovi84_family(mapping) => anyhow::bail!(
            "L1 measured through the Dolby Vision 8.4 HLG mapping cannot describe a non-HLG input"
        ),
        _ => Ok("8.1"),
    }
}

/// Profile 8.1 `source_min_pq` / `source_max_pq` (12-bit PQ codes) from the mastering display
/// luminance in nits (`min_dml` / `max_dml`), with the conversion the `dolby_vision` crate applies
/// to a Dolby CM XML. Without explicit values `dovi_tool generate` derives the range from a coarse
/// L6 lookup (mastering min other than <= 0.001 or exactly 0.005 nits becomes 0, a mastering peak
/// other than 1000/2000/4000/10000 nits becomes 3079). `None` when the values are not a plausible
/// mastering display, so the lookup stays in charge: not finite, a peak outside
/// [`MASTERING_PEAK_NITS`] (a mastering SEI written in the wrong units reads as 0.1 or 1 nit), or a
/// minimum outside [`MASTERING_MIN_NITS`] (a minimum stated in nits instead of 0.0001-nit units).
pub fn source_range_pq_81(metadata: &HashMap<String, f64>) -> Option<(u16, u16)> {
    let min_dml = *metadata.get("min_dml")?;
    let max_dml = *metadata.get("max_dml")?;
    let usable = MASTERING_MIN_NITS.contains(&min_dml) && MASTERING_PEAK_NITS.contains(&max_dml);
    if !usable {
        return None;
    }
    let range = (nits_to_pq_12_bit(min_dml), nits_to_pq_12_bit(max_dml));
    (range.0 < range.1).then_some(range)
}

/// Mastering display peaks taken at face value for the 8.1 source range.
pub const MASTERING_PEAK_NITS: std::ops::RangeInclusive<f64> = 100.0..=10_000.0;
/// Mastering display minima taken at face value for the 8.1 source range.
pub const MASTERING_MIN_NITS: std::ops::RangeInclusive<f64> = 0.0..=1.0;

/// Source range of every Profile 8.4 RPU, whatever the composer: the `Profile84` preset's
/// 62..3079 (about 0.005..1000 nits), which the HLG measurement clamps to.
pub fn source_range_pq_84() -> (u16, u16) {
    let dm = Profile84::dm_data();
    (dm.source_min_pq, dm.source_max_pq)
}

/// Source range the generated RPU of `profile` must carry, `None` when mkvdovi leaves it to
/// `dovi_tool` (a Profile 8.1 mastering range [`source_range_pq_81`] cannot use).
pub fn expected_source_range(profile: &str, metadata: &HashMap<String, f64>) -> Option<(u16, u16)> {
    match profile {
        "8.4" => Some(source_range_pq_84()),
        "8.1" => source_range_pq_81(metadata),
        _ => None,
    }
}

pub fn generate_extra_json(
    output_path: &Path,
    profile: &str,
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
        "profile": profile,
        "level6": {
            "max_display_mastering_luminance": max_dml as u32,
            "min_display_mastering_luminance": (min_dml * 10000.0) as u32,
            "max_content_light_level": max_cll as u32,
            "max_frame_average_light_level": max_fall as u32,
        }
    });

    // Profile 8.4 keeps the preset's range: both HLG composers and the HLG measurement assume it.
    if profile == "8.1" {
        if let Some((source_min_pq, source_max_pq)) = source_range_pq_81(metadata) {
            json_content["source_min_pq"] = json!(source_min_pq);
            json_content["source_max_pq"] = json!(source_max_pq);
        }
    }

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
        // Undecodable leading pictures come first in presentation order, so every scene moves
        // back by their count and the first scene also covers them (they are never displayed).
        let leading = sidecar.leading_frames();
        let length = sidecar.stream_frame_count();
        let shots: Vec<Value> = sidecar
            .scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| {
                let start = if index == 0 { 0 } else { scene.start + leading };
                json!({
                    "start": start,
                    "duration": scene.end + leading - start + 1,
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
            "8.1",
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

    fn mastering(min_dml: f64, max_dml: f64) -> HashMap<String, f64> {
        HashMap::from([
            ("min_dml".to_string(), min_dml),
            ("max_dml".to_string(), max_dml),
            ("max_cll".to_string(), 1000.0),
            ("max_fall".to_string(), 400.0),
        ])
    }

    #[test]
    fn source_range_matches_the_generator_lookup_on_standard_masters() {
        use dolby_vision::rpu::extension_metadata::blocks::ExtMetadataBlockLevel6;

        // The values dovi_tool derives from L6 (as written by generate_extra_json) when no
        // explicit range is given; explicit values must not move them.
        for (min_dml, max_dml) in [
            (0.0001, 1000.0),
            (0.005, 1000.0),
            (0.0001, 2000.0),
            (0.005, 4000.0),
            (0.0001, 4000.0),
            (0.005, 10000.0),
        ] {
            let level6 = ExtMetadataBlockLevel6 {
                max_display_mastering_luminance: max_dml as u16,
                min_display_mastering_luminance: (min_dml * 10000.0) as u16,
                ..Default::default()
            };
            assert_eq!(
                source_range_pq_81(&mastering(min_dml, max_dml)),
                Some(level6.source_meta_from_l6()),
                "mastering {min_dml}/{max_dml} nits"
            );
        }
    }

    #[test]
    fn source_range_follows_non_standard_masters() {
        // The L6 lookup would give 7/3079, 0/3079 and 7/3079.
        assert_eq!(
            source_range_pq_81(&mastering(0.001, 1000.0)),
            Some((26, 3079))
        );
        assert_eq!(
            source_range_pq_81(&mastering(0.05, 1000.0)),
            Some((189, 3079))
        );
        assert_eq!(
            source_range_pq_81(&mastering(0.0001, 600.0)),
            Some((7, 2851))
        );
        assert_eq!(
            source_range_pq_81(&mastering(0.0001, 1100.0)),
            Some((7, 3121))
        );
    }

    #[test]
    fn source_range_rejects_unusable_mastering_values() {
        for (min_dml, max_dml) in [
            (0.0001, 20000.0),
            (-1.0, 1000.0),
            (1000.0, 1000.0),
            (2000.0, 1000.0),
            (0.0, 0.0),
            (f64::NAN, 1000.0),
            (0.0001, f64::INFINITY),
            // Mastering SEIs written in the wrong units (x265 `L(1000,50)`, `L(10000,1)`) and a
            // minimum stated in nits.
            (0.005, 0.1),
            (0.0001, 1.0),
            (5.0, 1000.0),
        ] {
            assert_eq!(
                source_range_pq_81(&mastering(min_dml, max_dml)),
                None,
                "mastering {min_dml}/{max_dml} nits"
            );
        }
        assert_eq!(source_range_pq_81(&HashMap::new()), None);
    }

    #[test]
    fn expected_source_range_is_the_preset_for_profile_84() {
        assert_eq!(source_range_pq_84(), (62, 3079));
        // The mastering metadata never changes an 8.4 range.
        assert_eq!(
            expected_source_range("8.4", &mastering(0.05, 600.0)),
            Some((62, 3079))
        );
        assert_eq!(
            expected_source_range("8.1", &mastering(0.005, 1000.0)),
            Some((62, 3079))
        );
        assert_eq!(
            expected_source_range("8.1", &mastering(0.0001, 20000.0)),
            None
        );
    }

    #[test]
    fn extra_json_carries_the_source_range_for_profile_81_only() {
        let write = |profile: &str, meta: &HashMap<String, f64>| -> Value {
            let output = tempfile::NamedTempFile::new().unwrap();
            generate_extra_json(output.path(), profile, meta, &[], None, None, None).unwrap();
            serde_json::from_reader(File::open(output.path()).unwrap()).unwrap()
        };

        let json = write("8.1", &mastering(0.05, 600.0));
        let expected = source_range_pq_81(&mastering(0.05, 600.0)).unwrap();
        assert_eq!(json["source_min_pq"], expected.0);
        assert_eq!(json["source_max_pq"], expected.1);

        let json = write("8.4", &mastering(0.005, 1000.0));
        assert!(json.get("source_min_pq").is_none());
        assert!(json.get("source_max_pq").is_none());

        // An unusable mastering range leaves the range to dovi_tool.
        let json = write("8.1", &mastering(0.0001, 20000.0));
        assert!(json.get("source_min_pq").is_none());
    }

    #[test]
    fn extra_json_source_range_reaches_the_generated_rpu() {
        use dolby_vision::rpu::generate::{GenerateConfig, VideoShot};
        use dolby_vision::rpu::utils::parse_rpu_file;

        // dovi_tool deserializes these keys into `GenerateConfig`; the crate's serde feature is
        // not enabled here, so the two fields are carried over by hand.
        let dir = tempfile::tempdir().unwrap();
        let extra = dir.path().join("extra.json");
        let meta = mastering(0.05, 600.0);
        generate_extra_json(&extra, "8.1", &meta, &[], None, None, None).unwrap();
        let json: Value = serde_json::from_reader(File::open(&extra).unwrap()).unwrap();
        let code = |key: &str| json[key].as_u64().map(|v| u16::try_from(v).unwrap());
        let config = GenerateConfig {
            length: 2,
            shots: vec![VideoShot {
                start: 0,
                duration: 2,
                ..Default::default()
            }],
            source_min_pq: code("source_min_pq"),
            source_max_pq: code("source_max_pq"),
            ..Default::default()
        };
        let rpu_path = dir.path().join("RPU.bin");
        config.write_rpus(&rpu_path).unwrap();

        let rpus = parse_rpu_file(&rpu_path).unwrap();
        let dm = rpus[0].vdr_dm_data.as_ref().unwrap();
        assert_eq!(
            Some((dm.source_min_pq, dm.source_max_pq)),
            source_range_pq_81(&meta)
        );
    }

    #[test]
    fn json_generation_rejects_missing_static_metadata() {
        let output = tempfile::NamedTempFile::new().unwrap();
        let metadata = HashMap::new();

        let error = generate_extra_json(output.path(), "8.1", &metadata, &[], None, None, None)
            .unwrap_err();

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

        generate_extra_json(
            output.path(),
            "8.1",
            &metadata,
            &[],
            None,
            None,
            Some(&sidecar),
        )
        .unwrap();

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
    fn undecodable_leading_pictures_shift_the_shots_and_lengthen_the_rpu() {
        let output = tempfile::NamedTempFile::new().unwrap();
        let metadata = HashMap::from([
            ("min_dml".to_string(), 0.0001),
            ("max_dml".to_string(), 1000.0),
            ("max_cll".to_string(), 997.0),
            ("max_fall".to_string(), 91.0),
        ]);
        // Two RASL pictures before the first measured frame: measured frame 10 is stream frame 12.
        let sidecar = L1Sidecar {
            version: L1_SIDECAR_MAX_VERSION,
            scenes: vec![scene(0, 9, 1, 500, 2400), scene(10, 19, 2, 600, 2500)],
            source: accounted_source(22, 2),
            ..Default::default()
        };
        assert!(validate_l1_sidecar(&sidecar, &SidecarExpectation::default()).is_ok());
        assert_eq!(sidecar.stream_frame_count(), 22);

        generate_extra_json(
            output.path(),
            "8.1",
            &metadata,
            &[],
            None,
            None,
            Some(&sidecar),
        )
        .unwrap();

        let json: Value = serde_json::from_reader(File::open(output.path()).unwrap()).unwrap();
        assert_eq!(json["length"], 22);
        let shots = json["shots"].as_array().unwrap();
        assert_eq!(
            (&shots[0]["start"], &shots[0]["duration"]),
            (&json!(0), &json!(12))
        );
        assert_eq!(
            (&shots[1]["start"], &shots[1]["duration"]),
            (&json!(12), &json!(10))
        );
        assert_eq!(shots[1]["metadata_blocks"][0]["Level1"]["max_pq"], 2500);
    }

    #[test]
    fn a_v5_sidecar_must_account_for_every_stream_picture() {
        let mut missing = current_sidecar_json();
        missing["source"]
            .as_object_mut()
            .unwrap()
            .remove("stream_frames");
        let mut short = current_sidecar_json();
        short["source"]["stream_frames"] = json!(22);
        for (json, reason) in [(missing, "picture count"), (short, "the stream has 22")] {
            let sidecar: L1Sidecar = serde_json::from_value(json).unwrap();
            assert!(matches!(
                validate_l1_sidecar(&sidecar, &SidecarExpectation::default()),
                Err(SidecarError::Invalid(message)) if message.contains(reason)
            ));
        }
    }

    #[test]
    fn a_sidecar_before_v5_is_reused_only_on_an_exact_frame_count() {
        let mut json = v2_sidecar_json();
        json["version"] = json!(4);
        let sidecar: L1Sidecar = serde_json::from_value(json).unwrap();
        let expect = |frames| SidecarExpectation {
            frames: Some(frames),
            ..Default::default()
        };
        assert!(validate_l1_sidecar(&sidecar, &expect(20)).is_ok());
        // Two frames short is inside the MediaInfo tolerance, but it is exactly what two
        // skipped leading pictures look like.
        assert!(matches!(
            validate_l1_sidecar(&sidecar, &expect(22)),
            Err(SidecarError::Invalid(message)) if message.contains("undecodable leading pictures")
        ));
    }

    #[test]
    fn a_v5_sidecar_is_compared_with_the_input_by_stream_pictures() {
        let mut json = current_sidecar_json();
        json["source"]["stream_frames"] = json!(22);
        json["source"]["leading_skipped_frames"] = json!(2);
        let sidecar: L1Sidecar = serde_json::from_value(json).unwrap();
        let expect = SidecarExpectation {
            frames: Some(22),
            ..Default::default()
        };
        assert!(validate_l1_sidecar(&sidecar, &expect).unwrap().is_empty());
        assert!(sidecar
            .provenance_summary()
            .contains("2 undecodable leading pictures"));
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

    /// The version 2 fixture stamped with the newest version, for tests about other properties.
    fn current_sidecar_json() -> Value {
        let mut sidecar = v2_sidecar_json();
        sidecar["version"] = json!(L1_SIDECAR_MAX_VERSION);
        sidecar["source"]["stream_frames"] = json!(20);
        sidecar["source"]["leading_skipped_frames"] = json!(0);
        sidecar
    }

    /// A version 5 source record for `stream` pictures with `leading` undecodable ones.
    fn accounted_source(stream: u64, leading: u64) -> Option<L1SidecarSource> {
        Some(L1SidecarSource {
            file_name: "input.mkv".into(),
            size_bytes: 42,
            width: 3840,
            height: 2160,
            transfer_function: None,
            stream_frames: Some(stream),
            leading_skipped_frames: Some(leading),
        })
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
    fn current_sidecar_matching_the_input_validates_without_advisories() {
        let sidecar: L1Sidecar = serde_json::from_value(current_sidecar_json()).unwrap();
        let expect = SidecarExpectation {
            file_name: Some("input.mkv".into()),
            size_bytes: Some(42),
            frames: Some(20),
            hlg_composer: None,
        };
        assert!(validate_l1_sidecar(&sidecar, &expect).unwrap().is_empty());
        assert!(sidecar
            .provenance_summary()
            .contains("crop 3840x1600+0+280"));
    }

    #[test]
    fn sidecar_with_smoothed_averages_is_reused_with_an_advisory() {
        for version in [1, 2, 3] {
            let mut json = v2_sidecar_json();
            json["version"] = json!(version);
            let sidecar: L1Sidecar = serde_json::from_value(json).unwrap();
            let advisories = validate_l1_sidecar(&sidecar, &SidecarExpectation::default()).unwrap();
            assert_eq!(advisories.len(), 1, "version {version}");
            assert!(advisories[0].contains("smoothed over time"));
        }
    }

    #[test]
    fn sidecar_for_a_different_input_is_rejected() {
        let sidecar: L1Sidecar = serde_json::from_value(v2_sidecar_json()).unwrap();
        let expect = SidecarExpectation {
            file_name: Some("input.mkv".into()),
            size_bytes: Some(43),
            frames: None,
            hlg_composer: None,
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
            version: L1_SIDECAR_MAX_VERSION + 1,
            scenes: vec![scene(0, 1, 0, 1, 2)],
            ..Default::default()
        };
        assert!(matches!(
            validate_l1_sidecar(&future, &SidecarExpectation::default()),
            Err(SidecarError::UnsupportedVersion(version)) if version == L1_SIDECAR_MAX_VERSION + 1
        ));
    }

    #[test]
    fn avg_above_max_is_an_advisory_not_an_error() {
        let sidecar = L1Sidecar {
            version: L1_SIDECAR_MAX_VERSION,
            scenes: vec![scene(0, 9, 1, 3000, 2900), scene(10, 19, 1, 5, 40)],
            source: accounted_source(20, 0),
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
            version: L1_SIDECAR_MAX_VERSION,
            scenes,
            source: accounted_source(80, 0),
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
        let mut sidecar = current_sidecar_json();
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
        let sidecar: L1Sidecar = serde_json::from_value(current_sidecar_json()).unwrap();
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

    fn v1_sidecar_json() -> Value {
        json!({
            "version": 1,
            "scenes": [{"start": 0, "end": 4, "min_pq_12bit": 0, "avg_luma_pq_12bit": 10,
                        "avg_max_rgb_pq_12bit": 12, "max_pq_12bit": 100}]
        })
    }

    /// A version 3 sidecar as the analyzer writes it for an HLG source.
    fn v3_hlg_sidecar_json() -> Value {
        let mut sidecar = v2_sidecar_json();
        sidecar["version"] = json!(3);
        sidecar["source"]["transfer_function"] = json!("HLG (ARIB STD-B67)");
        sidecar["analysis"]["luminance_mapping"] = json!(Composer::Preset.luminance_mapping());
        sidecar
    }

    fn hlg_expectation() -> SidecarExpectation {
        SidecarExpectation::default().with_hlg_composer(Some(Composer::Preset))
    }

    #[test]
    fn v1_and_v2_sidecars_validate_for_pq_but_not_for_hlg() {
        for fixture in [v1_sidecar_json(), v2_sidecar_json()] {
            let sidecar: L1Sidecar = serde_json::from_value(fixture).unwrap();
            assert!(sidecar.luminance_mapping().is_none());
            assert!(validate_l1_sidecar(&sidecar, &SidecarExpectation::default()).is_ok());
            assert!(matches!(
                validate_l1_sidecar(&sidecar, &hlg_expectation()),
                Err(SidecarError::LuminanceMappingMismatch(_))
            ));
        }
    }

    #[test]
    fn v3_dovi84_sidecar_is_accepted_only_for_hlg() {
        let mut fixture = v3_hlg_sidecar_json();
        fixture["analysis"]["luminance_mapping"] = json!(Composer::Preset.luminance_mapping());
        let sidecar: L1Sidecar = serde_json::from_value(fixture).unwrap();
        assert_eq!(
            sidecar.luminance_mapping(),
            Some(Composer::Preset.luminance_mapping())
        );
        assert!(validate_l1_sidecar(&sidecar, &hlg_expectation()).is_ok());
        assert_eq!(
            dv_profile_for(HdrFormat::Hlg, Some(&sidecar), Composer::Preset).unwrap(),
            "8.4"
        );
        let error = validate_l1_sidecar(&sidecar, &SidecarExpectation::default()).unwrap_err();
        assert!(matches!(error, SidecarError::LuminanceMappingMismatch(_)));
        assert!(error.to_string().contains("re-analysis required"));
        assert!(dv_profile_for(
            HdrFormat::Hdr10Unsupported,
            Some(&sidecar),
            Composer::Preset
        )
        .is_err());
    }

    #[test]
    fn luma_only_dovi84_v1_sidecar_is_reanalyzed_for_hlg_and_rejected_for_pq() {
        let mut fixture = v3_hlg_sidecar_json();
        fixture["analysis"]["luminance_mapping"] = json!("dovi84-v1");
        let sidecar: L1Sidecar = serde_json::from_value(fixture).unwrap();
        let error = validate_l1_sidecar(&sidecar, &hlg_expectation()).unwrap_err();
        assert!(matches!(error, SidecarError::LuminanceMappingMismatch(_)));
        assert!(error.to_string().contains("luma-only"));
        assert!(error.to_string().contains("re-analysis required"));
        assert!(dv_profile_for(HdrFormat::Hlg, Some(&sidecar), Composer::Preset).is_err());
        assert!(matches!(
            validate_l1_sidecar(&sidecar, &SidecarExpectation::default()),
            Err(SidecarError::LuminanceMappingMismatch(_))
        ));
        assert!(dv_profile_for(
            HdrFormat::Hdr10Unsupported,
            Some(&sidecar),
            Composer::Preset
        )
        .is_err());
    }

    #[test]
    fn legacy_dovi84_mappings_are_re_analyzed() {
        for (legacy, measured_with) in dovi84_composer::LEGACY_LUMINANCE_MAPPINGS {
            let mut fixture = v3_hlg_sidecar_json();
            fixture["analysis"]["luminance_mapping"] = json!(legacy);
            let sidecar: L1Sidecar = serde_json::from_value(fixture).unwrap();
            // Under either composer: the legacy name, not the composer, decides the message.
            for composer in Composer::ALL {
                let expect = SidecarExpectation::default().with_hlg_composer(Some(composer));
                let error = validate_l1_sidecar(&sidecar, &expect).unwrap_err();
                let message = error.to_string();
                assert!(matches!(error, SidecarError::LuminanceMappingMismatch(_)));
                assert!(message.contains("pre-spec 4:2:0"), "{message}");
                assert!(message.contains(measured_with.cli_name()), "{message}");
                assert!(message.contains(composer.luminance_mapping()), "{message}");
                assert!(message.contains("re-analysis required"), "{message}");
                assert!(dv_profile_for(HdrFormat::Hlg, Some(&sidecar), composer).is_err());
            }
            // Still an 8.4 mapping: never valid for PQ.
            assert!(matches!(
                validate_l1_sidecar(&sidecar, &SidecarExpectation::default()),
                Err(SidecarError::LuminanceMappingMismatch(_))
            ));
        }
    }

    #[test]
    fn unknown_dovi84_revision_is_rejected_for_hlg() {
        let mut fixture = v3_hlg_sidecar_json();
        fixture["analysis"]["luminance_mapping"] = json!("dovi84-v9");
        let sidecar: L1Sidecar = serde_json::from_value(fixture).unwrap();
        assert!(matches!(
            validate_l1_sidecar(&sidecar, &hlg_expectation()),
            Err(SidecarError::LuminanceMappingMismatch(_))
        ));
        assert!(dv_profile_for(HdrFormat::Hlg, Some(&sidecar), Composer::Preset).is_err());
    }

    #[test]
    fn hlg_sidecar_must_name_the_selected_composer_exactly() {
        let preset: L1Sidecar = serde_json::from_value(v3_hlg_sidecar_json()).unwrap();
        let mut fixture = v3_hlg_sidecar_json();
        fixture["analysis"]["luminance_mapping"] = json!(Composer::Bt2100V1.luminance_mapping());
        let bt2100: L1Sidecar = serde_json::from_value(fixture).unwrap();
        let expect = |composer| SidecarExpectation::default().with_hlg_composer(Some(composer));

        assert!(validate_l1_sidecar(&bt2100, &expect(Composer::Bt2100V1)).is_ok());
        assert_eq!(
            dv_profile_for(HdrFormat::Hlg, Some(&bt2100), Composer::Bt2100V1).unwrap(),
            "8.4"
        );

        // A preset sidecar under bt2100 and the other way round: re-analysis, no fallback.
        for (sidecar, composer, measured) in [
            (
                &preset,
                Composer::Bt2100V1,
                "preset HLG composer (dovi84-v3)",
            ),
            (
                &bt2100,
                Composer::Preset,
                "bt2100 HLG composer (dovi84-bt2100-v1-spec420)",
            ),
        ] {
            let error = validate_l1_sidecar(sidecar, &expect(composer)).unwrap_err();
            assert!(matches!(error, SidecarError::LuminanceMappingMismatch(_)));
            let message = error.to_string();
            assert!(message.contains(measured), "{message}");
            assert!(
                message.contains(&format!("--hlg-composer {}", composer.cli_name())),
                "{message}"
            );
            assert!(message.contains("re-analysis required"), "{message}");
            assert!(dv_profile_for(HdrFormat::Hlg, Some(sidecar), composer).is_err());
        }

        // A bt2100 sidecar is still a Dolby Vision 8.4 mapping: never valid for PQ.
        assert!(matches!(
            validate_l1_sidecar(&bt2100, &SidecarExpectation::default()),
            Err(SidecarError::LuminanceMappingMismatch(_))
        ));
        assert!(
            dv_profile_for(HdrFormat::Hdr10Unsupported, Some(&bt2100), Composer::Preset).is_err()
        );

        // A name that only shares the prefix is not a composer.
        let mut fixture = v3_hlg_sidecar_json();
        fixture["analysis"]["luminance_mapping"] = json!("dovi84-bt2100-v2");
        let unknown: L1Sidecar = serde_json::from_value(fixture).unwrap();
        for composer in Composer::ALL {
            assert!(matches!(
                validate_l1_sidecar(&unknown, &expect(composer)),
                Err(SidecarError::LuminanceMappingMismatch(reason)) if reason.contains("does not know (dovi84-bt2100-v2)")
            ));
            assert!(dv_profile_for(HdrFormat::Hlg, Some(&unknown), composer).is_err());
        }
    }

    #[test]
    fn v3_pq_sidecar_is_accepted_for_pq_and_rejected_for_hlg() {
        let mut fixture = v2_sidecar_json();
        fixture["version"] = json!(3);
        fixture["analysis"]["luminance_mapping"] = json!("pq");
        let sidecar: L1Sidecar = serde_json::from_value(fixture).unwrap();
        assert!(validate_l1_sidecar(&sidecar, &SidecarExpectation::default()).is_ok());
        assert!(matches!(
            validate_l1_sidecar(&sidecar, &hlg_expectation()),
            Err(SidecarError::LuminanceMappingMismatch(_))
        ));
    }

    #[test]
    fn hlg_sidecar_must_record_an_hlg_source() {
        let mut fixture = v3_hlg_sidecar_json();
        fixture["source"]["transfer_function"] = json!("PQ (SMPTE 2084)");
        let sidecar: L1Sidecar = serde_json::from_value(fixture).unwrap();
        assert!(matches!(
            validate_l1_sidecar(&sidecar, &hlg_expectation()),
            Err(SidecarError::LuminanceMappingMismatch(reason)) if reason.contains("HLG source")
        ));
    }

    #[test]
    fn dv_profile_follows_the_source_transfer() {
        let hlg: L1Sidecar = serde_json::from_value(v3_hlg_sidecar_json()).unwrap();
        let pq: L1Sidecar = serde_json::from_value(v2_sidecar_json()).unwrap();

        assert_eq!(
            dv_profile_for(HdrFormat::Hlg, Some(&hlg), Composer::Preset).unwrap(),
            "8.4"
        );
        assert!(dv_profile_for(HdrFormat::Hlg, Some(&pq), Composer::Preset).is_err());
        assert!(dv_profile_for(HdrFormat::Hlg, None, Composer::Preset).is_err());

        for format in [
            HdrFormat::Hdr10Plus,
            HdrFormat::Hdr10WithMeasurements,
            HdrFormat::Hdr10Unsupported,
        ] {
            assert_eq!(
                dv_profile_for(format, None, Composer::Preset).unwrap(),
                "8.1"
            );
            assert_eq!(
                dv_profile_for(format, Some(&pq), Composer::Preset).unwrap(),
                "8.1"
            );
            assert!(dv_profile_for(format, Some(&hlg), Composer::Preset).is_err());
        }
    }

    #[test]
    fn extra_json_carries_the_requested_profile() {
        let output = tempfile::NamedTempFile::new().unwrap();
        let metadata = HashMap::from([
            ("min_dml".to_string(), 0.005),
            ("max_dml".to_string(), 1000.0),
            ("max_cll".to_string(), 1000.0),
            ("max_fall".to_string(), 400.0),
        ]);
        let sidecar: L1Sidecar = serde_json::from_value(v3_hlg_sidecar_json()).unwrap();

        generate_extra_json(
            output.path(),
            "8.4",
            &metadata,
            &[],
            None,
            None,
            Some(&sidecar),
        )
        .unwrap();

        let json: Value = serde_json::from_reader(File::open(output.path()).unwrap()).unwrap();
        assert_eq!(json["profile"], "8.4");
    }

    fn light_level_sidecar(cll: u32, fall: u32, downscale: u32, sample_rate: u32) -> L1Sidecar {
        L1Sidecar {
            version: L1_SIDECAR_MAX_VERSION,
            analysis: Some(L1SidecarAnalysis {
                downscale,
                sample_rate,
                gpu: false,
                no_crop: false,
                luminance_mapping: Some("pq".into()),
            }),
            light_level: Some(L1SidecarLightLevel {
                max_cll_nits: cll,
                max_fall_nits: fall,
            }),
            ..Default::default()
        }
    }

    fn light_levels(meta: &HashMap<String, f64>) -> (f64, f64) {
        (meta["max_cll"], meta["max_fall"])
    }

    #[test]
    fn measured_light_levels_fill_what_the_source_does_not_state() {
        let sidecar = light_level_sidecar(743, 96, 1, 1);

        let mut silent = HashMap::new();
        let messages = resolve_light_levels(&mut silent, Some(&sidecar), true);
        assert_eq!(light_levels(&silent), (743.0, 96.0));
        assert_eq!(messages.len(), 2);

        // Source-stated values win, field by field.
        let mut stated = HashMap::from([
            ("max_cll".to_string(), 997.0),
            ("max_fall".to_string(), 91.0),
        ]);
        assert!(resolve_light_levels(&mut stated, Some(&sidecar), false).is_empty());
        assert_eq!(light_levels(&stated), (997.0, 91.0));

        let mut partial = HashMap::from([("max_cll".to_string(), 997.0)]);
        resolve_light_levels(&mut partial, Some(&sidecar), false);
        assert_eq!(light_levels(&partial), (997.0, 96.0));
    }

    #[test]
    fn hlg_max_cll_is_capped_at_the_declared_mastering_peak() {
        let sidecar = light_level_sidecar(1001, 39, 1, 1);

        let mut hlg = HashMap::new();
        resolve_light_levels(&mut hlg, Some(&sidecar), true);
        assert_eq!(light_levels(&hlg), (1000.0, 39.0));

        let mut pq = HashMap::new();
        resolve_light_levels(&mut pq, Some(&sidecar), false);
        assert_eq!(light_levels(&pq), (1001.0, 39.0));
    }

    #[test]
    fn filled_light_levels_never_contradict_a_source_value() {
        let sidecar = light_level_sidecar(743, 96, 1, 1);

        // Source MaxCLL below the measured MaxFALL: the filled MaxFALL is lowered.
        let mut low_cll = HashMap::from([("max_cll".to_string(), 50.0)]);
        resolve_light_levels(&mut low_cll, Some(&sidecar), false);
        assert_eq!(light_levels(&low_cll), (50.0, 50.0));

        // Source MaxFALL above the measured MaxCLL: the filled MaxCLL is raised.
        let mut high_fall = HashMap::from([("max_fall".to_string(), 900.0)]);
        resolve_light_levels(&mut high_fall, Some(&sidecar), false);
        assert_eq!(light_levels(&high_fall), (900.0, 900.0));
    }

    #[test]
    fn unusable_sidecar_light_levels_fall_back_to_defaults() {
        let defaults = (1000.0, 400.0);
        let no_block = L1Sidecar {
            light_level: None,
            ..light_level_sidecar(1, 1, 1, 1)
        };
        for sidecar in [
            no_block,
            light_level_sidecar(743, 96, 1, 3), // frames skipped
            light_level_sidecar(96, 743, 1, 1), // MaxFALL above MaxCLL
            light_level_sidecar(0, 0, 1, 1),    // unknown
            light_level_sidecar(20_000, 96, 1, 1), // beyond PQ
        ] {
            assert!(sidecar.measured_light_levels().is_err());
            let mut meta = HashMap::new();
            let messages = resolve_light_levels(&mut meta, Some(&sidecar), true);
            assert_eq!(light_levels(&meta), defaults);
            assert!(messages[0].contains("Using defaults"), "{messages:?}");
        }

        // No sidecar (HDR10+, Dolby Vision input): defaults, no extra message.
        let mut meta = HashMap::new();
        assert!(resolve_light_levels(&mut meta, None, true).is_empty());
        assert_eq!(light_levels(&meta), defaults);
    }

    #[test]
    fn subsampled_analysis_supplies_max_fall_but_not_max_cll() {
        let sidecar = light_level_sidecar(743, 96, 2, 1);
        let measured = sidecar.measured_light_levels().unwrap();
        assert_eq!((measured.max_cll, measured.max_fall), (None, Some(96)));

        let mut meta = HashMap::new();
        resolve_light_levels(&mut meta, Some(&sidecar), true);
        assert_eq!(light_levels(&meta), (1000.0, 96.0));
    }

    #[test]
    fn zero_light_levels_are_not_source_values() {
        let mut meta = HashMap::from([("max_cll".to_string(), 800.0)]);
        insert_light_level(&mut meta, "max_cll", 0.0);
        insert_light_level(&mut meta, "max_fall", 0.0);
        assert_eq!(meta.get("max_cll"), Some(&800.0));
        assert!(!meta.contains_key("max_fall"));
    }

    #[test]
    fn sidecar_light_level_block_is_optional() {
        let mut fixture = current_sidecar_json();
        let without: L1Sidecar = serde_json::from_value(fixture.clone()).unwrap();
        assert!(without.light_level.is_none());

        fixture["light_level"] = json!({"max_cll_nits": 743, "max_fall_nits": 96});
        let with: L1Sidecar = serde_json::from_value(fixture).unwrap();
        assert_eq!(with.light_level.unwrap().max_cll_nits, 743);
    }

    fn mediainfo_video(fields: Value) -> Value {
        let mut track = json!({"@type": "Video"});
        track.as_object_mut().unwrap().extend(
            fields
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        json!({"media": {"track": [{"@type": "General"}, track]}})
    }

    fn ffprobe_video(fields: Value) -> Value {
        let mut stream = json!({"codec_type": "video"});
        stream.as_object_mut().unwrap().extend(
            fields
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        json!({"streams": [{"codec_type": "audio", "color_range": "pc"}, stream]})
    }

    #[test]
    fn colour_values_are_classified_in_mediainfo_and_ffprobe_spellings() {
        use ColourField::{Matrix, Primaries, Range};
        for (field, value, expected) in [
            (Range, "Limited", Some(true)),
            (Range, "Full", Some(false)),
            (Range, "tv", Some(true)),
            (Range, "pc", Some(false)),
            (Range, "unknown", None),
            (Matrix, "BT.2020 non-constant", Some(true)),
            (Matrix, "BT.2020 constant", Some(false)),
            (Matrix, "BT.709", Some(false)),
            (Matrix, "bt2020nc", Some(true)),
            (Matrix, "bt2020c", Some(false)),
            (Matrix, "bt709", Some(false)),
            (Matrix, "unknown", None),
            (Primaries, "BT.2020", Some(true)),
            (Primaries, "BT.709", Some(false)),
            (Primaries, "Display P3", Some(false)),
            (Primaries, "bt2020", Some(true)),
            (Primaries, "bt709", Some(false)),
            (Primaries, "unknown", None),
            (Primaries, "", None),
        ] {
            assert_eq!(field.conforms(value), expected, "{field:?} {value}");
        }
    }

    #[test]
    fn conforming_hlg_input_passes_without_warning() {
        let mediainfo = mediainfo_video(json!({
            "colour_range": "Limited",
            "matrix_coefficients": "BT.2020 non-constant",
            "colour_primaries": "BT.2020",
        }));
        let ffprobe = ffprobe_video(json!({
            "color_range": "tv",
            "color_space": "bt2020nc",
            "color_primaries": "bt2020",
        }));
        assert_eq!(
            hlg_colour_contract(Some(&mediainfo), Some(&ffprobe)),
            Ok(Vec::new())
        );
        assert_eq!(hlg_colour_contract(Some(&mediainfo), None), Ok(Vec::new()));
        assert_eq!(hlg_colour_contract(None, Some(&ffprobe)), Ok(Vec::new()));
    }

    #[test]
    fn full_range_constant_luminance_or_bt709_hlg_is_refused() {
        for (mediainfo, ffprobe, expected) in [
            (
                json!({"colour_range": "Full"}),
                json!({"color_range": "pc"}),
                "range: MediaInfo colour_range = Full, ffprobe color_range = pc",
            ),
            (
                json!({"matrix_coefficients": "BT.2020 constant"}),
                json!({"color_space": "bt2020c"}),
                "matrix: MediaInfo matrix_coefficients = BT.2020 constant, ffprobe color_space = bt2020c",
            ),
            (
                json!({"matrix_coefficients": "BT.709"}),
                json!({"color_space": "bt709"}),
                "matrix: MediaInfo matrix_coefficients = BT.709, ffprobe color_space = bt709",
            ),
            (
                json!({"colour_primaries": "Display P3"}),
                json!({}),
                "primaries: MediaInfo colour_primaries = Display P3",
            ),
            (
                json!({}),
                json!({"color_primaries": "bt709"}),
                "primaries: ffprobe color_primaries = bt709",
            ),
        ] {
            let error = hlg_colour_contract(
                Some(&mediainfo_video(mediainfo)),
                Some(&ffprobe_video(ffprobe)),
            )
            .unwrap_err();
            assert!(error.contains(expected), "{error}");
            assert!(error.contains("Profile 8.4"), "{error}");
        }
    }

    #[test]
    fn conflicting_hlg_colour_tags_are_refused_with_every_source() {
        // Container says full range / BT.709 matrix, the stream says limited / BT.2020 NCL
        // (MediaInfo `_Original`), and ffprobe follows the container.
        let mediainfo = mediainfo_video(json!({
            "colour_range": "Full",
            "colour_range_Original": "Limited",
            "matrix_coefficients": "BT.709",
            "matrix_coefficients_Original": "BT.2020 non-constant",
            "colour_primaries": "BT.2020",
        }));
        let ffprobe = ffprobe_video(json!({
            "color_range": "pc",
            "color_space": "bt709",
            "color_primaries": null,
        }));
        let error = hlg_colour_contract(Some(&mediainfo), Some(&ffprobe)).unwrap_err();
        assert!(
            error.contains(
                "range: MediaInfo colour_range = Full, MediaInfo colour_range_Original = Limited, ffprobe color_range = pc"
            ),
            "{error}"
        );
        assert!(
            error.contains("matrix: MediaInfo matrix_coefficients = BT.709, MediaInfo matrix_coefficients_Original = BT.2020 non-constant, ffprobe color_space = bt709"),
            "{error}"
        );
        assert!(!error.contains("primaries:"), "{error}");

        // One conforming source does not outvote a non-conforming one.
        let mediainfo = mediainfo_video(json!({"colour_range": "Limited"}));
        let ffprobe = ffprobe_video(json!({"color_range": "pc"}));
        assert!(hlg_colour_contract(Some(&mediainfo), Some(&ffprobe)).is_err());
    }

    #[test]
    fn untagged_hlg_fields_are_assumed_with_a_warning() {
        // MediaInfo tags only the range; ffprobe fills the primaries; the matrix is untagged.
        let mediainfo = mediainfo_video(json!({"colour_range": "Limited"}));
        let ffprobe = ffprobe_video(json!({
            "color_space": "unknown",
            "color_primaries": "bt2020",
        }));
        let warnings = hlg_colour_contract(Some(&mediainfo), Some(&ffprobe)).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("does not tag its colour matrix"),
            "{}",
            warnings[0]
        );
        assert!(
            warnings[0].contains("assuming BT.2020 non-constant-luminance matrix"),
            "{}",
            warnings[0]
        );

        let warnings = hlg_colour_contract(None, None).unwrap();
        assert!(
            warnings[0].contains("range, matrix, primaries"),
            "{}",
            warnings[0]
        );
        assert!(warnings[0].contains("limited range"), "{}", warnings[0]);
    }

    #[test]
    fn hlg_hints_are_recognized() {
        assert!(hints_indicate_hlg("arib-std-b67"));
        assert!(hints_indicate_hlg(
            "Dolby Vision, Version 1.0, Profile 8.4, dvhe.08.06, BL+RPU, HLG compatible"
        ));
        assert!(!hints_indicate_hlg("PQ\nSMPTE ST 2086, HDR10 compatible"));
    }

    fn meta(entries: &[(&str, f64)]) -> HashMap<String, f64> {
        entries
            .iter()
            .map(|&(key, value)| (key.to_string(), value))
            .collect()
    }

    const ALL_STATIC_KEYS: [&str; 4] = ["max_dml", "min_dml", "max_cll", "max_fall"];

    #[test]
    fn static_defaults_fill_every_requested_missing_key() {
        for hlg_source in [false, true] {
            let mut values = HashMap::new();
            apply_static_defaults(&mut values, &ALL_STATIC_KEYS, hlg_source);
            assert_eq!(
                values,
                meta(&[
                    ("max_dml", 1000.0),
                    ("min_dml", 0.005),
                    ("max_cll", 1000.0),
                    ("max_fall", 400.0),
                ]),
                "hlg_source {hlg_source}"
            );
        }
    }

    #[test]
    fn static_defaults_never_override_a_present_value() {
        for hlg_source in [false, true] {
            let mut values = meta(&[("max_dml", 4000.0), ("max_cll", 0.5)]);
            apply_static_defaults(&mut values, &ALL_STATIC_KEYS, hlg_source);
            assert_eq!(
                values,
                meta(&[
                    ("max_dml", 4000.0),
                    ("min_dml", 0.005),
                    ("max_cll", 0.5),
                    ("max_fall", 400.0),
                ]),
                "hlg_source {hlg_source}"
            );
        }
    }

    #[test]
    fn static_defaults_ignore_keys_not_requested() {
        for hlg_source in [false, true] {
            let mut values = meta(&[("min_dml", 0.0001)]);
            apply_static_defaults(&mut values, &["max_cll", "min_dml"], hlg_source);
            assert_eq!(
                values,
                meta(&[("min_dml", 0.0001), ("max_cll", 1000.0)]),
                "hlg_source {hlg_source}"
            );

            let mut values = HashMap::new();
            apply_static_defaults(&mut values, &[], hlg_source);
            assert!(values.is_empty());

            // Keys outside the four fallbacks are neither filled nor rejected.
            let mut values = HashMap::new();
            apply_static_defaults(&mut values, &["source_min_pq", "max_fall"], hlg_source);
            assert_eq!(values, meta(&[("max_fall", 400.0)]));
        }
    }

    #[test]
    fn zero_light_levels_are_neither_inserted_nor_clear_a_stated_value() {
        let mut values = HashMap::new();
        insert_light_level(&mut values, "max_cll", 0.0);
        assert!(values.is_empty());

        let mut values = meta(&[("max_cll", 1000.0)]);
        insert_light_level(&mut values, "max_cll", 0.0);
        insert_light_level(&mut values, "max_fall", -3.0);
        assert_eq!(values, meta(&[("max_cll", 1000.0)]));

        insert_light_level(&mut values, "max_cll", 0.25);
        assert_eq!(values, meta(&[("max_cll", 0.25)]));
    }

    #[test]
    fn mastering_primaries_parse_only_the_parenthesized_spellings() {
        let expected = Some((8500, 39850, 6550, 2300, 35400, 14600, 15635, 16450));
        assert_eq!(
            parse_mastering_display_color_primaries(
                "G(x=0.1700, y=0.7970), B(x=0.1310, y=0.0460), R(x=0.7080, y=0.2920), WP(x=0.3127, y=0.3290)"
            ),
            expected
        );
        assert_eq!(
            parse_mastering_display_color_primaries(
                "G(0.1700,0.7970)B(0.1310,0.0460)R(0.7080,0.2920)WP(0.3127,0.3290)"
            ),
            expected
        );
        // Out-of-range coordinates clamp to 0..=50000.
        assert_eq!(
            parse_mastering_display_color_primaries("G(1.5,0)B(0,0)R(0,0)WP(0,0)"),
            Some((50000, 0, 0, 0, 0, 0, 0, 0))
        );
        // A missing white point rejects the whole value.
        assert_eq!(
            parse_mastering_display_color_primaries("G(0.17,0.797)B(0.131,0.046)R(0.708,0.292)"),
            None
        );
    }

    #[test]
    fn mastering_primaries_in_mediainfo_spellings_are_not_parsed() {
        // Suspicious: MediaInfo 24.01 writes MasteringDisplay_ColorPrimaries either as a name or
        // as "R: x=.. y=..", and neither matches, so read_static_metadata never stores md_* keys.
        for spelling in [
            "Display P3",
            "BT.2020",
            "R: x=0.682000 y=0.318000, G: x=0.260000 y=0.680000, B: x=0.148000 y=0.062000, White point: x=0.312000 y=0.328000",
        ] {
            assert_eq!(
                parse_mastering_display_color_primaries(spelling),
                None,
                "{spelling}"
            );
        }
    }

    #[test]
    fn details_file_prefers_the_mkv_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("clip.mkv");
        assert_eq!(find_details_file(&input), None);
        // Only `<stem>_Details.txt` and `<stem>_mkv_Details.txt`; other spellings are ignored.
        fs::write(dir.path().join("clip.mkv_Details.txt"), "MaxCLL: 1").unwrap();
        fs::write(dir.path().join("Details.txt"), "MaxCLL: 1").unwrap();
        assert_eq!(find_details_file(&input), None);

        let plain = dir.path().join("clip_Details.txt");
        fs::write(&plain, "").unwrap();
        assert_eq!(find_details_file(&input), Some(plain));
        let mkv = dir.path().join("clip_mkv_Details.txt");
        fs::write(&mkv, "").unwrap();
        assert_eq!(find_details_file(&input), Some(mkv));
    }

    #[test]
    fn details_file_light_levels_are_read_without_mediainfo_data() {
        // The input is not a video, so MediaInfo (if installed) reports no video track and only
        // the Details.txt override contributes.
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("clip.mkv");
        fs::write(&input, b"not a video").unwrap();
        let read = || read_static_metadata(input.to_str().unwrap());
        assert!(read().is_empty());

        // Case-insensitive labels, a comma decimal, and a zero MaxFALL that is not a value.
        fs::write(
            dir.path().join("clip_Details.txt"),
            "Video\nmaxcll : 1234,5 cd/m2\nMaxFALL:0\n",
        )
        .unwrap();
        assert_eq!(read(), meta(&[("max_cll", 1234.5)]));

        // Suspicious: a thousands separator is read as a decimal comma (1,000 -> 1.0), and a
        // value with both separators does not parse at all. Only the first match counts.
        fs::write(
            dir.path().join("clip_mkv_Details.txt"),
            "MaxCLL: 1,000\nMaxFALL: 1,234.5\nMaxFALL: 300\n",
        )
        .unwrap();
        assert_eq!(read(), meta(&[("max_cll", 1.0)]));
    }

    /// Serializes `check_hdr_format` in tests: its Dolby Vision probes use one directory per
    /// process under the system temp dir (`mkvdovi_dv_sniff_<pid>`, `mkvdovi_dv_probe_<pid>`) and
    /// delete it afterwards, so two calls on parallel test threads would race.
    static HDR_FORMAT_PROBE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn hdr_format_of(input: &Path) -> HdrFormat {
        let _guard = HDR_FORMAT_PROBE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        check_hdr_format(input.to_str().unwrap())
    }

    /// Reason to skip a test on a generated clip, or `None` when ffmpeg with libx265 and every
    /// `(tool, version argument)` in `tools` run.
    fn missing_clip_tools(tools: &[(&str, &str)]) -> Option<String> {
        for &(tool, arg) in [("ffmpeg", "-version")].iter().chain(tools) {
            if Command::new(tool).arg(arg).output().is_err() {
                return Some(format!("{tool} not found in PATH"));
            }
        }
        let encoders = Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
            .ok()?;
        if !String::from_utf8_lossy(&encoders.stdout).contains("libx265") {
            return Some("ffmpeg has no libx265 encoder".into());
        }
        None
    }

    /// The tools `check_hdr_format` needs to take its MediaInfo path. Without MediaInfo it falls
    /// back to ffprobe's transfer tag and classifies some inputs differently (HDR10+ as HDR10).
    const HDR_FORMAT_TOOLS: &[(&str, &str)] =
        &[("mediainfo", "--version"), ("ffprobe", "-version")];

    /// HDR10 x265 parameters: BT.2020 PQ with a P3 1000-nit mastering display and 1000/400 nits
    /// MaxCLL/MaxFALL SEI.
    const PQ_X265: &str = "hdr10=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:\
        master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,50):\
        max-cll=1000,400";
    const HLG_X265: &str = "colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc";
    const SDR_X265: &str = "colorprim=bt709:transfer=bt709:colormatrix=bt709";

    /// Three 64x64 10-bit frames encoded with libx265 into `dir/name`; the extension picks the
    /// container (`.mkv`, or `.hevc` for a raw stream). Err holds ffmpeg's message.
    fn encode_clip(
        dir: &Path,
        name: &str,
        x265_params: &str,
    ) -> std::result::Result<PathBuf, String> {
        let output = dir.join(name);
        let result = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=64x64:r=24,format=yuv420p10le",
                "-frames:v",
                "3",
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-x265-params",
                &format!("log-level=error:{x265_params}"),
                "-pix_fmt",
                "yuv420p10le",
                "-y",
            ])
            .arg(&output)
            .output()
            .map_err(|error| error.to_string())?;
        if !result.status.success() {
            return Err(String::from_utf8_lossy(&result.stderr).into_owned());
        }
        Ok(output)
    }

    fn run_tool(command: &mut Command) {
        let output = command
            .output()
            .unwrap_or_else(|error| panic!("failed to start {command:?}: {error}"));
        assert!(
            output.status.success(),
            "{command:?} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn static_metadata_of_an_hdr10_clip_is_the_mediainfo_luminance_and_light_levels() {
        if let Some(reason) = missing_clip_tools(&[("mediainfo", "--version")]) {
            eprintln!("Skipping static_metadata_of_an_hdr10_clip_is_the_mediainfo_luminance_and_light_levels: {reason}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = encode_clip(dir.path(), "clip.mkv", PQ_X265).unwrap();
        // MediaInfo names the P3 primaries, which yields no md_* keys (see
        // mastering_primaries_in_mediainfo_spellings_are_not_parsed).
        assert_eq!(
            read_static_metadata(clip.to_str().unwrap()),
            meta(&[
                ("max_dml", 1000.0),
                ("min_dml", 0.005),
                ("max_cll", 1000.0),
                ("max_fall", 400.0),
            ])
        );
    }

    #[test]
    fn static_metadata_of_unlisted_primaries_and_zero_light_levels_is_luminance_only() {
        if let Some(reason) = missing_clip_tools(&[("mediainfo", "--version")]) {
            eprintln!("Skipping static_metadata_of_unlisted_primaries_and_zero_light_levels_is_luminance_only: {reason}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        // MediaInfo 24.01 leaves MaxCLL/MaxFALL out for a 0,0 light-level SEI, so this pins that
        // omission; the zero skip in read_static_metadata itself is pinned by
        // static_metadata_skips_a_zero_container_max_cll_and_hides_the_stream_value.
        let params = "hdr10=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:\
            master-display=G(13000,34000)B(7400,3100)R(34100,15900)WP(15600,16400)L(40000000,1):\
            max-cll=0,0";
        let clip = encode_clip(dir.path(), "clip.mkv", params).unwrap();
        assert_eq!(
            read_static_metadata(clip.to_str().unwrap()),
            meta(&[("max_dml", 4000.0), ("min_dml", 0.0001)])
        );
    }

    #[test]
    fn static_metadata_of_a_clip_without_hdr_sei_is_empty() {
        if let Some(reason) = missing_clip_tools(&[("mediainfo", "--version")]) {
            eprintln!("Skipping static_metadata_of_a_clip_without_hdr_sei_is_empty: {reason}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = encode_clip(dir.path(), "clip.mkv", HLG_X265).unwrap();
        assert!(read_static_metadata(clip.to_str().unwrap()).is_empty());
    }

    #[test]
    fn details_file_overrides_the_mediainfo_light_levels() {
        if let Some(reason) = missing_clip_tools(&[("mediainfo", "--version")]) {
            eprintln!("Skipping details_file_overrides_the_mediainfo_light_levels: {reason}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = encode_clip(dir.path(), "clip.mkv", PQ_X265).unwrap();
        // A zero MaxFALL keeps the stream's value; the mastering display is never overridden.
        fs::write(
            dir.path().join("clip_Details.txt"),
            "MaxCLL : 812,5 cd/m2\nMaxFALL : 0\nMaxDML: 4000\n",
        )
        .unwrap();
        assert_eq!(
            read_static_metadata(clip.to_str().unwrap()),
            meta(&[
                ("max_dml", 1000.0),
                ("min_dml", 0.005),
                ("max_cll", 812.5),
                ("max_fall", 400.0),
            ])
        );
    }

    #[test]
    fn static_metadata_skips_a_zero_container_max_cll_and_hides_the_stream_value() {
        if let Some(reason) =
            missing_clip_tools(&[("mediainfo", "--version"), ("mkvmerge", "--version")])
        {
            eprintln!("Skipping static_metadata_skips_a_zero_container_max_cll_and_hides_the_stream_value: {reason}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let raw = encode_clip(dir.path(), "clip.hevc", PQ_X265).unwrap();
        let clip = dir.path().join("clip.mkv");
        run_tool(
            Command::new("mkvmerge")
                .arg("-q")
                .arg("-o")
                .arg(&clip)
                .args(["--max-content-light", "0:0"])
                .arg(&raw),
        );
        // MediaInfo reports the container's MaxCLL as "0" and moves the stream's 1000 to
        // MaxCLL_Original, which is never read. The zero is not stored either, so max_cll ends up
        // missing although the stream states 1000 (suspicious; the L6 default fills it later).
        assert_eq!(
            read_static_metadata(clip.to_str().unwrap()),
            meta(&[("max_dml", 1000.0), ("min_dml", 0.005), ("max_fall", 400.0)])
        );
    }

    #[test]
    fn static_metadata_skips_a_zero_container_max_fall_and_hides_the_stream_value() {
        if let Some(reason) =
            missing_clip_tools(&[("mediainfo", "--version"), ("mkvmerge", "--version")])
        {
            eprintln!("Skipping static_metadata_skips_a_zero_container_max_fall_and_hides_the_stream_value: {reason}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let raw = encode_clip(dir.path(), "clip.hevc", PQ_X265).unwrap();
        let clip = dir.path().join("clip.mkv");
        run_tool(
            Command::new("mkvmerge")
                .arg("-q")
                .arg("-o")
                .arg(&clip)
                .args(["--max-frame-light", "0:0"])
                .arg(&raw),
        );
        // The MaxFALL twin of the MaxCLL case above: the stream's 400 moves to MaxFALL_Original
        // and the container's "0" is not stored (suspicious, as above).
        assert_eq!(
            read_static_metadata(clip.to_str().unwrap()),
            meta(&[("max_dml", 1000.0), ("min_dml", 0.005), ("max_cll", 1000.0)])
        );
    }

    #[test]
    fn hdr10_clip_without_measurements_is_hdr10_unsupported() {
        if let Some(reason) = missing_clip_tools(HDR_FORMAT_TOOLS) {
            eprintln!("Skipping hdr10_clip_without_measurements_is_hdr10_unsupported: {reason}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = encode_clip(dir.path(), "clip.mkv", PQ_X265).unwrap();
        assert_eq!(hdr_format_of(&clip), HdrFormat::Hdr10Unsupported);
    }

    #[test]
    fn hdr10_clip_with_measurements_next_to_it_is_hdr10_with_measurements() {
        if let Some(reason) = missing_clip_tools(HDR_FORMAT_TOOLS) {
            eprintln!("Skipping hdr10_clip_with_measurements_next_to_it_is_hdr10_with_measurements: {reason}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = encode_clip(dir.path(), "clip.mkv", PQ_X265).unwrap();
        // Only the file's existence counts, not its content.
        fs::write(dir.path().join("clip_measurements.bin"), b"").unwrap();
        assert_eq!(hdr_format_of(&clip), HdrFormat::Hdr10WithMeasurements);
    }

    #[test]
    fn hlg_clip_is_hlg() {
        if let Some(reason) = missing_clip_tools(HDR_FORMAT_TOOLS) {
            eprintln!("Skipping hlg_clip_is_hlg: {reason}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = encode_clip(dir.path(), "clip.mkv", HLG_X265).unwrap();
        assert_eq!(hdr_format_of(&clip), HdrFormat::Hlg);
        // A measurements file does not change an HLG classification.
        fs::write(dir.path().join("clip_measurements.bin"), b"").unwrap();
        assert_eq!(hdr_format_of(&clip), HdrFormat::Hlg);
    }

    #[test]
    fn sdr_bt709_clip_is_unsupported() {
        if let Some(reason) = missing_clip_tools(HDR_FORMAT_TOOLS) {
            eprintln!("Skipping sdr_bt709_clip_is_unsupported: {reason}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = encode_clip(dir.path(), "clip.mkv", SDR_X265).unwrap();
        assert_eq!(hdr_format_of(&clip), HdrFormat::Unsupported);
    }

    #[test]
    fn hdr10plus_clip_is_hdr10plus() {
        if let Some(reason) = missing_clip_tools(HDR_FORMAT_TOOLS) {
            eprintln!("Skipping hdr10plus_clip_is_hdr10plus: {reason}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let scene = |frame: u64| {
            json!({
                "LuminanceParameters": {
                    "AverageRGB": 1000,
                    "LuminanceDistributions": {
                        "DistributionIndex": [1, 5, 10, 25, 50, 75, 90, 95, 99],
                        "DistributionValues": [0, 10, 100, 1000, 5000, 10000, 20000, 30000, 40000]
                    },
                    "MaxScl": [40000, 40000, 40000]
                },
                "NumberOfWindows": 1,
                "TargetedSystemDisplayMaximumLuminance": 0,
                "SceneFrameIndex": frame,
                "SceneId": 0,
                "SequenceFrameIndex": frame
            })
        };
        let hdr10plus = json!({
            "JSONInfo": {"HDR10plusProfile": "A", "Version": "1.0"},
            "SceneInfo": [scene(0), scene(1), scene(2)],
            "SceneInfoSummary": {"SceneFirstFrameIndex": [0], "SceneFrameNumbers": [3]}
        });
        let json_path = dir.path().join("hdr10plus.json");
        fs::write(&json_path, hdr10plus.to_string()).unwrap();
        let params = format!("{PQ_X265}:dhdr10-info={}", json_path.display());
        let clip = match encode_clip(dir.path(), "clip.mkv", &params) {
            Ok(clip) => clip,
            // libx265 built without HDR10+ rejects the option by name; any other error fails.
            Err(error) if error.contains("dhdr10-info") => {
                eprintln!(
                    "Skipping hdr10plus_clip_is_hdr10plus: libx265 does not accept dhdr10-info: {}",
                    error.trim()
                );
                return;
            }
            Err(error) => panic!("HDR10+ encode failed: {error}"),
        };
        assert_eq!(hdr_format_of(&clip), HdrFormat::Hdr10Plus);
        // HDR10+ wins over a measurements file.
        fs::write(dir.path().join("clip_measurements.bin"), b"").unwrap();
        assert_eq!(hdr_format_of(&clip), HdrFormat::Hdr10Plus);
    }

    /// The tools for a generated Profile 8.1 clip, on top of `HDR_FORMAT_TOOLS`.
    const PROFILE81_TOOLS: &[(&str, &str)] =
        &[("dovi_tool", "--version"), ("mkvmerge", "--version")];

    /// A three-frame HDR10 stream with a generated Profile 8.1 RPU injected: the raw HEVC
    /// (`injected.hevc`) and its mkvmerge mux (`clip.mkv`, which carries the Dolby Vision
    /// configuration record).
    fn profile81_clip(dir: &Path) -> (PathBuf, PathBuf) {
        let raw = encode_clip(dir, "clip.hevc", PQ_X265).unwrap();
        let config = dir.join("generate.json");
        fs::write(
            &config,
            json!({
                "cm_version": "V40",
                "length": 3,
                "profile": "8.1",
                "level6": {
                    "max_display_mastering_luminance": 1000,
                    "min_display_mastering_luminance": 50,
                    "max_content_light_level": 1000,
                    "max_frame_average_light_level": 400
                }
            })
            .to_string(),
        )
        .unwrap();
        let rpu = dir.join("RPU.bin");
        run_tool(
            Command::new("dovi_tool")
                .arg("generate")
                .arg("-j")
                .arg(&config)
                .arg("-o")
                .arg(&rpu),
        );
        let injected = dir.join("injected.hevc");
        run_tool(
            Command::new("dovi_tool")
                .arg("inject-rpu")
                .arg("-i")
                .arg(&raw)
                .arg("--rpu-in")
                .arg(&rpu)
                .arg("-o")
                .arg(&injected),
        );
        let clip = dir.join("clip.mkv");
        run_tool(
            Command::new("mkvmerge")
                .arg("-q")
                .arg("-o")
                .arg(&clip)
                .arg(&injected),
        );
        (injected, clip)
    }

    /// MediaInfo's Dolby Vision fields for `input`, lowercased: what `check_hdr_format` matches
    /// "dvhe.08" / "dolby vision" against.
    fn mediainfo_dolby_vision_text(input: &Path) -> String {
        let output = Command::new("mediainfo")
            .arg("--Inform=Video;%HDR_Format%/%HDR_Format_Profile%/%HDR_Format_Compatibility%/%CodecID%")
            .arg(input)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).to_lowercase()
    }

    #[test]
    fn profile81_clip_is_dolby_vision_p8() {
        if let Some(reason) = missing_clip_tools(&[HDR_FORMAT_TOOLS, PROFILE81_TOOLS].concat()) {
            eprintln!("Skipping profile81_clip_is_dolby_vision_p8: {reason}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (_, clip) = profile81_clip(dir.path());
        assert_eq!(hdr_format_of(&clip), HdrFormat::DolbyVisionP8);
    }

    #[test]
    fn profile81_raw_stream_is_dolby_vision_p8_from_the_rpu_alone() {
        if let Some(reason) = missing_clip_tools(&[HDR_FORMAT_TOOLS, PROFILE81_TOOLS].concat()) {
            eprintln!(
                "Skipping profile81_raw_stream_is_dolby_vision_p8_from_the_rpu_alone: {reason}"
            );
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (injected, _) = profile81_clip(dir.path());
        // MediaInfo sees plain HDR10 in a raw stream, so only the RPU sniff can say P8.
        let mediainfo = mediainfo_dolby_vision_text(&injected);
        assert!(
            !mediainfo.contains("dvhe") && !mediainfo.contains("dolby vision"),
            "MediaInfo now reports Dolby Vision for a raw stream, so this test no longer isolates \
             the RPU path: {mediainfo}"
        );
        assert_eq!(hdr_format_of(&injected), HdrFormat::DolbyVisionP8);
    }

    #[test]
    fn profile8_configuration_without_rpu_is_dolby_vision_p8_from_mediainfo_alone() {
        if let Some(reason) = missing_clip_tools(&[HDR_FORMAT_TOOLS, PROFILE81_TOOLS].concat()) {
            eprintln!("Skipping profile8_configuration_without_rpu_is_dolby_vision_p8_from_mediainfo_alone: {reason}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (_, clip) = profile81_clip(dir.path());
        // Drop the RPU NAL units (type 62); the copy keeps the container's configuration record.
        let stripped = dir.path().join("stripped.mkv");
        run_tool(
            Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-i"])
                .arg(&clip)
                .args(["-c", "copy", "-bsf:v", "filter_units=remove_types=62", "-y"])
                .arg(&stripped),
        );
        let mediainfo = mediainfo_dolby_vision_text(&stripped);
        if !mediainfo.contains("dvhe.08") {
            eprintln!(
                "Skipping profile8_configuration_without_rpu_is_dolby_vision_p8_from_mediainfo_alone: \
                 ffmpeg did not keep the Dolby Vision configuration: {}",
                mediainfo.trim()
            );
            return;
        }
        assert!(
            !rpu_check::try_extract_rpu_quiet(
                stripped.to_str().unwrap(),
                &dir.path().join("sniff_RPU.bin"),
                Some(60)
            ),
            "the stripped clip still has an RPU, so this test no longer isolates the MediaInfo path"
        );
        assert_eq!(hdr_format_of(&stripped), HdrFormat::DolbyVisionP8);
    }

    /// Writes `generate_extra_json` output to a temp file and returns it byte for byte.
    fn extra_json_text(
        profile: &str,
        metadata: &HashMap<String, f64>,
        trim_targets: &[u32],
        cm_v40_config: Option<&CmV40Config>,
        level5_offsets: Option<Level5Offsets>,
        l1_sidecar: Option<&L1Sidecar>,
    ) -> String {
        let output = tempfile::NamedTempFile::new().unwrap();
        generate_extra_json(
            output.path(),
            profile,
            metadata,
            trim_targets,
            cm_v40_config,
            level5_offsets,
            l1_sidecar,
        )
        .unwrap();
        fs::read_to_string(output.path()).unwrap()
    }

    // The resume path compares regenerated extra.json bytes with the previous run's, so these
    // goldens pin the whole file: key order, formatting, truncation and every block.
    const EXTRA_JSON_81_GOLDEN: &str = r#"{
  "cm_version": "V40",
  "default_metadata_blocks": [
    {
      "Level2": {
        "ms_weight": 2048,
        "target_max_pq": 2081,
        "trim_chroma_weight": 2048,
        "trim_offset": 2048,
        "trim_power": 2048,
        "trim_saturation_gain": 2048,
        "trim_slope": 2048
      }
    },
    {
      "Level2": {
        "ms_weight": 2048,
        "target_max_pq": 2851,
        "trim_chroma_weight": 2048,
        "trim_offset": 2048,
        "trim_power": 2048,
        "trim_saturation_gain": 2048,
        "trim_slope": 2048
      }
    },
    {
      "Level2": {
        "ms_weight": 2048,
        "target_max_pq": 3079,
        "trim_chroma_weight": 2048,
        "trim_offset": 2048,
        "trim_power": 2048,
        "trim_saturation_gain": 2048,
        "trim_slope": 2048
      }
    },
    {
      "Level9": {
        "length": 1,
        "source_primary_index": 0
      }
    },
    {
      "Level11": {
        "content_type": 1,
        "reference_mode_flag": false,
        "whitepoint": 0
      }
    }
  ],
  "l1_avg_pq_cm_version": "V29",
  "length": 22,
  "level5": {
    "active_area_bottom_offset": 136,
    "active_area_left_offset": 0,
    "active_area_right_offset": 4,
    "active_area_top_offset": 140
  },
  "level6": {
    "max_content_light_level": 997,
    "max_display_mastering_luminance": 1000,
    "max_frame_average_light_level": 91,
    "min_display_mastering_luminance": 50
  },
  "profile": "8.1",
  "shots": [
    {
      "duration": 12,
      "metadata_blocks": [
        {
          "Level1": {
            "avg_pq": 500,
            "max_pq": 2400,
            "min_pq": 1
          }
        }
      ],
      "start": 0
    },
    {
      "duration": 10,
      "metadata_blocks": [
        {
          "Level1": {
            "avg_pq": 600,
            "max_pq": 2500,
            "min_pq": 2
          }
        }
      ],
      "start": 12
    }
  ],
  "source_max_pq": 3079,
  "source_min_pq": 62
}"#;

    #[test]
    fn extra_json_for_profile_81_with_sidecar_shots_is_byte_identical() {
        let metadata = meta(&[
            ("min_dml", 0.005),
            ("max_dml", 1000.0),
            // Fractions are truncated in L6.
            ("max_cll", 997.9),
            ("max_fall", 91.6),
        ]);
        let config = CmV40Config {
            source_primary_index: 0,
            content_type: 1,
            reference_mode: false,
        };
        // Four different offsets, so a swapped side changes the bytes.
        let offsets = Level5Offsets {
            left: 0,
            right: 4,
            top: 140,
            bottom: 136,
        };
        let sidecar = L1Sidecar {
            version: 5,
            scenes: vec![
                L1SidecarScene {
                    start: 0,
                    end: 9,
                    min_pq_12bit: 1,
                    avg_luma_pq_12bit: 480,
                    avg_max_rgb_pq_12bit: 500,
                    max_pq_12bit: 2400,
                },
                L1SidecarScene {
                    start: 10,
                    end: 19,
                    min_pq_12bit: 2,
                    avg_luma_pq_12bit: 590,
                    avg_max_rgb_pq_12bit: 600,
                    max_pq_12bit: 2500,
                },
            ],
            source: accounted_source(22, 2),
            ..Default::default()
        };
        let text = extra_json_text(
            "8.1",
            &metadata,
            &[100, 600, 1000],
            Some(&config),
            Some(offsets),
            Some(&sidecar),
        );
        assert_eq!(text, EXTRA_JSON_81_GOLDEN, "actual:\n{text}");
    }

    const EXTRA_JSON_84_GOLDEN: &str = r#"{
  "cm_version": "V40",
  "default_metadata_blocks": [
    {
      "Level2": {
        "ms_weight": 2048,
        "target_max_pq": 2081,
        "trim_chroma_weight": 2048,
        "trim_offset": 2048,
        "trim_power": 2048,
        "trim_saturation_gain": 2048,
        "trim_slope": 2048
      }
    },
    {
      "Level2": {
        "ms_weight": 2048,
        "target_max_pq": 2851,
        "trim_chroma_weight": 2048,
        "trim_offset": 2048,
        "trim_power": 2048,
        "trim_saturation_gain": 2048,
        "trim_slope": 2048
      }
    },
    {
      "Level9": {
        "length": 1,
        "source_primary_index": 2
      }
    },
    {
      "Level11": {
        "content_type": 3,
        "reference_mode_flag": true,
        "whitepoint": 0
      }
    }
  ],
  "l1_avg_pq_cm_version": "V29",
  "length": 20,
  "level6": {
    "max_content_light_level": 1000,
    "max_display_mastering_luminance": 1000,
    "max_frame_average_light_level": 400,
    "min_display_mastering_luminance": 50
  },
  "profile": "8.4",
  "shots": [
    {
      "duration": 10,
      "metadata_blocks": [
        {
          "Level1": {
            "avg_pq": 520,
            "max_pq": 2400,
            "min_pq": 1
          }
        }
      ],
      "start": 0
    },
    {
      "duration": 10,
      "metadata_blocks": [
        {
          "Level1": {
            "avg_pq": 610,
            "max_pq": 2500,
            "min_pq": 2
          }
        }
      ],
      "start": 10
    }
  ]
}"#;

    #[test]
    fn extra_json_for_profile_84_hlg_is_byte_identical() {
        let metadata = meta(&[
            ("min_dml", 0.005),
            ("max_dml", 1000.0),
            ("max_cll", 1000.0),
            ("max_fall", 400.0),
        ]);
        let config = CmV40Config {
            source_primary_index: 2,
            content_type: 3,
            reference_mode: true,
        };
        let sidecar: L1Sidecar = serde_json::from_value(v3_hlg_sidecar_json()).unwrap();
        let text = extra_json_text(
            "8.4",
            &metadata,
            &[100, 600],
            Some(&config),
            None,
            Some(&sidecar),
        );
        assert_eq!(text, EXTRA_JSON_84_GOLDEN, "actual:\n{text}");
    }

    /// The variant the manifest of a corpus cut implies. Both HDR10 variants count as HDR10,
    /// because they differ only by a measurements file next to the input.
    fn manifest_hdr_format(manifest: &Value) -> &'static str {
        let dolby_vision = &manifest["dolby_vision"];
        if manifest["transfer"] == "hlg" {
            "HLG"
        } else if dolby_vision["profile"] == 7 && dolby_vision["el_type"] == "MEL" {
            "MEL"
        } else if dolby_vision["profile"] == 7 && dolby_vision["el_type"] == "FEL" {
            "FEL"
        } else if dolby_vision["profile"] == 8 {
            "P8"
        } else if manifest["hdr_format"]
            .as_str()
            .is_some_and(|format| format.contains("2094"))
        {
            "HDR10+"
        } else {
            "HDR10"
        }
    }

    fn hdr_format_label(format: HdrFormat) -> &'static str {
        match format {
            HdrFormat::Hdr10Plus => "HDR10+",
            HdrFormat::Hdr10WithMeasurements | HdrFormat::Hdr10Unsupported => "HDR10",
            HdrFormat::Hlg => "HLG",
            HdrFormat::DolbyVisionMel => "MEL",
            HdrFormat::DolbyVisionFel => "FEL",
            HdrFormat::DolbyVisionP8 => "P8",
            HdrFormat::Unsupported => "Unsupported",
        }
    }

    /// Cuts whose classification today differs from the manifest rule, by directory name, with
    /// the variant `check_hdr_format` returns.
    const CORPUS_KNOWN_DIFFERENCES: &[(&str, &str)] = &[];

    #[test]
    fn corpus_cuts_are_classified_like_their_manifests() {
        let Some(corpus) = std::env::var_os("MKVDOVI_CORPUS_DIR") else {
            eprintln!(
                "Skipping corpus_cuts_are_classified_like_their_manifests: MKVDOVI_CORPUS_DIR is not set"
            );
            return;
        };
        let mut cuts: Vec<PathBuf> = fs::read_dir(&corpus)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|dir| dir.join("manifest.json").is_file() && dir.join("input.mkv").is_file())
            .collect();
        cuts.sort();
        assert!(
            !cuts.is_empty(),
            "no cuts under {}",
            Path::new(&corpus).display()
        );

        let mut mismatches = Vec::new();
        for cut in &cuts {
            let name = cut.file_name().unwrap().to_string_lossy().into_owned();
            let manifest: Value =
                serde_json::from_reader(File::open(cut.join("manifest.json")).unwrap()).unwrap();
            let expected = CORPUS_KNOWN_DIFFERENCES
                .iter()
                .find(|(cut_name, _)| *cut_name == name)
                .map_or_else(|| manifest_hdr_format(&manifest), |&(_, today)| today);
            let actual = hdr_format_label(hdr_format_of(&cut.join("input.mkv")));
            eprintln!("{name}: expected {expected}, got {actual}");
            if actual != expected {
                mismatches.push(format!("{name}: expected {expected}, got {actual}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
