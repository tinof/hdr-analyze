use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::path::Path;
use std::process::Command;

#[allow(deprecated)]
fn mkvdovi_cmd() -> Command {
    Command::cargo_bin("mkvdovi").expect("Failed to find mkvdovi binary")
}

fn have_dovi_tool() -> bool {
    Command::new("dovi_tool").arg("--help").output().is_ok()
}

#[test]
fn test_mkvdovi_help() {
    mkvdovi_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("mkvdovi"));
}

#[test]
fn test_mkvdovi_execution_sample() {
    if !have_dovi_tool() {
        eprintln!("Skipping: dovi_tool not found in PATH");
        return;
    }

    let sample = Path::new("../tests/hdr-media/LG_2_DEMO_4K_L_H_03_Daylight.mkv");
    if !sample.exists() {
        eprintln!("Skipping: sample not found at {sample:?}");
        return;
    }

    mkvdovi_cmd()
        .arg(sample)
        .arg("--keep-source")
        .assert()
        .success();
}
