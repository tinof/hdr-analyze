//! The playback manifest against the fixture cases (ROADMAP P15.2): its `rpu` object round-trips
//! every case's params, and RPUs built from a manifest through `AuMatcher` equal `dovi_tool`
//! 2.3.4's committed output byte for byte. Hermetic: no tool runs here.

mod cases;

use std::fs;
use std::path::PathBuf;

use dovi84_composer::Composer;
use dovi_rpu_builder::manifest::{
    crc32, AuMatcher, AuVerdict, Aus, Coverage, L1Policy, ManifestShot, PlaybackManifest, Producer,
    QualityClass, Source, Track, PQ_MAPPING,
};
use dovi_rpu_builder::{HlgComposer, Profile, RpuBuilder, RpuParams, RPU_NAL_HEADER};

const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// Every AU of the synthetic table: one byte.
const AU: [u8; 1] = [0];

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn push_record(out: &mut Vec<u8>, nal: &[u8]) {
    assert_eq!(nal[..2], RPU_NAL_HEADER);
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(&nal[2..]);
}

fn is_canary(params: &RpuParams) -> bool {
    matches!(
        params.profile,
        Profile::P84 {
            composer: HlgComposer::Canary { .. }
        }
    )
}

fn mapping(params: &RpuParams) -> &'static str {
    match params.profile {
        Profile::P81 { .. } => PQ_MAPPING,
        Profile::P84 {
            composer: HlgComposer::Preset,
        } => Composer::Preset.luminance_mapping(),
        Profile::P84 { .. } => Composer::Bt2100V1.luminance_mapping(),
    }
}

/// A manifest for the case: one AU per frame, decode order = presentation order, pts = frame.
fn manifest_for(case: &cases::ShotCase) -> PlaybackManifest {
    let extra = case.extra_shots();
    let count = extra.length;
    PlaybackManifest {
        producer: Producer {
            mkvdovi_version: "test".into(),
            analyzer_version: "test".into(),
            sidecar_version: 6,
            au_table_version: 1,
            rpu_builder_version: "test".into(),
            sidecar_sha256: DIGEST.into(),
        },
        source: Source {
            file_name: format!("{}.mkv", case.name),
            size_bytes: count,
            head_sha256: DIGEST.into(),
            tail_sha256: DIGEST.into(),
            lookup_key: DIGEST.into(),
        },
        track: Track {
            container: "matroska".into(),
            codec: "hevc".into(),
            width: 64,
            height: 64,
            codec_private_sha256: DIGEST.into(),
            time_base: (1, 24),
        },
        aus: Aus {
            pts: (0..count as i64).collect(),
            size: vec![1; count as usize],
            non_picture: Vec::new(),
            irap_crc32: vec![(0, crc32(&AU))],
        },
        leading_skipped: 0,
        analysis: serde_json::json!({ "luminance_mapping": mapping(&case.params) })
            .as_object()
            .unwrap()
            .clone(),
        rpu: case.params.clone(),
        l1_policy: L1Policy::Measured,
        shots: extra
            .shots
            .iter()
            .map(|shot| ManifestShot {
                start: shot.start,
                l1: shot.l1,
            })
            .collect(),
        coverage: vec![Coverage {
            start: 0,
            end: count,
            class: QualityClass::Full,
        }],
    }
}

#[test]
fn rpu_objects_round_trip_every_case_and_refuse_the_canary() {
    let mut checked = 0;
    for case in cases::shot_cases() {
        let json = case.params.to_manifest_json();
        if is_canary(&case.params) {
            assert!(json.is_err(), "{}: the canary must be refused", case.name);
            continue;
        }
        let back = RpuParams::from_manifest_json(&json.unwrap()).unwrap();
        assert_eq!(back, case.params, "{}", case.name);
        checked += 1;
    }
    assert!(checked > 0);
}

#[test]
fn manifest_rpus_equal_dovi_tool() {
    let mut checked = 0;
    for case in cases::shot_cases()
        .into_iter()
        .filter(|case| !is_canary(&case.params))
    {
        // Through the file format: write, parse, then match.
        let text = serde_json::to_string(&manifest_for(&case).to_json().unwrap()).unwrap();
        let manifest = PlaybackManifest::parse(&text).unwrap();
        let builder = RpuBuilder::new(&manifest.rpu).unwrap();
        let mut matcher = AuMatcher::new(&manifest).unwrap();
        let mut ours = Vec::new();
        for frame in 0..manifest.aus.pts.len() as i64 {
            let AuVerdict::Inject {
                l1, scene_refresh, ..
            } = matcher.next(frame, &AU, false)
            else {
                panic!("{}: frame {frame} got no manifest L1", case.name);
            };
            push_record(
                &mut ours,
                &builder.shot(l1).unwrap().write(scene_refresh).unwrap(),
            );
        }
        assert!(
            ours == fixture(&format!("{}.RPU.bin", case.name)),
            "{}: manifest RPUs differ from dovi_tool's RPU.bin",
            case.name
        );
        checked += 1;
    }
    assert!(checked > 0);
}
