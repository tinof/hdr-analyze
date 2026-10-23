//! One complete RPU NAL per access unit.
//!
//! [`RpuBuilder::new`] makes the base RPU once per [`RpuParams`] through the `dolby_vision`
//! crate's own Profile 8 constructors (`DoviRpu::profile81_config` / `profile84_config`, which add
//! the CM v4.0 default blocks and the source levels), [`RpuBuilder::shot`] adds one L1 per shot,
//! and [`ShotRpu::write`] writes one RPU per access unit: CRC and emulation prevention are done by
//! the crate on every write, so a changed field is always a newly written RPU, never a patch of
//! escaped bytes. The bytes equal what `dovi_tool generate` writes for the same `extra.json`
//! (`GenerateConfig::generate_rpu_list`, which clones the same base, sets the scene cut on a
//! shot's first frame and replaces its L1).
//!
//! Not supported: `long_play_mode` (scene refresh on every frame; pass `scene_refresh: true`
//! instead) and per-frame edits inside a shot (build another shot). Which frames start a shot,
//! and holding L1 for the rest of it, is the caller's job.

use dolby_vision::rpu::dovi_rpu::DoviRpu;
use dolby_vision::rpu::extension_metadata::blocks::{ExtMetadataBlock, ExtMetadataBlockLevel1};
use dolby_vision::rpu::generate::GenerateConfig;
use dovi84_composer::COEFFICIENT_LOG2_DENOM;

use crate::error::{Error, Result};
use crate::params::{CmVersion, Profile, RpuParams};

/// HEVC NAL unit header of an RPU (`UNSPEC62`): the first two bytes of every RPU this crate
/// returns.
pub const RPU_NAL_HEADER: [u8; 2] = [0x7C, 0x01];

/// L1 (per-shot luminance) in 12-bit PQ codes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct L1 {
    pub min_pq: u16,
    pub avg_pq: u16,
    pub max_pq: u16,
}

impl L1 {
    pub fn new(min_pq: u16, avg_pq: u16, max_pq: u16) -> Self {
        Self {
            min_pq,
            avg_pq,
            max_pq,
        }
    }

    /// Clamped as `dovi_tool generate` clamps L1 (`GenerateConfig::fixup_l1`): min at most 12,
    /// max within 2081..=4095, avg within the floor of `avg_cm_version` (819 under V29, 1229
    /// under V40) and max − 1.
    pub fn clamped(self, avg_cm_version: CmVersion) -> Self {
        let block = ExtMetadataBlockLevel1::from_stats_cm_version(
            self.min_pq,
            self.max_pq,
            self.avg_pq,
            avg_cm_version.to_crate(),
        );
        Self {
            min_pq: block.min_pq,
            avg_pq: block.avg_pq,
            max_pq: block.max_pq,
        }
    }
}

/// Levels a CM v2.9 RPU can carry, and those that need CM v4.0 DM data
/// (`CmV29DmData::ALLOWED_BLOCK_LEVELS` / `CmV40DmData::ALLOWED_BLOCK_LEVELS`).
const CMV29_LEVELS: [u8; 6] = [1, 2, 4, 5, 6, 255];
const CMV40_LEVELS: [u8; 6] = [3, 8, 9, 10, 11, 254];

/// Refuse a block the crate would have to place in DM data the RPU does not have. The crate
/// answers some of these with `unreachable!` (`VdrDmData::replace_metadata_block`), which aborts
/// the process under `panic = "abort"`.
fn check_block_levels(cm_version: CmVersion, blocks: &[ExtMetadataBlock]) -> Result<()> {
    for block in blocks {
        let level = block.level();
        let fits = CMV29_LEVELS.contains(&level)
            || (cm_version == CmVersion::V40 && CMV40_LEVELS.contains(&level));
        if matches!(block, ExtMetadataBlock::Reserved(_)) || !fits {
            return Err(Error::BlockLevel(format!(
                "level {level} cannot be carried by a {cm_version:?} RPU"
            )));
        }
    }
    Ok(())
}

/// The base RPU of one [`RpuParams`], without L1 and with `scene_refresh_flag` 0.
#[derive(Debug, Clone)]
pub struct RpuBuilder {
    base: DoviRpu,
    l1_avg_pq_cm_version: CmVersion,
}

impl RpuBuilder {
    pub fn new(params: &RpuParams) -> Result<Self> {
        let config: GenerateConfig = params.generate_config()?;
        check_block_levels(params.cm_version(), &config.default_metadata_blocks)?;
        let mut base = match params.profile {
            Profile::P81 { .. } => DoviRpu::profile81_config(&config),
            Profile::P84 { .. } => DoviRpu::profile84_config(&config),
        }
        .map_err(Error::rpu)?;
        if let Profile::P84 { composer } = params.profile {
            // `profile84_config` installs the preset. The composers are quantized to the header's
            // coefficient denominator, so the header must carry it.
            let denom = u32::try_from(base.header.coefficient_log2_denom).ok();
            if base.header.coefficient_data_type != 0 || denom != Some(COEFFICIENT_LOG2_DENOM) {
                return Err(Error::InvalidComposer(format!(
                    "the Profile 8 header uses coefficient_log2_denom {} (data type {}), the \
                     composers need {COEFFICIENT_LOG2_DENOM} (fixed point)",
                    base.header.coefficient_log2_denom, base.header.coefficient_data_type
                )));
            }
            base.rpu_data_mapping = Some(composer.rpu_data_mapping()?);
            base.modified = true;
        }
        let dm = base
            .vdr_dm_data
            .as_mut()
            .ok_or_else(|| Error::Rpu("the Profile 8 RPU has no DM data".into()))?;
        dm.set_scene_cut(false);
        // The checks above refuse bad input as a typed error. This write is the backstop: it
        // proves every later `ShotRpu::write` succeeds (a shot only adds a clamped, writable L1),
        // and a failure here is an internal one, `Error::Rpu`.
        base.write_hevc_unspec62_nalu().map_err(Error::rpu)?;
        Ok(Self {
            base,
            l1_avg_pq_cm_version: params.l1_avg_pq_cm_version,
        })
    }

    /// The RPU of one shot: the base with `l1`, clamped as `dovi_tool` clamps it
    /// ([`L1::clamped`] with the params' `l1_avg_pq_cm_version`).
    pub fn shot(&self, l1: L1) -> Result<ShotRpu> {
        let l1 = l1.clamped(self.l1_avg_pq_cm_version);
        let mut rpu = self.base.clone();
        rpu.vdr_dm_data
            .as_mut()
            .ok_or_else(|| Error::Rpu("the Profile 8 RPU has no DM data".into()))?
            .replace_metadata_block(ExtMetadataBlock::Level1(ExtMetadataBlockLevel1::new(
                l1.min_pq, l1.max_pq, l1.avg_pq,
            )))
            .map_err(Error::rpu)?;
        Ok(ShotRpu { rpu, l1 })
    }
}

/// The RPU of one shot, written once per access unit.
#[derive(Debug, Clone)]
pub struct ShotRpu {
    rpu: DoviRpu,
    l1: L1,
}

impl ShotRpu {
    /// The L1 this shot's RPUs carry (after clamping).
    pub fn l1(&self) -> L1 {
        self.l1
    }

    /// One RPU NAL unit: [`RPU_NAL_HEADER`] followed by the payload with emulation prevention,
    /// with no start code and no length prefix. `scene_refresh` sets `scene_refresh_flag`, which
    /// `dovi_tool` sets on the first frame of each shot only.
    pub fn write(&mut self, scene_refresh: bool) -> Result<Vec<u8>> {
        self.rpu
            .vdr_dm_data
            .as_mut()
            .ok_or_else(|| Error::Rpu("the Profile 8 RPU has no DM data".into()))?
            .set_scene_cut(scene_refresh);
        self.rpu.write_hevc_unspec62_nalu().map_err(Error::rpu)
    }
}

/// One RPU NAL for `params` with `l1`, in one call. A caller that writes many access units of a
/// shot keeps the [`ShotRpu`] instead, so each access unit costs one write.
pub fn build(params: &RpuParams, l1: L1, scene_refresh: bool) -> Result<Vec<u8>> {
    RpuBuilder::new(params)?.shot(l1)?.write(scene_refresh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composer::HlgComposer;
    use crate::params::{CmMetadata, CmV40Config, Level5, Level6};
    use dolby_vision::rpu::extension_metadata::blocks::{
        ExtMetadataBlockLevel2, ExtMetadataBlockLevel9, ReservedExtMetadataBlock,
    };
    use dovi84_composer::{mappings_equal, Composer};

    fn params(profile: Profile, cm: CmMetadata) -> RpuParams {
        RpuParams {
            profile,
            cm,
            level6: Level6::mkvdovi_default(),
            level5: None,
            l1_avg_pq_cm_version: CmVersion::V29,
        }
    }

    fn p81() -> RpuParams {
        params(
            Profile::P81 { source_range: None },
            CmMetadata::mkvdovi_default(),
        )
    }

    #[test]
    fn l1_inside_the_limits_is_kept() {
        let l1 = L1::new(12, 1229, 2081);
        assert_eq!(l1.clamped(CmVersion::V40), l1);
        let l1 = L1::new(0, 819, 4095);
        assert_eq!(l1.clamped(CmVersion::V29), l1);
    }

    #[test]
    fn l1_is_clamped_like_dovi_tool() {
        // min at most 12, max 2081..=4095.
        assert_eq!(L1::new(13, 1500, 3000).clamped(CmVersion::V29).min_pq, 12);
        assert_eq!(L1::new(0, 1500, 2080).clamped(CmVersion::V29).max_pq, 2081);
        assert_eq!(L1::new(0, 1500, 4096).clamped(CmVersion::V29).max_pq, 4095);
        // The avg floor is the selected CM version's.
        assert_eq!(L1::new(0, 818, 3000).clamped(CmVersion::V29).avg_pq, 819);
        assert_eq!(L1::new(0, 1228, 3000).clamped(CmVersion::V29).avg_pq, 1228);
        assert_eq!(L1::new(0, 1228, 3000).clamped(CmVersion::V40).avg_pq, 1229);
        // avg stays below max.
        assert_eq!(L1::new(0, 3000, 3000).clamped(CmVersion::V29).avg_pq, 2999);
        assert_eq!(L1::new(0, 2999, 3000).clamped(CmVersion::V29).avg_pq, 2999);
    }

    #[test]
    fn a_shot_carries_the_clamped_l1() {
        let shot = RpuBuilder::new(&p81())
            .unwrap()
            .shot(L1::new(20, 500, 2000))
            .unwrap();
        assert_eq!(shot.l1(), L1::new(12, 819, 2081));
    }

    #[test]
    fn block_levels_follow_the_cm_version() {
        let l2 = ExtMetadataBlock::Level2(ExtMetadataBlockLevel2::default());
        let l9 = ExtMetadataBlock::Level9(ExtMetadataBlockLevel9::default());
        assert!(check_block_levels(CmVersion::V29, std::slice::from_ref(&l2)).is_ok());
        assert!(check_block_levels(CmVersion::V40, &[l2, l9.clone()]).is_ok());
        assert!(matches!(
            check_block_levels(CmVersion::V29, &[l9]),
            Err(Error::BlockLevel(_))
        ));
        let reserved = ExtMetadataBlock::Reserved(ReservedExtMetadataBlock::default());
        assert!(check_block_levels(CmVersion::V40, &[reserved]).is_err());
    }

    #[test]
    fn the_scene_refresh_flag_is_set_on_every_write() {
        let mut shot = RpuBuilder::new(&p81())
            .unwrap()
            .shot(L1::new(0, 1500, 3000))
            .unwrap();
        let first = shot.write(true).unwrap();
        let held = shot.write(false).unwrap();
        let again = shot.write(true).unwrap();
        assert_ne!(first, held);
        assert_eq!(first, again);
        assert_eq!(held, shot.write(false).unwrap());
        assert_eq!(first[..2], RPU_NAL_HEADER);
        let parse = |nal: &[u8]| DoviRpu::parse_unspec62_nalu(nal).unwrap();
        let flag = |nal: &[u8]| parse(nal).vdr_dm_data.unwrap().scene_refresh_flag;
        assert_eq!((flag(&first), flag(&held)), (1, 0));
    }

    #[test]
    fn profile_84_carries_the_selected_composer() {
        for (composer, expected) in [
            (HlgComposer::Preset, Composer::Preset.rpu_data_mapping()),
            (HlgComposer::Bt2100, Composer::Bt2100V1.rpu_data_mapping()),
            (
                HlgComposer::canary(),
                dovi84_composer::canary_mapping(dovi84_composer::DEFAULT_CANARY_OFFSET).unwrap(),
            ),
        ] {
            let nal = build(
                &params(Profile::P84 { composer }, CmMetadata::V29),
                L1::new(0, 1283, 3079),
                true,
            )
            .unwrap();
            let rpu = DoviRpu::parse_unspec62_nalu(&nal).unwrap();
            assert!(
                mappings_equal(rpu.rpu_data_mapping.as_ref().unwrap(), &expected),
                "{composer:?}"
            );
            let dm = rpu.vdr_dm_data.unwrap();
            assert_eq!((dm.source_min_pq, dm.source_max_pq), (62, 3079));
        }
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        let canary = params(
            Profile::P84 {
                composer: HlgComposer::Canary { offset: f64::NAN },
            },
            CmMetadata::V29,
        );
        assert!(matches!(
            RpuBuilder::new(&canary),
            Err(Error::InvalidComposer(_))
        ));

        let mut too_bright = p81();
        too_bright.level6 = Level6::from_nits(0.0001, 70_000.0, 1000.0, 400.0);
        assert!(matches!(
            RpuBuilder::new(&too_bright),
            Err(Error::OutOfRange(_))
        ));

        let mut far_target = p81();
        far_target.cm = CmMetadata::V40 {
            config: Default::default(),
            trim_targets_nits: vec![u32::MAX],
        };
        // nits_to_pq_code caps at 10 000 nits, so even this target fits.
        assert!(RpuBuilder::new(&far_target).is_ok());
    }

    fn with_level6(edit: impl Fn(&mut Level6)) -> RpuParams {
        let mut params = p81();
        edit(&mut params.level6);
        params
    }

    fn with_level5(offset: u16) -> RpuParams {
        let mut params = p81();
        params.level5 = Some(Level5 {
            left: offset,
            right: offset,
            top: offset,
            bottom: offset,
        });
        params
    }

    fn with_v40(source_primary_index: u8, content_type: u8, targets: usize) -> RpuParams {
        let mut params = p81();
        params.cm = CmMetadata::V40 {
            config: CmV40Config {
                source_primary_index,
                content_type,
                reference_mode: false,
            },
            trim_targets_nits: (0..targets as u32).map(|i| 100 + 100 * i).collect(),
        };
        params
    }

    fn with_source_range(min: u16, max: u16) -> RpuParams {
        let mut params = p81();
        params.profile = Profile::P81 {
            source_range: Some((min, max)),
        };
        params
    }

    // The limits are the `dolby_vision` 3.4 writer's: `ExtMetadataBlockLevel6::validate` (10 000),
    // `ExtMetadataBlockLevel5::validate` (8191), `ExtMetadataBlockLevel9::validate` (index 255
    // only with custom primaries), `ExtMetadataBlockLevel11::validate` (15), `CmV29DmData::validate`
    // (8 L2) and the 12-bit source range fields. NuvioTV fork review (2026-10-23): before E16.2
    // `new` accepted every refused case below, and every later `write` failed with `Error::Rpu`.
    #[test]
    fn new_refuses_values_the_rpu_cannot_carry() {
        let refused = [
            (
                "L6 max_dml 10001",
                with_level6(|l6| l6.max_display_mastering_luminance = 10_001),
            ),
            (
                "L6 min_dml 10001",
                with_level6(|l6| l6.min_display_mastering_luminance = 10_001),
            ),
            (
                "L6 MaxCLL 10001",
                with_level6(|l6| l6.max_content_light_level = 10_001),
            ),
            (
                "L6 MaxFALL 10001",
                with_level6(|l6| l6.max_frame_average_light_level = 10_001),
            ),
            (
                "L6 MaxCLL 65535",
                with_level6(|l6| l6.max_content_light_level = 65_535),
            ),
            ("L5 8192", with_level5(8192)),
            ("L9 index 255", with_v40(255, 1, 3)),
            ("L11 content type 16", with_v40(2, 16, 3)),
            ("9 trim targets", with_v40(2, 1, 9)),
            ("source max 4096", with_source_range(7, 4096)),
            ("inverted source range", with_source_range(3079, 62)),
            ("empty source range", with_source_range(3000, 3000)),
        ];
        for (case, params) in refused {
            let result = RpuBuilder::new(&params).map(|_| ());
            assert!(
                matches!(result, Err(Error::OutOfRange(_))),
                "{case}: {result:?}"
            );
        }
    }

    #[test]
    fn values_at_the_limits_are_written() {
        let accepted = [
            (
                "L6 at 10000",
                with_level6(|l6| {
                    *l6 = Level6 {
                        max_display_mastering_luminance: 10_000,
                        min_display_mastering_luminance: 10_000,
                        max_content_light_level: 10_000,
                        max_frame_average_light_level: 10_000,
                    }
                }),
            ),
            ("L5 8191", with_level5(8191)),
            (
                "L9 index 0, L11 content type 15, 8 trim targets",
                with_v40(0, 15, 8),
            ),
            ("source range 0..4095", with_source_range(0, 4095)),
            ("source range 3078..3079", with_source_range(3078, 3079)),
        ];
        for (case, params) in accepted {
            let builder = RpuBuilder::new(&params).unwrap_or_else(|e| panic!("{case}: {e}"));
            let mut shot = builder.shot(L1::new(0, 1500, 3000)).unwrap();
            for scene_refresh in [true, false] {
                let nal = shot
                    .write(scene_refresh)
                    .unwrap_or_else(|e| panic!("{case}: {e}"));
                assert_eq!(nal[..2], RPU_NAL_HEADER, "{case}");
            }
        }
    }
}
