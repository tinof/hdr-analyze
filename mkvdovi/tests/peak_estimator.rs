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
    assert!(
        third.contains("Existing measurements provenance"),
        "reused:\n{third}"
    );
    assert!(dir.path().join(OUTPUT).exists());
}

/// mkvdovi on `clip.mkv` in `dir`, expected to fail; the log.
fn refused(dir: &Path, flags: &[&str]) -> String {
    let output = mkvdovi_cmd()
        .current_dir(dir)
        .arg(SOURCE)
        .args(["--keep-source", "--hwaccel", "none"])
        .args(flags)
        .output()
        .unwrap();
    let log = log_of(&output);
    assert!(!output.status.success(), "mkvdovi must refuse:\n{log}");
    log
}

fn temp_dirs(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("mkvdovi_temp_")
        })
        .count()
}

#[test]
fn st2094_10_is_refused_for_hlg_and_reduced_analysis_before_any_temp_work() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(dir.path(), "hlg", HLG, 24, HLG_TAGS);
    mux(dir.path(), SOURCE, &raw, &[]);
    let log = refused(dir.path(), &["--peak-estimator", "st2094-10"]);
    assert!(log.contains("measures PQ input only"), "{log}");
    assert_eq!(temp_dirs(dir.path()), 0, "no temp work:\n{log}");

    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "clip",
        STEP,
        24,
        &format!("{PQ_TAGS}:{MASTERING}"),
    );
    mux(dir.path(), SOURCE, &raw, &[]);
    let log = refused(
        dir.path(),
        &[
            "--peak-estimator",
            "st2094-10",
            "--analysis-quality",
            "balanced",
        ],
    );
    assert!(log.contains("--analysis-quality accurate"), "{log}");
    assert_eq!(temp_dirs(dir.path()), 0, "no temp work:\n{log}");
}

#[test]
fn mdfix_measures_with_the_chosen_peak_estimator() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    // A Profile 8.1 clip: x265, an RPU from `dovi_tool generate`, `dovi_tool inject-rpu`.
    let raw = encode(
        dir.path(),
        "clip",
        STEP,
        24,
        &format!("{PQ_TAGS}:{MASTERING}"),
    );
    let config = dir.path().join("generate.json");
    std::fs::write(
        &config,
        r#"{"cm_version": "V40", "length": 24, "profile": "8.1",
            "shots": [{"start": 0, "duration": 24,
                       "metadata_blocks": [{"Level1": {"min_pq": 0, "max_pq": 2800, "avg_pq": 1500}}]}]}"#,
    )
    .unwrap();
    let rpu = dir.path().join("RPU.bin");
    run(std::process::Command::new("dovi_tool")
        .arg("generate")
        .arg("-j")
        .arg(&config)
        .arg("-o")
        .arg(&rpu));
    let injected = dir.path().join("injected.hevc");
    run(std::process::Command::new("dovi_tool")
        .arg("inject-rpu")
        .arg("-i")
        .arg(&raw)
        .arg("--rpu-in")
        .arg(&rpu)
        .arg("-o")
        .arg(&injected));
    mux(dir.path(), SOURCE, &injected, &[]);
    // The rebuild's fresh sidecar must name percentile, or the run is refused: a success shows
    // the estimator reached the --mdfix analysis.
    let log = run_mkvdovi(dir.path(), &["--mdfix", "--peak-estimator", "percentile"]);
    assert!(!log.contains("cannot be used"), "{log}");
    assert!(dir.path().join("clip.mdfix.DV.mkv").exists(), "{log}");
}
