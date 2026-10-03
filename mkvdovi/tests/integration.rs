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
fn test_mkvdovi_help_contains_dovi_input() {
    mkvdovi_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--dovi-input"));
}

#[test]
fn test_mkvdovi_help_has_no_encode_interface() {
    // mkvdovi no longer encodes video: the FEL compositor, its flags and composite-pipe are gone.
    mkvdovi_cmd().arg("--help").assert().success().stdout(
        predicate::str::contains("--fel-")
            .or(predicate::str::contains("--encoder"))
            .or(predicate::str::contains("composite-pipe"))
            .or(predicate::str::contains("NVENC"))
            .not(),
    );
}

#[test]
fn test_mkvdovi_rejects_removed_fel_flags() {
    for flag in [
        "--fel-crf",
        "--fel-preset",
        "--fel-encoder",
        "--fel-nvenc-preset",
        "--encoder",
    ] {
        mkvdovi_cmd()
            .args([flag, "x", "movie.mkv"])
            .assert()
            .failure()
            .code(2)
            .stderr(predicate::str::contains("unexpected argument"));
    }
}

#[test]
fn test_mkvdovi_rejects_removed_composite_pipe_args() {
    mkvdovi_cmd()
        .args(["composite-pipe", "--bl", "BL.hevc"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("unexpected argument"));
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

/// Runs `convert_file` on a real Profile 7 FEL file and checks the refusal end to end: exit
/// code 1, the refusal message, no temp directory, input kept. Skips unless `MKVDOVI_FEL_SAMPLE`
/// names a Profile 7 FEL MKV (use a short cut: the file is copied once).
#[test]
fn test_mkvdovi_refuses_fel_before_temp_work() {
    let Some(sample) = std::env::var_os("MKVDOVI_FEL_SAMPLE") else {
        eprintln!("Skipping: MKVDOVI_FEL_SAMPLE not set");
        return;
    };
    if !have_dovi_tool() {
        eprintln!("Skipping: dovi_tool not found in PATH");
        return;
    }
    let sample = Path::new(&sample);
    assert!(
        sample.exists(),
        "MKVDOVI_FEL_SAMPLE does not exist: {sample:?}"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let copy = dir.path().join("fel_sample.mkv");
    std::fs::copy(sample, &copy).expect("copy FEL sample");
    let missing = dir.path().join("missing.mkv");

    let refusal = "Profile 7 FEL input is not supported";
    let assert_no_artifacts = |case: &str| {
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .expect("read tempdir")
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            ["fel_sample.mkv"],
            "{case}: only the input may remain"
        );
    };

    mkvdovi_cmd()
        .arg("--keep-source")
        .arg(&copy)
        .assert()
        .code(1)
        .stderr(predicate::str::contains(refusal));
    assert_no_artifacts("plain");

    // Without --keep-source: a refused input is never deleted.
    mkvdovi_cmd()
        .arg("--mdfix")
        .arg(&copy)
        .assert()
        .code(1)
        .stderr(predicate::str::contains(refusal));
    assert_no_artifacts("--mdfix");

    // The refusal is a per-file error: the run continues with the next input.
    mkvdovi_cmd()
        .arg("--keep-source")
        .arg(&copy)
        .arg(&missing)
        .assert()
        .code(1)
        .stderr(predicate::str::contains(refusal))
        .stderr(predicate::str::contains("Input file not found"))
        .stderr(predicate::str::contains("2 failed"));
    assert_no_artifacts("two inputs");
}
