//! The WS7 item gate (ROADMAP.md WS7; numbers in docs/ROADMAP_LOG.md "WS7 (Open)", recomputed
//! 2026-10-03): the simulator reproduces, within 0.06 percentage points of the printed numbers
//! (0.1 for integer-code input), the change of displayed luminance at 10 / 100 / 200 nits and at
//! the peak for a 1000-nit shot on a 1000-nit display: spline, L1 max +75 codes
//! -3.5 / -5.9 / -7.8 / -14.6 %, +140 codes -6.5 / -10.4 / -13.6 / -24.4 %; BT.2390 -2.5 % /
//! -4.5 % at the peak and nothing below 200 nits; nothing while L1 max stays below the display
//! peak; and on a 600-nit display an L1 max 100 codes low brightens mid-tones by 5 to 11 % and
//! clips above it.
//!
//! The printed numbers fix the scenario only partly; it was reconstructed (WS7.1 plan): L1 avg 10
//! nits, source and display black 0.005 nits, libplacebo's default constants (BT.2390 knee
//! offset 0.5), and the shot's L1 max the un-rounded code of 1000 nits (4095 x PQ(1000), about
//! 3078.74) plus the offset.

use super::*;
use crate::sim::{compare_frame, Response};

const LEVELS: [f64; 4] = [10.0, 100.0, 200.0, 1000.0];

fn code(nits: f64) -> f64 {
    4095.0 * nits_to_pq(nits)
}

fn display(panel: f64) -> Display {
    Display {
        source_black_nits: 0.005,
        panel_black_nits: 0.005,
        panel_peak_nits: panel,
    }
}

fn mapping(curve: Curve, panel: f64, max_code: f64) -> Mapping {
    Mapping::new(
        curve,
        &display(panel),
        &Metadata {
            max_code,
            avg_code: code(10.0),
        },
    )
    .unwrap()
}

/// Change of displayed nits in percent at `nits` when L1 max goes from `reference` to
/// `candidate` codes.
fn change_pct(curve: Curve, panel: f64, reference: f64, candidate: f64, nits: f64) -> f64 {
    let x = nits_to_pq(nits);
    let reference = mapping(curve, panel, reference).displayed_nits(x);
    let candidate = mapping(curve, panel, candidate).displayed_nits(x);
    (candidate / reference - 1.0) * 100.0
}

fn assert_close(what: &str, got: f64, printed: f64, tolerance: f64) {
    assert!(
        (got - printed).abs() <= tolerance,
        "{what}: {got:.4}% against the printed {printed}% (tolerance {tolerance} pp)"
    );
    println!("{what}: {got:.4}% (printed {printed}%)");
}

#[test]
fn gate_spline_on_a_1000_nit_display() {
    let shot = code(1000.0);
    for (offset, printed) in [
        (75.0, [-3.5, -5.9, -7.8, -14.6]),
        (140.0, [-6.5, -10.4, -13.6, -24.4]),
    ] {
        for (nits, printed) in LEVELS.into_iter().zip(printed) {
            let got = change_pct(Curve::Spline, 1000.0, shot, shot + offset, nits);
            assert_close(
                &format!("spline +{offset} at {nits} nits"),
                got,
                printed,
                0.06,
            );
        }
    }
}

#[test]
fn gate_spline_with_integer_codes() {
    let shot = code(1000.0).round();
    for (offset, printed) in [
        (75.0, [-3.5, -5.9, -7.8, -14.6]),
        (140.0, [-6.5, -10.4, -13.6, -24.4]),
    ] {
        for (nits, printed) in LEVELS.into_iter().zip(printed) {
            let got = change_pct(Curve::Spline, 1000.0, shot, shot + offset, nits);
            assert_close(
                &format!("spline, codes {shot}+{offset}, at {nits} nits"),
                got,
                printed,
                0.1,
            );
        }
    }
}

#[test]
fn gate_bt2390_changes_only_the_peak() {
    let shot = code(1000.0);
    for (offset, printed) in [(75.0, -2.5), (140.0, -4.5)] {
        let peak = change_pct(Curve::Bt2390, 1000.0, shot, shot + offset, 1000.0);
        assert_close(
            &format!("bt2390 +{offset} at the peak"),
            peak,
            printed,
            0.06,
        );
        for nits in [10.0, 100.0, 200.0] {
            let got = change_pct(Curve::Bt2390, 1000.0, shot, shot + offset, nits);
            assert!(got.abs() < 0.005, "bt2390 +{offset} at {nits} nits: {got}%");
        }
    }
}

/// L1 max 100 codes below the 1000-nit shot on a 600-nit display: the spline brightens mid-tones
/// by 5 to 11 % (printed as whole percents) and clips what lies above the lowered L1 max.
#[test]
fn gate_600_nit_display_with_l1_max_100_codes_low() {
    let shot = code(1000.0);
    for (nits, printed) in [(10.0, 5.0), (100.0, 8.0), (200.0, 11.0)] {
        let got = change_pct(Curve::Spline, 600.0, shot, shot - 100.0, nits);
        assert_eq!(got.round(), printed, "spline at {nits} nits: {got}%");
        println!("spline -100 on 600 nits at {nits} nits: {got:.4}% (printed {printed}%)");
    }
    let metadata = |max_code| Metadata {
        max_code,
        avg_code: code(10.0),
    };
    let reference = Response::new(Curve::Spline, &display(600.0), &metadata(shot)).unwrap();
    let candidate = Response::new(Curve::Spline, &display(600.0), &metadata(shot - 100.0)).unwrap();
    // The shot reaches 1000 nits (bin 3079); 100 pixels lie above the lowered L1 max.
    let mut counts = vec![0u32; 4096];
    counts[2000] = 900;
    counts[3000] = 50;
    counts[3079] = 50;
    let frame = compare_frame(&counts, &reference, &candidate);
    assert_eq!((frame.clipped_reference, frame.clipped_candidate), (0, 100));
    assert_eq!(
        candidate.bins[2990], candidate.bins[3079],
        "flat above L1 max"
    );
}

/// A 600-nit shot on a 1000-nit display: raising L1 max by 75 or 140 codes stays below the
/// display peak and changes nothing on any curve; 300 codes cross it and change the picture.
#[test]
fn gate_l1_max_below_the_display_peak_changes_nothing() {
    let shot = code(600.0);
    for curve in Curve::ALL {
        for offset in [75.0, 140.0] {
            assert!(mapping(curve, 1000.0, shot + offset).is_noop());
            for nits in [10.0, 100.0, 200.0, 600.0] {
                let got = change_pct(curve, 1000.0, shot, shot + offset, nits);
                assert_eq!(got, 0.0, "{} +{offset} at {nits} nits", curve.name());
            }
        }
        assert!(!mapping(curve, 1000.0, shot + 300.0).is_noop());
        // The spline and st2094-10 lower the shot's peak; BT.2390 keeps everything below its
        // knee and compresses only near the display peak.
        let nits = if curve == Curve::Bt2390 {
            1000.0
        } else {
            600.0
        };
        let got = change_pct(curve, 1000.0, shot, shot + 300.0, nits);
        assert!(got < -0.5, "{} +300 at {nits} nits: {got}%", curve.name());
    }
}

/// L1 max 600 nits for a 1000-nit shot on a 1000-nit display: a no-op on every curve, so the
/// shot displays unchanged and its 1000-nit pixels are not clipped (bin 3079 lies within half a
/// bin of the display peak).
#[test]
fn l1_max_below_the_display_peak_on_a_1000_nit_shot_clips_nothing() {
    let metadata = |max_code| Metadata {
        max_code,
        avg_code: code(10.0),
    };
    let mut counts = vec![0u32; 4096];
    counts[1500] = 10;
    counts[2851] = 5;
    counts[3079] = 3;
    for curve in Curve::ALL {
        let reference = Response::new(curve, &display(1000.0), &metadata(code(1000.0))).unwrap();
        let candidate = Response::new(curve, &display(1000.0), &metadata(code(600.0))).unwrap();
        let frame = compare_frame(&counts, &reference, &candidate);
        assert_eq!(frame.peak_change, Some(0.0), "{}", curve.name());
        assert_eq!(frame.light_change(), Some(0.0), "{}", curve.name());
        assert_eq!(
            (frame.clipped_reference, frame.clipped_candidate),
            (0, 0),
            "{}",
            curve.name()
        );
    }
}
