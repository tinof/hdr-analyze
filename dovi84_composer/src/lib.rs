//! Dolby Vision Profile 8.4 composers: the reshaping curves (`rpu_data_mapping`) an 8.4 RPU
//! carries to reconstruct PQ from an HLG base layer.
//!
//! Single source for both binaries: `hdr_analyzer_mvp` measures HLG through the selected
//! composer, and `mkvdovi` writes the same composer into the RPU. The sidecar's
//! `analysis.luminance_mapping` names it ([`Composer::luminance_mapping`]), so a measurement
//! is never paired with an RPU carrying a different composer. Design: `docs/HLG_COMPOSER.md`.

use dolby_vision::rpu::profiles::profile84::Profile84;
use dolby_vision::rpu::rpu_data_mapping::{
    DoviMMRCurve, DoviMappingMethod, DoviPolynomialCurve, DoviReshapingCurve, RpuDataMapping,
};

mod bt2100_v1;
pub mod rewrite;

pub use rewrite::{check_rpu_file, check_rpus, rewrite_rpu_file};

/// Fixed-point denominator (log2) of the reshaping coefficients: the RPU header's
/// `coefficient_log2_denom` in the Profile 8 headers `dovi_tool` generates
/// (`RpuDataHeader::p8_default`). Every composer here is quantized to it, and
/// [`rewrite_rpu_file`] refuses an RPU whose header uses another value.
pub const COEFFICIENT_LOG2_DENOM: u32 = 23;

/// `analysis.luminance_mapping` of HLG measured through the preset composer: luma curve,
/// chroma MMR at chroma resolution on down-sampled luma, bilinear composed chroma, matrix
/// (docs/HLG_COMPOSER.md section 9). The preset's counter counts decode revisions.
pub const PRESET_LUMINANCE_MAPPING: &str = "dovi84-v3";
/// `analysis.luminance_mapping` of HLG measured through the BT.2100-fitted composer, first
/// fit, with the same decode. `bt2100-v1` names the composer (the RPU), `spec420` the
/// decode, so a refit becomes `dovi84-bt2100-v2-spec420`.
pub const BT2100_V1_LUMINANCE_MAPPING: &str = "dovi84-bt2100-v1-spec420";

/// Names earlier analyzers wrote for the same composers, with each pixel's chroma reshaped on
/// its own luma (no chroma-resolution decode). Such a measurement is re-analyzed, never
/// reused; the RPU itself is the same.
pub const LEGACY_LUMINANCE_MAPPINGS: [(&str, Composer); 2] = [
    ("dovi84-v2", Composer::Preset),
    ("dovi84-bt2100-v1", Composer::Bt2100V1),
];

/// A Profile 8.4 composer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Composer {
    /// The `dolby_vision` crate's `Profile84` preset (from phone-recorded RPUs), which
    /// `dovi_tool generate` writes. It decodes neutral greys with a blue tint.
    Preset,
    /// Fitted to the BT.2100 / BT.2408 1000-nit HLG-to-PQ conversion with neutrals kept
    /// neutral (`tools/fit_hlg_composer`). The default. A refit gets a new variant and
    /// sidecar name.
    #[default]
    Bt2100V1,
}

impl Composer {
    /// Every composer, for tests and diagnostics.
    pub const ALL: [Composer; 2] = [Composer::Preset, Composer::Bt2100V1];

    /// The composer's reshaping curves, as written into the RPU.
    pub fn rpu_data_mapping(self) -> RpuDataMapping {
        match self {
            Composer::Preset => Profile84::rpu_data_mapping(),
            Composer::Bt2100V1 => bt2100_v1_mapping(),
        }
    }

    /// The sidecar's `analysis.luminance_mapping` value for HLG measured through this composer.
    pub fn luminance_mapping(self) -> &'static str {
        match self {
            Composer::Preset => PRESET_LUMINANCE_MAPPING,
            Composer::Bt2100V1 => BT2100_V1_LUMINANCE_MAPPING,
        }
    }

    /// The value of the `--hlg-composer` option that selects this composer.
    pub fn cli_name(self) -> &'static str {
        match self {
            Composer::Preset => "preset",
            Composer::Bt2100V1 => "bt2100",
        }
    }

    /// The composer an `--hlg-composer` value selects, if any.
    pub fn from_cli_name(name: &str) -> Option<Composer> {
        Composer::ALL
            .into_iter()
            .find(|composer| composer.cli_name() == name)
    }

    /// The composer a sidecar's `analysis.luminance_mapping` names, if any. Only current
    /// names: a legacy name ([`LEGACY_LUMINANCE_MAPPINGS`]) is not a usable measurement.
    pub fn from_luminance_mapping(name: &str) -> Option<Composer> {
        Composer::ALL
            .into_iter()
            .find(|composer| composer.luminance_mapping() == name)
    }

    /// The composer a legacy `analysis.luminance_mapping` (an earlier decode) names, if any.
    pub fn from_legacy_luminance_mapping(name: &str) -> Option<Composer> {
        LEGACY_LUMINANCE_MAPPINGS
            .into_iter()
            .find(|&(legacy, _)| legacy == name)
            .map(|(_, composer)| composer)
    }
}

fn bt2100_v1_mapping() -> RpuDataMapping {
    use bt2100_v1 as fit;

    let mut mapping = Profile84::rpu_data_mapping();
    mapping.curves[0] = DoviReshapingCurve {
        num_pivots_minus2: fit::LUMA_PIVOTS.len() as u64 - 2,
        pivots: fit::LUMA_PIVOTS.to_vec(),
        mapping_idc: DoviMappingMethod::Polynomial,
        polynomial: Some(DoviPolynomialCurve {
            poly_order_minus1: vec![1; fit::LUMA_POLY_INT.len()],
            linear_interp_flag: vec![],
            poly_coef_int: fit::LUMA_POLY_INT.iter().map(|&row| row.into()).collect(),
            poly_coef: fit::LUMA_POLY_FRAC.iter().map(|&row| row.into()).collect(),
        }),
        mmr: None,
    };
    for component in 0..2 {
        mapping.curves[component + 1] = DoviReshapingCurve {
            num_pivots_minus2: 0,
            pivots: vec![0, 1023],
            mapping_idc: DoviMappingMethod::MMR,
            polynomial: None,
            mmr: Some(DoviMMRCurve {
                mmr_order_minus1: vec![2],
                mmr_constant_int: vec![fit::MMR_CONSTANT_INT[component]],
                mmr_constant: vec![fit::MMR_CONSTANT_FRAC[component]],
                mmr_coef_int: vec![fit::MMR_COEF_INT[component]
                    .iter()
                    .map(|&row| row.into())
                    .collect()],
                mmr_coef: vec![fit::MMR_COEF_FRAC[component]
                    .iter()
                    .map(|&row| row.into())
                    .collect()],
            }),
        };
    }
    mapping
}

/// Whether two mappings carry the same curves and partitioning, field by field
/// (`RpuDataMapping` does not implement `PartialEq`).
pub fn mappings_equal(a: &RpuDataMapping, b: &RpuDataMapping) -> bool {
    let curves_equal = a.curves.iter().zip(&b.curves).all(|(x, y)| {
        x.num_pivots_minus2 == y.num_pivots_minus2
            && x.pivots == y.pivots
            && x.mapping_idc == y.mapping_idc
            && match (&x.polynomial, &y.polynomial) {
                (Some(p), Some(q)) => {
                    p.poly_order_minus1 == q.poly_order_minus1
                        // The parser fills one `false` per piece where the preset
                        // constructor leaves the vector empty; `true` is unsupported.
                        && !p.linear_interp_flag.iter().any(|&flag| flag)
                        && !q.linear_interp_flag.iter().any(|&flag| flag)
                        && p.poly_coef_int == q.poly_coef_int
                        && p.poly_coef == q.poly_coef
                }
                (None, None) => true,
                _ => false,
            }
            && match (&x.mmr, &y.mmr) {
                (Some(p), Some(q)) => {
                    p.mmr_order_minus1 == q.mmr_order_minus1
                        && p.mmr_constant_int == q.mmr_constant_int
                        && p.mmr_constant == q.mmr_constant
                        && p.mmr_coef_int == q.mmr_coef_int
                        && p.mmr_coef == q.mmr_coef
                }
                (None, None) => true,
                _ => false,
            }
    });
    curves_equal
        && a.vdr_rpu_id == b.vdr_rpu_id
        && a.mapping_color_space == b.mapping_color_space
        && a.mapping_chroma_format_idc == b.mapping_chroma_format_idc
        && a.num_x_partitions_minus1 == b.num_x_partitions_minus1
        && a.num_y_partitions_minus1 == b.num_y_partitions_minus1
        && a.nlq_method_idc == b.nlq_method_idc
        && a.nlq.is_none()
        && b.nlq.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_is_the_crate_profile84_mapping() {
        assert!(mappings_equal(
            &Composer::Preset.rpu_data_mapping(),
            &Profile84::rpu_data_mapping()
        ));
        assert!(!mappings_equal(
            &Composer::Preset.rpu_data_mapping(),
            &Composer::Bt2100V1.rpu_data_mapping()
        ));
    }

    #[test]
    fn luminance_mapping_names_round_trip() {
        for composer in Composer::ALL {
            assert_eq!(
                Composer::from_luminance_mapping(composer.luminance_mapping()),
                Some(composer)
            );
        }
        for composer in Composer::ALL {
            assert_eq!(Composer::from_cli_name(composer.cli_name()), Some(composer));
        }
        assert_eq!(Composer::from_luminance_mapping("dovi84-v1"), None);
        assert_eq!(Composer::from_luminance_mapping("pq"), None);
        // Legacy names identify their composer but are never a current measurement.
        for (legacy, composer) in LEGACY_LUMINANCE_MAPPINGS {
            assert_eq!(Composer::from_luminance_mapping(legacy), None);
            assert_eq!(
                Composer::from_legacy_luminance_mapping(legacy),
                Some(composer)
            );
            assert_ne!(legacy, composer.luminance_mapping());
        }
        assert_eq!(
            Composer::from_legacy_luminance_mapping(PRESET_LUMINANCE_MAPPING),
            None
        );
    }

    #[test]
    fn bt2100_v1_chroma_is_neutral_on_the_neutral_axis() {
        // docs/HLG_COMPOSER.md section 4.1: at u = v = 512/1023 each MMR curve is the cubic
        // A0 + A1·y + A2·y² + A3·y³ with A0 = 0.5 and A1..A3 = 0, up to quantization.
        let n = 512.0_f64 / 1023.0;
        let denom = f64::from(1_u32 << COEFFICIENT_LOG2_DENOM);
        let mapping = Composer::Bt2100V1.rpu_data_mapping();
        for component in [1, 2] {
            let mmr = mapping.curves[component].mmr.as_ref().unwrap();
            let value = |int: i64, frac: u64| int as f64 + frac as f64 / denom;
            let coef = |order: usize, term: usize| {
                value(
                    mmr.mmr_coef_int[0][order][term],
                    mmr.mmr_coef[0][order][term],
                )
            };
            let mut a = [
                value(mmr.mmr_constant_int[0], mmr.mmr_constant[0]),
                0.0,
                0.0,
                0.0,
            ];
            for order in 0..3 {
                let p = n.powi(order as i32 + 1);
                a[0] += p * (coef(order, 1) + coef(order, 2)) + p * p * coef(order, 5);
                a[order + 1] =
                    coef(order, 0) + p * (coef(order, 3) + coef(order, 4)) + p * p * coef(order, 6);
            }
            for (k, (actual, expected)) in a.iter().zip([0.5, 0.0, 0.0, 0.0]).enumerate() {
                assert!(
                    (actual - expected).abs() < 1e-7,
                    "component {component} A{k} = {actual:e}"
                );
            }
        }
    }

    #[test]
    fn bt2100_v1_has_the_preset_shape() {
        let mapping = Composer::Bt2100V1.rpu_data_mapping();
        let luma = &mapping.curves[0];
        assert_eq!(luma.pivots.len(), 9);
        assert!(luma.pivots.iter().map(|&p| u32::from(p)).sum::<u32>() <= 1023);
        let poly = luma.polynomial.as_ref().unwrap();
        assert_eq!(poly.poly_coef_int.len(), 8);
        let max_frac = 1_u64 << COEFFICIENT_LOG2_DENOM;
        assert!(poly.poly_coef.iter().flatten().all(|&f| f < max_frac));
        for component in [1, 2] {
            let curve = &mapping.curves[component];
            assert_eq!(curve.pivots, vec![0, 1023]);
            let mmr = curve.mmr.as_ref().unwrap();
            assert_eq!(mmr.mmr_order_minus1, vec![2]);
            assert!(mmr.mmr_constant[0] < max_frac);
            assert!(mmr.mmr_coef[0].iter().flatten().all(|&f| f < max_frac));
        }
    }
}
