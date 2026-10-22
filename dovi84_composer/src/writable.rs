//! Shape checks that keep the `dolby_vision` crate's RPU writer from panicking on a mapping.
//!
//! The crate (3.4.0) indexes every per-piece vector of a reshaping curve without bounds checks
//! and calls `unimplemented!` on a polynomial piece of order 1 with `linear_interp_flag` set
//! (`src/rpu/rpu_data_mapping.rs:229`). Under the release profile's `panic = "abort"` either one
//! ends the process, so a mapping is checked here before it is installed into an RPU that will be
//! written. Re-check the indexing in `RpuDataMapping::write` when the crate is bumped.

use anyhow::{bail, ensure, Result};
use dolby_vision::rpu::rpu_data_mapping::{DoviMappingMethod, DoviReshapingCurve, RpuDataMapping};

/// Coefficients per MMR order in the crate's layout (`dolby_vision::rpu::MMR_MAX_COEFFS`).
const MMR_COEFFS: usize = 7;
/// Highest `poly_order_minus1` whose `poly_order + 1` coefficients fit the crate's 3-entry rows.
const MAX_POLY_ORDER_MINUS1: u64 = 1;
/// Highest `mmr_order_minus1` whose orders fit the crate's 3-row coefficient blocks.
const MAX_MMR_ORDER_MINUS1: u8 = 2;

/// Check that the crate can write `mapping` without panicking: every curve has as many pivots
/// and per-piece entries as its `num_pivots_minus2` says, the mapping method matches the curve
/// data, polynomial and MMR orders fit the coefficient rows, no piece needs linear
/// interpolation (which the crate cannot write), and there is no NLQ (Profile 8 has none).
pub fn check_mapping_writable(mapping: &RpuDataMapping) -> Result<()> {
    ensure!(
        mapping.nlq.is_none() && mapping.nlq_method_idc.is_none(),
        "mapping carries NLQ data, which a Profile 8 RPU has none of"
    );
    for (component, curve) in mapping.curves.iter().enumerate() {
        check_curve(curve).map_err(|error| error.context(format!("component {component}")))?;
    }
    Ok(())
}

fn check_curve(curve: &DoviReshapingCurve) -> Result<()> {
    let pieces = usize::try_from(curve.num_pivots_minus2)
        .ok()
        .and_then(|minus2| minus2.checked_add(1))
        .filter(|&pieces| pieces <= 8)
        .ok_or_else(|| {
            anyhow::anyhow!("num_pivots_minus2 {} out of range", curve.num_pivots_minus2)
        })?;
    ensure!(
        curve.pivots.len() == pieces + 1,
        "{} pivots for {pieces} pieces",
        curve.pivots.len()
    );
    match (curve.mapping_idc, &curve.polynomial, &curve.mmr) {
        (DoviMappingMethod::Polynomial, Some(poly), None) => {
            ensure!(
                poly.poly_order_minus1.len() >= pieces
                    && poly.poly_coef_int.len() >= pieces
                    && poly.poly_coef.len() >= pieces,
                "polynomial data shorter than {pieces} pieces"
            );
            for piece in 0..pieces {
                let order_minus1 = poly.poly_order_minus1[piece];
                ensure!(
                    order_minus1 <= MAX_POLY_ORDER_MINUS1,
                    "piece {piece}: poly_order_minus1 {order_minus1} does not fit"
                );
                if order_minus1 == 0 {
                    // The writer reads the flag for every order-1 piece and cannot write `true`.
                    match poly.linear_interp_flag.get(piece) {
                        Some(false) => {}
                        Some(true) => bail!(
                            "piece {piece}: linear interpolation (linear_interp_flag) is not writable"
                        ),
                        None => bail!("piece {piece}: order-1 piece without a linear_interp_flag"),
                    }
                }
                let coefficients = order_minus1 as usize + 2;
                ensure!(
                    poly.poly_coef_int[piece].len() >= coefficients
                        && poly.poly_coef[piece].len() >= coefficients,
                    "piece {piece}: fewer than {coefficients} polynomial coefficients"
                );
            }
        }
        (DoviMappingMethod::MMR, None, Some(mmr)) => {
            ensure!(
                mmr.mmr_order_minus1.len() >= pieces
                    && mmr.mmr_constant_int.len() >= pieces
                    && mmr.mmr_constant.len() >= pieces
                    && mmr.mmr_coef_int.len() >= pieces
                    && mmr.mmr_coef.len() >= pieces,
                "MMR data shorter than {pieces} pieces"
            );
            for piece in 0..pieces {
                let order_minus1 = mmr.mmr_order_minus1[piece];
                ensure!(
                    order_minus1 <= MAX_MMR_ORDER_MINUS1,
                    "piece {piece}: mmr_order_minus1 {order_minus1} does not fit"
                );
                let orders = order_minus1 as usize + 1;
                let rows_fit = |rows: usize, row_len: &dyn Fn(usize) -> usize| {
                    rows >= orders && (0..orders).all(|order| row_len(order) >= MMR_COEFFS)
                };
                ensure!(
                    rows_fit(mmr.mmr_coef_int[piece].len(), &|o| mmr.mmr_coef_int[piece]
                        [o]
                        .len())
                        && rows_fit(mmr.mmr_coef[piece].len(), &|o| mmr.mmr_coef[piece][o].len()),
                    "piece {piece}: MMR coefficients do not cover {orders} orders"
                );
            }
        }
        (method, poly, mmr) => bail!(
            "mapping method {method:?} with polynomial data {} and MMR data {}",
            if poly.is_some() { "present" } else { "absent" },
            if mmr.is_some() { "present" } else { "absent" }
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{canary_mapping, Composer, DEFAULT_CANARY_OFFSET};

    #[test]
    fn every_composer_and_the_canary_are_writable() {
        for composer in Composer::ALL {
            check_mapping_writable(&composer.rpu_data_mapping()).unwrap();
        }
        check_mapping_writable(&canary_mapping(DEFAULT_CANARY_OFFSET).unwrap()).unwrap();
    }

    fn luma_poly(
        mapping: &mut RpuDataMapping,
    ) -> &mut dolby_vision::rpu::rpu_data_mapping::DoviPolynomialCurve {
        mapping.curves[0].polynomial.as_mut().unwrap()
    }

    #[test]
    fn refuses_a_linear_interpolation_piece() {
        let mut mapping = Composer::Bt2100V1.rpu_data_mapping();
        let pieces = mapping.curves[0].num_pivots_minus2 as usize + 1;
        let poly = luma_poly(&mut mapping);
        poly.poly_order_minus1[0] = 0;
        poly.linear_interp_flag = vec![false; pieces];
        check_mapping_writable(&mapping).unwrap();

        luma_poly(&mut mapping).linear_interp_flag[0] = true;
        let error = check_mapping_writable(&mapping).unwrap_err();
        assert!(
            format!("{error:#}").contains("linear_interp_flag"),
            "{error:#}"
        );
    }

    #[test]
    fn refuses_an_order_one_piece_without_its_flag() {
        let mut mapping = Composer::Bt2100V1.rpu_data_mapping();
        let poly = luma_poly(&mut mapping);
        poly.poly_order_minus1[0] = 0;
        poly.linear_interp_flag.clear();
        assert!(check_mapping_writable(&mapping).is_err());
    }

    #[test]
    fn refuses_curves_shorter_than_their_pieces() {
        let mut short_pivots = Composer::Preset.rpu_data_mapping();
        short_pivots.curves[0].pivots.pop();
        assert!(check_mapping_writable(&short_pivots).is_err());

        let mut short_poly = Composer::Preset.rpu_data_mapping();
        luma_poly(&mut short_poly).poly_coef.pop();
        assert!(check_mapping_writable(&short_poly).is_err());

        let mut high_order = Composer::Preset.rpu_data_mapping();
        luma_poly(&mut high_order).poly_order_minus1[0] = 2;
        assert!(check_mapping_writable(&high_order).is_err());

        let mut short_mmr = Composer::Preset.rpu_data_mapping();
        short_mmr.curves[1].mmr.as_mut().unwrap().mmr_coef[0].pop();
        assert!(check_mapping_writable(&short_mmr).is_err());

        let mut high_mmr = Composer::Preset.rpu_data_mapping();
        high_mmr.curves[2].mmr.as_mut().unwrap().mmr_order_minus1[0] = 3;
        assert!(check_mapping_writable(&high_mmr).is_err());
    }

    #[test]
    fn refuses_a_method_that_does_not_match_the_curve_data() {
        let mut mapping = Composer::Preset.rpu_data_mapping();
        mapping.curves[1].mapping_idc = DoviMappingMethod::Polynomial;
        assert!(check_mapping_writable(&mapping).is_err());

        let mut both = Composer::Preset.rpu_data_mapping();
        both.curves[0].mmr = both.curves[1].mmr.clone();
        assert!(check_mapping_writable(&both).is_err());
    }
}
