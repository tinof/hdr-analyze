//! Source consistency checks (ROADMAP P6.2, the owner's pick C1, C3, C4, C5 and C7 of the
//! `docs/ROADMAP_LOG.md` "P6 step 1, check list"). Each one compares two statements the source
//! makes about itself and returns a message when they disagree. They only warn: nothing here
//! changes a value, so every delivered RPU stays the same.
//!
//! `stated` is the static metadata the source states (`metadata::read_static_metadata`), before
//! any default or measured value fills a gap. A MaxCLL or MaxFALL of 0 means "unknown" and is
//! never compared.

use std::collections::HashMap;

use serde_json::Value;

use super::hdr10plus::hdr10plus_peak_nits;
use crate::cli::PeakSource;
use crate::rpu_check::{nits_to_pq_12_bit, L1Stats, SourceL6};

/// C1 and C4: a peak this many times the stated MaxCLL contradicts it. Chosen from the
/// development corpus (Fury Road measures 1.01× its MaxCLL near the PQ ceiling), not a spec value.
const PEAK_RATIO: f64 = 1.25;
/// C5: MaxCLL and MaxFALL may differ by this fraction of the container value.
const LIGHT_LEVEL_TOLERANCE: f64 = 0.02;
/// C5: the mastering minimum may differ by one unit of 0.0001 nits (rounding of the container's
/// decimal value).
const MIN_DML_TOLERANCE_RAW: i64 = 1;

/// How loud a check's message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Severity {
    Warn,
    Info,
}

/// A stated value that can be compared: present, finite and above 0.
fn stated_positive(stated: &HashMap<String, f64>, key: &str) -> Option<f64> {
    stated
        .get(key)
        .copied()
        .filter(|v| v.is_finite() && *v > 0.0)
}

/// Nits of a 12-bit PQ code (SMPTE ST 2084).
fn pq_12_bit_to_nits(code: u16) -> f64 {
    let (m1, m2) = (2610.0 / 16384.0, 2523.0 / 32.0);
    let (c1, c2, c3) = (3424.0 / 4096.0, 2413.0 / 128.0, 2392.0 / 128.0);
    let e = (f64::from(code) / 4095.0).powf(1.0 / m2);
    10_000.0 * ((e - c1).max(0.0) / (c2 - c3 * e)).powf(1.0 / m1)
}

/// C5: the source RPU's L6 against the container's ST 2086 and CTA-861.3 values. A value the
/// container does not state is not compared (P12 covers that case under `--mdfix`).
pub(super) fn rpu_l6_against_container(
    l6: &SourceL6,
    stated: &HashMap<String, f64>,
    mdfix: bool,
) -> Option<String> {
    let mut differences = Vec::new();
    if let Some(container) = stated_positive(stated, "max_dml") {
        if l6.max_dml_nits > 0 && container.round() != f64::from(l6.max_dml_nits) {
            differences.push(format!(
                "mastering peak {} against {container} nits",
                l6.max_dml_nits
            ));
        }
    }
    if let Some(&container) = stated
        .get("min_dml")
        .filter(|v| v.is_finite() && **v >= 0.0)
    {
        // In i64: a malformed container minimum above 6.5535 nits must not saturate a u16.
        let container_raw = (container * 10_000.0).round() as i64;
        if (container_raw - i64::from(l6.min_dml_raw)).abs() > MIN_DML_TOLERANCE_RAW {
            differences.push(format!(
                "mastering minimum {:.4} against {container:.4} nits",
                f64::from(l6.min_dml_raw) / 10_000.0
            ));
        }
    }
    for (label, key, rpu) in [
        ("MaxCLL", "max_cll", l6.max_cll),
        ("MaxFALL", "max_fall", l6.max_fall),
    ] {
        if let Some(container) = stated_positive(stated, key) {
            let rpu = f64::from(rpu);
            if rpu > 0.0 && (rpu - container).abs() > LIGHT_LEVEL_TOLERANCE * container {
                differences.push(format!("{label} {rpu} against {container} nits"));
            }
        }
    }
    if differences.is_empty() {
        return None;
    }
    let consequence = if mdfix {
        "--mdfix writes the container's values into the new L6"
    } else {
        "the Dolby Vision output keeps the RPU's L6; HDR10 playback uses the container's values"
    };
    Some(format!(
        "The source RPU's L6 disagrees with the container's HDR10 metadata (RPU against \
         container): {}. {consequence}.",
        differences.join(", ")
    ))
}

/// C4, RPU form: sampled source RPU frames whose L1 max is above 1.25 × the stated MaxCLL. The
/// sample covers short windows only, so a peak outside them is not seen.
pub(super) fn rpu_peak_against_max_cll(
    l1_frames: &[L1Stats],
    stated: &HashMap<String, f64>,
) -> Option<String> {
    let max_cll = stated_positive(stated, "max_cll")?;
    let threshold = nits_to_pq_12_bit(PEAK_RATIO * max_cll);
    let above = l1_frames.iter().filter(|f| f.max_pq > threshold).count();
    let highest = l1_frames.iter().map(|f| f.max_pq).max()?;
    (above > 0).then(|| {
        format!(
            "The source RPU's L1 peak is above 1.25 × the stated MaxCLL ({max_cll} nits) in \
             {above} of {} sampled frames (highest {:.0} nits): the MaxCLL or the RPU is wrong.",
            l1_frames.len(),
            pq_12_bit_to_nits(highest)
        )
    })
}

/// The `SceneInfo` frames of an HDR10+ metadata JSON, or `None` when there are none.
fn hdr10plus_frames(metadata: &Value) -> Option<&[Value]> {
    let frames = metadata.get("SceneInfo")?.as_array()?;
    (!frames.is_empty()).then_some(frames.as_slice())
}

/// The `MaxScl` components of one HDR10+ frame, `None` when the frame has no integer triple.
fn max_scl(frame: &Value) -> Option<Vec<u64>> {
    let values = frame.pointer("/LuminanceParameters/MaxScl")?.as_array()?;
    let values: Option<Vec<u64>> = values.iter().map(Value::as_u64).collect();
    values.filter(|v| !v.is_empty())
}

/// C4, HDR10+ form: frames whose max(MaxScl) or `--peak-source` peak is above 1.25 × the stated
/// MaxCLL. MaxScl is the per-component maximum, the quantity MaxCLL describes; the selected peak
/// is what reaches L1.
pub(super) fn hdr10plus_peak_against_max_cll(
    metadata: &Value,
    stated: &HashMap<String, f64>,
    peak_source: PeakSource,
) -> Option<String> {
    let max_cll = stated_positive(stated, "max_cll")?;
    let threshold = PEAK_RATIO * max_cll;
    let frames = hdr10plus_frames(metadata)?;
    let (mut above, mut highest_scl, mut highest_selected) = (0usize, 0.0_f64, 0.0_f64);
    for frame in frames {
        let scl = max_scl(frame)
            .and_then(|v| v.into_iter().max())
            .map(|tenths| tenths as f64 / 10.0);
        let selected = hdr10plus_peak_nits(frame, peak_source);
        highest_scl = highest_scl.max(scl.unwrap_or(0.0));
        highest_selected = highest_selected.max(selected.unwrap_or(0.0));
        if scl.is_some_and(|v| v > threshold) || selected.is_some_and(|v| v > threshold) {
            above += 1;
        }
    }
    (above > 0).then(|| {
        format!(
            "HDR10+ peaks are above 1.25 × the stated MaxCLL ({max_cll} nits) in {above} of {} \
             frames (highest MaxScl {highest_scl:.0} nits, highest --peak-source {peak_source} \
             peak {highest_selected:.0} nits): the MaxCLL or the HDR10+ metadata is wrong.",
            frames.len()
        )
    })
}

/// C7: HDR10+ metadata whose `MaxScl` is 0 on every frame. Only `dovi_tool generate` on the HDR10+
/// metadata path reads that 0 as the peak; the hybrid takes the measured peak of such a piece.
pub(super) fn hdr10plus_max_scl_all_zero(
    metadata: &Value,
    peak_source: PeakSource,
    hybrid: bool,
) -> Option<(Severity, String)> {
    let frames = hdr10plus_frames(metadata)?;
    let all_zero = frames
        .iter()
        .all(|frame| max_scl(frame).is_some_and(|v| v.iter().all(|&c| c == 0)));
    if !all_zero {
        return None;
    }
    let reads_max_scl = matches!(
        peak_source,
        PeakSource::MaxScl | PeakSource::MaxSclLuminance
    );
    Some(match (reads_max_scl, hybrid) {
        (true, false) => (
            Severity::Warn,
            format!(
                "HDR10+ MaxScl is 0 on every frame, so --peak-source {peak_source} gives dovi_tool \
                 no peak and L1 max falls to its 100-nit floor; --peak-source histogram reads the \
                 distribution values instead."
            ),
        ),
        (true, true) => (
            Severity::Info,
            format!(
                "HDR10+ MaxScl is 0 on every frame; with --peak-source {peak_source} the hybrid \
                 takes the measured peak of each piece."
            ),
        ),
        (false, _) => (
            Severity::Info,
            format!(
                "HDR10+ MaxScl is 0 on every frame; --peak-source {peak_source} does not read it."
            ),
        ),
    })
}

/// C1: a measured MaxCLL above 1.25 × the stated one. `measured` comes only from
/// `L1Sidecar::measured_light_levels`, which withholds a MaxCLL measured with skipped frames or
/// pixels.
pub(super) fn measured_max_cll_above_stated(
    stated: &HashMap<String, f64>,
    measured: Option<u32>,
) -> Option<String> {
    let max_cll = stated_positive(stated, "max_cll")?;
    let measured = measured?;
    (f64::from(measured) > PEAK_RATIO * max_cll).then(|| {
        format!(
            "The measured MaxCLL ({measured} nits) is above 1.25 × the stated MaxCLL ({max_cll} \
             nits): the stated value looks wrong. L6 keeps the stated value."
        )
    })
}

/// C3: a stated MaxFALL above the stated MaxCLL, which CTA-861.3 rules out. L6 carries both as
/// the source states them.
pub(super) fn stated_fall_above_cll(stated: &HashMap<String, f64>) -> Option<String> {
    let max_cll = stated_positive(stated, "max_cll")?;
    let max_fall = stated_positive(stated, "max_fall")?;
    (max_fall > max_cll).then(|| {
        format!(
            "The stated MaxFALL ({max_fall} nits) is above the stated MaxCLL ({max_cll} nits), \
             which CTA-861.3 rules out. L6 carries both as stated."
        )
    })
}

#[cfg(test)]
mod tests;
