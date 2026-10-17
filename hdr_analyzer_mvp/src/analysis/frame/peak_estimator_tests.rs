use super::*;

#[test]
fn high_percentile_mirrors_low_percentile() {
    let mut histogram = [0u64; 100];
    for bin in &mut histogram[10..=89] {
        *bin = 1;
    }
    assert_eq!(low_percentile_pq(&histogram, 10.0), 18.0 / 99.0);
    assert_eq!(high_percentile_pq(&histogram, 90.0), 81.0 / 99.0);
}

#[test]
fn sigma_from_constructed_gaussian_differences_is_within_five_percent() {
    let sigma_codes = 6.0;
    let mut state = 0x4d595df4d0f33173u64;
    let mut histogram = [[0u32; DIFF_BINS]; DIFF_VALUE_BANDS];
    let mut normal = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let first = state.wrapping_mul(0x2545f4914f6cdd1d);
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let second = state.wrapping_mul(0x2545f4914f6cdd1d);
        let u1 = ((first >> 11) as f64 + 1.0) / ((1u64 << 53) as f64 + 1.0);
        let u2 = ((second >> 11) as f64 + 1.0) / ((1u64 << 53) as f64 + 1.0);
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    };
    for _ in 0..100_000 {
        let difference = ((normal() - normal()).abs() * sigma_codes).round() as usize;
        histogram[12][difference.min(DIFF_BINS - 1)] += 1;
    }
    let mut pq_hist = [0u64; PQ_HIST_BINS];
    pq_hist[12 << 8] = 100_000;

    let measured_codes = sigma_from_diff_hist(&histogram, &pq_hist) * 4095.0;
    assert!((measured_codes - sigma_codes).abs() / sigma_codes < 0.05);
}

/// Upper-tail probability of the standard normal distribution (Abramowitz-Stegun 7.1.26).
fn normal_survival(z: f64) -> f64 {
    let magnitude = z.abs();
    let t = 1.0 / (1.0 + 0.231_641_9 * magnitude);
    let polynomial = t
        * (0.319_381_530
            + t * (-0.356_563_782
                + t * (1.781_477_937 + t * (-1.821_255_978 + t * 1.330_274_429))));
    let upper =
        polynomial * (-0.5 * magnitude * magnitude).exp() / (2.0 * std::f64::consts::PI).sqrt();
    if z >= 0.0 {
        upper
    } else {
        1.0 - upper
    }
}

/// Expected histogram of `count` pixels at `level` with Gaussian grain of `sigma` codes.
fn grainy_plateau(level: f64, sigma: f64, count: f64) -> [u64; PQ_HIST_BINS] {
    let mut histogram = [0u64; PQ_HIST_BINS];
    for (bin, slot) in histogram.iter_mut().enumerate() {
        let lower = (bin as f64 - 0.5 - level) / sigma;
        let upper = (bin as f64 + 0.5 - level) / sigma;
        *slot = (count * (normal_survival(lower) - normal_survival(upper))).round() as u64;
    }
    histogram
}

fn top_bin(histogram: &[u64]) -> usize {
    histogram.iter().rposition(|&count| count > 0).unwrap()
}

fn robust_code(histogram: &[u64], sigma: f64) -> f64 {
    let raw = top_bin(histogram) as f64 / 4095.0;
    robust_peak_pq(histogram, raw, sigma / 4095.0) * 4095.0
}

#[test]
fn normal_upper_quantile_matches_reference_values() {
    for (probability, z) in [
        (0.5, 0.0),
        (0.3, 0.524_400_513),
        (0.025, 1.959_963_985),
        (0.001, 3.090_232_306),
        (1.0e-6, 4.753_424_309),
        (0.7, -0.524_400_513),
        (0.999, -3.090_232_306),
    ] {
        let got = normal_upper_quantile(probability);
        assert!(
            (got - z).abs() < 1e-6,
            "Q^-1({probability}) = {got}, expected {z}"
        );
    }
}

#[test]
fn rank_level_is_the_bin_of_the_kth_brightest_pixel() {
    // Bins 10, 8 and 5 hold 2, 3 and 4 pixels; tail[d] counts bins >= 10 - d.
    let tail = [2u64, 2, 5, 5, 5, 9, 9, 9, 9, 9, 9];
    assert_eq!(rank_level(&tail, 10, 1), 10);
    assert_eq!(rank_level(&tail, 10, 2), 10);
    assert_eq!(rank_level(&tail, 10, 3), 8);
    assert_eq!(rank_level(&tail, 10, 5), 8);
    assert_eq!(rank_level(&tail, 10, 6), 5);
    assert_eq!(rank_level(&tail, 10, 9), 5);
    // More pixels than the histogram holds: the bottom bin.
    assert_eq!(rank_level(&tail, 10, 10), 0);
}

#[test]
fn grainy_plateau_is_corrected_to_its_level() {
    // The expected histogram of a plateau has no counting noise, so the fit is tight. Its
    // width is reduced by one sampling error, which leaves the level slightly low or high.
    for (sigma, count) in [(12.0, 1.0e4), (20.0, 1.0e5), (20.0, 4.0e6), (40.0, 1.0e6)] {
        let histogram = grainy_plateau(2000.0, sigma, count);
        let raw_code = top_bin(&histogram) as f64;
        let corrected = robust_code(&histogram, sigma);
        assert!(raw_code > 2000.0 + 2.0 * sigma);
        assert!(
            (corrected - 2000.0).abs() <= 0.5 * sigma + 1.0,
            "sigma {sigma}, {count} pixels: raw {raw_code}, corrected {corrected}"
        );
    }
}

/// The same plateau from a 10-bit source: only every 4095/876-th code occurs.
fn gridded_plateau(level: f64, sigma: f64, count: f64) -> [u64; PQ_HIST_BINS] {
    let step = 4095.0 / 876.0;
    let mut histogram = [0u64; PQ_HIST_BINS];
    for index in -200..=200 {
        let value = level + f64::from(index) * step;
        let lower = (value - 0.5 * step - level) / sigma;
        let upper = (value + 0.5 * step - level) / sigma;
        histogram[value.round() as usize] =
            (count * (normal_survival(lower) - normal_survival(upper))).round() as u64;
    }
    histogram
}

#[test]
fn code_grid_step_follows_the_spacing_of_occupied_bins() {
    let tail_of = |histogram: &[u64], top: usize| -> Vec<u64> {
        histogram[..=top]
            .iter()
            .rev()
            .scan(0u64, |sum, &count| {
                *sum += count;
                Some(*sum)
            })
            .collect()
    };
    let dense = grainy_plateau(2000.0, 20.0, 1.0e6);
    let top = top_bin(&dense);
    assert_eq!(code_grid_step(&dense, top, &tail_of(&dense, top)), 1);

    // Spacings alternate between 4 and 5 codes; the upper quartile is 5.
    let gridded = gridded_plateau(2000.0, 20.0, 1.0e6);
    let top = top_bin(&gridded);
    assert_eq!(code_grid_step(&gridded, top, &tail_of(&gridded, top)), 5);
}

#[test]
fn gridded_plateau_is_corrected_not_kept() {
    // The empty bins of the code grid are no gaps in the picture, and several pixels on
    // the top grid value are no flat highlight: the correction must not be skipped. The
    // two fitted levels are rounded to the grid, which moves the fitted width in steps of
    // about one grid step; where that step leaves the width just above sigma, part of the
    // excess stays (the upper bound is one sigma wider than for a dense plateau).
    for (sigma, count) in [(5.0, 6.0e4), (9.0, 6.0e4), (19.0, 6.0e4), (38.0, 1.0e6)] {
        let histogram = gridded_plateau(2000.0, sigma, count);
        let raw_code = top_bin(&histogram) as f64;
        let error = robust_code(&histogram, sigma) - 2000.0;
        assert!(raw_code > 2000.0 + 3.5 * sigma);
        assert!(
            error >= -(0.5 * sigma + 2.0) && error <= 1.5 * sigma,
            "sigma {sigma}, {count} pixels: raw {raw_code}, error {error}"
        );
    }
}

#[test]
fn picture_spread_wider_than_the_grain_is_mostly_kept() {
    // A top that is 10 times wider than the grain is picture: removing the grain variance
    // takes about z * sigma^2 / (2 * width) off the maximum, here under 3 codes.
    let histogram = grainy_plateau(2000.0, 100.0, 4.0e6);
    let raw_code = top_bin(&histogram) as f64;
    let corrected = robust_code(&histogram, 10.0);
    assert!(corrected < raw_code);
    assert!(
        raw_code - corrected < 5.0,
        "raw {raw_code}, corrected {corrected}"
    );
}

#[test]
fn flat_highlights_of_any_size_are_kept() {
    let sigma = 20.0 / 4095.0;
    let raw = 3534.0 / 4095.0;
    for size in [1u64, 4, 64, 1024, 100_000] {
        let mut histogram = grainy_plateau(2000.0, 20.0, 1.0e6);
        histogram[3534] = size;
        assert_eq!(robust_peak_pq(&histogram, raw, sigma), raw, "{size} pixels");
    }
}

#[test]
fn flat_highlight_just_above_a_grainy_plateau_is_kept() {
    // 64 flat pixels half a sigma above the highest grain pixel. The flat-top rule and the
    // gap test both keep them; the tests below separate the two.
    let mut histogram = grainy_plateau(2000.0, 40.0, 1.0e6);
    let highlight = top_bin(&histogram) + 20;
    histogram[highlight] = 64;
    assert_eq!(robust_code(&histogram, 40.0), highlight as f64);
}

#[test]
fn frames_without_measurable_grain_or_with_few_pixels_are_not_corrected() {
    let histogram = grainy_plateau(2000.0, 20.0, 1.0e6);
    let raw = top_bin(&histogram) as f64 / 4095.0;
    assert_eq!(robust_peak_pq(&histogram, raw, 0.2 / 4095.0), raw);
    assert_eq!(robust_peak_pq(&[0; PQ_HIST_BINS], 0.5, 20.0 / 4095.0), 0.5);

    // Too few pixels for the tail fit and no detached group: raw maximum.
    let small = grainy_plateau(2000.0, 20.0, 2000.0);
    let raw = top_bin(&small) as f64 / 4095.0;
    assert_eq!(robust_peak_pq(&small, raw, 20.0 / 4095.0), raw);
}

fn assert_code(actual: f64, expected: f64, case: &str) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "{case}: {actual}, expected {expected}"
    );
}

#[test]
fn code_grid_step_is_limited_on_a_narrow_top() {
    // Two occupied values above the 1024th brightest pixel and a highlight: the largest of
    // the two spacings is the gap below the highlight, which is not code grid.
    let mut histogram = gridded_plateau(2000.0, 3.0, 1.0e6);
    histogram[3500] = 1;
    let tail: Vec<u64> = histogram[..=3500]
        .iter()
        .rev()
        .scan(0u64, |sum, &count| {
            *sum += count;
            Some(*sum)
        })
        .collect();
    assert_eq!(code_grid_step(&histogram, 3500, &tail), MAX_CODE_GRID_STEP);
}

#[test]
fn flat_highlight_above_a_narrow_gridded_top_is_kept() {
    // A low-noise 10-bit plateau has few occupied values at its top. One pixel and a 2x2
    // highlight above it must not be read as part of the code grid. `read` is the sigma the
    // analyzer measures on such a plateau (a multiple of 1.048 codes).
    for (sigma, read) in [(3.0, 4.19), (5.0, 5.24), (8.0, 8.38)] {
        for count in [1.0e5, 1.0e6, 8.0e6] {
            for offset in [100, 500, 1500] {
                for size in [1u64, 4] {
                    let mut histogram = gridded_plateau(2000.0, sigma, count);
                    histogram[2000 + offset] = size;
                    assert_code(
                        robust_code(&histogram, read),
                        (2000 + offset) as f64,
                        &format!("sigma {sigma}, {count} pixels, +{offset}, {size} px"),
                    );
                }
            }
        }
    }
}

#[test]
fn flat_top_rule_keeps_a_few_flat_pixels_that_no_gap_separates() {
    // Flat pixels 3 codes above the highest grain pixel at sigma 40: the gap is far too
    // small for the gap test, so only the flat-top rule can keep them. It needs 4 pixels
    // here; two are not enough and are corrected as grain (a documented limit).
    let plateau = grainy_plateau(2000.0, 40.0, 1.0e6);
    let highlight = top_bin(&plateau) + 3;
    for size in [4u64, 8] {
        let mut histogram = plateau;
        histogram[highlight] = size;
        assert_code(
            robust_code(&histogram, 40.0),
            highlight as f64,
            &format!("{size} px"),
        );
    }
    let mut histogram = plateau;
    histogram[highlight] = 2;
    assert!(robust_code(&histogram, 40.0) < highlight as f64 - 40.0);
}

#[test]
fn gap_test_decides_for_one_and_two_pixel_highlights() {
    // Sigma 20, dense plateau. Too few pixels for the flat-top rule, so the gap decides:
    // one pixel 100 codes (5 sigma) and two pixels 60 codes above the highest grain pixel
    // are kept. Two pixels 30 codes above it and four pixels 10 codes above it are within
    // the reach of the grain and are corrected as grain (a documented limit); the
    // correction then stops at 5.2 sigma.
    let sigma = 20.0;
    let plateau = grainy_plateau(2000.0, sigma, 1.0e6);
    let grain_top = top_bin(&plateau);
    for (size, offset) in [(1u64, 100), (2, 60), (2, 100)] {
        let mut histogram = plateau;
        histogram[grain_top + offset] = size;
        assert_code(
            robust_code(&histogram, sigma),
            (grain_top + offset) as f64,
            &format!("{size} px at +{offset}"),
        );
    }
    for (size, offset) in [(2u64, 30), (4, 10)] {
        let mut histogram = plateau;
        histogram[grain_top + offset] = size;
        let lowered_by = (grain_top + offset) as f64 - robust_code(&histogram, sigma);
        assert!(
            lowered_by > 4.0 * sigma && lowered_by <= GRAIN_TAIL_SIGMAS * sigma + 1e-6,
            "{size} px at +{offset}: lowered by {lowered_by}"
        );
    }
}

#[test]
fn correction_is_limited_to_the_reach_of_the_grain() {
    // One pixel 60 codes (3 sigma) above the highest grain pixel is not detached, and the
    // fit puts the centre about 150 codes below it. Grain accounts for 5.2 sigma at most.
    let sigma = 20.0;
    let mut histogram = grainy_plateau(2000.0, sigma, 1.0e6);
    let pixel = top_bin(&histogram) + 60;
    histogram[pixel] = 1;
    assert_code(
        robust_code(&histogram, sigma),
        pixel as f64 - GRAIN_TAIL_SIGMAS * sigma,
        "one pixel within the gap limit",
    );
}

#[test]
fn detached_group_with_spread_reads_its_mean() {
    // 61 pixels, one per code, far above the picture: a detached group no wider than
    // sigma reads its mean, not its maximum. Only a single-valued group is kept exactly.
    let mut histogram = grainy_plateau(2000.0, 20.0, 1.0e6);
    for slot in &mut histogram[3500..=3560] {
        *slot = 1;
    }
    assert_code(robust_code(&histogram, 20.0), 3530.0, "ramp of 61 pixels");
}

#[test]
fn grainy_highlight_is_lowered_near_the_grain_and_read_at_its_centre_above_it() {
    // 16 pixels that carry the grain of the picture, placed at the quantiles of a Gaussian.
    // Centred one sigma above the highest grain pixel they are part of the tail and are
    // lowered by more than a sigma below their centre (a documented limit). Centred three
    // sigma above it they form a detached group and read their centre.
    let sigma = 20.0;
    let plateau = grainy_plateau(2000.0, sigma, 1.0e6);
    let grain_top = top_bin(&plateau) as f64;
    let with_highlight = |centre: f64| {
        let mut histogram = plateau;
        for index in 0..16 {
            let z = -normal_upper_quantile((f64::from(index) + 0.5) / 16.0);
            histogram[(centre + sigma * z).round() as usize] += 1;
        }
        histogram
    };
    let near = grain_top + sigma;
    assert!(robust_code(&with_highlight(near), sigma) < near - sigma);
    let clear = grain_top + 3.0 * sigma;
    assert!((robust_code(&with_highlight(clear), sigma) - clear).abs() <= 1.0);
}

#[test]
fn small_frame_is_corrected_only_through_a_detached_group() {
    // 2000 pixels: too few for the tail fit. Three single pixels above a gap still form a
    // detached group, which reads below its maximum.
    let mut histogram = grainy_plateau(2000.0, 20.0, 2000.0);
    for code in [3000, 3030, 3060] {
        histogram[code] = 1;
    }
    let corrected = robust_code(&histogram, 20.0);
    assert!(corrected < 3060.0 && corrected >= 3030.0, "{corrected}");
}

#[test]
fn correction_never_raises_the_peak_and_keeps_the_sub_code_part_of_the_raw_maximum() {
    let histogram = grainy_plateau(2000.0, 20.0, 1.0e6);
    let top = top_bin(&histogram) as f64;
    let raw = (top + 0.3) / 4095.0;
    let on_bin = robust_peak_pq(&histogram, top / 4095.0, 20.0 / 4095.0);
    let off_bin = robust_peak_pq(&histogram, raw, 20.0 / 4095.0);
    assert!(off_bin < raw);
    assert!(((off_bin - on_bin) * 4095.0 - 0.3).abs() < 1e-9);
}
