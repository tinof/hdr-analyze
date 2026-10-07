use anyhow::{Context, Result};
use dolby_vision::rpu::profiles::{profile84::Profile84, DoviProfile};
use dolby_vision::utils::nits_to_pq_12_bit;
use dovi84_composer::Composer;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

use super::sidecar::is_dovi84_family;
use super::{HdrFormat, L1Sidecar};
use crate::rpu_check::Level5Offsets;

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

pub(super) fn nits_to_pq_code(nits: u32) -> u32 {
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
