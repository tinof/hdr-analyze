//! [`RpuParams`] as the manifest's `rpu` object: everything a player needs to build the same RPUs
//! as mkvdovi. Unlike `extra.json` it names the 8.4 composer, because the player has no rewrite
//! step afterwards.

use serde_json::{json, Value};

use super::json::{array_at, bool_at, fail, field, object, str_at, uint_at, uints};
use crate::composer::HlgComposer;
use crate::error::Result;
use crate::params::{CmMetadata, CmV40Config, CmVersion, Level5, Level6, Profile, RpuParams};

const PATH: &str = "rpu";

fn cm_version_name(version: CmVersion) -> &'static str {
    match version {
        CmVersion::V29 => "V29",
        CmVersion::V40 => "V40",
    }
}

fn cm_version_from(name: &str, path: &str) -> Result<CmVersion> {
    match name {
        "V29" => Ok(CmVersion::V29),
        "V40" => Ok(CmVersion::V40),
        other => fail(format!("{path} '{other}' is not V29 or V40")),
    }
}

impl RpuParams {
    /// The manifest's `rpu` object. Refuses the P8 canary, which is diagnostic only and never
    /// describes measured metadata.
    pub fn to_manifest_json(&self) -> Result<Value> {
        let (profile, composer, source_range) = match self.profile {
            Profile::P81 { source_range } => (
                "8.1",
                Value::Null,
                source_range.map_or(Value::Null, |(min, max)| json!([min, max])),
            ),
            Profile::P84 { composer } => {
                let name = match composer {
                    HlgComposer::Preset => "preset",
                    HlgComposer::Bt2100 => "bt2100",
                    HlgComposer::Canary { .. } => {
                        return fail("the canary composer cannot be written to a manifest")
                    }
                };
                ("8.4", json!(name), Value::Null)
            }
        };
        let cm = match &self.cm {
            CmMetadata::V29 => json!({ "version": "V29" }),
            CmMetadata::V40 {
                config,
                trim_targets_nits,
            } => json!({
                "version": "V40",
                "source_primary_index": config.source_primary_index,
                "content_type": config.content_type,
                "reference_mode": config.reference_mode,
                "trim_targets_nits": trim_targets_nits,
            }),
        };
        let level6 = self.level6;
        let level5 = self.level5.map_or(Value::Null, |offsets| {
            json!({
                "left": offsets.left,
                "right": offsets.right,
                "top": offsets.top,
                "bottom": offsets.bottom,
            })
        });
        Ok(json!({
            "profile": profile,
            "composer": composer,
            "source_range": source_range,
            "cm": cm,
            "level6": {
                "max_display_mastering_luminance": level6.max_display_mastering_luminance,
                "min_display_mastering_luminance": level6.min_display_mastering_luminance,
                "max_content_light_level": level6.max_content_light_level,
                "max_frame_average_light_level": level6.max_frame_average_light_level,
            },
            "level5": level5,
            "l1_avg_pq_cm_version": cm_version_name(self.l1_avg_pq_cm_version),
        }))
    }

    /// Reads the `rpu` object back. Checks the types and field widths; whether the values make
    /// a writable RPU is `RpuBuilder::new`'s check, which the manifest parser runs.
    pub fn from_manifest_json(value: &Value) -> Result<Self> {
        let map = object(value, PATH)?;
        let profile = match str_at(map, "profile", PATH)? {
            "8.1" => {
                if !field(map, "composer", PATH)?.is_null() {
                    return fail("rpu.composer must be null for profile 8.1");
                }
                let range = field(map, "source_range", PATH)?;
                let source_range = if range.is_null() {
                    None
                } else {
                    let values = range
                        .as_array()
                        .filter(|values| values.len() == 2)
                        .map(|values| uints::<u16>(values, "rpu.source_range"))
                        .transpose()?
                        .ok_or_else(|| {
                            crate::Error::Manifest("rpu.source_range is not [min, max]".into())
                        })?;
                    Some((values[0], values[1]))
                };
                Profile::P81 { source_range }
            }
            "8.4" => {
                if !field(map, "source_range", PATH)?.is_null() {
                    return fail("rpu.source_range must be null for profile 8.4");
                }
                let composer = match str_at(map, "composer", PATH)? {
                    "preset" => HlgComposer::Preset,
                    "bt2100" => HlgComposer::Bt2100,
                    other => {
                        return fail(format!("rpu.composer '{other}' is not preset or bt2100"))
                    }
                };
                Profile::P84 { composer }
            }
            other => return fail(format!("rpu.profile '{other}' is not 8.1 or 8.4")),
        };

        let cm_value = field(map, "cm", PATH)?;
        let cm_map = object(cm_value, "rpu.cm")?;
        let cm = match cm_version_from(str_at(cm_map, "version", "rpu.cm")?, "rpu.cm.version")? {
            CmVersion::V29 => CmMetadata::V29,
            CmVersion::V40 => CmMetadata::V40 {
                config: CmV40Config {
                    source_primary_index: uint_at(cm_map, "source_primary_index", "rpu.cm")?,
                    content_type: uint_at(cm_map, "content_type", "rpu.cm")?,
                    reference_mode: bool_at(cm_map, "reference_mode", "rpu.cm")?,
                },
                trim_targets_nits: uints(
                    array_at(cm_map, "trim_targets_nits", "rpu.cm")?,
                    "rpu.cm.trim_targets_nits",
                )?,
            },
        };

        let level6_map = object(field(map, "level6", PATH)?, "rpu.level6")?;
        let level6 = Level6 {
            max_display_mastering_luminance: uint_at(
                level6_map,
                "max_display_mastering_luminance",
                "rpu.level6",
            )?,
            min_display_mastering_luminance: uint_at(
                level6_map,
                "min_display_mastering_luminance",
                "rpu.level6",
            )?,
            max_content_light_level: uint_at(level6_map, "max_content_light_level", "rpu.level6")?,
            max_frame_average_light_level: uint_at(
                level6_map,
                "max_frame_average_light_level",
                "rpu.level6",
            )?,
        };

        let level5_value = field(map, "level5", PATH)?;
        let level5 = if level5_value.is_null() {
            None
        } else {
            let level5_map = object(level5_value, "rpu.level5")?;
            Some(Level5 {
                left: uint_at(level5_map, "left", "rpu.level5")?,
                right: uint_at(level5_map, "right", "rpu.level5")?,
                top: uint_at(level5_map, "top", "rpu.level5")?,
                bottom: uint_at(level5_map, "bottom", "rpu.level5")?,
            })
        };

        Ok(RpuParams {
            profile,
            cm,
            level6,
            level5,
            l1_avg_pq_cm_version: cm_version_from(
                str_at(map, "l1_avg_pq_cm_version", PATH)?,
                "rpu.l1_avg_pq_cm_version",
            )?,
        })
    }
}
