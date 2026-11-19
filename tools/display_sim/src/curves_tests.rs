//! The shim's parameter setup against an independent oracle: libplacebo v7.387.0's
//! `src/tone_mapping.c` compiled on its own (no link to the built library, no shim) in
//! `~/mkvdovi-work/ws7-1-2026-11-18/oracle/harness/` (`harness5_precise.c`, rows A to G;
//! `harness6.c`, rows H to K, whose parameters after the renderer's steps were derived by hand).
//! The values are displayed nits, printed to nine significant digits.

use super::*;

const ROW_POINTS: [f64; 5] = [0.01, 1.0, 10.0, 100.0, 500.0];
const H_POINTS: [f64; 9] = [0.01, 1.0, 10.0, 100.0, 500.0, 600.0, 800.0, 1000.0, 1200.0];

struct Scenario {
    source_black: f64,
    panel_black: f64,
    l1_max_nits: f64,
    avg_nits: f64,
    panel: f64,
}

fn mapping(curve: Curve, s: &Scenario) -> Mapping {
    let code = |nits: f64| nits_to_pq(nits) * 4095.0;
    Mapping::new(
        curve,
        &Display {
            source_black_nits: s.source_black,
            panel_black_nits: s.panel_black,
            panel_peak_nits: s.panel,
        },
        &Metadata {
            max_code: code(s.l1_max_nits),
            avg_code: if s.avg_nits > 0.0 {
                code(s.avg_nits)
            } else {
                0.0
            },
        },
    )
    .unwrap()
}

fn assert_row(name: &str, curve: Curve, s: &Scenario, points: &[f64], expected: &[f64]) {
    let mapping = mapping(curve, s);
    for (&nits, &want) in points.iter().zip(expected) {
        let got = mapping.displayed_nits(nits_to_pq(nits));
        assert!(
            (got / want - 1.0).abs() < 5e-4,
            "row {name} {}: {nits} nits displays {got}, oracle {want}",
            curve.name()
        );
    }
}

const fn scenario(black: (f64, f64), l1_max_nits: f64, avg_nits: f64, panel: f64) -> Scenario {
    Scenario {
        source_black: black.0,
        panel_black: black.1,
        l1_max_nits,
        avg_nits,
        panel,
    }
}

/// Rows A to G of `harness5_precise.c`: L1 max at or above the panel peak, so the renderer's
/// steps change nothing; displayed nits at 0.01, 1, 10, 100 and 500 nits, then at L1 max.
#[rustfmt::skip]
const ORACLE_ROWS: [(&str, Scenario, [[f64; 6]; 3]); 6] = [
    ("A", scenario((0.005, 0.005), 4000.0, 20.0, 600.0), [
        [0.00907364115, 0.686932325, 6.74599028, 59.3648605, 190.050949, 600.007385],
        [0.00847881101, 0.69668752, 6.89669561, 63.7770309, 239.146454, 600.008728],
        [0.00999996811, 0.999990642, 9.99990177, 99.9997253, 413.393616, 600.007385]]),
    ("B", scenario((0.005, 0.005), 1000.0, 50.0, 100.0), [
        [0.00816354435, 0.419729054, 3.68770599, 33.1636391, 78.6055603, 99.9997253],
        [0.00711630145, 0.424798757, 4.10312605, 31.9904861, 80.8916626, 99.9957886],
        [0.00999996811, 0.999990642, 9.99990177, 69.4542465, 98.9461212, 99.9997253]]),
    ("C", scenario((0.0001, 0.05), 1000.0, 10.0, 600.0), [
        [0.095118694, 1.31548107, 10.2422581, 88.0441818, 345.618958, 600.007385],
        [0.0601626113, 1.07564402, 10.2422581, 95.8863525, 378.685211, 600.008728],
        [0.0965468362, 1.41477215, 10.7678232, 100.494034, 496.551514, 600.007385]]),
    ("D", scenario((0.005, 0.005), 1000.0, 0.0, 600.0), [
        [0.00966971926, 0.887139618, 8.93032074, 82.5178909, 338.869354, 600.007385],
        [0.00949709956, 0.899500489, 8.9503088, 85.6712952, 359.910919, 600.008728],
        [0.00999996811, 0.999990642, 9.99990177, 99.9997253, 496.527283, 600.007385]]),
    ("E", scenario((0.005, 0.005), 1000.0, 400.0, 600.0), [
        [0.00919835269, 0.709068418, 6.66499805, 65.0159683, 321.200043, 600.007385],
        [0.00837275293, 0.676094115, 6.73879242, 66.6293259, 317.553314, 600.008728],
        [0.00999996811, 0.999990642, 9.99990177, 99.9997253, 496.527283, 600.007385]]),
    // G fails on a libplacebo without the knee fade (before 0321aab1, v7.387.0).
    ("G", scenario((0.0001, 0.5), 1000.0, 1.0, 100.0), [
        [0.576858938, 2.00967956, 9.05726814, 34.5562096, 74.8712387, 99.9997253],
        [0.515150547, 2.0096736, 13.8882971, 63.2865753, 93.9300537, 99.9957886],
        [0.600525975, 2.34124708, 12.0316334, 70.4099731, 99.2632294, 99.9997253]]),
];

#[test]
fn rows_a_to_g_match_the_standalone_oracle() {
    for (name, s, rows) in &ORACLE_ROWS {
        for (curve, row) in Curve::ALL.into_iter().zip(rows) {
            assert_row(name, curve, s, &ROW_POINTS, &row[..5]);
            assert_row(name, curve, s, &[s.l1_max_nits], &row[5..]);
        }
    }
}

/// The gate scenario as the standalone harness computes it (`harness.c` on v7.387.0): L1 max the
/// f32 PQ of 1000 nits plus the offset over 4095, avg 10 nits, blacks 0.005, a 1000-nit display;
/// change against the input nits (the reference is the identity there), four decimals.
#[rustfmt::skip]
const HARNESS_GATE_ROWS: [(Curve, f32, [f64; 4]); 6] = [
    (Curve::Spline, 75.0, [-3.5383, -5.8513, -7.8096, -14.5774]),
    (Curve::St2094_10, 75.0, [-3.5385, -4.5708, -5.6882, -13.7954]),
    (Curve::Bt2390, 75.0, [-0.0010, -0.0003, 0.0024, -2.4561]),
    (Curve::Spline, 140.0, [-6.5001, -10.3991, -13.6483, -24.3785]),
    (Curve::St2094_10, 140.0, [-6.5001, -8.1980, -10.0132, -22.3032]),
    (Curve::Bt2390, 140.0, [-0.0010, -0.0003, 0.0024, -4.5369]),
];

#[test]
fn the_gate_scenario_matches_the_standalone_harness_to_0_005_points() {
    let display = |peak| Display {
        source_black_nits: 0.005,
        panel_black_nits: 0.005,
        panel_peak_nits: peak,
    };
    let avg_code = nits_to_pq(10.0) * 4095.0;
    for (curve, offset, row) in HARNESS_GATE_ROWS {
        let in_max = crate::lp::nits_to_pq(1000.0) + offset / 4095.0;
        let mapping = Mapping::new(
            curve,
            &display(1000.0),
            &Metadata {
                max_code: f64::from(in_max) * 4095.0,
                avg_code,
            },
        )
        .unwrap();
        for (nits, want) in [10.0, 100.0, 200.0, 1000.0].into_iter().zip(row) {
            let got = (mapping.displayed_nits(nits_to_pq(nits)) / nits - 1.0) * 100.0;
            assert!(
                (got - want).abs() <= 0.005,
                "{} +{offset} at {nits} nits: {got:.4}% vs the harness {want}%",
                curve.name()
            );
        }
    }
    // The 600-nit display, L1 max 100 codes below PQ(1000): candidate over reference.
    let metadata = |max: f32| Metadata {
        max_code: f64::from(max) * 4095.0,
        avg_code,
    };
    let peak = crate::lp::nits_to_pq(1000.0);
    let reference = Mapping::new(Curve::Spline, &display(600.0), &metadata(peak)).unwrap();
    let candidate = Mapping::new(
        Curve::Spline,
        &display(600.0),
        &metadata(peak - 100.0 / 4095.0),
    )
    .unwrap();
    for (nits, want) in [
        (10.0, 4.847),
        (20.0, 4.893),
        (50.0, 6.293),
        (100.0, 8.355),
        (200.0, 11.298),
    ] {
        let x = nits_to_pq(nits);
        let got = (candidate.displayed_nits(x) / reference.displayed_nits(x) - 1.0) * 100.0;
        assert!(
            (got - want).abs() <= 0.005,
            "600 nits, -100 at {nits} nits: {got:.4}%"
        );
    }
}

/// Rows H, I1 and I2 of `harness6.c`: L1 max below the panel peak or a panel black of 0, at
/// 0.01, 1, 10, 100, 500, 600, 800, 1000 and 1200 nits.
#[rustfmt::skip]
const RENDERER_ROWS: [(&str, Scenario, [[f64; 9]; 3]); 3] = [
    ("H", scenario((0.0001, 0.05), 600.0, 10.0, 1000.0), [
        [0.0987104326, 1.46976936, 11.4413137, 107.303886, 511.807983, 610.526917, 806.266663, 1000.04376, 1000.04376],
        [0.0612560175, 1.18653643, 11.393075, 111.203392, 510.272888, 600.008728, 600.008728, 600.008728, 600.008728],
        [0.0952722132, 1.37901342, 10.6145468, 100.238289, 500.012665, 600.007385, 600.007385, 600.007385, 600.007385]]),
    ("I1", scenario((0.005, 0.0), 1000.0, 10.0, 600.0), [
        [0.00106364931, 0.765402675, 8.52969265, 80.212265, 335.542816, 391.778839, 498.796112, 600.007385, 600.007385],
        [0.00428522285, 0.852410555, 8.52969265, 82.1842422, 352.946869, 409.065552, 510.611237, 600.008728, 600.008728],
        [0.0011845181, 0.874736845, 9.73190975, 99.8191986, 496.526062, 556.061951, 596.186523, 600.007385, 600.007385]]),
    ("I2", scenario((0.005, 0.0), 400.0, 10.0, 600.0), [
        [0.00117808767, 0.870252192, 9.55918312, 97.856514, 498.77179, 600.007385, 600.007385, 600.007385, 600.007385],
        [0.00477778073, 0.95088166, 9.56122971, 96.626297, 399.990723, 399.990723, 399.990723, 399.990723, 399.990723],
        [0.00120901945, 0.89059186, 9.82184505, 99.9586639, 399.991455, 399.991455, 399.991455, 399.991455, 399.991455]]),
];

#[test]
fn renderer_steps_match_the_oracle_below_the_panel_peak_and_on_a_zero_black() {
    for (name, s, rows) in &RENDERER_ROWS {
        for (curve, row) in Curve::ALL.into_iter().zip(rows) {
            let mapping = mapping(curve, s);
            assert!(
                !mapping.is_noop(),
                "row {name} {}: blacks differ",
                curve.name()
            );
            assert_row(name, curve, s, &H_POINTS, row);
        }
    }
}

/// The curve clamps its input at input_max, so above it every value displays the same: the
/// panel peak for the spline (input_max raised to it), L1 max for bt2390 and st2094-10 (their
/// display peak lowered to it). Row H.
#[test]
fn the_clip_ceiling_is_where_the_curve_turns_flat() {
    let (_, s, _) = &RENDERER_ROWS[0];
    for (curve, ceiling_nits) in [
        (Curve::Spline, 1000.0),
        (Curve::St2094_10, 600.0),
        (Curve::Bt2390, 600.0),
    ] {
        let mapping = mapping(curve, s);
        let ceiling = mapping.clip_ceiling();
        assert!(
            (ceiling - nits_to_pq(ceiling_nits)).abs() < 1e-6,
            "{}: ceiling {ceiling}",
            curve.name()
        );
        let at = mapping.displayed_nits(ceiling);
        assert!(mapping.displayed_nits(ceiling - 0.01) < at - 1.0);
        for above in [ceiling + 1e-4, ceiling + 0.02, 1.0] {
            assert_eq!(mapping.displayed_nits(above), at, "{}", curve.name());
        }
    }
}

/// With equal blacks and L1 max at or below the panel peak the renderer skips the curve: the
/// display shows the source and clips at its own peak, also where bt2390 and st2094-10 lowered
/// their display peak to L1 max.
#[test]
fn equal_blacks_below_the_panel_peak_are_a_no_op() {
    let s = scenario((0.005, 0.005), 600.0, 10.0, 1000.0);
    for curve in Curve::ALL {
        let mapping = mapping(curve, &s);
        assert!(mapping.is_noop(), "{}", curve.name());
        for nits in [0.005, 0.01, 1.0, 100.0, 600.0, 999.0] {
            let x = nits_to_pq(nits);
            assert_eq!(
                mapping.displayed_nits(x),
                crate::lp::pq_to_nits(x as f32) as f64
            );
        }
        let peak = mapping.displayed_nits(nits_to_pq(1000.0));
        assert_eq!(mapping.displayed_nits(1.0), peak, "clipped at the panel");
        assert_eq!(mapping.clip_ceiling(), nits_to_pq(1000.0));
        // Below the display black the display shows its black.
        assert_eq!(
            mapping.displayed_nits(0.0),
            mapping.displayed_nits(nits_to_pq(0.005))
        );
    }
}

/// Row J: bin 0 on an active curve displays the display black, as a no-op does.
#[test]
fn bin_zero_displays_the_black_on_an_active_curve() {
    let s = scenario((0.005, 0.005), 1000.0, 10.0, 600.0);
    for (curve, oracle) in Curve::ALL
        .into_iter()
        .zip([0.00499998825, 0.004999917, 0.00499998825])
    {
        let mapping = mapping(curve, &s);
        assert!(!mapping.is_noop());
        let got = mapping.displayed_nits(0.0);
        assert!((got / oracle - 1.0).abs() < 5e-4, "{}: {got}", curve.name());
    }
}

#[test]
fn parameters_follow_the_renderer() {
    let below = scenario((0.0001, 0.05), 600.0, 10.0, 1000.0);
    let spline = mapping(Curve::Spline, &below).params();
    assert_eq!(spline.knee_offset, 1.0);
    assert!((f64::from(spline.input_max) - nits_to_pq(1000.0)).abs() < 1e-6);
    assert!((f64::from(spline.output_max) - nits_to_pq(1000.0)).abs() < 1e-6);
    let bt2390 = mapping(Curve::Bt2390, &below).params();
    assert_eq!(bt2390.knee_offset, 0.5, "the ITU knee offset");
    assert!((f64::from(bt2390.input_max) - nits_to_pq(600.0)).abs() < 1e-6);
    assert!((f64::from(bt2390.output_max) - nits_to_pq(600.0)).abs() < 1e-6);
    // The average is clamped into the source range; 0 stays absent.
    let high_avg = Mapping::new(
        Curve::Spline,
        &Display {
            source_black_nits: 0.005,
            panel_black_nits: 0.005,
            panel_peak_nits: 600.0,
        },
        &Metadata {
            max_code: 3000.0,
            avg_code: 3500.0,
        },
    )
    .unwrap()
    .params();
    assert_eq!(high_avg.input_avg, 3000.0 / 4095.0);
    assert_eq!(
        mapping(Curve::Spline, &ORACLE_ROWS[3].1).params().input_avg,
        0.0
    );
    // A zero display black is true black (the renderer's signal-black rule).
    let zero = mapping(Curve::Spline, &RENDERER_ROWS[1].1).params();
    assert_eq!(zero.output_min, 0.0);
}

/// libplacebo 7.387.0 `pl_color_space_nominal_luma_ex` (src/colorspace.c:901-916): a peak of 0 is
/// absent and falls back to the PQ peak (1.0) while the black stays; a black at or above a
/// nonzero peak drops both, so the black becomes `PL_COLOR_HDR_BLACK` (1e-6 nits), which the
/// renderer's signal-black rule then reads as 0. Expected values come from that code, not from
/// the shim.
#[test]
fn an_absent_or_inverted_l1_max_falls_back_to_the_pq_peak() {
    for (label, max_code, source_black, expected_min) in [
        ("L1 max 0", 0.0, 0.005, nits_to_pq(0.005) as f32),
        ("L1 max below the source black", 50.0, 0.1, 0.0),
        (
            "L1 max at the source black",
            nits_to_pq(0.1) * 4095.0,
            0.1,
            0.0,
        ),
    ] {
        for curve in [Curve::Spline, Curve::St2094_10, Curve::Bt2390] {
            let params = Mapping::new(
                curve,
                &Display {
                    source_black_nits: source_black,
                    panel_black_nits: 0.005,
                    panel_peak_nits: 1000.0,
                },
                &Metadata {
                    max_code,
                    avg_code: 0.0,
                },
            )
            .unwrap()
            .params();
            assert_eq!(params.input_max, 1.0, "{label}, {curve:?}: input_max");
            assert!(
                (params.input_min - expected_min).abs() < 1e-6,
                "{label}, {curve:?}: input_min {} != {expected_min}",
                params.input_min
            );
        }
    }
}

fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// The knee libplacebo's st2094-10 and spline pass through, from the equations in the WS7.1
/// plan (written from `st2094_pick_knee`), in PQ.
fn knee(input: [f64; 3], output: [f64; 2]) -> (f64, f64) {
    let [in_min, in_max, in_avg] = input;
    let [out_min, out_max] = output;
    let source = if in_avg > 0.0 {
        in_avg
    } else {
        mix(in_min, in_max, 0.4)
    }
    .clamp(mix(in_min, in_max, 0.1), mix(in_min, in_max, 0.8));
    let t = (source - in_min) / (in_max - in_min);
    let adapted = mix(out_min, out_max, t);
    let tuning = 1.0 - smoothstep(0.8, 0.4, t) * smoothstep(0.1, 0.4, t);
    let adaptation = mix(0.4, 1.0, tuning);
    let adaptation = mix(
        1.0,
        adaptation,
        smoothstep(out_min, mix(out_min, out_max, 0.1), source),
    );
    let target = mix(source, adapted, adaptation)
        .clamp(mix(out_min, out_max, 0.1), mix(out_min, out_max, 0.8));
    (source, target)
}

/// libplacebo's st2094-10 is a rational curve through three anchors: (source black, display
/// black), the knee, and (source peak, display peak). Checked on every oracle row, besides the
/// intermediate points above.
#[test]
fn st2094_10_passes_its_three_anchors() {
    let nits = |pq: f64| f64::from(crate::lp::pq_to_nits(pq as f32));
    for (name, s) in ORACLE_ROWS
        .iter()
        .map(|(name, s, _)| (name, s))
        .chain(RENDERER_ROWS.iter().map(|(name, s, _)| (name, s)))
    {
        let mapping = mapping(Curve::St2094_10, s);
        let p = mapping.params();
        let [in_min, in_max, in_avg, out_min, out_max] = [
            p.input_min,
            p.input_max,
            p.input_avg,
            p.output_min,
            p.output_max,
        ]
        .map(f64::from);
        let (source_knee, target_knee) = knee([in_min, in_max, in_avg], [out_min, out_max]);
        for (x, want) in [
            (in_min, out_min),
            (source_knee, target_knee),
            (in_max, out_max),
        ] {
            let (got, want) = (mapping.displayed_nits(x), nits(want));
            assert!(
                (got - want).abs() <= 1e-3 * want.max(0.01),
                "row {name}: x {x} displays {got} nits, anchor {want}"
            );
        }
    }
}

/// Each mapping owns its libplacebo context: mappings built and sampled on several threads
/// give the single-threaded values bit for bit.
#[test]
fn mappings_on_several_threads_match_one_thread() {
    let sample = |curve: Curve, s: &Scenario| -> Vec<u64> {
        let mapping = mapping(curve, s);
        (0..4096)
            .map(|bin| mapping.displayed_nits(f64::from(bin) / 4095.0).to_bits())
            .collect()
    };
    let expected: Vec<Vec<u64>> = ORACLE_ROWS
        .iter()
        .flat_map(|(_, s, _)| Curve::ALL.map(|curve| sample(curve, s)))
        .collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    ORACLE_ROWS
                        .iter()
                        .flat_map(|(_, s, _)| Curve::ALL.map(|curve| sample(curve, s)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            assert_eq!(handle.join().unwrap(), expected);
        }
    });
}

#[test]
fn invalid_numbers_are_refused_before_libplacebo() {
    let display = |source, black, peak| Display {
        source_black_nits: source,
        panel_black_nits: black,
        panel_peak_nits: peak,
    };
    for (bad, message) in [
        (display(0.005, 0.005, 0.0), "panel peak 0"),
        (display(0.005, 0.005, 10_001.0), "panel peak 10001"),
        (display(0.005, 0.005, f64::NAN), "panel peak NaN"),
        (display(-0.1, 0.005, 600.0), "source black -0.1"),
        (display(0.005, 600.0, 600.0), "panel black 600"),
        (display(150.0, 0.005, 1000.0), "source black 150"),
        (display(0.005, f64::INFINITY, 600.0), "panel black inf"),
    ] {
        let error = bad.validate().unwrap_err().to_string();
        assert!(error.contains(message), "{bad:?}: {error}");
    }
    display(0.0, 0.0, 10_000.0).validate().unwrap();
    for metadata in [
        Metadata {
            max_code: 4095.5,
            avg_code: 0.0,
        },
        Metadata {
            max_code: 3000.0,
            avg_code: -1.0,
        },
        Metadata {
            max_code: f64::NAN,
            avg_code: 0.0,
        },
    ] {
        assert!(metadata.validate().is_err(), "{metadata:?}");
    }
}

#[test]
fn the_linked_library_is_7_387_0() {
    crate::lp::check_version().unwrap();
    assert!(
        crate::lp::version().contains("7.387"),
        "{}",
        crate::lp::version()
    );
}
