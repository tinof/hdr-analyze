use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[allow(deprecated)]
fn analyzer_cmd() -> Command {
    Command::cargo_bin("hdr_analyzer_mvp").expect("Failed to find hdr_analyzer_mvp binary")
}

#[test]
fn test_help_flag() {
    analyzer_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("HDR"))
        .stdout(predicate::str::contains("--crop-probes"))
        .stdout(predicate::str::contains("--peak-estimator"))
        .stdout(predicate::str::contains("--peak-percentile"))
        .stdout(predicate::str::contains("--dump-frame-stats"));
}

#[test]
fn test_version_flag() {
    analyzer_cmd().arg("--version").assert().success();
}

#[test]
fn test_missing_input_shows_error() {
    analyzer_cmd()
        .assert()
        .failure()
        .stderr(predicate::str::contains("required"));
}

#[test]
fn test_nonexistent_input_file() {
    analyzer_cmd()
        .arg("nonexistent_video.mkv")
        .assert()
        .failure();
}

#[test]
fn test_invalid_madvr_version() {
    analyzer_cmd()
        .arg("--madvr-version")
        .arg("99")
        .arg("input.mkv")
        .assert()
        .failure();
}

#[test]
fn test_invalid_downscale_value() {
    analyzer_cmd()
        .arg("--downscale")
        .arg("8")
        .arg("input.mkv")
        .assert()
        .failure();
}

#[test]
fn test_invalid_min_percentile() {
    analyzer_cmd()
        .args(["--min-percentile", "100.1", "input.mkv"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--min-percentile must be between 0 and 100",
        ));
}

#[test]
fn test_invalid_peak_percentile() {
    analyzer_cmd()
        .args(["--peak-percentile", "100.1", "input.mkv"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--peak-percentile must be between 0 and 100",
        ));
}

#[test]
fn test_invalid_peak_estimator() {
    analyzer_cmd()
        .args(["--peak-estimator", "unknown", "input.mkv"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value"));
}
