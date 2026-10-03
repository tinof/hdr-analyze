//! Sample sets of HLG colours, as the integer Y′CbCr codes the decoder sees.

use crate::model::codes_of_hlg_rgb;

/// Top of the narrow-range HLG signal: code 1019 is E′ = 955/876 ≈ 1.090.
pub const SIGNAL_MAX: f64 = 955.0 / 876.0;
const GRID_STEPS: usize = 18;

fn grid(values: &[f64]) -> Vec<(u16, u16, u16)> {
    let mut out = Vec::with_capacity(values.len().pow(3));
    for &r in values {
        for &g in values {
            for &b in values {
                out.push(codes_of_hlg_rgb([r, g, b]));
            }
        }
    }
    out
}

/// Training set: a 19³ grid over [0, 1.09]³, a 13³ grid over the low end [0, 0.35]³, and
/// near-neutral colours around 64 grey levels.
pub fn training_set() -> Vec<(u16, u16, u16)> {
    let step = SIGNAL_MAX / GRID_STEPS as f64;
    let main: Vec<f64> = (0..=GRID_STEPS).map(|i| i as f64 * step).collect();
    let low: Vec<f64> = (0..=12).map(|i| i as f64 * 0.35 / 12.0).collect();
    let mut out = grid(&main);
    out.extend(grid(&low));
    for level in 0..64 {
        let grey = level as f64 / 63.0 * SIGNAL_MAX;
        for dr in [-0.04, -0.02, 0.0, 0.02, 0.04] {
            for db in [-0.04, -0.02, 0.0, 0.02, 0.04] {
                out.push(codes_of_hlg_rgb([grey + dr, grey, grey + db]));
            }
        }
    }
    out
}

/// Held-out set: the main grid shifted by half a step, never used in the fit.
pub fn held_out_set() -> Vec<(u16, u16, u16)> {
    let step = SIGNAL_MAX / GRID_STEPS as f64;
    let values: Vec<f64> = (0..GRID_STEPS).map(|i| (i as f64 + 0.5) * step).collect();
    grid(&values)
}

/// One validation patch of `scripts/validate_hlg_dv84_color.sh`.
pub struct Patch {
    pub name: &'static str,
    pub saturation: u8,
    pub level: f64,
    pub codes: (u16, u16, u16),
}

/// The 52 flat patches of `scripts/validate_hlg_dv84_color.sh`, same order and rounding.
pub fn validation_patches() -> Vec<Patch> {
    const COLOURS: [(&str, [f64; 3]); 6] = [
        ("red", [1.0, 0.0, 0.0]),
        ("green", [0.0, 1.0, 0.0]),
        ("blue", [0.0, 0.0, 1.0]),
        ("yellow", [1.0, 1.0, 0.0]),
        ("cyan", [0.0, 1.0, 1.0]),
        ("magenta", [1.0, 0.0, 1.0]),
    ];
    let mut out = Vec::new();
    for level in [0.25, 0.5, 0.75, 1.0] {
        out.push(Patch {
            name: "grey",
            saturation: 100,
            level,
            codes: codes_of_hlg_rgb([level; 3]),
        });
        for saturation in [1.0, 0.75] {
            for (name, rgb) in COLOURS {
                out.push(Patch {
                    name,
                    saturation: (saturation * 100.0) as u8,
                    level,
                    codes: codes_of_hlg_rgb(
                        rgb.map(|c| level * (saturation * c + 1.0 - saturation)),
                    ),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_match_the_script() {
        let patches = validation_patches();
        assert_eq!(patches.len(), 52);
        // Values from the analyzer's libplacebo reference test (analysis/hlg.rs).
        let find = |name: &str, saturation: u8, level: f64| {
            patches
                .iter()
                .find(|p| p.name == name && p.saturation == saturation && p.level == level)
                .unwrap()
                .codes
        };
        assert_eq!(find("grey", 100, 0.75), (721, 512, 512));
        assert_eq!(find("red", 100, 0.75), (237, 418, 848));
        assert_eq!(find("red", 75, 0.75), (358, 442, 764));
        assert_eq!(find("blue", 100, 1.0), (116, 960, 476));
    }
}
