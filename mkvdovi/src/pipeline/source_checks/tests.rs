use serde_json::json;

use super::*;

fn stated(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
    pairs.iter().map(|&(k, v)| (k.to_string(), v)).collect()
}

/// Joker Folie à Deux (development corpus): RPU 0.0001 / 1000 nits, 992 / 285.
const JOKER_L6: SourceL6 = SourceL6 {
    max_dml_nits: 1000,
    min_dml_raw: 1,
    max_cll: 992,
    max_fall: 285,
};

fn container_matching_joker() -> HashMap<String, f64> {
    stated(&[
        ("max_dml", 1000.0),
        ("min_dml", 0.0001),
        ("max_cll", 992.0),
        ("max_fall", 285.0),
    ])
}

#[test]
fn pq_codes_convert_to_their_nits() {
    // ST 2084 reference points: 12-bit code 2081 is 100 nits, 3079 is 1000 nits, 4095 is 10000.
    for (code, nits) in [(2081, 100.0), (3079, 1000.0), (4095, 10_000.0)] {
        let got = pq_12_bit_to_nits(code);
        assert!((got - nits).abs() < nits * 0.003, "code {code}: {got}");
    }
    assert_eq!(pq_12_bit_to_nits(0), 0.0);
}

#[test]
fn c5_warns_on_the_joker_container_and_names_what_mdfix_writes() {
    let container = stated(&[
        ("max_dml", 1000.0),
        ("min_dml", 0.0001),
        ("max_cll", 676.0),
        ("max_fall", 87.0),
    ]);
    let message = rpu_l6_against_container(&JOKER_L6, &container, false).unwrap();
    assert!(
        message.contains("MaxCLL 992 against 676 nits, MaxFALL 285 against 87 nits"),
        "{message}"
    );
    assert!(!message.contains("mastering"), "{message}");
    assert!(message.contains("(RPU against stated)"), "{message}");
    assert!(
        message.contains("Dolby Vision playback reads the RPU's L6."),
        "{message}"
    );
    let message = rpu_l6_against_container(&JOKER_L6, &container, true).unwrap();
    assert!(
        message.contains("Under --mdfix the new L6 takes the stated values."),
        "{message}"
    );
}

#[test]
fn c5_is_silent_when_the_container_agrees_or_states_nothing() {
    assert_eq!(
        rpu_l6_against_container(&JOKER_L6, &container_matching_joker(), true),
        None
    );
    // Inside Out 2: no values in the container (P12's case).
    assert_eq!(
        rpu_l6_against_container(&JOKER_L6, &HashMap::new(), true),
        None
    );
}

#[test]
fn c5_mastering_peak_and_minimum_differ_at_all() {
    let mut container = container_matching_joker();
    container.insert("max_dml".into(), 1001.0);
    let message = rpu_l6_against_container(&JOKER_L6, &container, false).unwrap();
    assert!(
        message.contains("mastering peak 1000 against 1001 nits"),
        "{message}"
    );
    // A decimal container peak that rounds to the RPU's nits agrees.
    container.insert("max_dml".into(), 1000.4);
    assert_eq!(rpu_l6_against_container(&JOKER_L6, &container, false), None);

    let mut container = container_matching_joker();
    // Float noise around the RPU's 0.0001 nits is no difference.
    container.insert("min_dml".into(), 0.000_100_000_01);
    assert_eq!(rpu_l6_against_container(&JOKER_L6, &container, false), None);
    // One unit of 0.0001 nits apart, on either side, is a disagreement (ROADMAP_LOG C5: "MDL any").
    for (value, label) in [(0.0002, "0.0002"), (0.0, "0.0000")] {
        container.insert("min_dml".into(), value);
        let message = rpu_l6_against_container(&JOKER_L6, &container, false).unwrap();
        assert!(
            message.contains(&format!("mastering minimum 0.0001 against {label} nits")),
            "{message}"
        );
    }
    // A malformed minimum far above the u16 range still compares.
    container.insert("min_dml".into(), 50.0);
    assert!(rpu_l6_against_container(&JOKER_L6, &container, false).is_some());
}

#[test]
fn c5_light_levels_may_differ_by_two_percent_of_the_container_value() {
    let l6 = SourceL6 {
        max_cll: 1020,
        max_fall: 98,
        ..JOKER_L6
    };
    // 1020 against 1000 is exactly 2 %: silent; 1021 is above it.
    let mut container = container_matching_joker();
    container.insert("max_cll".into(), 1000.0);
    container.insert("max_fall".into(), 100.0);
    assert_eq!(rpu_l6_against_container(&l6, &container, false), None);
    let l6 = SourceL6 {
        max_cll: 1021,
        max_fall: 97,
        ..JOKER_L6
    };
    let message = rpu_l6_against_container(&l6, &container, false).unwrap();
    assert!(
        message.contains("MaxCLL 1021 against 1000 nits, MaxFALL 97 against 100 nits"),
        "{message}"
    );
}

#[test]
fn c5_skips_light_levels_an_rpu_leaves_unknown() {
    // Skyfall: the RPU states MaxCLL and MaxFALL 0.
    let l6 = SourceL6 {
        max_cll: 0,
        max_fall: 0,
        ..JOKER_L6
    };
    assert_eq!(
        rpu_l6_against_container(&l6, &container_matching_joker(), false),
        None
    );
}

fn l1(max_pq: u16) -> L1Stats {
    L1Stats {
        min_pq: 0,
        avg_pq: 0,
        max_pq,
    }
}

#[test]
fn c4_rpu_peak_warns_above_one_and_a_quarter_times_max_cll() {
    // 1.25 × 800 nits = 1000 nits = PQ code 3079.
    let container = stated(&[("max_cll", 800.0)]);
    assert_eq!(
        rpu_peak_against_max_cll(&[l1(2081), l1(3079)], &container),
        None
    );
    let message = rpu_peak_against_max_cll(&[l1(2081), l1(3080), l1(3500)], &container).unwrap();
    assert!(message.contains("in 2 of 3 sampled frames"), "{message}");
    assert!(message.contains("(800 nits)"), "{message}");
}

#[test]
fn c4_rpu_peak_needs_a_stated_max_cll_and_frames() {
    assert_eq!(rpu_peak_against_max_cll(&[l1(4000)], &HashMap::new()), None);
    let zero = stated(&[("max_cll", 0.0)]);
    assert_eq!(rpu_peak_against_max_cll(&[l1(4000)], &zero), None);
    assert_eq!(
        rpu_peak_against_max_cll(&[], &stated(&[("max_cll", 100.0)])),
        None
    );
}

/// HDR10+ JSON frames with the given MaxScl (tenths of a nit) and distribution maximum.
fn hdr10plus(frames: &[(Option<[u64; 3]>, u64)]) -> Value {
    let frames: Vec<Value> = frames
        .iter()
        .map(|(scl, dist)| {
            let mut luminance = json!({
                "LuminanceDistributions": { "DistributionValues": [0, *dist, 10] }
            });
            if let Some(scl) = scl {
                luminance["MaxScl"] = json!(scl);
            }
            json!({ "LuminanceParameters": luminance })
        })
        .collect();
    json!({ "SceneInfo": frames })
}

#[test]
fn c4_hdr10plus_counts_frames_where_either_peak_is_above_the_threshold() {
    // 1.25 × 800 nits = 1000 nits = 10000 tenths.
    let container = stated(&[("max_cll", 800.0)]);
    let silent = hdr10plus(&[(Some([10_000, 9_000, 100]), 10_000)]);
    assert_eq!(
        hdr10plus_peak_against_max_cll(&silent, &container, PeakSource::Histogram),
        None
    );
    // Frame 1: only MaxScl is above; frame 2: only the histogram peak; frame 3: neither.
    let metadata = hdr10plus(&[
        (Some([10_001, 0, 0]), 5_000),
        (Some([0, 0, 0]), 12_000),
        (None, 3_000),
    ]);
    let message =
        hdr10plus_peak_against_max_cll(&metadata, &container, PeakSource::Histogram).unwrap();
    assert!(message.contains("in 2 of 3 frames"), "{message}");
    assert!(message.contains("highest MaxScl 1000 nits"), "{message}");
    assert!(
        message.contains("--peak-source histogram peak 1200 nits"),
        "{message}"
    );
}

#[test]
fn c4_hdr10plus_needs_a_stated_max_cll_and_frames() {
    let metadata = hdr10plus(&[(Some([40_000, 0, 0]), 40_000)]);
    for container in [HashMap::new(), stated(&[("max_cll", 0.0)])] {
        assert_eq!(
            hdr10plus_peak_against_max_cll(&metadata, &container, PeakSource::MaxScl),
            None
        );
    }
    let empty = json!({ "SceneInfo": [] });
    assert_eq!(
        hdr10plus_peak_against_max_cll(&empty, &stated(&[("max_cll", 1.0)]), PeakSource::MaxScl),
        None
    );
}

#[test]
fn c7_warns_only_where_dovi_tool_reads_the_zero_max_scl() {
    let zero = hdr10plus(&[(Some([0, 0, 0]), 8_930), (Some([0, 0, 0]), 4_000)]);
    for source in [PeakSource::MaxScl, PeakSource::MaxSclLuminance] {
        let (severity, message) = hdr10plus_max_scl_all_zero(&zero, source, false).unwrap();
        assert_eq!(severity, Severity::Warn);
        assert!(message.contains("100-nit floor"), "{message}");
        let (severity, message) = hdr10plus_max_scl_all_zero(&zero, source, true).unwrap();
        assert_eq!(severity, Severity::Info);
        assert!(message.contains("measured peak"), "{message}");
    }
    for source in [PeakSource::Histogram, PeakSource::Histogram99] {
        for hybrid in [false, true] {
            let (severity, _) = hdr10plus_max_scl_all_zero(&zero, source, hybrid).unwrap();
            assert_eq!(severity, Severity::Info);
        }
    }
}

#[test]
fn c7_is_silent_unless_every_frame_states_a_zero_max_scl() {
    let one_lit = hdr10plus(&[(Some([0, 0, 0]), 0), (Some([0, 1, 0]), 0)]);
    let one_missing = hdr10plus(&[(Some([0, 0, 0]), 0), (None, 0)]);
    let empty = json!({ "SceneInfo": [] });
    let no_scenes = json!({});
    for metadata in [one_lit, one_missing, empty, no_scenes] {
        assert_eq!(
            hdr10plus_max_scl_all_zero(&metadata, PeakSource::MaxScl, false),
            None,
            "{metadata}"
        );
    }
}

#[test]
fn c1_warns_above_one_and_a_quarter_times_the_stated_max_cll() {
    // Joker: measured 972 against a stated 676 (1.44×); Fury Road: 10000 against 9918 (1.01×).
    let message = measured_max_cll_above_stated(&stated(&[("max_cll", 676.0)]), Some(972)).unwrap();
    assert!(message.contains("(972 nits)"), "{message}");
    assert!(message.contains("(676 nits)"), "{message}");
    assert_eq!(
        measured_max_cll_above_stated(&stated(&[("max_cll", 9918.0)]), Some(10_000)),
        None
    );
    // Exactly 1.25× stays silent.
    assert_eq!(
        measured_max_cll_above_stated(&stated(&[("max_cll", 800.0)]), Some(1000)),
        None
    );
    assert!(measured_max_cll_above_stated(&stated(&[("max_cll", 800.0)]), Some(1001)).is_some());
}

#[test]
fn c1_needs_a_stated_and_a_measured_max_cll() {
    assert_eq!(
        measured_max_cll_above_stated(&HashMap::new(), Some(4000)),
        None
    );
    assert_eq!(
        measured_max_cll_above_stated(&stated(&[("max_cll", 100.0)]), None),
        None
    );
}

#[test]
fn c3_warns_when_the_stated_max_fall_is_above_max_cll() {
    let message =
        stated_fall_above_cll(&stated(&[("max_cll", 100.0), ("max_fall", 400.0)])).unwrap();
    assert!(message.contains("MaxFALL (400 nits)"), "{message}");
    assert!(message.contains("MaxCLL (100 nits)"), "{message}");
    // House of the Dragon: 859 below 997; equal values are allowed.
    for (cll, fall) in [(997.0, 859.0), (500.0, 500.0)] {
        assert_eq!(
            stated_fall_above_cll(&stated(&[("max_cll", cll), ("max_fall", fall)])),
            None
        );
    }
    assert_eq!(stated_fall_above_cll(&stated(&[("max_fall", 400.0)])), None);
}
