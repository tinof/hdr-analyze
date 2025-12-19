use assert_cmd::cargo::cargo_bin;
use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn verifier_cmd() -> Command {
    Command::new(cargo_bin("verifier"))
}

#[test]
fn test_missing_input_shows_usage() {
    verifier_cmd()
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage:"));
}

#[test]
fn test_nonexistent_file() {
    verifier_cmd().arg("nonexistent_file.bin").assert().failure();
}

#[test]
fn test_invalid_file_content() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let invalid_file = temp_dir.path().join("invalid.bin");
    fs::write(&invalid_file, b"not a valid measurement file").expect("Failed to write test file");

    verifier_cmd().arg(&invalid_file).assert().failure();
}
