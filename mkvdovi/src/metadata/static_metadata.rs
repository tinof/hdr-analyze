use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::{find_details_file, get_mediainfo_json, L1Sidecar, MeasuredLightLevels};

/// Store a source-stated MaxCLL / MaxFALL. Zero means "unknown" in CTA-861.3, so it is not a
/// stated value and never replaces one.
pub(super) fn insert_light_level(meta: &mut HashMap<String, f64>, key: &str, value: f64) {
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

pub(super) fn parse_mastering_display_color_primaries(
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

pub(super) fn detect_source_primaries_from_mediainfo(json: &Value) -> Option<u8> {
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
