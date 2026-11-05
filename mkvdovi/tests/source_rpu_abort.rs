//! E15.1: a source RPU with a piece the `dolby_vision` parser cannot read (a linear-interpolation
//! polynomial, `tests/fixtures/linear_interp_RPU.bin`) must not end a multi-file run. Release
//! builds abort on the crate's `unimplemented!`; mkvdovi now reports an error for that file and
//! goes on to the next one.
//!
//! `dovi_tool` and `mediainfo` are shims on a PATH sandbox: `dovi_tool extract-rpu` writes the
//! fixture, and `mediainfo` reports Dolby Vision Profile 8 for file names containing `_dv`.
//! The first file is then sampled for its authored metadata (`sample_rpu_windows`), the second
//! is sniffed for an RPU (`classify_rpu_format`). Both reach the bad RPU.
//!
//! Skipped (with a message) when ffmpeg with libx265, ffprobe, mkvmerge or `which` is missing.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::Command;

use assert_cmd::prelude::*;

#[allow(deprecated)]
fn mkvdovi_cmd() -> Command {
    Command::cargo_bin("mkvdovi").expect("Failed to find mkvdovi binary")
}

fn locate(tool: &str) -> Option<String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !path.is_empty()).then_some(path)
}

fn missing_tools() -> Option<String> {
    for tool in ["ffmpeg", "ffprobe", "mkvmerge", "which", "cp"] {
        if locate(tool).is_none() {
            return Some(format!("{tool} not found in PATH"));
        }
    }
    let encoders = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()?;
    if !String::from_utf8_lossy(&encoders.stdout).contains("libx265") {
        return Some("ffmpeg has no libx265 encoder".into());
    }
    None
}

fn write_script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A one-second 10-bit HEVC clip (no HDR tags: only the shims make it Dolby Vision).
fn synthesize_clip(path: &Path) {
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=128x72:rate=24:duration=1")
        .args(["-c:v", "libx265", "-pix_fmt", "yuv420p10le"])
        .args(["-x265-params", "log-level=error", "-y"])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg failed to synthesize {path:?}");
}

#[test]
fn an_unparseable_source_rpu_does_not_end_a_multi_file_run() {
    if let Some(reason) = missing_tools() {
        eprintln!("Skipping source RPU abort test: {reason}");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first_dv.mkv");
    let second = dir.path().join("second.mkv");
    synthesize_clip(&first);
    fs::copy(&first, &second).unwrap();

    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/linear_interp_RPU.bin");
    let calls = dir.path().join("dovi_tool_calls.log");
    let bin = tempfile::tempdir().unwrap();
    for tool in ["ffmpeg", "ffprobe", "mkvmerge", "which"] {
        std::os::unix::fs::symlink(locate(tool).unwrap(), bin.path().join(tool)).unwrap();
    }
    let cp = locate("cp").unwrap();
    write_script(
        &bin.path().join("dovi_tool"),
        &format!(
            r#"echo "$*" >> '{calls}'
case "$1" in
  --version) echo "dovi_tool 2.3.4" ;;
  extract-rpu)
    out=""
    while [ $# -gt 0 ]; do
      if [ "$1" = "-o" ]; then out="$2"; fi
      shift
    done
    exec '{cp}' '{fixture}' "$out" ;;
  *) exit 1 ;;
esac
"#,
            calls = calls.display(),
            fixture = fixture.display(),
        ),
    );
    write_script(
        &bin.path().join("mediainfo"),
        r#"case "$*" in
  *_dv*) echo "Dolby Vision, Version 1.0, Profile 8.1, dvhe.08.06" ;;
esac
"#,
    );

    let output = mkvdovi_cmd()
        .current_dir(dir.path())
        .env("PATH", bin.path())
        .args([
            "first_dv.mkv",
            "second.mkv",
            "--keep-source",
            "--hwaccel",
            "none",
        ])
        .output()
        .unwrap();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // A release build aborts on the parent commit; this debug build panics, ends the run at the
    // first file and never prints "File 2/2".
    assert_eq!(
        output.status.signal(),
        None,
        "mkvdovi was killed (the parser aborted):\n{log}"
    );
    assert!(
        log.contains("Could not inspect source Dolby Vision RPU")
            && log.contains("linear interpolation"),
        "the first file's sampled RPU was not refused with the guard's error:\n{log}"
    );
    assert!(
        log.contains("File 2/2"),
        "the run stopped after the first file:\n{log}"
    );
    let calls = fs::read_to_string(&calls).unwrap();
    assert!(
        calls
            .lines()
            .any(|line| line.starts_with("extract-rpu") && line.contains("second.mkv")),
        "the second file was never sniffed for an RPU:\n{calls}\n{log}"
    );
    assert!(
        first.exists() && second.exists(),
        "--keep-source keeps both sources"
    );
}
