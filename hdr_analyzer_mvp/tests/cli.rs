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
        .stdout(predicate::str::contains("HDR"));
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
