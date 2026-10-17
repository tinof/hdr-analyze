use super::*;

#[test]
fn full_precision_mean_uses_sum_and_count() {
    assert_eq!(mean_or_zero(1.5, 3), 0.5);
    assert_eq!(mean_or_zero(123.0, 0), 0.0);
}

#[test]
fn low_percentile_rejects_only_the_configured_dark_tail() {
    let mut histogram = [0u64; PQ_HIST_BINS];
    histogram[2] = 1;
    histogram[51] = 999;

    assert_eq!(low_percentile_pq(&histogram, 0.0), 2.0 / 4095.0);
    assert_eq!(low_percentile_pq(&histogram, 0.1), 51.0 / 4095.0);
    assert_eq!(low_percentile_pq(&[0; PQ_HIST_BINS], 0.1), 0.0);
}

#[test]
fn mean_nits_averages_in_linear_light() {
    assert_eq!(mean_nits_from_pq_hist(&[0; PQ_HIST_BINS]), 0.0);

    // 12-bit PQ 2081 is about 100 nits and 3079 about 1000 nits.
    let mut histogram = [0u64; PQ_HIST_BINS];
    histogram[3079] = 10;
    let peak = pq_to_nits(3079.0 / 4095.0);
    assert!((mean_nits_from_pq_hist(&histogram) - peak).abs() < 1e-9);

    histogram[2081] = 30;
    let expected = (10.0 * peak + 30.0 * pq_to_nits(2081.0 / 4095.0)) / 40.0;
    assert!((mean_nits_from_pq_hist(&histogram) - expected).abs() < 1e-9);
    // The PQ-domain mean would give far less than the linear mean.
    let pq_mean = (10.0 * 3079.0 + 30.0 * 2081.0) / 40.0 / 4095.0;
    assert!(pq_to_nits(pq_mean) < 0.6 * expected);
}
