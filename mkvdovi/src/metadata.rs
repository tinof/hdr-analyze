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
mod tests;
