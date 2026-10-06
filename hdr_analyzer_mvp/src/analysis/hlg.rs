//! HLG measurement through the Dolby Vision Profile 8.4 reconstruction.
//!
//! A Profile 8.4 stream keeps the HLG base layer untouched and carries an RPU whose
//! reshaping curves (the composer) reconstruct PQ. The analyzer measures HLG through the
//! curves of the composer mkvdovi writes into that RPU (`dovi84_composer`: the
//! `dolby_vision` crate's `Profile84` preset that `dovi_tool` embeds, or the BT.2100 fit
//! selected by `--hlg-composer bt2100`): the polynomial luma curve for luma, and for max-RGB
//! the full decode a DV decoder performs, i.e. luma curve plus the chroma MMR curves, then
//! the RPU's YCbCr->RGB matrix (see [`Dovi84Decoder`]). The DM block (matrix, offsets,
//! source range) is `Profile84::dm_data()` for every composer.

use std::sync::OnceLock;

use dolby_vision::rpu::profiles::{profile84::Profile84, DoviProfile};
use dolby_vision::rpu::rpu_data_mapping::DoviReshapingCurve;
use dovi84_composer::{Composer, COEFFICIENT_LOG2_DENOM};

/// `analysis.luminance_mapping` value for PQ signals measured directly. HLG runs write the
/// composer's name instead ([`Composer::luminance_mapping`]): `"dovi84-v3"` for the preset,
/// `"dovi84-bt2100-v1-spec420"` for the BT.2100 fit. Both mean the full DV 8.4 decode (luma
/// through the luma curve, max-RGB through luma curve + chroma MMR at chroma resolution, bilinear
/// composed chroma + RPU matrix). The earlier `"dovi84-v1"` (luma curve only) and `"dovi84-v2"`
/// / `"dovi84-bt2100-v1"` (chroma reshaped per pixel) are no longer written.
pub const PQ_MAPPING: &str = "pq";

/// Fixed-point denominator of the reshaping coefficients (`int + frac / 2^23`); every
/// composer is quantized to the RPU header's `coefficient_log2_denom`.
const COEFFICIENT_DENOM: f64 = (1_u32 << COEFFICIENT_LOG2_DENOM) as f64;
/// Fixed-point scale of `ycc_to_rgb_offset0` (2^28).
const YCC_OFFSET_SCALE: f64 = 268_435_456.0;
/// Fixed-point scale of `ycc_to_rgb_coef0` (2^13).
const YCC_COEF_SCALE: f64 = 8192.0;

/// Map a 10-bit HLG luma code to normalized PQ (0.0-1.0) through the luma reshaping curve
/// of a Dolby Vision Profile 8.4 composer.
///
/// Steps: s = code/1023; pick the polynomial piece from the cumulative pivots; evaluate
/// `c0 + c1*s + c2*s^2` with coefficients `int + frac / 2^23`; clamp to the first/last
/// pivot; convert to PQ with the DM `ycc_to_rgb_offset0`/`coef0` of `Profile84::dm_data()`.
///
/// The result is finally clamped to the RPU's declared source range
/// `[source_min_pq, source_max_pq] / 4095` (62..3079, about 0.005..1000 nits). This is a policy
/// choice that keeps L1 inside the range the 8.4 RPU declares, not a property of the curve:
/// through the preset, nominal peak white (code 940) decodes to about 3155 (~1150 nits), and
/// superwhite codes above the last pivot would extrapolate towards 10,000 nits. So any scene
/// reaching peak white reports L1 max 3079. (libplacebo's apparent plateau at this level is its
/// display tone mapping to a peak taken from L1 max_pq, or from source_max_pq when L1 is
/// absent.) The BT.2100 composer holds 1000 nits (3078.7) from code 940 up, so the clamp
/// engages there only at black.
///
/// The preset's curve pieces meet with a tiny seam (about 3e-7 PQ, 0.001 of a 12-bit code) at
/// code 910/911; this is a property of the RPU coefficients and is kept verbatim.
pub fn dovi84_luma_to_pq(composer: Composer, code: u16) -> f64 {
    let (source_min, source_max) = source_range_pq();
    dovi84_luma_term(&composer.rpu_data_mapping().curves[0], code).clamp(source_min, source_max)
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
/// a 10-bit HLG luma code: the luma curve (`luma`, the composer's `curves[0]`) followed by the
/// luma column of the RPU's YCbCr->RGB matrix, before any clamp to the source range.
fn dovi84_luma_term(luma: &DoviReshapingCurve, code: u16) -> f64 {
    let dm = Profile84::dm_data();
    let pivots = cumulative_pivots(luma);
    let y = luma_curve_unclamped(luma, &pivots, code).clamp(pivots[0], pivots[pivots.len() - 1]);

    let offset0 = f64::from(dm.ycc_to_rgb_offset0) / YCC_OFFSET_SCALE;
    let coef0 = f64::from(dm.ycc_to_rgb_coef0) / YCC_COEF_SCALE;
    (y - offset0) * coef0
}

/// The luma curve's output for a 10-bit code, before the decoder clamps it to the first/last
/// pivot (`pivots` from [`cumulative_pivots`]).
fn luma_curve_unclamped(luma: &DoviReshapingCurve, pivots: &[f64], code: u16) -> f64 {
    let poly = luma
        .polynomial
        .as_ref()
        .expect("Profile 8.4 luma reshaping curve is polynomial");
    let s = (f64::from(code) / 1023.0).clamp(0.0, 1.0);

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
    y
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
/// `source_max_pq` in either peak domain. Through the preset, only at those clamped ends does a
/// neutral pixel read alike in both domains: mid-tones differ (code 721 is 2389 as luma, 2439 as
/// max-RGB), because the preset's chroma curves and matrix do not map neutral input to exactly
/// neutral R'G'B'. The BT.2100 composer keeps neutrals neutral (code 721 is 2378.6 in both).
///
/// Built per composer ([`dovi84_decoder`]); the DM block is `Profile84::dm_data()` for all.
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
    fn new(composer: Composer) -> Self {
        let mapping = composer.rpu_data_mapping();
        let dm = Profile84::dm_data();
        let mut luma_term = [0.0_f32; 1024];
        for (code, entry) in luma_term.iter_mut().enumerate() {
            *entry = dovi84_luma_term(&mapping.curves[0], code as u16) as f32;
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

    /// The composed (reshaped) Cb and Cr of one chroma sample, normalized and clamped, before
    /// the RPU's chroma offset. `mmr_luma_code` is the MMR's luma input: in the spec composer
    /// the luma down-sampled to the chroma position ([`downsample_luma`]).
    pub fn composed_chroma(&self, mmr_luma_code: u16, chroma: &Dovi84Chroma) -> [f32; 2] {
        let y = Self::normalize(mmr_luma_code);
        [
            self.reshape_chroma(&self.mmr[0], y, chroma),
            self.reshape_chroma(&self.mmr[1], y, chroma),
        ]
    }

    /// Reconstructed R', G', B' in normalized PQ for one luma code and the composed chroma at
    /// its position, before the clamp to the declared source range.
    pub fn rgb_pq_composed(&self, y_code: u16, composed: [f32; 2]) -> [f32; 3] {
        let cb = composed[0] - self.chroma_offset[0];
        let cr = composed[1] - self.chroma_offset[1];
        let luma = self.luma_term[usize::from(y_code.min(1023))];
        let m = &self.ycc_chroma;
        [
            luma + m[0] * cb + m[1] * cr,
            luma + m[2] * cb + m[3] * cr,
            luma + m[4] * cb + m[5] * cr,
        ]
    }

    /// max(R', G', B') in normalized PQ for one luma code and the composed chroma at its
    /// position, clamped to the declared source range.
    pub fn max_rgb_pq_composed(&self, y_code: u16, composed: [f32; 2]) -> f32 {
        let [red, green, blue] = self.rgb_pq_composed(y_code, composed);
        red.max(green)
            .max(blue)
            .clamp(self.source_range[0], self.source_range[1])
    }

    /// Reconstructed R', G', B' with the chroma sample reshaped on the pixel's own luma: the
    /// decode of a flat field, where the down-sampled luma is the pixel's (the section 6 tests).
    #[cfg(test)]
    pub fn rgb_pq(&self, y_code: u16, chroma: &Dovi84Chroma) -> [f32; 3] {
        self.rgb_pq_composed(y_code, self.composed_chroma(y_code, chroma))
    }

    /// max(R', G', B') of [`Self::rgb_pq`], clamped to the declared source range.
    #[cfg(test)]
    pub fn max_rgb_pq(&self, y_code: u16, chroma: &Dovi84Chroma) -> f32 {
        self.max_rgb_pq_composed(y_code, self.composed_chroma(y_code, chroma))
    }
}

/// Position of the 4:2:0 chroma samples relative to luma (H.273 chroma location type).
///
/// The spec composer runs the chroma MMR at chroma resolution and leaves the upsampling of the
/// composed chroma to the display; the analyzer upsamples bilinearly at the stream's own
/// location (docs/HLG_COMPOSER.md section 9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChromaSiting {
    /// Type 0, the H.265 default: co-sited with even luma columns, midway between luma rows.
    #[default]
    Left,
    /// Type 2: co-sited with even luma columns and even luma rows.
    TopLeft,
}

impl ChromaSiting {
    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::TopLeft => "top-left",
        }
    }
}

/// The spec composer's luma down-sampling to chroma sample `(cx, cy)` (ETSI GS CCM 001
/// §5.4.2.3.3): `[1 2 1]` around luma column `2·cx` on rows `2·cy` and `2·cy + 1`, each rounded
/// (`(a + 2b + c + 2) >> 2`), then their rounded mean (`(r0 + r1 + 1) >> 1`). Samples outside
/// the frame repeat the edge. `luma(x, y)` returns the 10-bit code at an in-frame position.
pub fn downsample_luma(
    luma: impl Fn(usize, usize) -> u16,
    width: usize,
    height: usize,
    cx: usize,
    cy: usize,
) -> u16 {
    let x = 2 * cx;
    let (left, right) = (x.saturating_sub(1), (x + 1).min(width - 1));
    let x = x.min(width - 1);
    let row = |y: usize| {
        let y = y.min(height - 1);
        (u32::from(luma(left, y)) + 2 * u32::from(luma(x, y)) + u32::from(luma(right, y)) + 2) >> 2
    };
    ((row(2 * cy) + row(2 * cy + 1) + 1) >> 1) as u16
}

/// The two chroma samples (index, weight) that bilinearly interpolate luma position `pos` along
/// one axis of `len` chroma samples. `centred` places chroma sample `j` midway between luma
/// positions `2j` and `2j + 1`; otherwise it is co-sited with `2j`. Edges repeat.
pub fn chroma_taps(pos: usize, len: usize, centred: bool) -> [(usize, f32); 2] {
    let j = (pos / 2).min(len - 1);
    let last = len - 1;
    match (centred, pos % 2) {
        (false, 0) => [(j, 1.0), (j, 0.0)],
        (false, _) => [(j, 0.5), ((j + 1).min(last), 0.5)],
        (true, 0) => [(j, 0.75), (j.saturating_sub(1), 0.25)],
        (true, _) => [(j, 0.75), ((j + 1).min(last), 0.25)],
    }
}

/// Bilinear upsampling of one composed-chroma component to luma position `(x, y)`.
/// `at(cx, cy)` reads the component at a chroma sample. The operation order is fixed
/// (`w0·a + w1·b` per chroma row, then the same across the two rows) so the CUDA kernel can
/// reproduce it bit for bit.
pub fn upsample_chroma(
    at: impl Fn(usize, usize) -> f32,
    chroma_width: usize,
    chroma_height: usize,
    siting: ChromaSiting,
    x: usize,
    y: usize,
) -> f32 {
    let columns = chroma_taps(x, chroma_width, false);
    let rows = chroma_taps(y, chroma_height, siting == ChromaSiting::Left);
    let row = |cy: usize| columns[0].1 * at(columns[0].0, cy) + columns[1].1 * at(columns[1].0, cy);
    rows[0].1 * row(rows[0].0) + rows[1].1 * row(rows[1].0)
}

/// Slot of a composer in the per-composer caches below.
fn cache_slot(composer: Composer) -> usize {
    Composer::ALL
        .iter()
        .position(|&candidate| candidate == composer)
        .expect("Composer::ALL lists every composer")
}

/// The shared Profile 8.4 decoder of a composer, built once per composer. Single source of
/// truth for the CPU path and the parameters uploaded to the CUDA kernel.
pub fn dovi84_decoder(composer: Composer) -> &'static Dovi84Decoder {
    static DECODERS: [OnceLock<Dovi84Decoder>; Composer::ALL.len()] =
        [const { OnceLock::new() }; Composer::ALL.len()];
    DECODERS[cache_slot(composer)].get_or_init(|| Dovi84Decoder::new(composer))
}

/// Lookup table of [`dovi84_luma_to_pq`] for every 10-bit code, built once per composer.
///
/// Single source of truth for the HLG mapping on both the CPU and CUDA paths.
pub fn dovi84_pq_lut(composer: Composer) -> &'static [f32; 1024] {
    static LUTS: [OnceLock<[f32; 1024]>; Composer::ALL.len()] =
        [const { OnceLock::new() }; Composer::ALL.len()];
    LUTS[cache_slot(composer)].get_or_init(|| {
        let mut lut = [0.0_f32; 1024];
        for (code, entry) in lut.iter_mut().enumerate() {
            *entry = dovi84_luma_to_pq(composer, code as u16) as f32;
        }
        lut
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE_12BIT: f64 = 1.0 / 4095.0;

    fn assert_pq_near(code: u16, expected: f64, tolerance_codes: f64) {
        let actual = dovi84_luma_to_pq(Composer::Preset, code);
        assert!(
            (actual - expected).abs() <= tolerance_codes * CODE_12BIT,
            "code {code}: expected PQ {expected} ± {tolerance_codes}/4095, got {actual}"
        );
    }

    #[test]
    fn lut_is_monotonic_in_12bit_codes() {
        // The 910/911 piece seam dips by ~3e-7 PQ; monotonicity holds in the 12-bit unit
        // every consumer sees, and within 1e-6 in raw PQ.
        let lut = dovi84_pq_lut(Composer::Preset);
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
            assert_eq!(
                dovi84_pq_lut(Composer::Preset)[code],
                source_min,
                "code {code}"
            );
        }
    }

    #[test]
    fn superwhite_plateaus_at_source_max() {
        let source_max = (3079.0_f64 / 4095.0) as f32;
        for code in 940..1024 {
            assert_eq!(
                dovi84_pq_lut(Composer::Preset)[code],
                source_max,
                "code {code}"
            );
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
        let decoder = dovi84_decoder(Composer::Preset);
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

    /// SMPTE ST 2084 EOTF: normalized PQ to nits.
    fn pq_to_nits(e: f64) -> f64 {
        let (m1, m2) = (2610.0 / 16384.0, 2523.0 / 4096.0 * 128.0);
        let (c1, c2, c3) = (
            3424.0 / 4096.0,
            2413.0 / 4096.0 * 32.0,
            2392.0 / 4096.0 * 32.0,
        );
        let p = e.max(0.0).powf(1.0 / m2);
        10_000.0 * ((p - c1).max(0.0) / (c2 - c3 * p)).powf(1.0 / m1)
    }

    /// SMPTE ST 2084 inverse EOTF: nits to normalized PQ.
    fn nits_to_pq(nits: f64) -> f64 {
        let (m1, m2) = (2610.0 / 16384.0, 2523.0 / 4096.0 * 128.0);
        let (c1, c2, c3) = (
            3424.0 / 4096.0,
            2413.0 / 4096.0 * 32.0,
            2392.0 / 4096.0 * 32.0,
        );
        let y = (nits.max(0.0) / 10_000.0).powf(m1);
        ((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2)
    }

    /// ITU-R BT.2100 ICtCp (PQ) of linear BT.2020 RGB in nits.
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

    /// ITU-R BT.2124 ΔE_ITP between two linear BT.2020 colours in nits.
    fn delta_e_itp(a: [f64; 3], b: [f64; 3]) -> f64 {
        let (a, b) = (ictcp(a), ictcp(b));
        let (di, dt, dp) = (a[0] - b[0], 0.5 * (a[1] - b[1]), a[2] - b[2]);
        720.0 * (di * di + dt * dt + dp * dp).sqrt()
    }

    /// Decoded R'G'B' of a neutral HLG luma code (Cb = Cr = 512) in 12-bit PQ codes, and
    /// ΔE_ITP against the neutral of the same BT.2020 luminance.
    fn neutral_tint(decoder: &Dovi84Decoder, y_code: u16) -> ([f64; 3], f64) {
        let rgb_pq = decoder
            .rgb_pq(y_code, &decoder.chroma(512, 512))
            .map(f64::from);
        let linear = rgb_pq.map(pq_to_nits);
        let luminance = 0.2627 * linear[0] + 0.6780 * linear[1] + 0.0593 * linear[2];
        (
            rgb_pq.map(|pq| pq * 4095.0),
            delta_e_itp(linear, [luminance; 3]),
        )
    }

    #[test]
    fn profile84_preset_tints_neutrals_blue() {
        // Regression record of the preset's tint (ROADMAP P8): neutral HLG input does not
        // decode to neutral R'G'B'. ΔE_ITP (BT.2124) is taken against R = G = B with the same
        // BT.2020 luminance.
        let decoder = dovi84_decoder(Composer::Preset);
        for (code, expected_rgb, expected_de) in [
            (200, [879.0, 868.0, 793.0], 10.26),
            (502, [1819.0, 1813.0, 1851.0], 4.48),
            (721, [2384.0, 2387.0, 2439.0], 6.75),
            (940, [3134.0, 3143.0, 3155.0], 2.72),
        ] {
            let (rgb, de) = neutral_tint(decoder, code);
            for (channel, (actual, expected)) in rgb.iter().zip(expected_rgb).enumerate() {
                assert!(
                    (actual - expected).abs() <= 0.5,
                    "code {code} channel {channel}: expected {expected}, got {actual:.2}"
                );
            }
            assert!(
                (de - expected_de).abs() <= 0.05,
                "code {code}: expected ΔE_ITP {expected_de}, got {de:.3}"
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
        let decoder = dovi84_decoder(Composer::Preset);
        let (source_min, source_max) = source_range_pq();
        for (code, (&term, &clamped)) in decoder
            .luma_term
            .iter()
            .zip(dovi84_pq_lut(Composer::Preset).iter())
            .enumerate()
        {
            let expected = (f64::from(term)).clamp(source_min, source_max) as f32;
            assert_eq!(expected, clamped, "code {code}");
        }
    }

    /// `source_min_pq` (62) in normalized PQ: the floor of the acceptance comparisons, where
    /// both sides read the same after the analyzer's source clamp.
    const SOURCE_MIN_PQ: f64 = 62.0 / 4095.0;

    /// BT.2100 / BT.2408 HLG-to-PQ reference of a neutral 10-bit code in nits
    /// (docs/HLG_COMPOSER.md section 3): limited-range signal, normalized inverse OETF, OOTF of
    /// a reference display with L_W = 1000 and L_B = 0 (gamma 1.2), per-channel clip to 1000.
    fn bt2100_neutral_nits(code: u16) -> f64 {
        let signal = ((f64::from(code) - 64.0) / 876.0).max(0.0);
        let a = 0.178_832_77_f64;
        let b = 1.0 - 4.0 * a;
        let c = 0.5 - a * (4.0 * a).ln();
        let scene = if signal <= 0.5 {
            signal * signal / 3.0
        } else {
            (((signal - c) / a).exp() + b) / 12.0
        };
        // Neutral: the scene luminance Y_S equals every channel, so F_D = 1000 * E^0.2 * E.
        (1000.0 * scene.powf(0.2) * scene).min(1000.0)
    }

    /// Raw decoded R'G'B' (before the source clamp) of a neutral code, in 12-bit PQ codes.
    fn neutral_rgb_12bit(decoder: &Dovi84Decoder, y_code: u16) -> [f64; 3] {
        decoder
            .rgb_pq(y_code, &decoder.chroma(512, 512))
            .map(|pq| f64::from(pq) * 4095.0)
    }

    #[test]
    fn bt2100_composer_keeps_neutrals_neutral() {
        let decoder = dovi84_decoder(Composer::Bt2100V1);
        for code in 64..=1019 {
            let rgb = neutral_rgb_12bit(decoder, code);
            let spread = rgb.iter().copied().fold(f64::MIN, f64::max)
                - rgb.iter().copied().fold(f64::MAX, f64::min);
            assert!(
                spread <= 0.05,
                "code {code}: R'G'B' {rgb:?} spreads {spread:.4} codes"
            );
        }
    }

    #[test]
    fn bt2100_composer_tracks_the_reference_up_to_nominal_white() {
        let decoder = dovi84_decoder(Composer::Bt2100V1);
        for code in 64..=940 {
            let reference_pq = nits_to_pq(bt2100_neutral_nits(code)).max(SOURCE_MIN_PQ);
            let luma_pq = f64::from(decoder.luma_term[usize::from(code)]).max(SOURCE_MIN_PQ);
            let luma_error = (luma_pq - reference_pq).abs() * 4095.0;
            assert!(
                luma_error <= 1.0,
                "code {code}: luma {:.2} vs reference {:.2}",
                luma_pq * 4095.0,
                reference_pq * 4095.0
            );
            let rgb = decoder
                .rgb_pq(code, &decoder.chroma(512, 512))
                .map(|pq| pq_to_nits(f64::from(pq).max(SOURCE_MIN_PQ)));
            let de = delta_e_itp(rgb, [pq_to_nits(reference_pq); 3]);
            assert!(
                de <= 0.5,
                "code {code}: ΔE_ITP {de:.3} against the reference"
            );
        }
    }

    #[test]
    fn bt2100_composer_holds_superwhite_at_1000_nits() {
        let peak = nits_to_pq(1000.0) * 4095.0;
        assert!((peak - 3078.73).abs() < 0.01, "PQ(1000 nits) = {peak}");
        let decoder = dovi84_decoder(Composer::Bt2100V1);
        for code in 941..=1019 {
            for (channel, value) in neutral_rgb_12bit(decoder, code).into_iter().enumerate() {
                assert!(
                    (value - peak).abs() <= 1.0,
                    "code {code} channel {channel}: {value:.2}, expected {peak:.2}"
                );
            }
        }
    }

    #[test]
    fn bt2100_composer_matches_the_fitter_anchors() {
        // tools/fit_hlg_composer report; the BT.2100 reference reads 878.94 and 2378.24 at
        // codes 200 and 721.
        let decoder = dovi84_decoder(Composer::Bt2100V1);
        for (code, expected) in [(200, 879.40), (721, 2378.60), (940, 3078.73)] {
            for (channel, value) in neutral_rgb_12bit(decoder, code).into_iter().enumerate() {
                assert!(
                    (value - expected).abs() <= 0.05,
                    "code {code} channel {channel}: {value:.3}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn luma_lut_is_monotonic_for_every_composer() {
        for composer in Composer::ALL {
            let codes: Vec<u16> = dovi84_pq_lut(composer)
                .iter()
                .map(|&pq| (f64::from(pq) * 4095.0).round() as u16)
                .collect();
            assert!(
                codes.windows(2).all(|pair| pair[0] <= pair[1]),
                "{composer:?}"
            );
        }
    }

    #[test]
    fn bt2100_luma_output_clamp_holds_only_black() {
        // The first pivot is black (64), so the decoder's output clamp holds black and
        // sub-black at 64/1023 instead of letting the curve fall below PQ 0. It may engage only
        // where the reference itself is below source_min_pq; above that it must be inactive.
        let mapping = Composer::Bt2100V1.rpu_data_mapping();
        let luma = &mapping.curves[0];
        let pivots = cumulative_pivots(luma);
        let (first, last) = (pivots[0], pivots[pivots.len() - 1]);
        assert_eq!(first, 64.0 / 1023.0);
        for code in 64..=1019 {
            let y = luma_curve_unclamped(luma, &pivots, code);
            if bt2100_neutral_nits(code) >= pq_to_nits(SOURCE_MIN_PQ) {
                assert!(
                    (first..=last).contains(&y),
                    "code {code}: curve output {y} outside [{first}, {last}]"
                );
            }
        }
        // Black and sub-black decode to black (0.3 of a 12-bit code above PQ 0), not below it.
        let decoder = dovi84_decoder(Composer::Bt2100V1);
        for code in 4..=64 {
            let green = neutral_rgb_12bit(decoder, code)[1];
            assert!((0.0..=1.0).contains(&green), "code {code}: {green}");
        }
    }

    #[test]
    fn max_rgb_source_clamp_holds_for_every_composer() {
        for composer in Composer::ALL {
            let decoder = dovi84_decoder(composer);
            let neutral = decoder.chroma(512, 512);
            for code in [0, 64] {
                let black = f64::from(decoder.max_rgb_pq(code, &neutral)) * 4095.0;
                assert_eq!(black.round(), 62.0, "{composer:?} code {code}");
            }
            for code in [940, 1019] {
                let white = decoder.max_rgb_pq(code, &neutral);
                assert!(white <= decoder.source_range[1], "{composer:?} code {code}");
                assert_eq!(
                    (f64::from(white) * 4095.0).round(),
                    3079.0,
                    "{composer:?} code {code}"
                );
            }
        }
    }

    /// BT.2100 / BT.2408 reference of any limited-range BT.2020 NCL code triplet, per channel in
    /// nits (docs/HLG_COMPOSER.md section 3; the neutral case is [`bt2100_neutral_nits`]).
    fn bt2100_nits(y: u16, cb: u16, cr: u16) -> [f64; 3] {
        let (kr, kb) = (0.2627, 0.0593);
        let luma = (f64::from(y) - 64.0) / 876.0;
        let red = luma + 2.0 * (1.0 - kr) * (f64::from(cr) - 512.0) / 896.0;
        let blue = luma + 2.0 * (1.0 - kb) * (f64::from(cb) - 512.0) / 896.0;
        let green = (luma - kr * red - kb * blue) / (1.0 - kr - kb);
        let a = 0.178_832_77_f64;
        let (b, c) = (1.0 - 4.0 * a, 0.5 - a * (4.0 * a).ln());
        let scene = [red, green, blue].map(|signal: f64| {
            let signal = signal.max(0.0);
            if signal <= 0.5 {
                signal * signal / 3.0
            } else {
                (((signal - c) / a).exp() + b) / 12.0
            }
        });
        let scene_luminance = kr * scene[0] + (1.0 - kr - kb) * scene[1] + kb * scene[2];
        let gain = 1000.0 * scene_luminance.max(0.0).powf(0.2);
        scene.map(|e| (gain * e).clamp(0.0, 1000.0))
    }

    #[test]
    fn bt2100_composer_beats_the_preset_on_colour_patches() {
        // Criterion 5 of docs/HLG_COMPOSER.md section 6 on the 52 patches of
        // scripts/validate_hlg_dv84_color.sh (R, G, B, Y, C, M at 100% and 75% saturation and
        // grey, at HLG levels 0.25 to 1.0), through the f32 decoder against the reference.
        let (kr, kb) = (0.2627, 0.0593);
        let encode = |rgb: [f64; 3]| {
            let y = kr * rgb[0] + (1.0 - kr - kb) * rgb[1] + kb * rgb[2];
            let code = |v: f64| v.round_ties_even() as u16;
            (
                code(64.0 + 876.0 * y),
                code(512.0 + 896.0 * (rgb[2] - y) / (2.0 * (1.0 - kb))),
                code(512.0 + 896.0 * (rgb[0] - y) / (2.0 * (1.0 - kr))),
            )
        };
        let colours = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
            [1.0, 0.0, 1.0],
        ];
        let mut patches = Vec::new();
        for level in [0.25, 0.5, 0.75, 1.0] {
            patches.push(encode([level; 3]));
            for saturation in [1.0, 0.75] {
                for rgb in colours {
                    patches.push(encode(
                        rgb.map(|c| level * (saturation * c + 1.0 - saturation)),
                    ));
                }
            }
        }
        assert_eq!(patches.len(), 52);
        let mean_delta_e = |composer: Composer| {
            let decoder = dovi84_decoder(composer);
            patches
                .iter()
                .map(|&(y, cb, cr)| {
                    let decoded = decoder
                        .rgb_pq(y, &decoder.chroma(cb, cr))
                        .map(|pq| pq_to_nits(f64::from(pq).clamp(0.0, 1.0)));
                    delta_e_itp(decoded, bt2100_nits(y, cb, cr))
                })
                .sum::<f64>()
                / patches.len() as f64
        };
        let (preset, fitted) = (
            mean_delta_e(Composer::Preset),
            mean_delta_e(Composer::Bt2100V1),
        );
        // Fitter report: preset 26.14, bt2100 11.87.
        assert!(
            fitted < 0.5 * preset,
            "mean ΔE_ITP: bt2100 {fitted:.2}, preset {preset:.2}"
        );
        assert!(
            (fitted - 11.87).abs() < 0.05,
            "bt2100 mean ΔE_ITP {fitted:.3}"
        );
        assert!(
            (bt2100_nits(721, 512, 512)[1] - bt2100_neutral_nits(721)).abs() < 1e-9,
            "the colour reference agrees with the neutral one"
        );
    }
}
