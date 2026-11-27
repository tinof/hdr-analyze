//! The in-process RPU injection end to end (ROADMAP E20.1): one source converted with the
//! default `--dovi-inject internal` and with `--dovi-inject external` (ffmpeg's `BL.hevc` and
//! `dovi_tool inject-rpu`) must give the same video stream, byte for byte. The default run has a
//! `dovi_tool` first on PATH that fails `inject-rpu`, so it shows that the default never runs it.
//! Cases: HDR10, HDR10+, and HLG with each composer.
//!
//! Skipped (with a message) when ffmpeg with libx265, ffprobe, mkvmerge, mediainfo, dovi_tool or
//! hdr10plus_tool is missing, or when no current `hdr_analyzer_mvp` is built next to mkvdovi.

#[macro_use]
#[path = "support/playback.rs"]
mod playback;
#[path = "support/conversion.rs"]
mod conversion;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

use self::conversion::*;
use self::playback::*;

/// A `dovi_tool` first on PATH in `<dir>/bin` that fails `inject-rpu` and runs the real one
/// otherwise. Returns the PATH value.
#[cfg(unix)]
fn without_inject_rpu(dir: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;

    let real = run(Command::new("sh").arg("-c").arg("command -v dovi_tool"));
    let real = String::from_utf8_lossy(&real.stdout).trim().to_owned();
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let shim = bin.join("dovi_tool");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nif [ \"$1\" = inject-rpu ]; then echo 'inject-rpu must not run' >&2; exit 1; fi\nexec '{real}' \"$@\"\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// The first video stream of `mkv` as raw HEVC, as ffmpeg extracts it.
fn video_stream(mkv: &Path) -> Vec<u8> {
    let out = mkv.with_extension("video.hevc");
    run(Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(mkv)
        .args(["-map", "0:v:0", "-c:v", "copy", "-f", "hevc", "-y"])
        .arg(&out));
    std::fs::read(out).unwrap()
}

/// Converts `source` in both modes with `flags` (in `<dir>/internal` and `<dir>/external`) and
/// asserts equal video streams, one RPU per frame in each.
#[cfg(unix)]
fn assert_both_modes_agree(dir: &Path, source: &Path, flags: &[&str], frames: usize) {
    let internal_dir = dir.join("internal");
    let path = without_inject_rpu(dir);
    let (run, _, internal) = convert_copy(&internal_dir, source, flags, Some(&path));
    let log = log_of(&run);
    assert!(run.status.success(), "the default run failed:\n{log}");
    assert!(
        log.contains("Reading the base layer's frame order"),
        "{log}"
    );
    assert!(!log.contains("inject-rpu must not run"), "{log}");

    let external_dir = dir.join("external");
    let flags: Vec<&str> = flags
        .iter()
        .copied()
        .chain(["--dovi-inject", "external"])
        .collect();
    let (run, _, external) = convert_copy(&external_dir, source, &flags, None);
    let log = log_of(&run);
    assert!(run.status.success(), "the external run failed:\n{log}");
    assert!(log.contains("Extracting base layer HEVC stream"), "{log}");

    let (internal, external) = (video_stream(&internal), video_stream(&external));
    assert!(
        internal == external,
        "the video streams differ: {} and {} bytes, SHA-256 {} and {}",
        internal.len(),
        external.len(),
        sha256(&internal),
        sha256(&external)
    );
    assert_eq!(rpus_of(&internal), frames, "one RPU per frame");
}

/// The RPU NAL units (type 62) of a raw HEVC stream.
fn rpus_of(stream: &[u8]) -> usize {
    nal_units(stream)
        .iter()
        .filter(|unit| !unit.is_empty() && (unit[0] >> 1) & 0x3f == 62)
        .count()
}

#[cfg(unix)]
#[test]
fn hdr10_converts_to_the_same_stream_in_both_injection_modes() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let source = pq_source(dir.path(), &[]);
    assert_both_modes_agree(dir.path(), &source, &["--keep-source"], 48);
}

#[cfg(unix)]
#[test]
fn hlg_converts_to_the_same_stream_in_both_injection_modes_with_each_composer() {
    require_tools!();
    for composer in ["bt2100", "preset"] {
        let dir = tempfile::tempdir().unwrap();
        let raw = encode(
            dir.path(),
            "hlg",
            HLG,
            48,
            &format!("keyint=24:min-keyint=24:bframes=2:b-adapt=0:{HLG_TAGS}"),
        );
        let source = mux(dir.path(), SOURCE, &raw, &[]);
        assert_both_modes_agree(
            dir.path(),
            &source,
            &["--keep-source", "--hlg-composer", composer],
            48,
        );
    }
}

/// HDR10+ metadata for `frames` frames in `hdr10plus_tool`'s JSON layout, one scene.
fn hdr10plus_json(frames: usize) -> serde_json::Value {
    let scenes: Vec<_> = (0..frames)
        .map(|frame| {
            json!({
                "BezierCurveData": {"Anchors": [102, 205, 307, 410, 512, 614, 717, 819, 922],
                                    "KneePointX": 0, "KneePointY": 0},
                "LuminanceParameters": {
                    "AverageRGB": 300,
                    "LuminanceDistributions": {
                        "DistributionIndex": [1, 5, 10, 25, 50, 75, 90, 95, 99],
                        "DistributionValues": [0, 10, 100, 200, 300, 400, 500, 600, 10000],
                    },
                    "MaxScl": [0, 0, 0],
                },
                "NumberOfWindows": 1,
                "TargetedSystemDisplayMaximumLuminance": 400,
                "SceneFrameIndex": frame,
                "SceneId": 0,
                "SequenceFrameIndex": frame,
            })
        })
        .collect();
    json!({
        "JSONInfo": {"HDR10plusProfile": "B", "Version": "1.0"},
        "SceneInfo": scenes,
        "SceneInfoSummary": {"SceneFirstFrameIndex": [0], "SceneFrameNumbers": [frames]},
        "ToolInfo": {"Tool": "hdr10plus_tool", "Version": "1.7.2"},
    })
}

/// The PQ source with HDR10+ in the SEI of every frame (`hdr10plus_tool inject`).
fn hdr10plus_source(dir: &Path) -> PathBuf {
    let raw = encode(
        dir,
        "pq",
        STEP,
        48,
        &format!("keyint=24:min-keyint=24:bframes=2:b-adapt=0:{PQ_TAGS}:{MASTERING}"),
    );
    let metadata = dir.join("hdr10plus.json");
    std::fs::write(&metadata, hdr10plus_json(48).to_string()).unwrap();
    let with_sei = dir.join("hdr10plus.hevc");
    run(Command::new("hdr10plus_tool")
        .arg("inject")
        .arg("-i")
        .arg(&raw)
        .arg("-j")
        .arg(&metadata)
        .arg("-o")
        .arg(&with_sei));
    with_audio(dir, &with_sei, &[])
}

#[cfg(unix)]
#[test]
fn hdr10plus_converts_to_the_same_stream_in_both_injection_modes() {
    require_tools!(("hdr10plus_tool", "--version"));
    let dir = tempfile::tempdir().unwrap();
    let source = hdr10plus_source(dir.path());
    assert_both_modes_agree(dir.path(), &source, &["--keep-source"], 48);
}
