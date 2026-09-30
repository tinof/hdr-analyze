//! HLG luminance mapping through the Dolby Vision Profile 8.4 luma reshaping curve.
//!
//! A Profile 8.4 stream keeps the HLG base layer untouched and carries an RPU whose
//! polynomial luma curve reconstructs PQ. The analyzer measures HLG through that exact
//! curve (taken from the `dolby_vision` crate that `dovi_tool` embeds), so the L1
//! metadata describes the luma a DV decoder reconstructs. Chroma MMR is not modelled.

use std::sync::OnceLock;

use dolby_vision::rpu::profiles::{profile84::Profile84, DoviProfile};

/// `analysis.luminance_mapping` value for HLG measured through the DV 8.4 luma curve.
pub const DOVI84_MAPPING: &str = "dovi84-v1";
/// `analysis.luminance_mapping` value for PQ signals measured directly.
pub const PQ_MAPPING: &str = "pq";

/// Fixed-point denominator (log2) of the reshaping polynomial coefficients.
///
/// This is the RPU header's `coefficient_log2_denom`, which is 23 for the Profile 8
/// headers `dovi_tool` generates (`RpuDataHeader` default, `rpu_data_header.rs`). It is
/// not reachable from `Profile84::rpu_data_mapping()`, so it is pinned here.
const COEFFICIENT_LOG2_DENOM: u32 = 23;
const COEFFICIENT_DENOM: f64 = (1_u32 << COEFFICIENT_LOG2_DENOM) as f64;
/// Fixed-point scale of `ycc_to_rgb_offset0` (2^28).
const YCC_OFFSET_SCALE: f64 = 268_435_456.0;
/// Fixed-point scale of `ycc_to_rgb_coef0` (2^13).
const YCC_COEF_SCALE: f64 = 8192.0;

/// Map a 10-bit HLG luma code to normalized PQ (0.0-1.0) through the Dolby Vision
/// Profile 8.4 luma reshaping curve.
///
/// Steps: s = code/1023; pick the polynomial piece from the cumulative pivots; evaluate
/// `c0 + c1*s + c2*s^2` with coefficients `int + frac / 2^23`; clamp to the first/last
/// pivot; convert to PQ with the DM `ycc_to_rgb_offset0`/`coef0` of `Profile84::dm_data()`.
///
/// The result is finally clamped to the RPU's declared source range
/// `[source_min_pq, source_max_pq] / 4095` (62..3079, about 0.005..1000 nits). This clamp
/// was established empirically against libplacebo's Dolby Vision renderer, which plateaus
/// at the declared source range for superwhite codes instead of extrapolating the curve.
///
/// The curve pieces meet with a tiny seam (about 3e-7 PQ, 0.001 of a 12-bit code) at
/// code 910/911; this is a property of the RPU coefficients and is kept verbatim.
pub fn dovi84_luma_to_pq(code: u16) -> f64 {
    let mapping = Profile84::rpu_data_mapping();
    let luma = &mapping.curves[0];
    let poly = luma
        .polynomial
        .as_ref()
        .expect("Profile 8.4 luma reshaping curve is polynomial");
    let dm = Profile84::dm_data();

    let s = (f64::from(code) / 1023.0).clamp(0.0, 1.0);

    let mut cumulative = 0.0;
    let pivots: Vec<f64> = luma
        .pivots
        .iter()
        .map(|&pivot| {
            cumulative += f64::from(pivot);
            cumulative / 1023.0
        })
        .collect();
    let num_pieces = pivots.len() - 1;
    let piece = (0..num_pieces).rev().find(|&k| s >= pivots[k]).unwrap_or(0);

    let order = poly.poly_order_minus1[piece] as usize + 1;
    let mut y = 0.0;
    let mut s_pow = 1.0;
    for j in 0..=order {
        let coefficient = poly.poly_coef_int[piece][j] as f64
            + poly.poly_coef[piece][j] as f64 / COEFFICIENT_DENOM;
        y += coefficient * s_pow;
        s_pow *= s;
    }
    let y = y.clamp(pivots[0], pivots[num_pieces]);

    let offset0 = f64::from(dm.ycc_to_rgb_offset0) / YCC_OFFSET_SCALE;
    let coef0 = f64::from(dm.ycc_to_rgb_coef0) / YCC_COEF_SCALE;
    let pq = (y - offset0) * coef0;

    let source_min = f64::from(dm.source_min_pq) / 4095.0;
    let source_max = f64::from(dm.source_max_pq) / 4095.0;
    pq.clamp(source_min, source_max)
}

/// Lookup table of [`dovi84_luma_to_pq`] for every 10-bit code.
///
/// Single source of truth for the HLG mapping on both the CPU and CUDA paths.
pub fn dovi84_pq_lut() -> &'static [f32; 1024] {
    static LUT: OnceLock<[f32; 1024]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut lut = [0.0_f32; 1024];
        for (code, entry) in lut.iter_mut().enumerate() {
            *entry = dovi84_luma_to_pq(code as u16) as f32;
        }
        lut
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE_12BIT: f64 = 1.0 / 4095.0;

    fn assert_pq_near(code: u16, expected: f64, tolerance_codes: f64) {
        let actual = dovi84_luma_to_pq(code);
        assert!(
            (actual - expected).abs() <= tolerance_codes * CODE_12BIT,
            "code {code}: expected PQ {expected} ± {tolerance_codes}/4095, got {actual}"
        );
    }

    #[test]
    fn lut_is_monotonic_in_12bit_codes() {
        // The 910/911 piece seam dips by ~3e-7 PQ; monotonicity holds in the 12-bit unit
        // every consumer sees, and within 1e-6 in raw PQ.
        let lut = dovi84_pq_lut();
        assert!(lut
            .windows(2)
            .all(|pair| f64::from(pair[1]) >= f64::from(pair[0]) - 1.0e-6));
        let codes: Vec<u16> = lut
            .iter()
            .map(|&pq| (f64::from(pq) * 4095.0).round() as u16)
            .collect();
        assert!(codes.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn black_and_sub_black_map_to_source_min() {
        let source_min = (62.0_f64 / 4095.0) as f32;
        for code in 0..=64 {
            assert_eq!(dovi84_pq_lut()[code], source_min, "code {code}");
        }
    }

    #[test]
    fn superwhite_plateaus_at_source_max() {
        let source_max = (3079.0_f64 / 4095.0) as f32;
        for code in 940..1024 {
            assert_eq!(dovi84_pq_lut()[code], source_max, "code {code}");
        }
    }

    #[test]
    fn matches_libplacebo_reference_luma() {
        // libplacebo DV render of a lossless HLG ramp with an 8.4 RPU (step 0 ground truth).
        assert_pq_near(304, 0.31403, 2.0);
        assert_pq_near(720, 0.58253, 2.0);
        assert_pq_near(896, 0.71762, 2.0);
    }

    #[test]
    fn code_721_pins_model_value() {
        // 721 → ≈208.5 nits.
        assert_pq_near(721, 2389.44 / 4095.0, 1.0);
    }

    #[test]
    fn profile84_constants_are_pinned() {
        let mapping = Profile84::rpu_data_mapping();
        let luma = &mapping.curves[0];
        assert_eq!(luma.pivots, vec![63, 69, 230, 256, 256, 37, 16, 8, 7]);
        let poly = luma.polynomial.as_ref().unwrap();
        assert_eq!(poly.poly_order_minus1, vec![1; 8]);
        assert_eq!(poly.poly_coef_int.len(), 8);
        assert_eq!(poly.poly_coef_int[0].as_slice(), &[-1, 1, -3]);
        assert_eq!(poly.poly_coef_int[7].as_slice(), &[28, -62, 34]);
        assert_eq!(poly.poly_coef[0].as_slice(), &[7978928, 8332855, 4889184]);
        assert_eq!(poly.poly_coef[7].as_slice(), &[1947392, 1244640, 6094272]);

        let dm = Profile84::dm_data();
        assert_eq!(dm.ycc_to_rgb_offset0, 16_777_216);
        assert_eq!(dm.ycc_to_rgb_coef0, 9574);
        assert_eq!(dm.source_min_pq, 62);
        assert_eq!(dm.source_max_pq, 3079);
    }
}
