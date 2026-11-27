//! `mkvdovi --peak-estimator` (ROADMAP WS1.7) on a real HDR10 clip: the analyzer measures with the
//! chosen estimator, and measurements taken with another one are re-analyzed instead of reused.
//!
//! Skipped (with a message) when ffmpeg with libx265, ffprobe, mkvmerge, mediainfo or dovi_tool is
//! missing, or when no current `hdr_analyzer_mvp` is built next to mkvdovi.

#[macro_use]
#[path = "support/playback.rs"]
mod playback;

use self::playback::*;
use std::path::Path;

const SIDECAR: &str = "clip_measurements.bin.l1.json";

/// The `peak_estimator` the analyzer recorded in the sidecar beside the source.
fn recorded_estimator(dir: &Path) -> String {
    read_json(&dir.join(SIDECAR))["peak_estimator"]
        .as_str()
        .expect("the sidecar names its peak estimator")
        .to_owned()
}

/// mkvdovi on `clip.mkv` in `dir` with these flags; the log, after asserting success.
fn run_mkvdovi(dir: &Path, flags: &[&str]) -> String {
    let output = mkvdovi_cmd()
        .current_dir(dir)
        .arg(SOURCE)
        .args(["--keep-source", "--hwaccel", "none"])
        .args(flags)
        .output()
        .unwrap();
    let log = log_of(&output);
    assert!(output.status.success(), "mkvdovi failed:\n{log}");
    log
}

#[test]
fn the_chosen_peak_estimator_measures_and_other_measurements_are_re_analyzed() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "clip",
        STEP,
        24,
        &format!("{PQ_TAGS}:{MASTERING}"),
    );
    mux(dir.path(), SOURCE, &raw, &[]);

    let first = run_mkvdovi(dir.path(), &[]);
    assert!(!first.contains("--peak-estimator"), "{first}");
    assert_eq!(recorded_estimator(dir.path()), "max");

    std::fs::remove_file(dir.path().join(OUTPUT)).unwrap();
    let second = run_mkvdovi(dir.path(), &["--peak-estimator", "percentile"]);
    assert!(
        second.contains(
            "follow the max peak estimator, not --peak-estimator percentile; re-analyzing"
        ),
        "{second}"
    );
    assert_eq!(recorded_estimator(dir.path()), "percentile");

    // The same estimator again reuses them.
    std::fs::remove_file(dir.path().join(OUTPUT)).unwrap();
    let third = run_mkvdovi(dir.path(), &["--peak-estimator", "percentile"]);
    assert!(!third.contains("re-analyzing"), "{third}");
    assert!(dir.path().join(OUTPUT).exists());
}
