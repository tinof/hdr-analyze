//! The builders against committed `dovi_tool` 2.3.4 output (`tests/fixtures`, written by
//! `make_fixtures.sh`; the bt2100 and canary RPUs went through `fit_hlg_composer`). Hermetic: no
//! tool runs here.

mod cases;

use std::fs;
use std::path::PathBuf;

use dovi_rpu_builder::{
    CmVersion, Hdr10PlusFrame, Hdr10PlusHold, RpuBuilder, ShotRpu, L1, RPU_NAL_HEADER,
};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// One access unit's NAL in the `RPU.bin` layout: start code, then the NAL without its header.
fn push_record(out: &mut Vec<u8>, nal: &[u8]) {
    assert_eq!(nal[..2], RPU_NAL_HEADER);
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(&nal[2..]);
}

fn read_payloads(name: &str) -> Vec<Vec<u8>> {
    let data = fixture(name);
    let mut frames = Vec::new();
    let mut rest = data.as_slice();
    while !rest.is_empty() {
        let len = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
        frames.push(rest[2..2 + len].to_vec());
        rest = &rest[2 + len..];
    }
    frames
}

#[test]
fn shot_cases_equal_dovi_tool() {
    for case in cases::shot_cases() {
        let builder = RpuBuilder::new(&case.params).unwrap();
        let mut ours = Vec::new();
        for &(duration, l1) in &case.shots {
            let mut shot: ShotRpu = builder.shot(l1).unwrap();
            for frame in 0..duration {
                push_record(&mut ours, &shot.write(frame == 0).unwrap());
            }
        }
        assert!(
            ours == fixture(&format!("{}.RPU.bin", case.name)),
            "{} differs from dovi_tool's RPU.bin",
            case.name
        );
    }
}

#[test]
fn extra_json_fixtures_are_the_current_definition() {
    // The committed extra.json files are what make_fixtures.sh fed to dovi_tool. Compared as
    // values: the tool that wrote them orders keys by insertion.
    for case in cases::shot_cases() {
        let committed: serde_json::Value =
            serde_json::from_slice(&fixture(&format!("{}.extra.json", case.name))).unwrap();
        assert_eq!(
            committed,
            case.params.extra_json(Some(&case.extra_shots())),
            "{}: rerun make_fixtures.sh",
            case.name
        );
    }
    for cm in [CmVersion::V29, CmVersion::V40] {
        let name = format!("hdr10plus.{}.extra.json", cases::cm_name(cm));
        let committed: serde_json::Value = serde_json::from_slice(&fixture(&name)).unwrap();
        assert_eq!(
            committed,
            cases::hdr10plus_params(cm).extra_json(None),
            "{name}"
        );
    }
}

/// The builder over a sequence's payloads in presentation order, held by [`Hdr10PlusHold`].
fn hdr10plus_rpus(
    sequence: &str,
    peak: dovi_rpu_builder::PeakSource,
    cm: CmVersion,
) -> (Vec<u8>, Vec<(L1, bool)>) {
    let params = cases::hdr10plus_params(cm);
    let builder = RpuBuilder::new(&params).unwrap();
    let mut hold = Hdr10PlusHold::new(peak, params.l1_avg_pq_cm_version);
    let mut out = Vec::new();
    let mut frames = Vec::new();
    for payload in read_payloads(&format!("{sequence}.payloads")) {
        let (l1, scene_refresh) = hold
            .next(&Hdr10PlusFrame::parse(&payload).unwrap())
            .unwrap();
        push_record(
            &mut out,
            &builder.shot(l1).unwrap().write(scene_refresh).unwrap(),
        );
        frames.push((l1, scene_refresh));
    }
    (out, frames)
}

#[test]
fn hdr10plus_sequences_equal_dovi_tool_for_every_peak_source() {
    for sequence in cases::HDR10PLUS_SEQUENCES {
        for cm in [CmVersion::V29, CmVersion::V40] {
            for (peak, name) in cases::PEAK_SOURCES {
                let (ours, _) = hdr10plus_rpus(sequence, peak, cm);
                let fixture_name = format!("{sequence}.{name}.{}.RPU.bin", cases::cm_name(cm));
                assert!(ours == fixture(&fixture_name), "{fixture_name} differs");
            }
        }
    }
}

#[test]
fn hdr10plus_scene_starts_equal_hdr10plus_tools() {
    for sequence in cases::HDR10PLUS_SEQUENCES {
        let json: serde_json::Value =
            serde_json::from_slice(&fixture(&format!("{sequence}.json"))).unwrap();
        let expected: Vec<usize> = json["SceneInfoSummary"]["SceneFirstFrameIndex"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as usize)
            .collect();
        let (_, frames) = hdr10plus_rpus(
            sequence,
            dovi_rpu_builder::PeakSource::Histogram,
            CmVersion::V40,
        );
        let ours: Vec<usize> = frames
            .iter()
            .enumerate()
            .filter(|(_, (_, refresh))| *refresh)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(ours, expected, "{sequence}");
    }
}

#[test]
fn hdr10plus_fixtures_cover_both_sides_of_the_scene_rule() {
    // The sequences are only a test of the rule if they contain its edge cases (see
    // tools/rpu_builder_check/src/hdr10plus_fixtures.rs).
    let (_, a) = hdr10plus_rpus(
        "profile_a",
        dovi_rpu_builder::PeakSource::Histogram,
        CmVersion::V40,
    );
    assert!(!a[1].1, "identical payload holds");
    assert!(!a[2].1, "a fraction_bright_pixels change holds");
    assert!(a[3].1, "a luminance change starts a scene");
    assert!(
        a[4].1 && a[4].0 == a[3].0,
        "a new scene with the same L1 (rounding)"
    );
    assert!(
        a[7].1 && a[7].0 == a[6].0,
        "a new scene with the same L1 (clamping)"
    );
    let (_, b) = hdr10plus_rpus(
        "profile_b",
        dovi_rpu_builder::PeakSource::Histogram,
        CmVersion::V40,
    );
    assert!(
        b[2].1 && b[2].0 == b[1].0,
        "a Bezier-only change starts a scene"
    );
    // Histogram and Histogram99 differ on the first frame (the 98% percentile is the highest).
    let (_, a99) = hdr10plus_rpus(
        "profile_a",
        dovi_rpu_builder::PeakSource::Histogram99,
        CmVersion::V40,
    );
    assert_ne!(a[0].0.max_pq, a99[0].0.max_pq);
}
