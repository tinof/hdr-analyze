//! A deliberately wrong Profile 8.4 composer for playback tests: the `bt2100` composer with
//! both chroma MMR constants shifted by the same offset, so a device that applies the RPU's
//! composer shows a strong tint (green for a negative offset) and a device that ignores it
//! shows the normal picture. Diagnostic only: not a [`Composer`] (mkvdovi's `--hlg-composer`
//! does not offer it, and no analyzer measures through it). `tools/fit_hlg_composer canary-rpu`
//! and the `dovi_rpu_builder` crate both take it from here.

use anyhow::{ensure, Context, Result};
use dolby_vision::rpu::rpu_data_mapping::RpuDataMapping;

use crate::{Composer, COEFFICIENT_LOG2_DENOM};

/// Default shift of both normalized chroma outputs: −1/16, about −64 10-bit codes.
pub const DEFAULT_CANARY_OFFSET: f64 = -0.0625;

/// The `bt2100` mapping with `offset` added to both chroma MMR constants (fixed point,
/// `int + frac / 2^23`).
pub fn canary_mapping(offset: f64) -> Result<RpuDataMapping> {
    ensure!(
        offset.is_finite() && offset.abs() <= 1.0,
        "offset {offset} must be finite and within [-1, 1]"
    );
    let one = 1_i64 << COEFFICIENT_LOG2_DENOM;
    let shift = (offset * one as f64).round() as i64;
    ensure!(shift != 0, "offset {offset} rounds to zero");
    let mut mapping = Composer::Bt2100V1.rpu_data_mapping();
    for curve in &mut mapping.curves[1..] {
        let mmr = curve.mmr.as_mut().context("bt2100 chroma is not MMR")?;
        let value = mmr.mmr_constant_int[0] * one + mmr.mmr_constant[0] as i64 + shift;
        mmr.mmr_constant_int[0] = value.div_euclid(one);
        mmr.mmr_constant[0] = value.rem_euclid(one) as u64;
    }
    Ok(mapping)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mappings_equal;

    fn constants(mapping: &RpuDataMapping) -> Vec<i64> {
        let one = 1_i64 << COEFFICIENT_LOG2_DENOM;
        mapping.curves[1..]
            .iter()
            .map(|curve| {
                let mmr = curve.mmr.as_ref().unwrap();
                assert!(mmr.mmr_constant[0] < one as u64);
                mmr.mmr_constant_int[0] * one + mmr.mmr_constant[0] as i64
            })
            .collect()
    }

    #[test]
    fn default_offset_shifts_both_chroma_constants_only() {
        let bt2100 = Composer::Bt2100V1.rpu_data_mapping();
        let canary = canary_mapping(DEFAULT_CANARY_OFFSET).unwrap();
        let shift =
            (DEFAULT_CANARY_OFFSET * f64::from(1_u32 << COEFFICIENT_LOG2_DENOM)).round() as i64;
        let expected: Vec<i64> = constants(&bt2100).iter().map(|c| c + shift).collect();
        assert_eq!(constants(&canary), expected);

        let mut restored = canary.clone();
        for (curve, original) in restored.curves[1..].iter_mut().zip(&bt2100.curves[1..]) {
            let (mmr, base) = (curve.mmr.as_mut().unwrap(), original.mmr.as_ref().unwrap());
            mmr.mmr_constant_int[0] = base.mmr_constant_int[0];
            mmr.mmr_constant[0] = base.mmr_constant[0];
        }
        assert!(mappings_equal(&restored, &bt2100));
    }

    #[test]
    fn rejects_offsets_that_round_to_zero_or_are_out_of_range() {
        for offset in [
            1e-9,
            -1e13,
            1e13,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NAN,
            -1.5,
        ] {
            assert!(canary_mapping(offset).is_err(), "offset {offset} accepted");
        }
    }
}
