//! The config reading below follows dovi_tool 2.3.4 `src/dovi/generator.rs`, Copyright (c) 2026
//! quietvoid, MIT License; the full notice is in `NOTICE` at the repository root.
//!
//! `RpuParams::extra_json` read the way `dovi_tool generate` 2.3.4 reads a config
//! (`src/dovi/generator.rs`: serde into `GenerateConfig`, `l1_avg_pq_cm_version` defaulted to
//! the CM version, `fixup_l1`, `generate_rpu_list`) gives the same bytes as the in-process
//! builder, over a grid of parameters. No `dovi_tool` needed; the fixtures and the live test in
//! `dovi_rpu_builder` check the binary itself.

use dolby_vision::rpu::generate::GenerateConfig;
use dovi_rpu_builder::{
    source_range_81, CmMetadata, CmV40Config, CmVersion, ExtraShot, ExtraShots, HlgComposer,
    Level5, Level6, Profile, RpuBuilder, RpuParams, L1,
};

use crate::cases;

fn dovi_tool_rpus(params: &RpuParams, shots: &ExtraShots) -> Vec<Vec<u8>> {
    let json = params.extra_json(Some(shots));
    let mut config: GenerateConfig = serde_json::from_value(json).unwrap();
    config.l1_avg_pq_cm_version.get_or_insert(config.cm_version);
    config.fixup_l1();
    config
        .generate_rpu_list()
        .unwrap()
        .iter()
        .map(|rpu| rpu.write_hevc_unspec62_nalu().unwrap())
        .collect()
}

fn builder_rpus(params: &RpuParams, shots: &ExtraShots) -> Vec<Vec<u8>> {
    let builder = RpuBuilder::new(params).unwrap();
    let mut out = Vec::new();
    for shot in &shots.shots {
        let mut rpu = builder.shot(shot.l1).unwrap();
        for frame in 0..shot.duration {
            out.push(rpu.write(frame == 0).unwrap());
        }
    }
    out
}

fn shots(l1s: &[L1]) -> ExtraShots {
    let mut start = 0;
    let shots = l1s
        .iter()
        .enumerate()
        .map(|(index, &l1)| {
            let duration = 1 + index as u64 % 3;
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

#[test]
fn extra_json_through_dovi_tools_reader_equals_the_builder() {
    let l1s = [
        L1::new(0, 0, 0),
        L1::new(0, 900, 2800),
        L1::new(20, 500, 2000),
        L1::new(5, 3000, 3000),
        L1::new(12, 1229, 2081),
        L1::new(4095, 4095, 4095),
    ];
    let profiles = [
        Profile::P81 { source_range: None },
        Profile::P81 {
            source_range: source_range_81(0.0001, 4000.0),
        },
        Profile::P84 {
            composer: HlgComposer::Preset,
        },
    ];
    let cms = [
        CmMetadata::V29,
        CmMetadata::mkvdovi_default(),
        CmMetadata::V40 {
            config: CmV40Config {
                source_primary_index: 1,
                content_type: 0,
                reference_mode: true,
            },
            trim_targets_nits: vec![],
        },
        CmMetadata::V40 {
            config: CmV40Config::default(),
            trim_targets_nits: vec![48, 300, 600, 1000, 2000],
        },
    ];
    let level5s = [
        None,
        Some(Level5 {
            left: 1,
            right: 2,
            top: 3,
            bottom: 4,
        }),
    ];
    let level6s = [
        Level6::mkvdovi_default(),
        Level6::from_nits(0.0001, 4000.0, 0.0, 0.0),
    ];
    let mut checked = 0;
    for profile in profiles {
        for cm in &cms {
            for level5 in level5s {
                for level6 in level6s {
                    for l1_avg_pq_cm_version in [CmVersion::V29, CmVersion::V40] {
                        let params = RpuParams {
                            profile,
                            cm: cm.clone(),
                            level6,
                            level5,
                            l1_avg_pq_cm_version,
                        };
                        let shots = shots(&l1s);
                        assert_eq!(
                            builder_rpus(&params, &shots),
                            dovi_tool_rpus(&params, &shots),
                            "{params:?}"
                        );
                        checked += 1;
                    }
                }
            }
        }
    }
    assert_eq!(checked, 3 * 4 * 2 * 2 * 2);
}

#[test]
fn fixture_cases_through_dovi_tools_reader_equal_the_builder() {
    // The composer is installed after generation (`Post`), so only the preset cases compare here.
    for case in cases::shot_cases() {
        if case.post != cases::Post::None {
            continue;
        }
        let shots = case.extra_shots();
        assert_eq!(
            builder_rpus(&case.params, &shots),
            dovi_tool_rpus(&case.params, &shots),
            "{}",
            case.name
        );
    }
}
