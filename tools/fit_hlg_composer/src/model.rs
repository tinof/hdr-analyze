//! Colour science of the fitting target (docs/HLG_COMPOSER.md section 3) and the 8.4 decode.

use dolby_vision::rpu::profiles::{profile84::Profile84, DoviProfile};
use dolby_vision::rpu::rpu_data_mapping::RpuDataMapping;

/// Display peak of the HLG reference display and of the PQ target, cd/m².
pub const PEAK_NITS: f64 = 1000.0;
pub const KR: f64 = 0.2627;
pub const KB: f64 = 0.0593;
pub const KG: f64 = 1.0 - KR - KB;
/// Neutral chroma code as the decoder sees it (`512 / 1023`).
pub const NEUTRAL: f64 = 512.0 / 1023.0;
/// Coefficient denominator, 2^23.
pub const DENOM: f64 = (1_u64 << dovi84_composer::COEFFICIENT_LOG2_DENOM) as f64;
/// `ycc_to_rgb_coef0` and `offset0` of the 8.4 DM block: `L = (y_out − 1/16) · LUMA_GAIN`.
pub const LUMA_GAIN: f64 = 9574.0 / 8192.0;
pub const LUMA_OFFSET: f64 = 1.0 / 16.0;
/// Declared `source_min_pq` of the 8.4 RPU, normalized PQ.
pub const SOURCE_MIN: f64 = 62.0 / 4095.0;

const M1: f64 = 2610.0 / 16384.0;
const M2: f64 = 2523.0 / 4096.0 * 128.0;
const C1: f64 = 3424.0 / 4096.0;
const C2: f64 = 2413.0 / 4096.0 * 32.0;
const C3: f64 = 2392.0 / 4096.0 * 32.0;

/// ST 2084 inverse EOTF: cd/m² to normalized PQ.
pub fn nits_to_pq(nits: f64) -> f64 {
    let y = (nits.max(0.0) / 10_000.0).powf(M1);
    ((C1 + C2 * y) / (1.0 + C3 * y)).powf(M2)
}

/// ST 2084 EOTF: normalized PQ to cd/m².
pub fn pq_to_nits(pq: f64) -> f64 {
    let p = pq.max(0.0).powf(1.0 / M2);
    10_000.0 * ((p - C1).max(0.0) / (C2 - C3 * p)).powf(1.0 / M1)
}

/// BT.2100 HLG inverse OETF, scene light normalized to [0, 1] at E′ = 1.
pub fn hlg_inverse_oetf(signal: f64) -> f64 {
    const A: f64 = 0.178_832_77;
    let b = 1.0 - 4.0 * A;
    let c = 0.5 - A * (4.0 * A).ln();
    let signal = signal.max(0.0);
    if signal <= 0.5 {
        signal * signal / 3.0
    } else {
        (((signal - c) / A).exp() + b) / 12.0
    }
}

/// HLG R′G′B′ (normalized, BT.2020) of limited-range 10-bit BT.2020 NCL codes.
pub fn hlg_rgb_of_codes(y: u16, cb: u16, cr: u16) -> [f64; 3] {
    let luma = (f64::from(y) - 64.0) / 876.0;
    let blue_diff = (f64::from(cb) - 512.0) / 896.0;
    let red_diff = (f64::from(cr) - 512.0) / 896.0;
    let red = luma + 2.0 * (1.0 - KR) * red_diff;
    let blue = luma + 2.0 * (1.0 - KB) * blue_diff;
    let green = (luma - KR * red - KB * blue) / KG;
    [red, green, blue]
}

/// Limited-range 10-bit BT.2020 NCL codes of HLG R′G′B′, rounded half to even like the
/// validation scripts' Python, and clamped to the narrow-range limits 4..1019.
pub fn codes_of_hlg_rgb(rgb: [f64; 3]) -> (u16, u16, u16) {
    let [r, g, b] = rgb;
    let y = KR * r + KG * g + KB * b;
    let code = |value: f64| value.round_ties_even().clamp(4.0, 1019.0) as u16;
    (
        code(64.0 + 876.0 * y),
        code(512.0 + 896.0 * (b - y) / (2.0 * (1.0 - KB))),
        code(512.0 + 896.0 * (r - y) / (2.0 * (1.0 - KR))),
    )
}

/// The BT.2100 / BT.2408 1000-nit HLG-to-PQ reference: display light per channel in cd/m²,
/// clipped per channel to [0, 1000] (the project's peak policy).
pub fn reference_nits(y: u16, cb: u16, cr: u16) -> [f64; 3] {
    let scene = hlg_rgb_of_codes(y, cb, cr).map(hlg_inverse_oetf);
    let scene_luminance = KR * scene[0] + KG * scene[1] + KB * scene[2];
    let gain = if scene_luminance > 0.0 {
        PEAK_NITS * scene_luminance.powf(0.2)
    } else {
        0.0
    };
    scene.map(|e| (gain * e).clamp(0.0, PEAK_NITS))
}

/// [`reference_nits`] as normalized PQ R′G′B′.
pub fn reference_pq(y: u16, cb: u16, cr: u16) -> [f64; 3] {
    reference_nits(y, cb, cr).map(nits_to_pq)
}

/// Target of the luma curve for a neutral luma code: limited-range reshaped luma.
pub fn neutral_luma_target(code: u16) -> f64 {
    LUMA_OFFSET + reference_pq(code, 512, 512)[1] / LUMA_GAIN
}

/// BT.2100 ICtCp of linear BT.2020 RGB in cd/m².
fn ictcp(rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb;
    let l = nits_to_pq((1688.0 * r + 2146.0 * g + 262.0 * b) / 4096.0);
    let m = nits_to_pq((683.0 * r + 2951.0 * g + 462.0 * b) / 4096.0);
    let s = nits_to_pq((99.0 * r + 309.0 * g + 3688.0 * b) / 4096.0);
    [
        0.5 * l + 0.5 * m,
        (6610.0 * l - 13613.0 * m + 7003.0 * s) / 4096.0,
        (17933.0 * l - 17390.0 * m - 543.0 * s) / 4096.0,
    ]
}

/// Jacobian of BT.2124's scaled ITP (`720·[I, T/2, P]`) with respect to PQ R′G′B′ at `pq`,
/// by central differences. Weighting a PQ R′G′B′ residual with it approximates ΔE_ITP.
pub fn itp_jacobian(pq: [f64; 3]) -> [[f64; 3]; 3] {
    let itp = |rgb: [f64; 3]| {
        let [i, t, p] = ictcp(rgb.map(|c| pq_to_nits(c.clamp(0.0, 1.0))));
        [720.0 * i, 360.0 * t, 720.0 * p]
    };
    let h = 1e-5;
    let mut jacobian = [[0.0; 3]; 3];
    for channel in 0..3 {
        let (mut up, mut down) = (pq, pq);
        up[channel] += h;
        down[channel] -= h;
        let (a, b) = (itp(up), itp(down));
        for k in 0..3 {
            jacobian[k][channel] = (a[k] - b[k]) / (2.0 * h);
        }
    }
    jacobian
}

/// ITU-R BT.2124 ΔE_ITP between two linear BT.2020 colours in cd/m².
pub fn delta_e_itp(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (a, b) = (ictcp(a), ictcp(b));
    let (di, dt, dp) = (a[0] - b[0], 0.5 * (a[1] - b[1]), a[2] - b[2]);
    720.0 * (di * di + dt * dt + dp * dp).sqrt()
}

/// Chroma columns of the 8.4 `ycc_to_rgb`: `[R←Cb, R←Cr, G←Cb, G←Cr, B←Cb, B←Cr]`.
pub fn ycc_chroma() -> [f64; 6] {
    let dm = Profile84::dm_data();
    [
        dm.ycc_to_rgb_coef1,
        dm.ycc_to_rgb_coef2,
        dm.ycc_to_rgb_coef4,
        dm.ycc_to_rgb_coef5,
        dm.ycc_to_rgb_coef7,
        dm.ycc_to_rgb_coef8,
    ]
    .map(|c| f64::from(c) / 8192.0)
}

/// The 22 MMR input terms of one sample: constant, then `[y, u, v, yu, yv, uv, yuv]` raised to
/// powers 1, 2, 3, in the serialized coefficient order.
pub fn mmr_features(y: f64, u: f64, v: f64) -> [f64; 22] {
    let base = [y, u, v, y * u, y * v, u * v, y * u * v];
    let mut out = [0.0; 22];
    out[0] = 1.0;
    for order in 0..3 {
        for (term, value) in base.iter().enumerate() {
            out[1 + order * 7 + term] = value.powi(order as i32 + 1);
        }
    }
    out
}

pub fn fixed(int: i64, frac: u64) -> f64 {
    int as f64 + frac as f64 / DENOM
}

/// The Profile 8.4 decode of one composer, replicating `hdr_analyzer_mvp`'s `Dovi84Decoder`
/// (`analysis/hlg.rs`): the luma term in f64 then f32, chroma and matrix in f32, same order.
pub struct Decoder {
    luma_term: Vec<f32>,
    luma_clamp_active: Vec<bool>,
    mmr: [(f32, [[f32; 7]; 3]); 2],
    m: [f32; 6],
}

impl Decoder {
    pub fn new(mapping: &RpuDataMapping) -> Self {
        let luma = &mapping.curves[0];
        let poly = luma.polynomial.as_ref().expect("polynomial luma");
        let mut cumulative = 0.0;
        let pivots: Vec<f64> = luma
            .pivots
            .iter()
            .map(|&p| {
                cumulative += f64::from(p);
                cumulative / 1023.0
            })
            .collect();
        let pieces = pivots.len() - 1;
        let mut luma_term = Vec::with_capacity(1024);
        let mut luma_clamp_active = Vec::with_capacity(1024);
        for code in 0..1024_u16 {
            let s = f64::from(code) / 1023.0;
            let piece = (0..pieces).rev().find(|&k| s >= pivots[k]).unwrap_or(0);
            let order = poly.poly_order_minus1[piece] as usize + 1;
            let mut value = 0.0;
            let mut s_pow = 1.0;
            for j in 0..=order {
                value += fixed(poly.poly_coef_int[piece][j], poly.poly_coef[piece][j]) * s_pow;
                s_pow *= s;
            }
            let clamped = value.clamp(pivots[0], pivots[pieces]);
            luma_clamp_active.push(clamped != value);
            luma_term.push(((clamped - LUMA_OFFSET) * LUMA_GAIN) as f32);
        }
        let mmr = [1, 2].map(|component| {
            let curve = mapping.curves[component].mmr.as_ref().expect("MMR chroma");
            let mut coef = [[0.0_f32; 7]; 3];
            for (order, row) in coef
                .iter_mut()
                .enumerate()
                .take(usize::from(curve.mmr_order_minus1[0]) + 1)
            {
                for (term, value) in row.iter_mut().enumerate() {
                    *value = fixed(
                        curve.mmr_coef_int[0][order][term],
                        curve.mmr_coef[0][order][term],
                    ) as f32;
                }
            }
            (
                fixed(curve.mmr_constant_int[0], curve.mmr_constant[0]) as f32,
                coef,
            )
        });
        Self {
            luma_term,
            luma_clamp_active,
            mmr,
            m: ycc_chroma().map(|c| c as f32),
        }
    }

    /// The luma term `(y_out − 1/16) · coef0` of a code, as the decoder stores it.
    pub fn luma_term(&self, code: u16) -> f64 {
        f64::from(self.luma_term[usize::from(code.min(1023))])
    }

    pub fn luma_clamp_active(&self, code: u16) -> bool {
        self.luma_clamp_active[usize::from(code)]
    }

    /// Raw R′G′B′ in normalized PQ (no source-range clamp) and whether a chroma output clamp
    /// engaged.
    pub fn rgb_pq(&self, y_code: u16, cb_code: u16, cr_code: u16) -> ([f32; 3], bool) {
        let (chroma, clamped) = self.reshape_chroma(
            Self::normalize(y_code),
            Self::normalize(cb_code),
            Self::normalize(cr_code),
        );
        (self.compose(self.luma_term_f32(y_code), chroma), clamped)
    }

    /// A 10-bit code as the analyzer and libplacebo normalize it (`code / 1023`).
    pub fn normalize(code: u16) -> f32 {
        f32::from(code.min(1023)) / 1023.0
    }

    /// The stored luma term of a code.
    pub fn luma_term_f32(&self, code: u16) -> f32 {
        self.luma_term[usize::from(code.min(1023))]
    }

    /// R′G′B′ from a luma term and the two reshaped chroma components (normalized, before the
    /// 0.5 offset), in the analyzer's operation order.
    pub fn compose(&self, luma: f32, chroma: [f32; 2]) -> [f32; 3] {
        let cb = chroma[0] - 0.5;
        let cr = chroma[1] - 0.5;
        let m = &self.m;
        [
            luma + m[0] * cb + m[1] * cr,
            luma + m[2] * cb + m[3] * cr,
            luma + m[4] * cb + m[5] * cr,
        ]
    }

    /// The reshaped Cb and Cr (clamped to [0, 1]) for normalized MMR inputs, and whether the
    /// output clamp engaged. `y` is the MMR's luma input, which need not be the pixel's own luma.
    pub fn reshape_chroma(&self, y: f32, u: f32, v: f32) -> ([f32; 2], bool) {
        let uv = u * v;
        let uv_pow = [uv, uv * uv, uv * uv * uv];
        let u_pow = [u, u * u, u * u * u];
        let v_pow = [v, v * v, v * v * v];
        let yu = y * u;
        let yv = y * v;
        let yuv = yu * v;
        let y_pow = [y, y * y, y * y * y];
        let yu_pow = [yu, yu * yu, yu * yu * yu];
        let yv_pow = [yv, yv * yv, yv * yv * yv];
        let yuv_pow = [yuv, yuv * yuv, yuv * yuv * yuv];
        let mut clamped = false;
        let mut reshape = |(constant, coef): &(f32, [[f32; 7]; 3])| {
            let mut s = *constant;
            for (o, c) in coef.iter().enumerate() {
                s += c[0] * y_pow[o];
                s += c[1] * u_pow[o];
                s += c[2] * v_pow[o];
                s += c[3] * yu_pow[o];
                s += c[4] * yv_pow[o];
                s += c[5] * uv_pow[o];
                s += c[6] * yuv_pow[o];
            }
            let out = s.clamp(0.0, 1.0);
            clamped |= out != s;
            out
        };
        let chroma = [reshape(&self.mmr[0]), reshape(&self.mmr[1])];
        (chroma, clamped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_anchors() {
        // BT.2408: 75% HLG is 203 cd/m² on a 1000-nit display; nominal white is the peak.
        assert!((reference_nits(721, 512, 512)[1] - 203.0).abs() < 1.5);
        assert!((reference_nits(940, 512, 512)[1] - 1000.0).abs() < 1e-9);
        assert_eq!(reference_nits(64, 512, 512), [0.0; 3]);
        // Superwhite holds at the peak.
        assert_eq!(reference_nits(1019, 512, 512), [1000.0; 3]);
    }

    #[test]
    fn decoder_reproduces_the_preset_tint() {
        let decoder = Decoder::new(&Profile84::rpu_data_mapping());
        let (rgb, _) = decoder.rgb_pq(721, 512, 512);
        let codes = rgb.map(|c| (f64::from(c) * 4095.0).round());
        assert_eq!(codes, [2384.0, 2387.0, 2439.0]);
    }
}
