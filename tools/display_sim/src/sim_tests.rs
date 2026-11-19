use super::*;

/// A response with the given displayed nits per listed bin (others 1 nit), levels and first
/// clipped bin, so the arithmetic is checked without libplacebo.
fn response(bins: &[(usize, f64)], levels: [f64; 3], first_clipped: usize) -> Response {
    let mut nits = vec![1.0; BINS];
    for &(bin, value) in bins {
        nits[bin] = value;
    }
    Response {
        bins: nits,
        levels,
        first_clipped,
        noop: false,
    }
}

fn counts(pixels: &[(usize, u32)]) -> Vec<u32> {
    let mut counts = vec![0; BINS];
    for &(bin, count) in pixels {
        counts[bin] = count;
    }
    counts
}

#[test]
fn the_first_clipped_bin_is_half_a_bin_above_the_ceiling() {
    // 4095 x PQ(1000 nits) is 3078.74: bin 3079 (from 3078.5) is not clipped, 3080 is.
    assert_eq!(first_clipped_bin(3078.74 / 4095.0), 3080);
    assert_eq!(first_clipped_bin(3079.0 / 4095.0), 3080);
    assert_eq!(first_clipped_bin(3079.6 / 4095.0), 3081);
    assert_eq!(first_clipped_bin(0.0), 1);
    assert_eq!(first_clipped_bin(1.0), BINS);
}

#[test]
fn a_frame_is_weighted_by_its_pixels() {
    let reference = response(&[(100, 2.0), (3000, 400.0)], [10.0, 100.0, 200.0], 3001);
    let candidate = response(&[(100, 3.0), (3000, 300.0)], [11.0, 90.0, 200.0], 2900);
    let frame = compare_frame(&counts(&[(100, 6), (3000, 2)]), &reference, &candidate);
    assert_eq!(frame.pixels, 8);
    assert_eq!(frame.light_reference, 6.0 * 2.0 + 2.0 * 400.0);
    assert_eq!(frame.light_candidate, 6.0 * 3.0 + 2.0 * 300.0);
    assert_eq!(frame.light_change(), Some(618.0 / 812.0 - 1.0));
    assert_eq!(frame.peak_bin, Some(3000));
    assert_eq!(frame.peak_change, Some(300.0 / 400.0 - 1.0));
    assert_eq!((frame.clipped_reference, frame.clipped_candidate), (0, 2));
    let levels = frame.level_changes();
    assert_eq!(levels[0], Some(11.0 / 10.0 - 1.0));
    assert_eq!(levels[2], Some(0.0));
}

#[test]
fn zero_denominators_give_no_change() {
    let dark = response(&[(0, 0.0)], [10.0, 100.0, 200.0], BINS);
    let frame = compare_frame(&counts(&[(0, 5)]), &dark, &dark);
    assert_eq!(frame.light_change(), None);
    assert_eq!(frame.peak_change, None);
    let empty = compare_frame(&counts(&[]), &dark, &dark);
    assert_eq!((empty.pixels, empty.peak_bin), (0, None));
    let mut summary = Summary::default();
    summary.add(&empty);
    assert_eq!(summary.light_change(), None);
    assert_eq!(summary.clipped_shares(), None);
    assert_eq!(summary.frame_light.digest(), None);
}

fn frame_with_levels(reference: [f64; 3], candidate: [f64; 3]) -> FrameResult {
    FrameResult {
        level_reference: reference,
        level_candidate: candidate,
        peak_bin: None,
        peak_change: None,
        light_reference: 0.0,
        light_candidate: 0.0,
        pixels: 0,
        clipped_reference: 0,
        clipped_candidate: 0,
        noop: (false, false),
    }
}

#[test]
fn steps_count_both_sides_and_what_the_candidate_adds() {
    let first = frame_with_levels([10.0, 100.0, 200.0], [10.0, 100.0, 200.0]);
    // Reference: +2 % at 10 nits, +0.5 % at 100, 0 at 200. Candidate: +2 %, -3 %, +0.5 %.
    let second = frame_with_levels([10.2, 100.5, 200.0], [10.2, 97.0, 201.0]);
    let mut steps = Steps::default();
    steps.add(&first, &second, 0.01);
    assert_eq!(steps.pairs, 1);
    assert_eq!((steps.above_reference, steps.above_candidate), (1, 2));
    assert_eq!(steps.added, 1, "only the -3 % step is the candidate's own");
    assert!((steps.max_reference - 0.02).abs() < 1e-12);
    assert!((steps.max_candidate - 0.03).abs() < 1e-12);
}

#[test]
fn spreads_give_min_lower_median_and_max() {
    let mut spread = Spread::default();
    for value in [0.3, -0.1, 0.2, 0.0] {
        spread.push(Some(value));
    }
    spread.push(None);
    assert_eq!(spread.digest(), Some([-0.1, 0.0, 0.3]));
}

/// Two frames through libplacebo, every expectation from the standalone oracle (`harness6.c`
/// row K): the reference is L1 max 2800, avg 1500 codes on a 600-nit display, below its peak and
/// so a no-op (the display clips at 600.007385 nits, PQ(600) in f32); the candidate equals it
/// on frame 0 and is L1 max 3200, avg 1500 on frame 1 (the spline compresses).
#[test]
fn two_frames_aggregate_light_clipping_and_steps() {
    let display = Display {
        source_black_nits: 0.005,
        panel_black_nits: 0.005,
        panel_peak_nits: 600.0,
    };
    let reference_metadata = Metadata {
        max_code: 2800.0,
        avg_code: 1500.0,
    };
    let compressed = Metadata {
        max_code: 3200.0,
        avg_code: 1500.0,
    };
    let reference = Response::new(Curve::Spline, &display, &reference_metadata).unwrap();
    let candidate = Response::new(Curve::Spline, &display, &compressed).unwrap();
    let pixels = [(1000, 3u32), (2000, 2), (2500, 1), (2900, 1), (3500, 1)];
    let histogram = counts(&pixels);
    // Displayed nits of bins 1000, 2000, 2500, 2900, 3500 and of 10 / 100 / 200 nits.
    let reference_nits = [4.74743128, 82.0855789, 269.51532, 600.007385, 600.007385];
    let candidate_nits = [3.84731364, 64.256958, 179.448563, 367.066376, 600.007385];
    let reference_levels = [9.99990177, 99.9997253, 200.004837];
    let candidate_levels = [8.13171577, 76.7752991, 140.077087];
    for (index, &(bin, _)) in pixels.iter().enumerate() {
        for (got, want) in [
            (reference.bins[bin], reference_nits[index]),
            (candidate.bins[bin], candidate_nits[index]),
        ] {
            assert!(
                (got / want - 1.0).abs() < 5e-4,
                "bin {bin}: {got} vs {want}"
            );
        }
    }

    let frames = [
        compare_frame(&histogram, &reference, &reference),
        compare_frame(&histogram, &reference, &candidate),
    ];
    let mut run = Summary::default();
    for frame in &frames {
        run.add(frame);
    }
    run.steps.add(&frames[0], &frames[1], 0.01);

    let light = |nits: &[f64]| -> f64 {
        pixels
            .iter()
            .zip(nits)
            .map(|(&(_, count), &nits)| f64::from(count) * nits)
            .sum()
    };
    let (reference_light, candidate_light) = (light(&reference_nits), light(&candidate_nits));
    let frame_change = candidate_light / reference_light - 1.0;
    let run_change = (reference_light + candidate_light) / (2.0 * reference_light) - 1.0;
    let close = |got: Option<f64>, want: f64| (got.unwrap() - want).abs() < 2e-4;
    assert!(
        close(frames[1].light_change(), frame_change),
        "{:?}",
        frames[1]
    );
    assert!(
        close(run.light_change(), run_change),
        "{:?}",
        run.light_change()
    );
    assert_eq!(frames[0].light_change(), Some(0.0));
    assert_eq!(
        (frames[1].noop, run.noop_reference, run.noop_candidate),
        ((true, false), 2, 1)
    );

    // PQ(600) is code 2851.4: bins 2900 and 3500 clip on the no-op; the candidate's curve
    // clips above its L1 max 3200, so only bin 3500.
    assert_eq!(run.clipped_shares(), Some((4.0 / 16.0, 3.0 / 16.0)));

    // The candidate steps at all three levels from frame 0 to 1, the reference at none.
    let steps = &run.steps;
    assert_eq!(
        (
            steps.pairs,
            steps.above_reference,
            steps.above_candidate,
            steps.added
        ),
        (1, 0, 3, 3)
    );
    let largest = (0..3)
        .map(|level| (candidate_levels[level] / reference_levels[level] - 1.0_f64).abs())
        .fold(0.0, f64::max);
    assert!((steps.max_candidate - largest).abs() < 2e-4, "{steps:?}");
    // Of the frame changes 0 and frame 1's, the lower median is frame 1's.
    let [_, median, _] = run.levels[1].digest().unwrap();
    let want = candidate_levels[1] / reference_levels[1] - 1.0;
    assert!((median - want).abs() < 2e-4, "{median} vs {want}");
}
