use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;

use super::{find_measurements_file, get_ffprobe_json, get_mediainfo_json, HdrFormat};
use crate::rpu_check::{self, RpuFormatKind};

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
pub(super) fn hints_indicate_hlg(hints: &str) -> bool {
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
pub(super) enum ColourField {
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
    pub(super) fn conforms(self, value: &str) -> Option<bool> {
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
pub(super) fn hlg_colour_contract(
    mediainfo: Option<&Value>,
    ffprobe: Option<&Value>,
) -> Result<Vec<String>, String> {
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
pub fn check_hlg_colour_contract(input_file: &str) -> Result<Vec<String>, String> {
    let mediainfo = get_mediainfo_json(input_file).ok();
    let ffprobe = get_ffprobe_json(input_file).ok();
    hlg_colour_contract(mediainfo.as_ref(), ffprobe.as_ref())
}

pub(super) fn classify_hdr_hints(hints: &str, measurements: bool) -> Option<HdrFormat> {
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
