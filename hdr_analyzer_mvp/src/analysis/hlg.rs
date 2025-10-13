/// Constants derived from ITU-R BT.2100 for the HLG inverse EOTF.
const A: f64 = 0.178_832_77;
const B: f64 = 0.284_668_92;
const C: f64 = 0.559_910_73;

/// Convert a normalized HLG signal (0.0-1.0) to relative linear light.
///
/// Returns a normalized value where 1.0 corresponds to the nominal peak.
pub fn hlg_signal_to_relative(signal: f64) -> f64 {
    let x = signal.clamp(0.0, 1.0);
    if x <= 0.5 {
        (x * x) / 3.0
    } else {
        ((x - C) / A).exp().mul_add(1.0, B) / 12.0
    }
}

/// Convert a normalized HLG signal (0.0-1.0) directly to absolute luminance in nits.
pub fn hlg_signal_to_nits(signal: f64, peak_nits: f64) -> f64 {
    let relative = hlg_signal_to_relative(signal);
    (relative * peak_nits).min(10_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, eps: f64) {
        assert!((a - b).abs() <= eps, "expected {a} ≈ {b} within {eps}");
    }

    #[test]
    fn test_hlg_relative_low_segment() {
        let rel = hlg_signal_to_relative(0.25);
        approx_eq(rel, (0.25 * 0.25) / 3.0, 1e-9);
    }

    #[test]
    fn test_hlg_relative_high_segment() {
        let rel = hlg_signal_to_relative(0.75);
        approx_eq(rel, 0.265, 0.02);
    }

    #[test]
    fn test_hlg_signal_to_nits() {
        let nits = hlg_signal_to_nits(0.75, 1000.0);
        approx_eq(nits, 265.0, 20.0);
    }
}
