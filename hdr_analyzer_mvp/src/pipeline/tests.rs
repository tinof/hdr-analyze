use super::*;
use clap::Parser;
use std::collections::VecDeque;

#[test]
fn the_sidecar_records_the_peak_source_the_frames_follow() {
    use crate::cli::PeakDomain::{Luma, MaxRgb};
    let no_pass = ["--hist-bin-ema-beta", "0"];
    for (flags, domain, applied) in [
        (&[][..], MaxRgb, "max"),
        (&[][..], Luma, "histogram99"),
        (&["--optimizer-profile", "conservative"][..], Luma, "max"),
        (&["--optimizer-profile", "CONSERVATIVE"][..], Luma, "max"),
        (
            &["--optimizer-profile", "aggressive"][..],
            Luma,
            "histogram99",
        ),
        (&["--peak-source", "histogram99"][..], MaxRgb, "histogram99"),
        (&["--peak-source", "histogram999"][..], Luma, "histogram999"),
        (&["--peak-source", "max"][..], Luma, "max"),
        // select_peak_pq applies an unknown name as the direct peak.
        (&["--peak-source", "bogus"][..], MaxRgb, "max"),
        (&["--peak-source", "Histogram99"][..], Luma, "max"),
        // Without the histogram pass nothing applies --peak-source.
        (&no_pass[..], Luma, "max"),
        (
            &["--peak-source", "histogram99", no_pass[0], no_pass[1]][..],
            MaxRgb,
            "max",
        ),
        // The temporal median alone runs the pass.
        (
            &[
                "--peak-source",
                "histogram99",
                no_pass[0],
                no_pass[1],
                "--hist-temporal-median",
                "3",
            ][..],
            MaxRgb,
            "histogram99",
        ),
    ] {
        let cli = Cli::parse_from(["hdr_analyzer_mvp"].iter().chain(flags));
        assert_eq!(
            applied_peak_source_for(&cli, domain),
            applied,
            "{flags:?} {domain:?}"
        );
    }
}

#[test]
fn average_smoothing_has_no_scene_initialization_bias() {
    let mut ema = None;
    let mut history = VecDeque::new();
    assert_eq!(smooth_average(0.75, &mut ema, &mut history, 0.1, 3), 0.75);
}
