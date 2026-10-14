//! PQ inputs tagged full range are refused before any conversion work: the analyzer interprets
//! samples as limited range, and the Profile 8.1 RPU's `ycc_to_rgb` is limited range while the
//! base layer is copied unchanged. Covers a stream tagged full range, and an MKV that overrides a
//! full-range stream as limited in the container, with MediaInfo hidden so that only ffprobe's
//! decoded frame shows the stream's own range.
//!
//! Skipped (with a message) when ffmpeg with libx265, ffprobe, mkvmerge or dovi_tool is missing.
//! The refusal stops before analysis, so no analyzer is needed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::prelude::*;

const SOURCE: &str = "pq.mkv";
const OUTPUT: &str = "pq.DV.mkv";

#[allow(deprecated)]
fn mkvdovi_cmd() -> Command {
    Command::cargo_bin("mkvdovi").expect("Failed to find mkvdovi binary")
}

fn run(command: &mut Command) -> Output {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to start {command:?}: {error}"));
    assert!(
        output.status.success(),
        "{command:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Reason to skip, or `None` when every tool is there.
fn missing_tools() -> Option<String> {
    for (tool, arg) in [
        ("ffmpeg", "-version"),
        ("ffprobe", "-version"),
        ("mkvmerge", "--version"),
        ("dovi_tool", "--help"),
    ] {
        if Command::new(tool).arg(arg).output().is_err() {
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

/// A short 10-bit PQ clip whose HEVC stream is tagged full range, muxed by mkvmerge with
/// `mkvmerge_args` (for example a container colour-range override). An audio track comes first,
/// so the file's first packet is audio, as in many remuxes.
fn synthesize_full_range_pq(dir: &Path, mkvmerge_args: &[&str]) -> PathBuf {
    let audio = dir.join("pq.flac");
    run(Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=d=1",
            "-c:a",
            "flac",
            "-y",
        ])
        .arg(&audio));
    let raw = dir.join("pq.hevc");
    run(Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=128x72:r=24",
            "-frames:v",
            "12",
            "-pix_fmt",
            "yuv420p10le",
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-x265-params",
            "log-level=error:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:range=full:\
             hdr10=1:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)\
             L(10000000,1):max-cll=1000,400",
            "-f",
            "hevc",
            "-y",
        ])
        .arg(&raw));
    let source = dir.join(SOURCE);
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&source)
        .arg(&audio)
        .args(mkvmerge_args)
        .arg(&raw));
    std::fs::remove_file(raw).unwrap();
    std::fs::remove_file(audio).unwrap();
    source
}

/// Convert without `--keep-source`, so a kept source shows that the run stopped before the
/// source-deletion step. `path` replaces PATH when given.
fn convert(dir: &Path, path: Option<&Path>) -> (bool, String) {
    let mut command = mkvdovi_cmd();
    command
        .current_dir(dir)
        .arg(SOURCE)
        .args(["--hwaccel", "none"]);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let conversion = command.output().unwrap();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&conversion.stdout),
        String::from_utf8_lossy(&conversion.stderr)
    );
    (conversion.status.success(), log)
}

fn assert_refused_before_any_work(dir: &Path, source: &Path, log: &str, success: bool) {
    assert!(!success, "mkvdovi accepted a full-range PQ input:\n{log}");
    assert!(
        log.contains("PQ input is tagged full range") && log.contains("Profile 8.1"),
        "no refusal:\n{log}"
    );
    assert!(source.exists(), "a refused source must be kept");
    assert!(!dir.join(OUTPUT).exists());
    let leftovers: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("mkvdovi_temp_") || name.contains("measurements"))
        .collect();
    assert!(leftovers.is_empty(), "refusal left {leftovers:?}");
}

#[test]
fn full_range_pq_is_refused_before_any_work() {
    if let Some(reason) = missing_tools() {
        eprintln!("Skipping PQ range refusal test: {reason}");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let source = synthesize_full_range_pq(dir.path(), &[]);
    let (success, log) = convert(dir.path(), None);
    assert_refused_before_any_work(dir.path(), &source, &log, success);
    assert!(log.contains("ffprobe color_range = pc"), "{log}");
}

/// mkvmerge `--colour-range 0:1` (track 0 of the HEVC file) tags the video limited in the
/// container. ffprobe's stream entry then shows `tv`; only the decoded frame (and MediaInfo's
/// `_Original`) shows the stream's `pc`. MediaInfo is hidden by a PATH that holds only the tools
/// mkvdovi requires, plus `which`, which mkvdovi uses to find them.
#[cfg(unix)]
#[test]
fn container_override_of_full_range_pq_is_refused_without_mediainfo() {
    if let Some(reason) = missing_tools() {
        eprintln!("Skipping PQ range refusal test: {reason}");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let source = synthesize_full_range_pq(dir.path(), &["--colour-range", "0:1"]);

    let bin = tempfile::tempdir().unwrap();
    for tool in ["ffmpeg", "ffprobe", "mkvmerge", "dovi_tool", "which"] {
        let located = run(Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {tool}")));
        let target = String::from_utf8_lossy(&located.stdout).trim().to_owned();
        std::os::unix::fs::symlink(target, bin.path().join(tool)).unwrap();
    }

    let (success, log) = convert(dir.path(), Some(bin.path()));
    assert_refused_before_any_work(dir.path(), &source, &log, success);
    // The decoded frame names the stream's full range in every FFmpeg. The stream entry depends
    // on the ffprobe build: FFmpeg 9 reports the container's `tv`, while Ubuntu 24.04's FFmpeg
    // 6.1 (the release test job) still reports the bitstream's `pc`.
    assert!(log.contains("ffprobe frame color_range = pc"), "{log}");
    assert!(
        log.contains("ffprobe color_range = tv, ffprobe frame")
            || log.contains("ffprobe color_range = pc, ffprobe frame"),
        "{log}"
    );
    assert!(
        !log.contains("MediaInfo"),
        "MediaInfo was not hidden:\n{log}"
    );
}
