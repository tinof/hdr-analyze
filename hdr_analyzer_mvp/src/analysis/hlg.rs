//! HLG measurement through the Dolby Vision Profile 8.4 reconstruction.
//!
//! A Profile 8.4 stream keeps the HLG base layer untouched and carries an RPU whose
//! reshaping curves reconstruct PQ. The analyzer measures HLG through those exact curves
//! (taken from the `dolby_vision` crate that `dovi_tool` embeds): the polynomial luma curve
//! for luma, and for max-RGB the full decode a DV decoder performs, i.e. luma curve plus the
//! chroma MMR curves, then the RPU's YCbCr->RGB matrix (see [`Dovi84Decoder`]).

use std::sync::OnceLock;

use dolby_vision::rpu::profiles::{profile84::Profile84, DoviProfile};
use dolby_vision::rpu::rpu_data_mapping::DoviReshapingCurve;

/// `analysis.luminance_mapping` value for HLG measured through the full DV 8.4 decode: luma
/// through the luma curve, max-RGB through luma curve + chroma MMR + RPU matrix. The earlier
/// `"dovi84-v1"` (luma curve only, max-RGB equal to luma) is no longer written.
pub const DOVI84_MAPPING: &str = "dovi84-v2";
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
/// `[source_min_pq, source_max_pq] / 4095` (62..3079, about 0.005..1000 nits). This is a policy
/// choice that keeps L1 inside the range the 8.4 RPU declares, not a property of the curve:
/// nominal peak white (code 940) decodes to about 3155 (~1150 nits), and superwhite codes above
/// the last pivot would extrapolate towards 10,000 nits. So any scene reaching peak white reports
/// L1 max 3079. (libplacebo's apparent plateau at this level is its display tone mapping to a
/// peak taken from L1 max_pq, or from source_max_pq when L1 is absent.)
///
/// The curve pieces meet with a tiny seam (about 3e-7 PQ, 0.001 of a 12-bit code) at
/// code 910/911; this is a property of the RPU coefficients and is kept verbatim.
pub fn dovi84_luma_to_pq(code: u16) -> f64 {
    let (source_min, source_max) = source_range_pq();
    dovi84_luma_term(code).clamp(source_min, source_max)
}

/// The RPU's declared source range `[source_min_pq, source_max_pq] / 4095`.
fn source_range_pq() -> (f64, f64) {
    let dm = Profile84::dm_data();
    (
        f64::from(dm.source_min_pq) / 4095.0,
        f64::from(dm.source_max_pq) / 4095.0,
    )
}

/// Luma contribution to every reconstructed R'G'B' channel, `coef0 * (Y' - offset0)`, for
/// a 10-bit HLG luma code: the luma curve followed by the luma column of the RPU's
/// YCbCr->RGB matrix, before any clamp to the source range.
fn dovi84_luma_term(code: u16) -> f64 {
    let mapping = Profile84::rpu_data_mapping();
    let luma = &mapping.curves[0];
    let poly = luma
        .polynomial
        .as_ref()
        .expect("Profile 8.4 luma reshaping curve is polynomial");
    let dm = Profile84::dm_data();

    let s = (f64::from(code) / 1023.0).clamp(0.0, 1.0);

    let pivots = cumulative_pivots(luma);
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
    (y - offset0) * coef0
}

/// A curve's pivots as cumulative normalized signal values (`/ 1023`).
fn cumulative_pivots(curve: &DoviReshapingCurve) -> Vec<f64> {
    let mut cumulative = 0.0;
    curve
        .pivots
        .iter()
        .map(|&pivot| {
            cumulative += f64::from(pivot);
            cumulative / 1023.0
        })
        .collect()
}

/// Number of MMR input terms: Y, Cb, Cr, Y·Cb, Y·Cr, Cb·Cr, Y·Cb·Cr.
pub const MMR_TERMS: usize = 7;
/// Highest MMR order a DV RPU can signal; lower orders are zero-filled.
pub const MMR_MAX_ORDER: usize = 3;

/// One single-piece chroma MMR curve in `f32`, zero-filled up to [`MMR_MAX_ORDER`].
#[derive(Clone, Copy, Debug)]
pub struct MmrCurve {
    pub constant: f32,
    /// `coef[order - 1][term]`, terms in [`MMR_TERMS`] order.
    pub coef: [[f32; MMR_TERMS]; MMR_MAX_ORDER],
}

/// Per-chroma-sample MMR inputs: normalized Cb/Cr and their products that do not involve
/// luma. Shared by the up to four luma samples of a 4:2:0 quad.
#[derive(Clone, Copy, Debug)]
pub struct Dovi84Chroma {
    u: f32,
    v: f32,
    uv: [f32; MMR_MAX_ORDER],
    u_pow: [f32; MMR_MAX_ORDER],
    v_pow: [f32; MMR_MAX_ORDER],
}

/// The Dolby Vision Profile 8.4 decode of one HLG Y'CbCr sample to max(R', G', B') in PQ.
///
/// Mirrors libplacebo's DV path (`pl_shader_dovi_reshape` + `pl_color_repr_decode`):
/// inputs are normalized as `code / 1023`; luma goes through the polynomial curve, each
/// chroma component through its MMR curve (terms `x`, `x²`, `x³` of
/// `[Y, Cb, Cr, Y·Cb, Y·Cr, Cb·Cr, Y·Cb·Cr]`), each clamped to its first/last pivot; then
/// `R'G'B' = M · (Y'Cb'Cr' − offset)` with the RPU's `ycc_to_rgb` matrix. libplacebo also
/// round-trips linear light through the RPU's `rgb_to_lms` and its own LMS->BT.2020 matrix;
/// their product is identity to within 1.4e-4, below 0.1 of a 12-bit code for in-range
/// values, so it is omitted. The resulting max-RGB is clamped to the same declared source
/// range as the luma table ([`dovi84_luma_to_pq`]), so superwhite never exceeds the RPU's
/// `source_max_pq` in either peak domain. Only at those clamped ends does a neutral pixel read
/// alike in both domains: mid-tones differ (code 721 is 2389 as luma, 2439 as max-RGB), because
/// the 8.4 chroma curves and matrix do not map neutral input to exactly neutral R'G'B'.
///
/// Arithmetic is plain `f32` multiply/add in a fixed order, so the CUDA kernel
/// (`kernels.cu`, which uses non-contracting `__fmul_rn`/`__fadd_rn`) reproduces it bit for bit.
#[derive(Clone, Debug)]
pub struct Dovi84Decoder {
    /// [`dovi84_luma_term`] for every 10-bit code, unclamped.
    pub luma_term: [f32; 1024],
    /// MMR curves for Cb (index 0) and Cr (index 1).
    pub mmr: [MmrCurve; 2],
    /// Chroma columns of `ycc_to_rgb`: `[R←Cb, R←Cr, G←Cb, G←Cr, B←Cb, B←Cr]`.
    pub ycc_chroma: [f32; 6],
    /// Chroma offsets `ycc_to_rgb_offset1/2` (normalized).
    pub chroma_offset: [f32; 2],
    /// Reshaped chroma clamp `[first pivot, last pivot]` (normalized).
    pub chroma_clamp: [f32; 2],
    /// Output clamp `[source_min_pq, source_max_pq] / 4095`.
    pub source_range: [f32; 2],
}

impl Dovi84Decoder {
    fn from_profile84() -> Self {
        let mapping = Profile84::rpu_data_mapping();
        let dm = Profile84::dm_data();
        let mut luma_term = [0.0_f32; 1024];
        for (code, entry) in luma_term.iter_mut().enumerate() {
            *entry = dovi84_luma_term(code as u16) as f32;
        }

        let chroma_pivots = cumulative_pivots(&mapping.curves[1]);
        assert_eq!(
            chroma_pivots,
            cumulative_pivots(&mapping.curves[2]),
            "Profile 8.4 chroma curves share their pivots"
        );
        let mmr = [1, 2].map(|component| {
            let curve = mapping.curves[component]
                .mmr
                .as_ref()
                .expect("Profile 8.4 chroma reshaping curves are MMR");
            assert_eq!(
                curve.mmr_order_minus1.len(),
                1,
                "Profile 8.4 chroma MMR has one piece"
            );
            let fixed = |int: i64, frac: u64| int as f64 + frac as f64 / COEFFICIENT_DENOM;
            let mut coef = [[0.0_f32; MMR_TERMS]; MMR_MAX_ORDER];
            let order = usize::from(curve.mmr_order_minus1[0]) + 1;
            for (o, row) in coef.iter_mut().enumerate().take(order) {
                for (term, value) in row.iter_mut().enumerate() {
                    *value =
                        fixed(curve.mmr_coef_int[0][o][term], curve.mmr_coef[0][o][term]) as f32;
                }
            }
            MmrCurve {
                constant: fixed(curve.mmr_constant_int[0], curve.mmr_constant[0]) as f32,
                coef,
            }
        });

        let coef = |value: i32| (f64::from(value) / YCC_COEF_SCALE) as f32;
        assert!(
            dm.ycc_to_rgb_coef0 == dm.ycc_to_rgb_coef3
                && dm.ycc_to_rgb_coef0 == dm.ycc_to_rgb_coef6,
            "every R'G'B' row shares the luma coefficient"
        );
        let (source_min, source_max) = source_range_pq();
        Self {
            luma_term,
            mmr,
            ycc_chroma: [
                coef(dm.ycc_to_rgb_coef1.into()),
                coef(dm.ycc_to_rgb_coef2.into()),
                coef(dm.ycc_to_rgb_coef4.into()),
                coef(dm.ycc_to_rgb_coef5.into()),
                coef(dm.ycc_to_rgb_coef7.into()),
                coef(dm.ycc_to_rgb_coef8.into()),
            ],
            chroma_offset: [
                (f64::from(dm.ycc_to_rgb_offset1) / YCC_OFFSET_SCALE) as f32,
                (f64::from(dm.ycc_to_rgb_offset2) / YCC_OFFSET_SCALE) as f32,
            ],
            chroma_clamp: [
                chroma_pivots[0] as f32,
                chroma_pivots[chroma_pivots.len() - 1] as f32,
            ],
            source_range: [source_min as f32, source_max as f32],
        }
    }

    /// Normalize one 10-bit code as libplacebo samples it (`code / 1023`).
    fn normalize(code: u16) -> f32 {
        f32::from(code.min(1023)) / 1023.0
    }

    /// MMR inputs that depend only on one chroma sample.
    pub fn chroma(&self, cb_code: u16, cr_code: u16) -> Dovi84Chroma {
        let u = Self::normalize(cb_code);
        let v = Self::normalize(cr_code);
        let uv = u * v;
        Dovi84Chroma {
            u,
            v,
            uv: [uv, uv * uv, uv * uv * uv],
            u_pow: [u, u * u, u * u * u],
            v_pow: [v, v * v, v * v * v],
        }
    }

    fn reshape_chroma(&self, curve: &MmrCurve, y: f32, chroma: &Dovi84Chroma) -> f32 {
        let yu = y * chroma.u;
        let yv = y * chroma.v;
        let yuv = yu * chroma.v;
        let y_pow = [y, y * y, y * y * y];
        let yu_pow = [yu, yu * yu, yu * yu * yu];
        let yv_pow = [yv, yv * yv, yv * yv * yv];
        let yuv_pow = [yuv, yuv * yuv, yuv * yuv * yuv];
        let mut s = curve.constant;
        for (o, coef) in curve.coef.iter().enumerate() {
            s += coef[0] * y_pow[o];
            s += coef[1] * chroma.u_pow[o];
            s += coef[2] * chroma.v_pow[o];
            s += coef[3] * yu_pow[o];
            s += coef[4] * yv_pow[o];
            s += coef[5] * chroma.uv[o];
            s += coef[6] * yuv_pow[o];
        }
        s.clamp(self.chroma_clamp[0], self.chroma_clamp[1])
    }

    /// max(R', G', B') in normalized PQ for one luma code and its 4:2:0 chroma sample.
    pub fn max_rgb_pq(&self, y_code: u16, chroma: &Dovi84Chroma) -> f32 {
        let y = Self::normalize(y_code);
        let cb = self.reshape_chroma(&self.mmr[0], y, chroma) - self.chroma_offset[0];
        let cr = self.reshape_chroma(&self.mmr[1], y, chroma) - self.chroma_offset[1];
        let luma = self.luma_term[usize::from(y_code.min(1023))];
        let m = &self.ycc_chroma;
        let red = luma + m[0] * cb + m[1] * cr;
        let green = luma + m[2] * cb + m[3] * cr;
        let blue = luma + m[4] * cb + m[5] * cr;
        red.max(green)
            .max(blue)
            .clamp(self.source_range[0], self.source_range[1])
    }
}

/// The shared Profile 8.4 decoder. Single source of truth for the CPU path and the
/// parameters uploaded to the CUDA kernel.
pub fn dovi84_decoder() -> &'static Dovi84Decoder {
    static DECODER: OnceLock<Dovi84Decoder> = OnceLock::new();
    DECODER.get_or_init(Dovi84Decoder::from_profile84)
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

    #[test]
    fn profile84_chroma_constants_are_pinned() {
        let mapping = Profile84::rpu_data_mapping();
        for component in [1, 2] {
            let curve = &mapping.curves[component];
            assert_eq!(curve.pivots, vec![0, 1023]);
            let mmr = curve.mmr.as_ref().unwrap();
            assert_eq!(mmr.mmr_order_minus1, vec![2]);
        }
        let cb = mapping.curves[1].mmr.as_ref().unwrap();
        assert_eq!((cb.mmr_constant_int[0], cb.mmr_constant[0]), (1, 1_150_183));
        assert_eq!(
            cb.mmr_coef_int[0][0].as_slice(),
            &[-1, -2, -5, 2, 5, 9, -12]
        );
        assert_eq!(cb.mmr_coef[0][2][6], 6_236_848);
        let cr = mapping.curves[2].mmr.as_ref().unwrap();
        assert_eq!(
            (cr.mmr_constant_int[0], cr.mmr_constant[0]),
            (-2, 6_266_112)
        );
        assert_eq!(cr.mmr_coef_int[0][2].as_slice(), &[1, 0, 2, -1, -8, -1, 4]);

        let dm = Profile84::dm_data();
        assert_eq!(
            [
                dm.ycc_to_rgb_coef1,
                dm.ycc_to_rgb_coef2,
                dm.ycc_to_rgb_coef4,
                dm.ycc_to_rgb_coef5,
                dm.ycc_to_rgb_coef7,
                dm.ycc_to_rgb_coef8,
            ],
            [0, 13802, -1540, -5348, 17610, 0]
        );
        assert_eq!(dm.ycc_to_rgb_offset1, 134_217_728);
        assert_eq!(dm.ycc_to_rgb_offset2, 134_217_728);
    }

    fn decoded_max_rgb_12bit(y: u16, cb: u16, cr: u16) -> f64 {
        let decoder = dovi84_decoder();
        f64::from(decoder.max_rgb_pq(y, &decoder.chroma(cb, cr))) * 4095.0
    }

    #[test]
    fn max_rgb_matches_libplacebo_reference() {
        // libplacebo DV render (apply_dolbyvision, no FBOs, L1 max_pq 4095 so no display
        // tone mapping) of lossless flat HLG patches: max(R,G,B) * 4095.
        for (codes, reference) in [
            ((721, 512, 512), 2439.1), // 75% grey: the preset's MMR tints neutrals blue
            ((237, 418, 848), 2484.8), // 75% red
            ((682, 176, 539), 2391.5), // 75% yellow
            ((387, 575, 288), 1907.9), // 50% cyan
            ((116, 960, 476), 3065.7), // 100% blue
        ] {
            let (y, cb, cr) = codes;
            let actual = decoded_max_rgb_12bit(y, cb, cr);
            assert!(
                (actual - reference).abs() <= 0.5,
                "{codes:?}: expected {reference}, got {actual}"
            );
        }
    }

    #[test]
    fn max_rgb_is_clamped_to_the_declared_source_range() {
        // 100% red decodes to 3226 and 100% grey to 3155; both stop at source_max_pq.
        assert_eq!(decoded_max_rgb_12bit(294, 387, 960).round(), 3079.0);
        assert_eq!(decoded_max_rgb_12bit(940, 512, 512).round(), 3079.0);
        assert_eq!(decoded_max_rgb_12bit(0, 512, 512).round(), 62.0);
    }

    #[test]
    fn luma_term_lut_agrees_with_the_clamped_luma_lut() {
        let decoder = dovi84_decoder();
        let (source_min, source_max) = source_range_pq();
        for (code, (&term, &clamped)) in decoder
            .luma_term
            .iter()
            .zip(dovi84_pq_lut().iter())
            .enumerate()
        {
            let expected = (f64::from(term)).clamp(source_min, source_max) as f32;
            assert_eq!(expected, clamped, "code {code}");
        }
    }
}
