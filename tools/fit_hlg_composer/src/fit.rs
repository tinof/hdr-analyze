//! The fit itself (docs/HLG_COMPOSER.md sections 4 and 5): luma curve on the grey axis,
//! chroma MMR by equality-constrained least squares, quantization with the neutral
//! constraints restored.

use nalgebra::{DMatrix, DVector};

use crate::model::itp_jacobian;
use crate::model::{
    hlg_rgb_of_codes, mmr_features, neutral_luma_target, reference_pq, ycc_chroma, DENOM,
    LUMA_GAIN, NEUTRAL, SOURCE_MIN,
};
use crate::samples::training_set;

/// Nominal white: the luma curve holds 1000 cd/m² from this code on.
pub const WHITE_CODE: u16 = 940;
/// First pivot (absolute): black. The decoder clamps the luma output to `[64, 1023] / 1023`,
/// which holds black and sub-black at 64/1023 (0.3 of a 12-bit code above PQ 0) instead of
/// letting the curve extrapolate below it: with a first pivot of 0 the fit decoded sub-black to
/// −299 codes of PQ.
pub const FIRST_PIVOT: u16 = 64;
/// Last pivot (absolute): the top of the 10-bit range.
pub const LAST_PIVOT: u16 = 1023;
/// Pieces fitted below nominal white; one more piece holds the peak above it.
const FITTED_PIECES: usize = 7;

/// Fixed-point coefficient as serialized: signed integer part, unsigned 23-bit fraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fixed {
    pub int: i64,
    pub frac: u64,
}

impl Fixed {
    pub fn quantize(value: f64) -> Self {
        let int = value.floor();
        let mut frac = ((value - int) * DENOM).round() as u64;
        let mut int = int as i64;
        if frac == DENOM as u64 {
            int += 1;
            frac = 0;
        }
        Self { int, frac }
    }

    pub fn value(self) -> f64 {
        self.int as f64 + self.frac as f64 / DENOM
    }
}

pub struct LumaFit {
    /// Absolute pivot codes, 9 entries.
    pub pivots: [u16; 9],
    pub coef: [[Fixed; 3]; 8],
    /// Maximum error in 12-bit PQ codes over codes 64..=1019 (both sides floored at
    /// `source_min_pq`), before quantization.
    pub max_error: f64,
}

pub struct ChromaFit {
    /// Per component (Cb, Cr): 22 coefficients, constant first.
    pub coef: [[Fixed; 22]; 2],
    /// Residuals of the four neutral equalities per component after quantization.
    pub constraint_residuals: [[f64; 4]; 2],
}

/// 12-bit PQ error of a reshaped-luma value against the target, with both sides floored at
/// the declared `source_min_pq` (below it the decode is black by the RPU's own range).
fn luma_error_codes(value: f64, target: f64) -> f64 {
    let to_pq = |y: f64| ((y - crate::model::LUMA_OFFSET) * LUMA_GAIN).max(SOURCE_MIN);
    (to_pq(value) - to_pq(target)) * 4095.0
}

fn piece_of(code: u16, pivots: &[u16; 9]) -> usize {
    (0..pivots.len() - 1)
        .rev()
        .find(|&k| code >= pivots[k])
        .unwrap_or(0)
}

/// Solve `min ‖W^½(Ax − b)‖² + ridge‖x‖²` subject to `Cx = d` through the KKT system.
fn constrained_least_squares(
    normal: &DMatrix<f64>,
    rhs: &DVector<f64>,
    constraints: &DMatrix<f64>,
    targets: &DVector<f64>,
) -> DVector<f64> {
    let n = normal.nrows();
    let m = constraints.nrows();
    let mut kkt = DMatrix::zeros(n + m, n + m);
    kkt.view_mut((0, 0), (n, n)).copy_from(normal);
    kkt.view_mut((n, 0), (m, n)).copy_from(constraints);
    kkt.view_mut((0, n), (n, m))
        .copy_from(&constraints.transpose());
    let mut b = DVector::zeros(n + m);
    b.rows_mut(0, n).copy_from(rhs);
    b.rows_mut(n, m).copy_from(targets);
    let solution = kkt.lu().solve(&b).expect("KKT system is singular");
    solution.rows(0, n).into_owned()
}

/// Fit the 7 lower pieces for fixed pivots by iteratively reweighted least squares towards
/// the minimax error. Returns the f64 coefficients and the maximum error.
fn fit_luma_pieces(pivots: &[u16; 9], targets: &[f64]) -> ([[f64; 3]; FITTED_PIECES], f64) {
    let unknowns = FITTED_PIECES * 3;
    let white = targets[usize::from(WHITE_CODE)];
    // C0 at the inner pivots 1..=6, and piece 6 meets the white hold at nominal white.
    let mut constraints = DMatrix::zeros(FITTED_PIECES, unknowns);
    let mut constraint_targets = DVector::zeros(FITTED_PIECES);
    for k in 1..FITTED_PIECES {
        let s = f64::from(pivots[k]) / 1023.0;
        for j in 0..3 {
            constraints[(k - 1, (k - 1) * 3 + j)] = s.powi(j as i32);
            constraints[(k - 1, k * 3 + j)] = -s.powi(j as i32);
        }
    }
    let s_white = f64::from(WHITE_CODE) / 1023.0;
    for j in 0..3 {
        constraints[(FITTED_PIECES - 1, (FITTED_PIECES - 1) * 3 + j)] = s_white.powi(j as i32);
    }
    constraint_targets[FITTED_PIECES - 1] = white;

    let codes: Vec<u16> = (0..WHITE_CODE).collect();
    let mut weights = vec![1.0; codes.len()];
    let mut best = ([[0.0; 3]; FITTED_PIECES], f64::INFINITY);
    for _ in 0..40 {
        let mut normal = DMatrix::<f64>::zeros(unknowns, unknowns);
        let mut rhs = DVector::<f64>::zeros(unknowns);
        for (&code, &weight) in codes.iter().zip(&weights) {
            let piece = piece_of(code, pivots);
            let s = f64::from(code) / 1023.0;
            let basis = [1.0, s, s * s];
            for a in 0..3 {
                rhs[piece * 3 + a] += weight * basis[a] * targets[usize::from(code)];
                for b in 0..3 {
                    normal[(piece * 3 + a, piece * 3 + b)] += weight * basis[a] * basis[b];
                }
            }
        }
        for i in 0..unknowns {
            normal[(i, i)] += 1e-12;
        }
        let x = constrained_least_squares(&normal, &rhs, &constraints, &constraint_targets);
        let mut coef = [[0.0; 3]; FITTED_PIECES];
        for (k, row) in coef.iter_mut().enumerate() {
            row.copy_from_slice(&[x[k * 3], x[k * 3 + 1], x[k * 3 + 2]]);
        }
        let errors: Vec<f64> = codes
            .iter()
            .map(|&code| {
                let piece = piece_of(code, pivots);
                let s = f64::from(code) / 1023.0;
                let c = coef[piece];
                luma_error_codes(c[0] + c[1] * s + c[2] * s * s, targets[usize::from(code)])
            })
            .collect();
        let max_error = errors
            .iter()
            .zip(&codes)
            .filter(|(_, &code)| code >= 64)
            .map(|(e, _)| e.abs())
            .fold(0.0, f64::max);
        if max_error < best.1 {
            best = (coef, max_error);
        }
        // Lawson update: weight grows with the error, which drives the fit towards minimax.
        let total: f64 = weights
            .iter()
            .zip(&errors)
            .map(|(w, e)| w * (e.abs() + 1e-6))
            .sum();
        for (w, e) in weights.iter_mut().zip(&errors) {
            *w = (*w * (e.abs() + 1e-6) / total * codes.len() as f64).max(1e-9);
        }
    }
    best
}

/// Fit the luma curve: pivot search by coordinate descent on the minimax error.
pub fn fit_luma() -> LumaFit {
    let targets: Vec<f64> = (0..1024).map(neutral_luma_target).collect();
    // Start from pivots evenly spaced in target PQ between black and nominal white.
    let mut pivots = [0_u16; 9];
    pivots[0] = FIRST_PIVOT;
    pivots[FITTED_PIECES] = WHITE_CODE;
    pivots[8] = LAST_PIVOT;
    for (k, pivot) in pivots.iter_mut().enumerate().take(FITTED_PIECES).skip(1) {
        let wanted = k as f64 / FITTED_PIECES as f64 * targets[usize::from(WHITE_CODE)]
            + (1.0 - k as f64 / FITTED_PIECES as f64) * targets[64];
        *pivot = (64..WHITE_CODE)
            .find(|&code| targets[usize::from(code)] >= wanted)
            .unwrap_or(WHITE_CODE - 1);
    }
    let (mut coef, mut max_error) = fit_luma_pieces(&pivots, &targets);
    loop {
        let mut improved = false;
        for k in 1..FITTED_PIECES {
            for step in [-16_i32, -8, -4, -2, -1, 1, 2, 4, 8, 16] {
                let moved = i32::from(pivots[k]) + step;
                if moved <= i32::from(pivots[k - 1]) || moved >= i32::from(pivots[k + 1]) {
                    continue;
                }
                let mut candidate = pivots;
                candidate[k] = moved as u16;
                let (c, e) = fit_luma_pieces(&candidate, &targets);
                if e < max_error - 1e-6 {
                    (pivots, coef, max_error) = (candidate, c, e);
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
    let white = targets[usize::from(WHITE_CODE)];
    let mut fixed = [[Fixed::quantize(0.0); 3]; 8];
    for (k, row) in coef.iter().enumerate() {
        fixed[k] = row.map(Fixed::quantize);
    }
    fixed[FITTED_PIECES] = [white, 0.0, 0.0].map(Fixed::quantize);
    LumaFit {
        pivots,
        coef: fixed,
        max_error,
    }
}

/// Indices of the serialized MMR coefficients the neutral equalities are solved for: the
/// constant and the pure-luma terms `y`, `y²`, `y³`.
const RESTORED: [usize; 4] = [0, 1, 8, 15];

/// The four neutral-axis equalities of one MMR curve as rows over its 22 coefficients
/// (docs/HLG_COMPOSER.md section 4.1): at `u = v = 512/1023` the curve is the constant 0.5.
fn neutral_constraint_rows() -> ([[f64; 22]; 4], [f64; 4]) {
    let n = NEUTRAL;
    let mut rows = [[0.0; 22]; 4];
    rows[0][0] = 1.0;
    for o in 0..3 {
        let p = n.powi(o as i32 + 1);
        let base = 1 + o * 7;
        // A0: u^o, v^o, (uv)^o
        rows[0][base + 1] = p;
        rows[0][base + 2] = p;
        rows[0][base + 5] = p * p;
        // A_o: y^o, (yu)^o, (yv)^o, (yuv)^o
        let row = &mut rows[o + 1];
        row[base] = 1.0;
        row[base + 3] = p;
        row[base + 4] = p;
        row[base + 6] = p * p;
    }
    (rows, [0.5, 0.0, 0.0, 0.0])
}

/// Fit both chroma MMR curves jointly against the reference R′G′B′, given the (quantized)
/// luma curve's decoded luma term per code.
pub fn fit_chroma(luma_term: &[f64], ridge: f64) -> ChromaFit {
    let m = ycc_chroma();
    let unknowns = 44;
    let mut normal = DMatrix::<f64>::zeros(unknowns, unknowns);
    let mut rhs = DVector::<f64>::zeros(unknowns);
    for (y, cb, cr) in training_set() {
        let target = reference_pq(y, cb, cr);
        let features = mmr_features(
            f64::from(y) / 1023.0,
            f64::from(cb) / 1023.0,
            f64::from(cr) / 1023.0,
        );
        let luma = luma_term[usize::from(y)];
        // channel c = luma + mc·(θcb·φ − 0.5) + mr·(θcr·φ − 0.5); the residual of each channel
        // is linear in θ. Weighting the residual vector with the ITP Jacobian at the target
        // makes the squared error approximate ΔE_ITP² (one Gauss-Newton step).
        let mut channel_rows = [[0.0; 44]; 3];
        let mut channel_rhs = [0.0; 3];
        for channel in 0..3 {
            let (mc, mr) = (m[channel * 2], m[channel * 2 + 1]);
            channel_rhs[channel] = target[channel] - luma + 0.5 * (mc + mr);
            for i in 0..22 {
                channel_rows[channel][i] = mc * features[i];
                channel_rows[channel][22 + i] = mr * features[i];
            }
        }
        let jacobian = itp_jacobian(target);
        for jac_row in jacobian {
            let mut row = [0.0; 44];
            let mut b = 0.0;
            for channel in 0..3 {
                b += jac_row[channel] * channel_rhs[channel];
                for i in 0..44 {
                    row[i] += jac_row[channel] * channel_rows[channel][i];
                }
            }
            for i in 0..unknowns {
                if row[i] == 0.0 {
                    continue;
                }
                rhs[i] += row[i] * b;
                for j in 0..unknowns {
                    normal[(i, j)] += row[i] * row[j];
                }
            }
        }
    }
    let scale = (0..unknowns).map(|i| normal[(i, i)]).sum::<f64>() / unknowns as f64;
    for i in 0..unknowns {
        normal[(i, i)] += ridge * scale;
    }
    let (rows, targets) = neutral_constraint_rows();
    let mut constraints = DMatrix::zeros(8, unknowns);
    let mut constraint_targets = DVector::zeros(8);
    for component in 0..2 {
        for (r, row) in rows.iter().enumerate() {
            for (i, &value) in row.iter().enumerate() {
                constraints[(component * 4 + r, component * 22 + i)] = value;
            }
            constraint_targets[component * 4 + r] = targets[r];
        }
    }
    let x = constrained_least_squares(&normal, &rhs, &constraints, &constraint_targets);

    let mut coef = [[Fixed::quantize(0.0); 22]; 2];
    let mut constraint_residuals = [[0.0; 4]; 2];
    for component in 0..2 {
        let mut values: Vec<f64> = (0..22).map(|i| x[component * 22 + i]).collect();
        for (i, value) in values.iter_mut().enumerate() {
            if !RESTORED.contains(&i) {
                *value = Fixed::quantize(*value).value();
            }
        }
        // Restore each equality by solving it for its own restored coefficient, which has
        // weight 1 in that row and 0 in the others.
        for (r, row) in rows.iter().enumerate() {
            let own = RESTORED[r];
            let rest: f64 = (0..22)
                .filter(|&i| i != own)
                .map(|i| row[i] * values[i])
                .sum();
            values[own] = Fixed::quantize(targets[r] - rest).value();
        }
        for (r, row) in rows.iter().enumerate() {
            let sum: f64 = (0..22).map(|i| row[i] * values[i]).sum();
            constraint_residuals[component][r] = sum - targets[r];
        }
        coef[component] = std::array::from_fn(|i| Fixed::quantize(values[i]));
    }
    ChromaFit {
        coef,
        constraint_residuals,
    }
}

/// HLG R′G′B′ is only used to report which training samples are superwhite.
pub fn is_superwhite(y: u16, cb: u16, cr: u16) -> bool {
    hlg_rgb_of_codes(y, cb, cr).iter().any(|&e| e > 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_round_trips_and_carries() {
        assert_eq!(
            Fixed::quantize(-0.25),
            Fixed {
                int: -1,
                frac: 6_291_456
            }
        );
        assert_eq!(Fixed::quantize(1.0 - 1e-12), Fixed { int: 1, frac: 0 });
        let value = -12.345_678_9;
        assert!((Fixed::quantize(value).value() - value).abs() <= 0.5 / DENOM);
    }

    #[test]
    fn constraint_rows_match_the_feature_layout() {
        // A curve that is 0.5 + y·(u − n) must satisfy all four equalities.
        let (rows, targets) = neutral_constraint_rows();
        let mut coef = [0.0; 22];
        coef[0] = 0.5;
        coef[4] = 1.0; // y·u
        coef[1] = -NEUTRAL; // y
        for (row, target) in rows.iter().zip(targets) {
            let sum: f64 = row.iter().zip(&coef).map(|(a, b)| a * b).sum();
            assert!((sum - target).abs() < 1e-15);
        }
        let features = mmr_features(0.7, NEUTRAL, NEUTRAL);
        let value: f64 = features.iter().zip(&coef).map(|(a, b)| a * b).sum();
        assert!((value - 0.5).abs() < 1e-15);
    }
}
