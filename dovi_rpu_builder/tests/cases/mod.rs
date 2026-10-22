//! The fixture cases, one definition for the tests of this crate and for
//! `tools/rpu_builder_check` (which includes this file with `#[path]`), the tool that writes the
//! `extra.json` files `tests/fixtures/make_fixtures.sh` feeds to `dovi_tool generate`.
#![allow(dead_code)]

use dovi_rpu_builder::{
    source_range_81, static_84_params, CmMetadata, CmV40Config, CmVersion, ExtraShot, ExtraShots,
    HlgComposer, Level5, Level6, PeakSource, Profile, RpuParams, L1, STATIC_84_L1,
};

/// What turns `dovi_tool generate`'s output into the case's expected RPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Post {
    /// `dovi_tool`'s output as is.
    None,
    /// `fit_hlg_composer rewrite-rpu bt2100` (mkvdovi's composer rewrite).
    Bt2100,
    /// The bt2100 result, then `fit_hlg_composer canary-rpu` at the default offset.
    Canary,
}

pub struct ShotCase {
    pub name: &'static str,
    pub params: RpuParams,
    /// (frames, L1) per shot, unclamped.
    pub shots: Vec<(u64, L1)>,
    pub post: Post,
}

impl ShotCase {
    pub fn extra_shots(&self) -> ExtraShots {
        let mut start = 0;
        let shots = self
            .shots
            .iter()
            .map(|&(duration, l1)| {
                let shot = ExtraShot {
                    start,
                    duration,
                    l1,
                };
                start += duration;
                shot
            })
            .collect();
        ExtraShots {
            length: start,
            shots,
        }
    }
}

/// Three shots: one inside every limit, one below every floor (min above 12, max below 2081, avg
/// below both floors), one with avg not below max.
fn clamping_shots() -> Vec<(u64, L1)> {
    vec![
        (3, L1::new(0, 900, 2800)),
        (2, L1::new(20, 500, 2000)),
        (2, L1::new(5, 3000, 3000)),
    ]
}

fn hlg_cm() -> CmMetadata {
    CmMetadata::V40 {
        config: CmV40Config {
            source_primary_index: 2,
            content_type: 3,
            reference_mode: true,
        },
        trim_targets_nits: vec![100, 600],
    }
}

fn p84_shots(name: &'static str, composer: HlgComposer, post: Post) -> ShotCase {
    ShotCase {
        name,
        params: RpuParams {
            profile: Profile::P84 { composer },
            cm: hlg_cm(),
            level6: Level6::mkvdovi_default(),
            level5: None,
            l1_avg_pq_cm_version: CmVersion::V29,
        },
        shots: clamping_shots(),
        post,
    }
}

pub fn shot_cases() -> Vec<ShotCase> {
    vec![
        ShotCase {
            name: "p81_v40_shots",
            params: RpuParams {
                profile: Profile::P81 {
                    source_range: source_range_81(0.0001, 1000.0),
                },
                cm: CmMetadata::mkvdovi_default(),
                level6: Level6::from_nits(0.0001, 1000.0, 997.9, 91.6),
                level5: Some(Level5 {
                    left: 0,
                    right: 4,
                    top: 140,
                    bottom: 136,
                }),
                l1_avg_pq_cm_version: CmVersion::V29,
            },
            shots: clamping_shots(),
            post: Post::None,
        },
        ShotCase {
            name: "p81_v40_shots_avg_v40",
            params: RpuParams {
                profile: Profile::P81 {
                    source_range: source_range_81(0.005, 4000.0),
                },
                cm: CmMetadata::V40 {
                    config: CmV40Config {
                        source_primary_index: 0,
                        content_type: 4,
                        reference_mode: false,
                    },
                    trim_targets_nits: vec![100, 600, 1000],
                },
                level6: Level6::from_nits(0.005, 4000.0, 1500.0, 350.0),
                level5: None,
                l1_avg_pq_cm_version: CmVersion::V40,
            },
            shots: clamping_shots(),
            post: Post::None,
        },
        ShotCase {
            name: "p81_v29_shots",
            params: RpuParams {
                profile: Profile::P81 { source_range: None },
                cm: CmMetadata::V29,
                level6: Level6::mkvdovi_default(),
                level5: Some(Level5 {
                    left: 0,
                    right: 0,
                    top: 276,
                    bottom: 276,
                }),
                l1_avg_pq_cm_version: CmVersion::V29,
            },
            shots: clamping_shots(),
            post: Post::None,
        },
        p84_shots("p84_preset_shots", HlgComposer::Preset, Post::None),
        p84_shots("p84_bt2100_shots", HlgComposer::Bt2100, Post::Bt2100),
        p84_shots("p84_canary_shots", HlgComposer::canary(), Post::Canary),
        ShotCase {
            name: "static_81",
            params: RpuParams {
                profile: Profile::P81 {
                    source_range: source_range_81(0.005, 1000.0),
                },
                cm: CmMetadata::mkvdovi_default(),
                level6: Level6::from_nits(0.005, 1000.0, 1000.0, 400.0),
                level5: None,
                l1_avg_pq_cm_version: CmVersion::V40,
            },
            shots: vec![(4, L1::new(0, 1500, 3200))],
            post: Post::None,
        },
        ShotCase {
            name: "static_84_bt2100",
            params: static_84_params(HlgComposer::Bt2100),
            shots: vec![(4, STATIC_84_L1)],
            post: Post::Bt2100,
        },
        ShotCase {
            name: "static_84_canary",
            params: static_84_params(HlgComposer::canary()),
            shots: vec![(4, STATIC_84_L1)],
            post: Post::Canary,
        },
    ]
}

/// The HDR10+ sequences (payloads and JSON written by `rpu_builder_check hdr10plus-fixtures`).
pub const HDR10PLUS_SEQUENCES: [&str; 2] = ["profile_a", "profile_b"];

pub const PEAK_SOURCES: [(PeakSource, &str); 4] = [
    (PeakSource::Histogram, "histogram"),
    (PeakSource::Histogram99, "histogram99"),
    (PeakSource::MaxScl, "max-scl"),
    (PeakSource::MaxSclLuminance, "max-scl-luminance"),
];

/// The RPU parameters mkvdovi writes for an HDR10+ input under `cm` (no sidecar, so the L1
/// average floor is the CM version's).
pub fn hdr10plus_params(cm: CmVersion) -> RpuParams {
    let cm = match cm {
        CmVersion::V29 => CmMetadata::V29,
        CmVersion::V40 => CmMetadata::mkvdovi_default(),
    };
    let l1_avg_pq_cm_version = cm.cm_version();
    RpuParams {
        profile: Profile::P81 {
            source_range: source_range_81(0.0001, 1000.0),
        },
        cm,
        level6: Level6::from_nits(0.0001, 1000.0, 1000.0, 400.0),
        level5: None,
        l1_avg_pq_cm_version,
    }
}

pub fn cm_name(cm: CmVersion) -> &'static str {
    match cm {
        CmVersion::V29 => "v29",
        CmVersion::V40 => "v40",
    }
}
