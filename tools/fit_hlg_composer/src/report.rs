//! Evaluation of a composer against the BT.2100 reference (docs/HLG_COMPOSER.md section 6).

use std::fmt::Write as _;

use dolby_vision::rpu::rpu_data_mapping::RpuDataMapping;

use crate::fit::is_superwhite;
use crate::model::{
    delta_e_itp, pq_to_nits, reference_nits, reference_pq, ycc_chroma, Decoder, KB, KG, KR,
    SOURCE_MIN,
};
use crate::samples::{held_out_set, validation_patches};

fn decoded_nits(rgb: [f32; 3]) -> [f64; 3] {
    rgb.map(|c| pq_to_nits(f64::from(c).clamp(0.0, 1.0)))
}

struct Stats {
    mean: f64,
    p95: f64,
    max: f64,
    n: usize,
}

fn stats(mut values: Vec<f64>) -> Stats {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    Stats {
        mean: values.iter().sum::<f64>() / n.max(1) as f64,
        p95: values
            .get((n * 95 / 100).min(n.saturating_sub(1)))
            .copied()
            .unwrap_or(0.0),
        max: values.last().copied().unwrap_or(0.0),
        n,
    }
}

/// Summary numbers of one composer, used by the report and by the acceptance check.
/// R′G′B′ with this composer's luma term and the least-squares best chroma for the sample:
/// the lower bound any chroma curve can reach with this luma curve.
fn oracle_rgb(decoder: &Decoder, y: u16, cb: u16, cr: u16) -> [f32; 3] {
    let luma = decoder.luma_term(y);
    let target = reference_pq(y, cb, cr);
    let m = ycc_chroma();
    // Normal equations of min ‖luma + [mc mr]·(cb′, cr′) − target‖² over the three channels.
    let (mut a11, mut a12, mut a22, mut b1, mut b2) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for channel in 0..3 {
        let (mc, mr) = (m[channel * 2], m[channel * 2 + 1]);
        let r = target[channel] - luma;
        a11 += mc * mc;
        a12 += mc * mr;
        a22 += mr * mr;
        b1 += mc * r;
        b2 += mr * r;
    }
    let det = a11 * a22 - a12 * a12;
    let cb_prime = (b1 * a22 - b2 * a12) / det;
    let cr_prime = (a11 * b2 - a12 * b1) / det;
    std::array::from_fn(|channel| {
        (luma + m[channel * 2] * cb_prime + m[channel * 2 + 1] * cr_prime) as f32
    })
}

pub struct Evaluation {
    pub neutral_spread_max: f64,
    pub neutral_luma_error_max: f64,
    pub neutral_delta_e_max: f64,
    pub superwhite_error_max: f64,
    pub luma_monotonic: bool,
    pub luma_clamp_codes: usize,
    /// Range of the decoded neutral luma of codes 4..=64, 12-bit PQ codes.
    pub sub_black: (f64, f64),
    pub colour_mean: f64,
    pub colour_p95: f64,
    pub text: String,
}

pub fn evaluate(name: &str, mapping: &RpuDataMapping) -> Evaluation {
    let decoder = Decoder::new(mapping);
    let mut text = String::new();
    let _ = writeln!(text, "== {name}");

    let _ = writeln!(
        text,
        "neutral ramp (12-bit PQ, raw R'G'B'; reference; ΔE_ITP vs reference):"
    );
    for code in [80_u16, 120, 200, 300, 502, 721, 850, 940, 1019] {
        let (rgb, _) = decoder.rgb_pq(code, 512, 512);
        let reference = reference_pq(code, 512, 512)[1] * 4095.0;
        let de = delta_e_itp(decoded_nits(rgb), reference_nits(code, 512, 512));
        let codes = rgb.map(|c| f64::from(c) * 4095.0);
        let _ = writeln!(
            text,
            "  code {code:4}: {:7.2} {:7.2} {:7.2}  ref {reference:7.2}  ΔE {de:6.2}",
            codes[0], codes[1], codes[2]
        );
    }

    let (mut spread_max, mut luma_max, mut de_max, mut superwhite_max) =
        (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
    let (mut luma_max_code, mut de_max_code) = (0_u16, 0_u16);
    let mut clamp_codes = Vec::new();
    let mut previous = f64::NEG_INFINITY;
    let mut monotonic = true;
    let mut luma_clamp_codes = 0;
    for code in 64..=1019_u16 {
        let (rgb, _) = decoder.rgb_pq(code, 512, 512);
        let codes = rgb.map(|c| f64::from(c) * 4095.0);
        let spread = codes.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b))
            - codes.iter().fold(f64::INFINITY, |a, &b| a.min(b));
        spread_max = spread_max.max(spread);
        let luma = KR * codes[0] + KG * codes[1] + KB * codes[2];
        let reference = reference_pq(code, 512, 512)[1] * 4095.0;
        // Below the declared source_min_pq the RPU's own range makes both sides black.
        let floored = |pq: f64| pq.max(SOURCE_MIN * 4095.0);
        let luma_error = (floored(luma) - floored(reference)).abs();
        if code <= 940 {
            if luma_error > luma_max {
                (luma_max, luma_max_code) = (luma_error, code);
            }
            // Both sides floored at source_min_pq, like the luma error.
            let floor = pq_to_nits(SOURCE_MIN);
            let de = delta_e_itp(
                decoded_nits(rgb).map(|c| c.max(floor)),
                reference_nits(code, 512, 512).map(|c| c.max(floor)),
            );
            if de > de_max {
                (de_max, de_max_code) = (de, code);
            }
        } else {
            superwhite_max = superwhite_max.max(luma_error);
        }
        let rounded = codes[1].round();
        monotonic &= rounded >= previous;
        previous = rounded;
        // The clamp may hold black; it must not change a value whose reference is visible
        // (above source_min_pq).
        if decoder.luma_clamp_active(code) && reference >= SOURCE_MIN * 4095.0 {
            luma_clamp_codes += 1;
            clamp_codes.push(code);
        }
    }
    // Sub-black and black (codes 4..=64) must decode to black, not below it.
    let sub_black = (4..=64_u16)
        .map(|code| f64::from(decoder.rgb_pq(code, 512, 512).0[1]) * 4095.0)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        });
    let _ = writeln!(
        text,
        "neutral 64..=1019: max R'G'B' spread {spread_max:.3} codes; luma max |err| 64..=940 \
         {luma_max:.2} codes (at {luma_max_code}), 941..=1019 {superwhite_max:.2}; ΔE_ITP max \
         64..=940 {de_max:.2} (at {de_max_code}); monotonic {monotonic}; luma clamp active above \
         source_min on {luma_clamp_codes} codes {clamp_codes:?}; codes 4..=64 decode to \
         {:.2}..{:.2}",
        sub_black.0, sub_black.1
    );

    let mut colour = Vec::new();
    let mut colour_superwhite = Vec::new();
    let mut chroma_clamps = 0;
    let held_out = held_out_set();
    for &(y, cb, cr) in &held_out {
        let (rgb, clamped) = decoder.rgb_pq(y, cb, cr);
        chroma_clamps += usize::from(clamped);
        let de = delta_e_itp(decoded_nits(rgb), reference_nits(y, cb, cr));
        if is_superwhite(y, cb, cr) {
            colour_superwhite.push(de);
        } else {
            colour.push(de);
        }
    }
    let colour = stats(colour);
    let oracle = stats(
        held_out
            .iter()
            .filter(|&&(y, cb, cr)| !is_superwhite(y, cb, cr))
            .map(|&(y, cb, cr)| {
                let rgb = oracle_rgb(&decoder, y, cb, cr);
                delta_e_itp(decoded_nits(rgb), reference_nits(y, cb, cr))
            })
            .collect(),
    );
    let superwhite = stats(colour_superwhite);
    let _ =
        writeln!(
        text,
        "held-out colours (ΔE_ITP vs reference): {} in range: mean {:.2}, p95 {:.2}, max {:.2}; \
         {} superwhite: mean {:.2}, p95 {:.2}, max {:.2}; chroma clamp on {} of {}",
        colour.n, colour.mean, colour.p95, colour.max, superwhite.n, superwhite.mean,
        superwhite.p95, superwhite.max, chroma_clamps, held_out.len()
    );

    let _ = writeln!(
        text,
        "  per-sample least squares in PQ R'G'B' with this luma (a comparison point, not a ΔE bound): mean {:.2}, p95 {:.2}, \
         max {:.2}",
        oracle.mean, oracle.p95, oracle.max
    );
    let _ = writeln!(text, "validation patches (ΔE_ITP vs reference):");
    let mut patch_values = Vec::new();
    for patch in validation_patches() {
        let (y, cb, cr) = patch.codes;
        let (rgb, _) = decoder.rgb_pq(y, cb, cr);
        let de = delta_e_itp(decoded_nits(rgb), reference_nits(y, cb, cr));
        patch_values.push(de);
        let _ = writeln!(
            text,
            "  {:8} {:3}% L{:.2} ({y:4},{cb:4},{cr:4}): ΔE {de:6.2}",
            patch.name, patch.saturation, patch.level
        );
    }
    let patches = stats(patch_values);
    let _ = writeln!(
        text,
        "validation patches: mean {:.2}, max {:.2}",
        patches.mean, patches.max
    );

    Evaluation {
        neutral_spread_max: spread_max,
        neutral_luma_error_max: luma_max,
        neutral_delta_e_max: de_max,
        superwhite_error_max: superwhite_max,
        luma_monotonic: monotonic,
        luma_clamp_codes,
        sub_black,
        colour_mean: colour.mean,
        colour_p95: colour.p95,
        text,
    }
}

/// The acceptance criteria of docs/HLG_COMPOSER.md section 6 that the fitter can check
/// (CPU/CUDA identity and the source clamp are checked in the analyzer).
pub fn acceptance(fitted: &Evaluation, preset: &Evaluation) -> Vec<(String, bool)> {
    vec![
        (
            format!("1. neutral spread {:.3} <= 0.05", fitted.neutral_spread_max),
            fitted.neutral_spread_max <= 0.05,
        ),
        (
            format!(
                "2. neutral luma error {:.2} <= 1.0, ΔE_ITP {:.2} <= 0.5",
                fitted.neutral_luma_error_max, fitted.neutral_delta_e_max
            ),
            fitted.neutral_luma_error_max <= 1.0 && fitted.neutral_delta_e_max <= 0.5,
        ),
        (
            format!(
                "3. superwhite error {:.2} <= 1.0",
                fitted.superwhite_error_max
            ),
            fitted.superwhite_error_max <= 1.0,
        ),
        (
            format!(
                "4. monotonic {}, luma clamp codes {}",
                fitted.luma_monotonic, fitted.luma_clamp_codes
            ),
            fitted.luma_monotonic && fitted.luma_clamp_codes == 0,
        ),
        (
            format!(
                "4b. codes 4..=64 decode to {:.2}..{:.2}, within [-1, 1] of black",
                fitted.sub_black.0, fitted.sub_black.1
            ),
            fitted.sub_black.0 >= -1.0 && fitted.sub_black.1 <= 1.0,
        ),
        (
            format!(
                "5. colour mean {:.2} < preset {:.2}, p95 {:.2} < preset {:.2}",
                fitted.colour_mean, preset.colour_mean, fitted.colour_p95, preset.colour_p95
            ),
            fitted.colour_mean < preset.colour_mean && fitted.colour_p95 < preset.colour_p95,
        ),
    ]
}
