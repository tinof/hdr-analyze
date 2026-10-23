//! What an RPU carries apart from L1, as one definition for both front ends: mkvdovi writes it
//! as the `extra.json` of `dovi_tool generate` ([`RpuParams::extra_json`]), and [`RpuBuilder`]
//! builds the same RPU in-process from it.
//!
//! [`RpuBuilder`]: crate::RpuBuilder

use dolby_vision::rpu::extension_metadata::blocks::level6::MAX_PQ_LUMINANCE;
use dolby_vision::rpu::extension_metadata::blocks::{
    ExtMetadataBlock, ExtMetadataBlockLevel11, ExtMetadataBlockLevel2, ExtMetadataBlockLevel5,
    ExtMetadataBlockLevel6, ExtMetadataBlockLevel9, MAX_12_BIT_VALUE,
};
use dolby_vision::rpu::generate::{GenerateConfig, GenerateProfile};
use dolby_vision::rpu::profiles::{profile84::Profile84, DoviProfile};
use dolby_vision::rpu::vdr_dm_data::CmVersion as CrateCmVersion;
use dolby_vision::utils::nits_to_pq_12_bit;
use serde_json::{json, Value};

use crate::builder::L1;
use crate::composer::HlgComposer;
use crate::error::{Error, Result};

/// Content mapping version of the RPU's display management data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CmVersion {
    /// CM v2.9: L1, L2, L5 and L6 only.
    V29,
    /// CM v4.0: also L3, L9, L11 and L254 (the `dovi_tool generate` default).
    #[default]
    V40,
}

impl CmVersion {
    pub(crate) fn to_crate(self) -> CrateCmVersion {
        match self {
            CmVersion::V29 => CrateCmVersion::V29,
            CmVersion::V40 => CrateCmVersion::V40,
        }
    }

    fn json_name(self) -> &'static str {
        match self {
            CmVersion::V29 => "V29",
            CmVersion::V40 => "V40",
        }
    }
}

/// Configuration for the CM v4.0 metadata blocks (L9 and L11).
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Trim targets mkvdovi writes by default (`--trim-targets`), in nits.
pub const DEFAULT_TRIM_TARGETS_NITS: [u32; 3] = [100, 600, 1000];

/// The most L2 blocks, and so trim targets, an RPU carries (`CmV29DmData::validate`).
pub const MAX_TRIM_TARGETS: usize = 8;

/// A block the `dolby_vision` writer would refuse (its `validate`, run on every write) as
/// `Error::OutOfRange`, so that `RpuBuilder::new` refuses what every later write would.
fn writable(level: &str, check: anyhow::Result<()>) -> Result<()> {
    check.map_err(|error| Error::OutOfRange(format!("{level}: {error:#}")))
}

/// The CM version and, under CM v4.0, the default blocks mkvdovi writes for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CmMetadata {
    /// A CM v2.9 RPU: no default blocks (no L2, no CM v4.0 levels).
    V29,
    /// A CM v4.0 RPU with one neutral L2 per trim target, L9 and L11.
    V40 {
        config: CmV40Config,
        trim_targets_nits: Vec<u32>,
    },
}

impl CmMetadata {
    /// The blocks mkvdovi writes by default: CM v4.0, [`CmV40Config::default`] and
    /// [`DEFAULT_TRIM_TARGETS_NITS`].
    pub fn mkvdovi_default() -> Self {
        CmMetadata::V40 {
            config: CmV40Config::default(),
            trim_targets_nits: DEFAULT_TRIM_TARGETS_NITS.to_vec(),
        }
    }

    pub fn cm_version(&self) -> CmVersion {
        match self {
            CmMetadata::V29 => CmVersion::V29,
            CmMetadata::V40 { .. } => CmVersion::V40,
        }
    }
}

/// Mastering display and content light levels mkvdovi falls back to when the source does not
/// state them (`apply_static_defaults`); usual for HLG.
pub const DEFAULT_MAX_DML_NITS: f64 = 1000.0;
pub const DEFAULT_MIN_DML_NITS: f64 = 0.005;
pub const DEFAULT_MAX_CLL_NITS: f64 = 1000.0;
pub const DEFAULT_MAX_FALL_NITS: f64 = 400.0;

/// L6 as `extra.json` states it: mastering peak in nits, mastering minimum in 0.0001 nits,
/// MaxCLL and MaxFALL in nits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level6 {
    pub max_display_mastering_luminance: u32,
    pub min_display_mastering_luminance: u32,
    pub max_content_light_level: u32,
    pub max_frame_average_light_level: u32,
}

impl Level6 {
    /// The most an L6 field can state (`ExtMetadataBlockLevel6::validate`): 10 000 nits, or 1 nit
    /// for the mastering minimum in 0.0001-nit units.
    pub const MAX_VALUE: u32 = MAX_PQ_LUMINANCE as u32;

    /// From values in nits, truncated toward zero as mkvdovi always has (`as u32`).
    pub fn from_nits(min_dml: f64, max_dml: f64, max_cll: f64, max_fall: f64) -> Self {
        Self {
            max_display_mastering_luminance: max_dml as u32,
            min_display_mastering_luminance: (min_dml * 10000.0) as u32,
            max_content_light_level: max_cll as u32,
            max_frame_average_light_level: max_fall as u32,
        }
    }

    /// The defaults mkvdovi writes when the source states none (`DEFAULT_*_NITS`).
    pub fn mkvdovi_default() -> Self {
        Self::from_nits(
            DEFAULT_MIN_DML_NITS,
            DEFAULT_MAX_DML_NITS,
            DEFAULT_MAX_CLL_NITS,
            DEFAULT_MAX_FALL_NITS,
        )
    }

    /// Every field capped at [`MAX_VALUE`](Self::MAX_VALUE), for values taken from a stream's
    /// SEI messages; a larger value makes `RpuBuilder::new` fail. MaxCLL 65535 is common there and
    /// often means "not measured": capping only makes it writable (10 000 nits). A caller that
    /// reads it as unknown passes the defaults (`DEFAULT_*_NITS`) instead.
    pub fn clamped(self) -> Self {
        let cap = |value: u32| value.min(Self::MAX_VALUE);
        Self {
            max_display_mastering_luminance: cap(self.max_display_mastering_luminance),
            min_display_mastering_luminance: cap(self.min_display_mastering_luminance),
            max_content_light_level: cap(self.max_content_light_level),
            max_frame_average_light_level: cap(self.max_frame_average_light_level),
        }
    }

    fn to_block(self) -> Result<ExtMetadataBlockLevel6> {
        let field = |name: &str, value: u32| {
            u16::try_from(value).map_err(|_| Error::OutOfRange(format!("L6 {name} {value}")))
        };
        let block = ExtMetadataBlockLevel6 {
            max_display_mastering_luminance: field(
                "max_display_mastering_luminance",
                self.max_display_mastering_luminance,
            )?,
            min_display_mastering_luminance: field(
                "min_display_mastering_luminance",
                self.min_display_mastering_luminance,
            )?,
            max_content_light_level: field(
                "max_content_light_level",
                self.max_content_light_level,
            )?,
            max_frame_average_light_level: field(
                "max_frame_average_light_level",
                self.max_frame_average_light_level,
            )?,
        };
        writable("L6", block.validate())?;
        Ok(block)
    }
}

/// L5 active area offsets in pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Level5 {
    pub left: u16,
    pub right: u16,
    pub top: u16,
    pub bottom: u16,
}

/// Dolby Vision profile of the RPU.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Profile {
    /// Profile 8.1 (HDR10 base layer). `source_range` is `source_min_pq` / `source_max_pq`; `None`
    /// leaves them to the generator's L6 lookup ([`source_range_81`]).
    P81 { source_range: Option<(u16, u16)> },
    /// Profile 8.4 (HLG base layer) with the composer the RPU carries. The source range is always
    /// the preset's ([`source_range_84`]). `extra.json` does not name the composer:
    /// `dovi_tool generate` always writes the preset, and mkvdovi rewrites it afterwards.
    P84 { composer: HlgComposer },
}

impl Profile {
    fn name(&self) -> &'static str {
        match self {
            Profile::P81 { .. } => "8.1",
            Profile::P84 { .. } => "8.4",
        }
    }
}

/// Everything in an RPU apart from L1 and the scene-refresh flag.
#[derive(Debug, Clone, PartialEq)]
pub struct RpuParams {
    pub profile: Profile,
    pub cm: CmMetadata,
    pub level6: Level6,
    pub level5: Option<Level5>,
    /// The CM version whose L1 average floor applies (819 under V29, 1229 under V40). mkvdovi uses
    /// V29 for measured L1 and the CM version itself otherwise (HDR10+).
    pub l1_avg_pq_cm_version: CmVersion,
}

/// One `extra.json` shot: frames `start .. start + duration` with this L1, unclamped (the
/// generator clamps it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtraShot {
    pub start: u64,
    pub duration: u64,
    pub l1: L1,
}

/// The shots of an `extra.json` and the RPU count they cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtraShots {
    pub length: u64,
    pub shots: Vec<ExtraShot>,
}

/// Mastering display peaks taken at face value for the 8.1 source range.
pub const MASTERING_PEAK_NITS: std::ops::RangeInclusive<f64> = 100.0..=10_000.0;
/// Mastering display minima taken at face value for the 8.1 source range.
pub const MASTERING_MIN_NITS: std::ops::RangeInclusive<f64> = 0.0..=1.0;

/// Profile 8.1 `source_min_pq` / `source_max_pq` (12-bit PQ codes) from the mastering display
/// luminance in nits, with the conversion the `dolby_vision` crate applies to a Dolby CM XML.
/// Without explicit values `dovi_tool generate` derives the range from a coarse L6 lookup
/// (mastering min other than <= 0.001 or exactly 0.005 nits becomes 0, a mastering peak other than
/// 1000/2000/4000/10000 nits becomes 3079). `None` when the values are not a plausible mastering
/// display, so the lookup stays in charge: not finite, a peak outside [`MASTERING_PEAK_NITS`] (a
/// mastering SEI written in the wrong units reads as 0.1 or 1 nit), or a minimum outside
/// [`MASTERING_MIN_NITS`] (a minimum stated in nits instead of 0.0001-nit units).
pub fn source_range_81(min_dml: f64, max_dml: f64) -> Option<(u16, u16)> {
    let usable = MASTERING_MIN_NITS.contains(&min_dml) && MASTERING_PEAK_NITS.contains(&max_dml);
    if !usable {
        return None;
    }
    let range = (nits_to_pq_12_bit(min_dml), nits_to_pq_12_bit(max_dml));
    (range.0 < range.1).then_some(range)
}

/// Source range of every Profile 8.4 RPU, whatever the composer: the `Profile84` preset's
/// 62..3079 (about 0.005..1000 nits), which the HLG measurement clamps to.
pub fn source_range_84() -> (u16, u16) {
    let dm = Profile84::dm_data();
    (dm.source_min_pq, dm.source_max_pq)
}

/// PQ code (12-bit) of `nits`, as mkvdovi computes the L2 `target_max_pq`.
pub fn nits_to_pq_code(nits: u32) -> u32 {
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

/// The neutral L2 trim mkvdovi writes for one target.
const NEUTRAL_TRIM: u16 = 2048;

impl RpuParams {
    pub fn cm_version(&self) -> CmVersion {
        self.cm.cm_version()
    }

    /// The `extra.json` of `dovi_tool generate` for these parameters, byte-identical (through
    /// `serde_json::to_writer_pretty`) to what mkvdovi has always written. `shots` adds `length`,
    /// `l1_avg_pq_cm_version` and the shots; without shots `l1_avg_pq_cm_version` is written only
    /// when it differs from the CM version, because `dovi_tool` defaults it to the CM version.
    pub fn extra_json(&self, shots: Option<&ExtraShots>) -> Value {
        let level6 = self.level6;
        let mut json_content = json!({
            "profile": self.profile.name(),
            "level6": {
                "max_display_mastering_luminance": level6.max_display_mastering_luminance,
                "min_display_mastering_luminance": level6.min_display_mastering_luminance,
                "max_content_light_level": level6.max_content_light_level,
                "max_frame_average_light_level": level6.max_frame_average_light_level,
            }
        });

        // Profile 8.4 keeps the preset's range: every HLG composer and the HLG measurement assume it.
        if let Profile::P81 {
            source_range: Some((source_min_pq, source_max_pq)),
        } = self.profile
        {
            json_content["source_min_pq"] = json!(source_min_pq);
            json_content["source_max_pq"] = json!(source_max_pq);
        }

        if let Some(offsets) = self.level5 {
            json_content["level5"] = json!({
                "active_area_left_offset": offsets.left,
                "active_area_right_offset": offsets.right,
                "active_area_top_offset": offsets.top,
                "active_area_bottom_offset": offsets.bottom,
            });
        }

        match &self.cm {
            CmMetadata::V40 {
                config,
                trim_targets_nits,
            } => {
                json_content["cm_version"] = json!("V40");
                let mut default_blocks: Vec<Value> = trim_targets_nits
                    .iter()
                    .map(|&target| {
                        json!({
                            "Level2": {
                                "target_max_pq": nits_to_pq_code(target),
                                "trim_slope": NEUTRAL_TRIM,
                                "trim_offset": NEUTRAL_TRIM,
                                "trim_power": NEUTRAL_TRIM,
                                "trim_chroma_weight": NEUTRAL_TRIM,
                                "trim_saturation_gain": NEUTRAL_TRIM,
                                "ms_weight": NEUTRAL_TRIM
                            }
                        })
                    })
                    .collect();
                default_blocks.push(json!({
                    "Level9": {
                        "length": 1,
                        "source_primary_index": config.source_primary_index
                    }
                }));
                default_blocks.push(json!({
                    "Level11": {
                        "content_type": config.content_type,
                        "whitepoint": 0,
                        "reference_mode_flag": config.reference_mode
                    }
                }));
                json_content["default_metadata_blocks"] = json!(default_blocks);
            }
            // `dovi_tool generate` defaults to CM v4.0 and would add its own L3, L9, L11 and
            // L254 (ROADMAP P13), so a CM v2.9 RPU must be asked for explicitly.
            CmMetadata::V29 => json_content["cm_version"] = json!("V29"),
        }

        let l1_avg_name = self.l1_avg_pq_cm_version.json_name();
        if let Some(shots) = shots {
            let shots_json: Vec<Value> = shots
                .shots
                .iter()
                .map(|shot| {
                    json!({
                        "start": shot.start,
                        "duration": shot.duration,
                        "metadata_blocks": [{
                            "Level1": {
                                "min_pq": shot.l1.min_pq,
                                "max_pq": shot.l1.max_pq,
                                "avg_pq": shot.l1.avg_pq,
                            }
                        }],
                    })
                })
                .collect();
            json_content["length"] = json!(shots.length);
            json_content["l1_avg_pq_cm_version"] = json!(l1_avg_name);
            json_content["shots"] = json!(shots_json);
        } else if self.l1_avg_pq_cm_version != self.cm_version() {
            json_content["l1_avg_pq_cm_version"] = json!(l1_avg_name);
        }

        json_content
    }

    /// The default blocks of [`extra_json`](Self::extra_json) as `dolby_vision` blocks, in the
    /// same order.
    pub(crate) fn default_blocks(&self) -> Result<Vec<ExtMetadataBlock>> {
        let CmMetadata::V40 {
            config,
            trim_targets_nits,
        } = &self.cm
        else {
            return Ok(Vec::new());
        };
        if trim_targets_nits.len() > MAX_TRIM_TARGETS {
            return Err(Error::OutOfRange(format!(
                "{} L2 trim targets, at most {MAX_TRIM_TARGETS}",
                trim_targets_nits.len()
            )));
        }
        let mut blocks = Vec::with_capacity(trim_targets_nits.len() + 2);
        for &target in trim_targets_nits {
            let target_max_pq = u16::try_from(nits_to_pq_code(target))
                .map_err(|_| Error::OutOfRange(format!("L2 target {target} nits")))?;
            let level2 = ExtMetadataBlockLevel2 {
                target_max_pq,
                trim_slope: NEUTRAL_TRIM,
                trim_offset: NEUTRAL_TRIM,
                trim_power: NEUTRAL_TRIM,
                trim_chroma_weight: NEUTRAL_TRIM,
                trim_saturation_gain: NEUTRAL_TRIM,
                ms_weight: NEUTRAL_TRIM as i16,
            };
            writable("L2", level2.validate())?;
            blocks.push(ExtMetadataBlock::Level2(level2));
        }
        let level9 = ExtMetadataBlockLevel9 {
            length: 1,
            source_primary_index: config.source_primary_index,
            ..Default::default()
        };
        writable("L9", level9.validate())?;
        blocks.push(ExtMetadataBlock::Level9(level9));
        let level11 = ExtMetadataBlockLevel11 {
            content_type: config.content_type,
            whitepoint: 0,
            reference_mode_flag: config.reference_mode,
            reserved_byte2: 0,
            reserved_byte3: 0,
        };
        writable("L11", level11.validate())?;
        blocks.push(ExtMetadataBlock::Level11(level11));
        Ok(blocks)
    }

    /// The `GenerateConfig` `dovi_tool generate` reads from [`extra_json`](Self::extra_json)
    /// without shots, after its defaulting of `l1_avg_pq_cm_version`; `length` 1. Refuses, as
    /// `Error::OutOfRange`, every value the RPU writer would refuse, and an empty or inverted 8.1
    /// source range.
    pub(crate) fn generate_config(&self) -> Result<GenerateConfig> {
        let (profile, source_range) = match self.profile {
            Profile::P81 { source_range } => (GenerateProfile::Profile81, source_range),
            Profile::P84 { .. } => (GenerateProfile::Profile84, None),
        };
        // The writer refuses a value above 12 bits. It writes an empty or inverted range, but no
        // mastering display has one, and `source_range_81` never returns one.
        if let Some((min, max)) = source_range {
            if max > MAX_12_BIT_VALUE || min >= max {
                return Err(Error::OutOfRange(format!(
                    "8.1 source range {min}..{max}: needs min < max <= {MAX_12_BIT_VALUE}"
                )));
            }
        }
        let level5 = self
            .level5
            .map(|offsets| ExtMetadataBlockLevel5 {
                active_area_left_offset: offsets.left,
                active_area_right_offset: offsets.right,
                active_area_top_offset: offsets.top,
                active_area_bottom_offset: offsets.bottom,
            })
            .unwrap_or_default();
        writable("L5", level5.validate())?;
        Ok(GenerateConfig {
            cm_version: self.cm_version().to_crate(),
            profile,
            long_play_mode: false,
            length: 1,
            source_min_pq: source_range.map(|range| range.0),
            source_max_pq: source_range.map(|range| range.1),
            l1_avg_pq_cm_version: Some(self.l1_avg_pq_cm_version.to_crate()),
            level5,
            level6: Some(self.level6.to_block()?),
            level254: None,
            default_metadata_blocks: self.default_blocks()?,
            shots: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(cm: CmMetadata, l1_avg_pq_cm_version: CmVersion) -> RpuParams {
        RpuParams {
            profile: Profile::P81 { source_range: None },
            cm,
            level6: Level6::mkvdovi_default(),
            level5: None,
            l1_avg_pq_cm_version,
        }
    }

    fn shots() -> ExtraShots {
        ExtraShots {
            length: 2,
            shots: vec![ExtraShot {
                start: 0,
                duration: 2,
                l1: L1::new(0, 1500, 3000),
            }],
        }
    }

    #[test]
    fn the_avg_cm_version_is_written_with_shots_or_when_it_is_not_the_default() {
        let key = |params: &RpuParams, shots: Option<&ExtraShots>| {
            params
                .extra_json(shots)
                .get("l1_avg_pq_cm_version")
                .cloned()
        };
        let v40_default = params(CmMetadata::mkvdovi_default(), CmVersion::V40);
        assert_eq!(key(&v40_default, None), None);
        assert_eq!(key(&v40_default, Some(&shots())), Some(json!("V40")));
        let v40_low_floor = params(CmMetadata::mkvdovi_default(), CmVersion::V29);
        assert_eq!(key(&v40_low_floor, None), Some(json!("V29")));
        let v29 = params(CmMetadata::V29, CmVersion::V29);
        assert_eq!(key(&v29, None), None);
        assert_eq!(key(&v29, Some(&shots())), Some(json!("V29")));
    }

    #[test]
    fn the_source_range_is_written_for_profile_81_only() {
        let mut p81 = params(CmMetadata::V29, CmVersion::V29);
        p81.profile = Profile::P81 {
            source_range: Some((7, 3696)),
        };
        let json = p81.extra_json(None);
        assert_eq!(
            (json["source_min_pq"].clone(), json["source_max_pq"].clone()),
            (json!(7), json!(3696))
        );
        let without = params(CmMetadata::V29, CmVersion::V29).extra_json(None);
        assert!(without.get("source_min_pq").is_none());
        let mut p84 = p81.clone();
        p84.profile = Profile::P84 {
            composer: HlgComposer::Bt2100,
        };
        let json = p84.extra_json(None);
        assert_eq!(json["profile"], "8.4");
        assert!(json.get("source_min_pq").is_none() && json.get("source_max_pq").is_none());
    }

    #[test]
    fn level6_truncates_like_mkvdovi() {
        let l6 = Level6::from_nits(0.0001, 999.9, 997.9, 91.6);
        assert_eq!(
            (
                l6.min_display_mastering_luminance,
                l6.max_display_mastering_luminance,
                l6.max_content_light_level,
                l6.max_frame_average_light_level
            ),
            (1, 999, 997, 91)
        );
        assert_eq!(
            Level6::mkvdovi_default().min_display_mastering_luminance,
            50
        );
    }

    #[test]
    fn level6_clamped_caps_each_field_at_10000() {
        let sei = Level6 {
            max_display_mastering_luminance: 10_001,
            min_display_mastering_luminance: 50,
            max_content_light_level: 65_535,
            max_frame_average_light_level: 10_000,
        };
        assert_eq!(
            sei.clamped(),
            Level6 {
                max_display_mastering_luminance: 10_000,
                min_display_mastering_luminance: 50,
                max_content_light_level: 10_000,
                max_frame_average_light_level: 10_000,
            }
        );
        assert!(sei.to_block().is_err());
        assert!(sei.clamped().to_block().is_ok());
        let default = Level6::mkvdovi_default();
        assert_eq!(default.clamped(), default);
    }

    #[test]
    fn the_81_source_range_needs_a_plausible_mastering_display() {
        assert_eq!(source_range_81(0.005, 1000.0), Some((62, 3079)));
        assert_eq!(source_range_81(0.0001, 1000.0), Some((7, 3079)));
        assert_eq!(source_range_81(0.0001, 99.0), None);
        assert_eq!(source_range_81(0.0001, 10_001.0), None);
        assert_eq!(source_range_81(1.5, 1000.0), None);
        assert_eq!(source_range_81(f64::NAN, 1000.0), None);
        assert_eq!(source_range_84(), (62, 3079));
    }

    #[test]
    fn l2_targets_and_cm_v29_blocks() {
        assert_eq!([100, 600, 1000].map(nits_to_pq_code), [2081, 2851, 3079]);
        assert!(params(CmMetadata::V29, CmVersion::V29)
            .default_blocks()
            .unwrap()
            .is_empty());
        let levels: Vec<u8> = params(CmMetadata::mkvdovi_default(), CmVersion::V29)
            .default_blocks()
            .unwrap()
            .iter()
            .map(ExtMetadataBlock::level)
            .collect();
        assert_eq!(levels, [2, 2, 2, 9, 11]);
    }
}
