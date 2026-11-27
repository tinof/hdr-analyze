//! mkvdovi's opt-in `--peak-estimator` (ROADMAP WS1.7): which values reach the analyzer, which
//! runs are refused before any temp work, and which measurements are reused.

use std::path::Path;

use crate::cli::{Args, PeakEstimatorChoice};
use crate::external;
use crate::metadata;
use crate::progress;

use super::analyzer::{analysis_sampling, analyzer_executable};

/// The analyzer's default `--peak-percentile`, the one a `percentile` measurement by mkvdovi uses.
pub(super) const PEAK_PERCENTILE: f64 = 99.99;

/// Why this run cannot measure with `--peak-estimator`, decided before any temp work so that a
/// resumable temp dir is not discarded for a run that is refused anyway. `hlg`: the input is HLG.
/// `st2094-10` measures PQ input at full resolution only (the analyzer's own preconditions), and
/// needs an analyzer whose `--help` names it.
pub(super) fn preflight_refusal(args: &Args, hlg: bool) -> Option<String> {
    if args.peak_estimator != PeakEstimatorChoice::St2094_10 {
        return None;
    }
    if hlg {
        return Some(
            "--peak-estimator st2094-10 measures PQ input only; this input is HLG. Use max or percentile."
                .to_owned(),
        );
    }
    let (downscale, _) = analysis_sampling(args.analysis_quality);
    if downscale != 1 {
        return Some(format!(
            "--peak-estimator st2094-10 needs full-resolution analysis; --analysis-quality {} analyzes at downscale {downscale}. Use --analysis-quality accurate.",
            format!("{:?}", args.analysis_quality).to_lowercase()
        ));
    }
    if !external::analyzer_lists_option(&analyzer_executable(), "st2094-10") {
        return Some(
            "--peak-estimator st2094-10 needs an hdr_analyzer_mvp whose --help names st2094-10; rebuild or update hdr_analyzer_mvp."
                .to_owned(),
        );
    }
    None
}

/// The analyzer flags of a fresh measurement for `--peak-estimator`: none for the default `max`.
pub(super) fn analyzer_args(args: &Args) -> Vec<String> {
    match args.peak_estimator {
        PeakEstimatorChoice::Max => Vec::new(),
        estimator => vec![
            "--peak-estimator".to_owned(),
            estimator.analyzer_name().to_owned(),
        ],
    }
}

/// The estimator a sidecar's peaks follow, as `--peak-estimator` names it: a sidecar without the
/// field predates it and was measured with `max`; a `percentile` sidecar counts only at
/// [`PEAK_PERCENTILE`], the value mkvdovi measures with (`None` for another value).
pub(super) fn measured_estimator(sidecar: &metadata::L1Sidecar) -> Option<PeakEstimatorChoice> {
    match sidecar.peak_estimator.as_deref().unwrap_or("max") {
        "max" => Some(PeakEstimatorChoice::Max),
        "st2094-10" => Some(PeakEstimatorChoice::St2094_10),
        "percentile"
            if sidecar
                .peak_percentile
                .is_none_or(|p| (p - PEAK_PERCENTILE).abs() < 1e-9) =>
        {
            Some(PeakEstimatorChoice::Percentile)
        }
        _ => None,
    }
}

/// A short name for what a sidecar measured, for messages.
fn describe(sidecar: &metadata::L1Sidecar) -> String {
    match (sidecar.peak_estimator.as_deref(), sidecar.peak_percentile) {
        (Some("percentile"), Some(p)) => format!("percentile ({p})"),
        (estimator, _) => estimator.unwrap_or("max").to_owned(),
    }
}

/// Measurements whose peaks follow another estimator than `--peak-estimator` are re-analyzed,
/// not reused. Returns true when the candidate was rejected (and says so).
pub(super) fn rejects_other_estimator(
    sidecar: &metadata::L1Sidecar,
    measurements: &Path,
    args: &Args,
) -> bool {
    if measured_estimator(sidecar) == Some(args.peak_estimator) {
        return false;
    }
    progress::print_warn(&format!(
        "Existing measurements '{}' follow the {} peak estimator, not --peak-estimator {}; re-analyzing.",
        measurements.display(),
        describe(sidecar),
        args.peak_estimator.analyzer_name()
    ));
    true
}

/// `Err` when a fresh analysis measured with another estimator than the run asked for: an
/// analyzer that ignores or lacks the value (older analyzers have no `percentile`).
pub(super) fn check_fresh(sidecar: &metadata::L1Sidecar, args: &Args) -> Result<(), String> {
    if measured_estimator(sidecar) == Some(args.peak_estimator) {
        return Ok(());
    }
    Err(format!(
        "the analyzer measured with the {} peak estimator, not --peak-estimator {}; update hdr_analyzer_mvp",
        describe(sidecar),
        args.peak_estimator.analyzer_name()
    ))
}
