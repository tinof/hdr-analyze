use assert_cmd::cargo::cargo_bin;
use predicates::prelude::*;
use std::path::Path;
use std::process::Command;

fn mkvdolby_cmd() -> Command {
    Command::new(cargo_bin("mkvdolby"))
}

fn have_dovi_tool() -> bool {
    Command::new("dovi_tool").arg("--help").output().is_ok()
}

#[test]
fn test_mkvdolby_help() {
    mkvdolby_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("mkvdolby"));
}

#[test]
fn test_mkvdolby_execution_sample() {
    if !have_dovi_tool() {
        eprintln!("Skipping: dovi_tool not found in PATH");
        return;
    }

    let sample = Path::new("../tests/hdr-media/LG_2_DEMO_4K_L_H_03_Daylight.mkv");
    if !sample.exists() {
        eprintln!("Skipping: sample not found at {sample:?}");
        return;
    }

    mkvdolby_cmd()
        .arg(sample)
        .arg("--keep-source")
        .assert()
        .success();
}
