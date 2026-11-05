use serde_json::{json, Value};

use super::*;
use crate::params::{CmMetadata, Level6};
use crate::static_l1::static_84_params;

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Decode order: IDR(pts 0) P(3) B(1) B(2) EOS(4, no picture) CRA(5) P(7) B(6).
/// Pictures by pts: 0 1 2 3 5 6 7, so presentation ranks are 0 3 1 2 - 4 6 5.
const PTS: [i64; 8] = [0, 3, 1, 2, 4, 5, 7, 6];
const SIZES: [usize; 8] = [10, 6, 4, 4, 2, 8, 5, 4];

fn data(index: usize) -> Vec<u8> {
    vec![index as u8 + 1; SIZES[index]]
}

fn manifest() -> PlaybackManifest {
    PlaybackManifest {
        producer: Producer {
            mkvdovi_version: "0.8.0".into(),
            analyzer_version: "0.8.0".into(),
            sidecar_version: 6,
            au_table_version: 1,
            rpu_builder_version: "0.8.0".into(),
            sidecar_sha256: DIGEST.into(),
        },
        source: Source {
            file_name: "title.mkv".into(),
            size_bytes: 1000,
            head_sha256: DIGEST.into(),
            tail_sha256: DIGEST.into(),
            lookup_key: DIGEST.into(),
        },
        track: Track {
            container: "matroska".into(),
            codec: "hevc".into(),
            width: 3840,
            height: 2160,
            codec_private_sha256: DIGEST.into(),
            time_base: (1, 1000),
        },
        aus: Aus {
            pts: PTS.to_vec(),
            size: SIZES.iter().map(|&size| size as u32).collect(),
            non_picture: vec![4],
            irap_crc32: vec![(0, crc32(&data(0))), (5, crc32(&data(5)))],
        },
        leading_skipped: 0,
        analysis:
            json!({ "luminance_mapping": Composer::Bt2100V1.luminance_mapping(), "downscale": 1 })
                .as_object()
                .unwrap()
                .clone(),
        rpu: static_84_params(HlgComposer::Bt2100),
        l1_policy: L1Policy::Measured,
        shots: vec![
            ManifestShot {
                start: 0,
                l1: L1::new(0, 1000, 3000),
            },
            ManifestShot {
                start: 4,
                l1: L1::new(10, 1500, 3500),
            },
        ],
        coverage: vec![Coverage {
            start: 0,
            end: 7,
            class: QualityClass::Full,
        }],
    }
}

fn valid_json() -> Value {
    manifest().to_json().unwrap()
}

/// The fixture as profile 8.1, measured through PQ.
fn p81_manifest() -> PlaybackManifest {
    let mut m = manifest();
    m.rpu.profile = Profile::P81 { source_range: None };
    m.analysis
        .insert("luminance_mapping".into(), json!(PQ_MAPPING));
    m
}

/// Parses `json` after `edit` and expects a refusal that mentions `needle`.
fn refused(edit: impl FnOnce(&mut Value), needle: &str) {
    refused_from(valid_json(), edit, needle);
}

fn refused_from(mut json: Value, edit: impl FnOnce(&mut Value), needle: &str) {
    edit(&mut json);
    let error = PlaybackManifest::from_value(&json).unwrap_err();
    assert_eq!(error.code(), 8, "{error}");
    assert!(
        error.to_string().contains(needle),
        "'{needle}' not in: {error}"
    );
}

#[test]
fn round_trip() {
    let json = valid_json();
    let text = serde_json::to_string(&json).unwrap();
    let parsed = PlaybackManifest::parse(&text).unwrap();
    assert_eq!(parsed, manifest());
    assert_eq!(json["aus"]["pts_delta"], json!([3, -2, 1, 2, 1, 2, -1]));
    assert_eq!(json["aus"]["count"], 8);
    assert_eq!(json["shots"]["start"], json!([0, 4]));
}

#[test]
fn not_json_and_wrong_format_or_version_are_refused() {
    let error = PlaybackManifest::parse("{").unwrap_err();
    assert!(error.to_string().contains("not JSON"));
    refused(|json| json["format"] = json!("other"), "format");
    refused(|json| json["version"] = json!(2), "version 2");
    refused(
        |json| {
            json.as_object_mut()
                .unwrap()
                .remove("producer")
                .map(drop)
                .unwrap()
        },
        "producer is missing",
    );
}

#[test]
fn au_table_rules() {
    refused(
        |json| json["aus"]["count"] = json!(9),
        "pts_delta has 7 entries for 9",
    );
    refused(|json| json["aus"]["size"] = json!([1, 2]), "aus.size has 2");
    refused(|json| json["aus"]["size"][0] = json!(-1), "aus.size[0]");
    refused(
        |json| json["aus"]["non_picture"] = json!([4, 4]),
        "non_picture is not strictly rising",
    );
    refused(
        |json| json["aus"]["non_picture"] = json!([8]),
        "non_picture[0] = 8",
    );
    refused(
        |json| json["aus"]["irap_crc32"] = json!([[5, 1], [0, 1]]),
        "irap_crc32 is not strictly rising",
    );
    refused(
        |json| json["aus"]["irap_crc32"] = json!([[4, 1]]),
        "names non-picture AU 4",
    );
    refused(
        |json| json["aus"]["irap_crc32"] = json!([[0]]),
        "irap_crc32[0]",
    );
    // pts 3 twice: the second AU (pts 3) and a changed delta on the third.
    refused(
        |json| json["aus"]["pts_delta"][1] = json!(0),
        "two AUs have pts 3",
    );
    // A non-picture AU may not share a pts with a picture either.
    refused(
        |json| json["aus"]["pts_delta"][3] = json!(-2),
        "two AUs have pts 0",
    );
    refused(|json| json["leading_skipped"] = json!(8), "leading_skipped");
    // Every picture skipped is still a valid table: 7 pictures, 7 skipped.
    let mut json = valid_json();
    json["leading_skipped"] = json!(7);
    assert_eq!(
        PlaybackManifest::from_value(&json).unwrap().leading_skipped,
        7
    );
    refused(
        |json| json["track"]["time_base"] = json!([1, 2_147_483_648_i64]),
        "two 32-bit integers",
    );
    refused(
        |json| json["track"]["time_base"] = json!([0, 1000]),
        "time_base is not positive",
    );
    refused(
        |json| json["track"]["time_base"] = json!([1]),
        "time_base is not [num, den]",
    );
    refused(
        |json| json["track"]["codec"] = json!("avc"),
        "matroska/hevc",
    );
}

#[test]
fn digests_must_be_lowercase_hex() {
    refused(
        |json| json["source"]["lookup_key"] = json!("abc"),
        "lookup_key is not a SHA-256",
    );
    refused(
        |json| json["track"]["codec_private_sha256"] = json!(DIGEST.to_uppercase()),
        "codec_private_sha256",
    );
}

#[test]
fn shot_rules() {
    refused(
        |json| json["shots"] = json!({"start": [], "min": [], "avg": [], "max": []}),
        "shots is empty",
    );
    refused(
        |json| json["shots"]["start"] = json!([1, 4]),
        "does not start at 0",
    );
    refused(
        |json| json["shots"]["start"] = json!([0, 0]),
        "does not start after",
    );
    refused(
        |json| {
            json["shots"]["start"] = json!([0, 7]);
            json["coverage"] = json!([{"start": 0, "end": 7, "class": "full"}]);
        },
        "not below 7 pictures",
    );
    refused(|json| json["shots"]["max"][1] = json!(4096), "above 4095");
    refused(
        |json| json["shots"]["max"] = json!([3000]),
        "columns differ",
    );
}

#[test]
fn coverage_rules() {
    refused(|json| json["coverage"] = json!([]), "coverage is empty");
    refused(
        |json| json["coverage"][0]["end"] = json!(8),
        "beyond 7 pictures",
    );
    refused(
        |json| json["coverage"][0]["class"] = json!("exact"),
        "class 'exact'",
    );
    // A boundary inside a shot would mix manifest L1 and the fallback in one shot.
    refused(
        |json| json["coverage"][0]["end"] = json!(5),
        "shot boundaries",
    );
    refused(
        |json| {
            json["coverage"] = json!([
                {"start": 4, "end": 7, "class": "full"},
                {"start": 0, "end": 4, "class": "full"}
            ]);
        },
        "overlaps or is not sorted",
    );
    // A start inside a shot: picture 1 is in shot 0, which starts at 0.
    refused(
        |json| json["coverage"][0]["start"] = json!(1),
        "shot boundaries",
    );
    refused(
        |json| json["coverage"] = json!([{"start": 4, "end": 4, "class": "full"}]),
        "is empty or beyond",
    );
    let mut json = valid_json();
    json["coverage"] = json!([{"start": 4, "end": 7, "class": "approximate"}]);
    assert!(PlaybackManifest::from_value(&json).is_ok());
}

#[test]
fn composer_and_mapping_must_agree() {
    refused(
        |json| json["analysis"]["luminance_mapping"] = json!(Composer::Preset.luminance_mapping()),
        "does not fit the rpu",
    );
    refused(
        |json| json["rpu"]["composer"] = json!("canary"),
        "not preset or bt2100",
    );
    // An 8.1 RPU needs a PQ measurement.
    let mut p81 = manifest();
    p81.rpu.profile = Profile::P81 { source_range: None };
    assert!(p81.to_json().is_err());
    let p81 = p81_manifest();
    let json = p81.to_json().unwrap();
    assert!(PlaybackManifest::from_value(&json).is_ok());

    let parsed = manifest();
    assert!(parsed.check_composer(HlgComposer::Bt2100).is_ok());
    assert!(parsed.check_composer(HlgComposer::Preset).is_err());
    let error = p81.check_composer(HlgComposer::Bt2100).unwrap_err();
    assert!(error.to_string().contains("profile 8.1"), "{error}");
}

#[test]
fn rpu_object_rules() {
    let p81 = || p81_manifest().to_json().unwrap();
    refused_from(
        p81(),
        |json| json["rpu"]["composer"] = json!("bt2100"),
        "composer must be null for profile 8.1",
    );
    refused_from(
        p81(),
        |json| json["rpu"]["source_range"] = json!([1, 2, 3]),
        "source_range is not [min, max]",
    );
    let mut ranged = p81_manifest();
    ranged.rpu.profile = Profile::P81 {
        source_range: Some((7, 3696)),
    };
    let json = ranged.to_json().unwrap();
    assert_eq!(json["rpu"]["source_range"], json!([7, 3696]));
    assert_eq!(PlaybackManifest::from_value(&json).unwrap(), ranged);
    refused(
        |json| json["rpu"]["profile"] = json!("9"),
        "rpu.profile '9' is not 8.1 or 8.4",
    );
    refused(
        |json| json["rpu"]["cm"]["version"] = json!("V50"),
        "rpu.cm.version 'V50'",
    );
    refused(
        |json| json["rpu"]["l1_avg_pq_cm_version"] = json!("x"),
        "rpu.l1_avg_pq_cm_version 'x'",
    );
}

#[test]
fn l1_policy_rules() {
    refused(
        |json| json["l1_policy"] = json!("auto"),
        "l1_policy 'auto' is not measured or rpu-bin",
    );
    let mut m = manifest();
    m.l1_policy = L1Policy::RpuBin;
    let json = m.to_json().unwrap();
    assert_eq!(json["l1_policy"], json!("rpu-bin"));
    assert_eq!(PlaybackManifest::from_value(&json).unwrap(), m);
}

#[test]
fn unwritable_rpus_are_refused() {
    refused(
        |json| json["rpu"]["level6"]["max_content_light_level"] = json!(10_001),
        "cannot be built",
    );
    refused(
        |json| {
            json["rpu"]["cm"]["trim_targets_nits"] =
                json!([100, 200, 300, 400, 500, 600, 700, 800, 900])
        },
        "cannot be built",
    );
    refused(
        |json| json["rpu"]["level5"] = json!({"left": 70000, "right": 0, "top": 0, "bottom": 0}),
        "out of range",
    );
    refused(
        |json| json["rpu"]["source_range"] = json!([1, 2]),
        "must be null for profile 8.4",
    );
}

#[test]
fn rpu_json_round_trips_and_refuses_the_canary() {
    let mut params = static_84_params(HlgComposer::Preset);
    params.cm = CmMetadata::V29;
    params.level5 = Some(crate::Level5 {
        left: 1,
        right: 2,
        top: 3,
        bottom: 4,
    });
    params.level6 = Level6::from_nits(0.0001, 4000.0, 1200.0, 300.0);
    let json = params.to_manifest_json().unwrap();
    assert_eq!(RpuParams::from_manifest_json(&json).unwrap(), params);
    assert!(static_84_params(HlgComposer::canary())
        .to_manifest_json()
        .is_err());
}

#[test]
fn source_and_track_checks() {
    let parsed = manifest();
    assert!(parsed.check_source(1000, DIGEST, DIGEST).is_ok());
    assert!(parsed.check_source(1001, DIGEST, DIGEST).is_err());
    assert!(parsed.check_source(1000, DIGEST, &"0".repeat(64)).is_err());
    assert!(parsed.check_track(DIGEST, 3840, 2160, (1, 1000)).is_ok());
    assert!(parsed
        .check_track(&"0".repeat(64), 3840, 2160, (1, 1000))
        .is_err());
    let error = parsed
        .check_track(DIGEST, 1920, 1080, (1, 1000))
        .unwrap_err();
    assert!(error.to_string().contains("1920x1080"), "{error}");
    assert!(parsed
        .check_track(DIGEST, 3840, 2160, (1, 1_000_000))
        .is_err());
}

// --- AuMatcher ---

fn feed(matcher: &mut AuMatcher, index: usize) -> AuVerdict {
    matcher.next(PTS[index], &data(index), false)
}

fn inject(l1: L1, scene_refresh: bool, shot: usize) -> AuVerdict {
    AuVerdict::Inject {
        l1,
        scene_refresh,
        shot,
        class: QualityClass::Full,
    }
}

fn reason(verdict: &AuVerdict) -> Option<RejectReason> {
    match verdict {
        AuVerdict::Rejected(rejection) => Some(rejection.reason),
        _ => None,
    }
}

#[test]
fn a_start_from_zero_gives_shots_by_presentation_rank() {
    let m = manifest();
    let (a, b) = (m.shots[0].l1, m.shots[1].l1);
    let mut matcher = AuMatcher::new(&m).unwrap();
    let verdicts: Vec<AuVerdict> = (0..8).map(|index| feed(&mut matcher, index)).collect();
    assert_eq!(
        verdicts,
        vec![
            inject(a, true, 0),  // pts 0, rank 0: shot 0 starts
            inject(a, false, 0), // pts 3, rank 3
            inject(a, false, 0), // pts 1
            inject(a, false, 0), // pts 2
            AuVerdict::Fallback, // no picture
            inject(b, true, 1),  // pts 5, rank 4: shot 1 starts
            inject(b, false, 1), // pts 7
            inject(b, false, 1), // pts 6
        ]
    );
    assert_eq!(reason(&feed(&mut matcher, 0)), Some(RejectReason::PastEnd));
}

#[test]
fn a_different_encode_is_rejected_at_au_0() {
    let mut matcher = AuMatcher::new(&manifest()).unwrap();
    let verdict = matcher.next(0, &[1; 11], false);
    assert_eq!(reason(&verdict), Some(RejectReason::Size));
    // Sticky, also across a reset.
    matcher.reset();
    assert_eq!(reason(&feed(&mut matcher, 0)), Some(RejectReason::Size));
    assert_eq!(matcher.rejection().unwrap().decode_index, Some(0));
}

#[test]
fn an_irap_with_other_bytes_is_rejected() {
    let mut matcher = AuMatcher::new(&manifest()).unwrap();
    let verdict = matcher.next(0, &[9; 10], false);
    assert_eq!(reason(&verdict), Some(RejectReason::Crc));
}

#[test]
fn a_skipped_au_breaks_continuity() {
    let mut matcher = AuMatcher::new(&manifest()).unwrap();
    feed(&mut matcher, 0);
    assert_eq!(reason(&feed(&mut matcher, 2)), Some(RejectReason::Order));
    let mut matcher = AuMatcher::new(&manifest()).unwrap();
    feed(&mut matcher, 0);
    assert_eq!(
        reason(&matcher.next(99, &data(1), false)),
        Some(RejectReason::UnknownPts)
    );
}

#[test]
fn after_a_seek_the_matcher_waits_for_an_irap() {
    let m = manifest();
    let b = m.shots[1].l1;
    let mut matcher = AuMatcher::new(&m).unwrap();
    feed(&mut matcher, 0);
    matcher.reset();
    // Non-IRAP pictures and a non-picture packet before the IRAP: fallback, no rejection.
    assert_eq!(feed(&mut matcher, 2), AuVerdict::Fallback);
    assert_eq!(feed(&mut matcher, 4), AuVerdict::Fallback);
    assert_eq!(feed(&mut matcher, 3), AuVerdict::Fallback);
    // The CRA validates, then continuity holds from it.
    assert_eq!(feed(&mut matcher, 5), inject(b, true, 1));
    assert_eq!(feed(&mut matcher, 6), inject(b, false, 1));
    assert_eq!(reason(&feed(&mut matcher, 5)), Some(RejectReason::Order));
}

#[test]
fn a_seek_to_an_irap_with_other_bytes_is_rejected() {
    let mut matcher = AuMatcher::new(&manifest()).unwrap();
    matcher.reset();
    assert_eq!(feed(&mut matcher, 1), AuVerdict::Fallback);
    assert_eq!(
        reason(&matcher.next(PTS[5], &[0; 8], false)),
        Some(RejectReason::Crc)
    );
    // A pts that is not in the table, while waiting.
    let mut matcher = AuMatcher::new(&manifest()).unwrap();
    assert_eq!(
        reason(&matcher.next(1234, &[0], false)),
        Some(RejectReason::UnknownPts)
    );
}

#[test]
fn continuity_runs_through_a_non_picture_packet_and_checks_its_size() {
    let mut matcher = AuMatcher::new(&manifest()).unwrap();
    for index in 0..4 {
        feed(&mut matcher, index);
    }
    assert_eq!(
        reason(&matcher.next(PTS[4], &[0; 3], false)),
        Some(RejectReason::Size)
    );
}

#[test]
fn a_laced_au_is_rejected() {
    let mut matcher = AuMatcher::new(&manifest()).unwrap();
    assert_eq!(
        reason(&matcher.next(0, &data(0), true)),
        Some(RejectReason::Laced)
    );
}

#[test]
fn uncovered_shots_get_the_fallback_and_are_still_checked() {
    let mut m = manifest();
    m.coverage = vec![Coverage {
        start: 4,
        end: 7,
        class: QualityClass::Approximate,
    }];
    let b = m.shots[1].l1;
    let mut matcher = AuMatcher::new(&m).unwrap();
    for index in 0..5 {
        assert_eq!(feed(&mut matcher, index), AuVerdict::Fallback, "AU {index}");
    }
    assert_eq!(
        feed(&mut matcher, 5),
        AuVerdict::Inject {
            l1: b,
            scene_refresh: true,
            shot: 1,
            class: QualityClass::Approximate
        }
    );
    // An uncovered AU with the wrong size still rejects.
    let mut matcher = AuMatcher::new(&m).unwrap();
    feed(&mut matcher, 0);
    assert_eq!(
        reason(&matcher.next(PTS[1], &[0; 1], false)),
        Some(RejectReason::Size)
    );
}

/// The fields are public, so a manifest can skip the parser; the matcher validates it itself.
#[test]
fn the_matcher_refuses_a_hand_built_manifest_that_validate_refuses() {
    let refused_by_new = |edit: fn(&mut PlaybackManifest), needle: &str| {
        let mut m = manifest();
        edit(&mut m);
        let error = AuMatcher::new(&m).unwrap_err();
        assert_eq!(error.code(), 8, "{error}");
        assert!(
            error.to_string().contains(needle),
            "'{needle}' not in: {error}"
        );
    };
    refused_by_new(|m| m.shots.clear(), "shots is empty");
    refused_by_new(|m| m.aus.size.truncate(7), "differ in length");
    refused_by_new(|m| m.aus.non_picture = vec![4, 1], "not strictly rising");
}

#[test]
fn picture_count_does_not_underflow() {
    let mut aus = manifest().aus;
    aus.non_picture = (0..9).collect();
    assert_eq!(aus.picture_count(), 0);
}

#[test]
fn reject_codes_are_fixed() {
    let codes = [
        RejectReason::UnknownPts,
        RejectReason::Size,
        RejectReason::Crc,
        RejectReason::Order,
        RejectReason::Laced,
        RejectReason::PastEnd,
    ]
    .map(RejectReason::code);
    assert_eq!(codes, [1, 2, 3, 4, 5, 6]);
}
