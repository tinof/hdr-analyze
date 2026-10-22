//! Static Profile 8.4 RPUs: one L1 for a whole HLG title, for players that cannot measure it.
//!
//! A new policy, not a port: mkvdovi refuses HLG without L1 measured through the selected
//! composer (`mkvdovi/src/metadata/rpu_config.rs`, `dv_profile_for`), and that rule stays.

use crate::builder::L1;
use crate::composer::HlgComposer;
use crate::params::{CmMetadata, CmVersion, Level6, Profile, RpuParams};

/// The nominal L1 of a static 8.4 RPU (owner decision, 2026-10-22).
///
/// - `max_pq` 3079 (about 1000 nits): the 8.4 source-range ceiling ([`source_range_84`]), the peak
///   of a 1000-nit HLG system, not a measured value. The 65 measured scenes of the four HLG
///   development cuts peak at 2293 to 3079, 36 of them at 3079.
/// - `avg_pq` 1283 (about 11.8 nits): the frame-weighted mean of the measured scene averages of
///   the four HLG development cuts (`--hlg-composer bt2100`, mapping
///   `dovi84-bt2100-v1-spec420`, accurate sampling, CUDA; 9160 frames: Wimbledon 2024 final rally
///   874.9, Green Planet tropical canopy 1377.1, Glastonbury 2025 Supergrass stage 1511.9, UCL
///   Bayern–PSG match action 1528.2; pooled 1282.67, rounded). Four cuts, mostly sport, so a
///   conservative middle value rather than a model of HLG content.
/// - `min_pq` 0, which the generator's clamp keeps (at most 12).
///
/// [`source_range_84`]: crate::source_range_84
pub const STATIC_84_L1: L1 = L1 {
    min_pq: 0,
    avg_pq: 1283,
    max_pq: 3079,
};

/// The L1 average floor of a static 8.4 RPU: V29 (819), as for every measured 8.4 RPU mkvdovi
/// delivers. `STATIC_84_L1.avg_pq` is above both floors, so the choice changes no value.
pub const STATIC_84_L1_AVG_CM_VERSION: CmVersion = CmVersion::V29;

/// What mkvdovi would write for an HLG title apart from L1: Profile 8.4 with `composer`, CM v4.0
/// with its default L2/L9/L11 blocks, the default L6 HLG sources get (1000 / 0.005 nits
/// mastering, MaxCLL 1000, MaxFALL 400), no L5, and [`STATIC_84_L1_AVG_CM_VERSION`]. Use with
/// [`STATIC_84_L1`].
pub fn static_84_params(composer: HlgComposer) -> RpuParams {
    RpuParams {
        profile: Profile::P84 { composer },
        cm: CmMetadata::mkvdovi_default(),
        level6: Level6::mkvdovi_default(),
        level5: None,
        l1_avg_pq_cm_version: STATIC_84_L1_AVG_CM_VERSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build, RpuBuilder};

    #[test]
    fn the_static_l1_passes_the_generator_clamp_unchanged() {
        assert_eq!(
            STATIC_84_L1.clamped(STATIC_84_L1_AVG_CM_VERSION),
            STATIC_84_L1
        );
        // Above both avg floors, so the CM version choice changes no value.
        assert_eq!(STATIC_84_L1.clamped(CmVersion::V40), STATIC_84_L1);
        assert_eq!(STATIC_84_L1.max_pq, crate::source_range_84().1);
    }

    #[test]
    fn static_84_params_build_for_every_composer() {
        for composer in [
            HlgComposer::Preset,
            HlgComposer::Bt2100,
            HlgComposer::canary(),
        ] {
            let params = static_84_params(composer);
            assert_eq!(params.profile, Profile::P84 { composer });
            let shot = RpuBuilder::new(&params)
                .unwrap()
                .shot(STATIC_84_L1)
                .unwrap();
            assert_eq!(shot.l1(), STATIC_84_L1);
            assert!(build(&params, STATIC_84_L1, true).is_ok());
        }
    }
}
