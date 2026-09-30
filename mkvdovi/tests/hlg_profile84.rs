//! End-to-end HLG -> Dolby Vision Profile 8.4: a synthetic HLG MKV is converted without a
//! re-encode, and the output's base layer must be the source bitstream.
//!
//! Skipped (with a message) when a required tool is missing, or when the workspace-built
//! `hdr_analyzer_mvp` next to the test's mkvdovi binary is missing or predates the Dolby Vision
//! 8.4 HLG mapping. mkvdovi prefers that sibling over PATH, so build it first with
//! `cargo build -p hdr_analyzer_mvp` (or run `cargo test --workspace`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::prelude::*;

const FRAMES: &str = "48";
const SOURCE: &str = "hlg_sample.mkv";
const OUTPUT: &str = "hlg_sample.DV.mkv";

#[allow(deprecated)]
fn mkvdovi_cmd() -> Command {
    Command::cargo_bin("mkvdovi").expect("Failed to find mkvdovi binary")
}

fn tool_runs(tool: &str, arg: &str) -> bool {
    Command::new(tool).arg(arg).output().is_ok()
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

/// Reason to skip, or `None` when every tool this test (and mkvdovi) needs is available.
fn missing_prerequisite() -> Option<String> {
    for (tool, arg) in [
        ("ffmpeg", "-version"),
        ("ffprobe", "-version"),
        ("mkvmerge", "--version"),
        ("dovi_tool", "--help"),
    ] {
        if !tool_runs(tool, arg) {
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

    // Same resolution as `pipeline::analyzer_executable`: the sibling of mkvdovi wins.
    #[allow(deprecated)]
    let mkvdovi = assert_cmd::cargo::cargo_bin("mkvdovi");
    let analyzer = mkvdovi
        .parent()
        .map(|dir| dir.join(format!("hdr_analyzer_mvp{}", std::env::consts::EXE_SUFFIX)))
        .filter(|path| path.exists());
    let Some(analyzer) = analyzer else {
        return Some(
            "hdr_analyzer_mvp is not built next to mkvdovi (cargo build -p hdr_analyzer_mvp)"
                .into(),
        );
    };
    let help = Command::new(&analyzer).arg("--help").output().ok()?;
    if String::from_utf8_lossy(&help.stdout).contains("--hlg-peak-nits") {
        return Some(format!(
            "{} predates the Dolby Vision 8.4 HLG mapping; rebuild it",
            analyzer.display()
        ));
    }
    None
}

/// Short 10-bit HLG clip with a moving gradient, muxed by mkvmerge like a real source.
fn synthesize_hlg_mkv(dir: &Path) -> PathBuf {
    let raw = dir.join("synthetic.hevc");
    run(Command::new("ffmpeg").args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "nullsrc=s=256x144:r=24,format=yuv420p10le,\
         geq=lum='64+mod(X*3+N*20\\,896)':cb='512':cr='512+64*sin(N/6)'",
        "-frames:v",
        FRAMES,
        "-c:v",
        "libx265",
        "-preset",
        "ultrafast",
        "-x265-params",
        "log-level=error:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc:range=limited",
        "-pix_fmt",
        "yuv420p10le",
        "-f",
        "hevc",
        "-y",
    ])
    .arg(&raw));

    let mkv = dir.join(SOURCE);
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&mkv)
        .arg(&raw));
    std::fs::remove_file(&raw).unwrap();
    mkv
}

/// First video track as an Annex B HEVC elementary stream.
fn extract_annexb(mkv: &Path, out: &Path) {
    run(Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(mkv)
        .args([
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-bsf:v",
            "hevc_mp4toannexb",
            "-f",
            "hevc",
            "-y",
        ])
        .arg(out));
}

/// NAL units of an Annex B stream (start codes and trailing zero bytes removed).
fn nal_units(stream: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 <= stream.len() {
        if stream[index..index + 3] == [0, 0, 1] {
            starts.push(index + 3);
            index += 3;
        } else {
            index += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).map_or(stream.len(), |next| next - 3);
            let mut unit = &stream[start..end];
            while let [rest @ .., 0] = unit {
                unit = rest;
            }
            unit
        })
        .filter(|unit| !unit.is_empty())
        .collect()
}

fn nal_type(unit: &[u8]) -> u8 {
    (unit[0] >> 1) & 0x3f
}

const AUD: u8 = 35;
const RPU: u8 = 62;

/// Canonical byte form of a stream for base-layer comparison: every NAL unit in order, except
/// access unit delimiters and a non-VCL unit that repeats one already seen since the last slice.
///
/// Both differences are container round-trip artifacts, not base-layer changes: `dovi_tool
/// inject-rpu` adds an AUD to every access unit, and each MKV -> Annex B extraction prepends the
/// hvcC parameter sets (and x265's info SEI) to the first IRAP again although they are in-band.
/// Every slice NAL and every distinct parameter set must still match byte for byte.
fn canonical_stream(stream: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut canonical = Vec::new();
    let mut types = Vec::new();
    let mut since_slice: Vec<&[u8]> = Vec::new();
    for unit in nal_units(stream) {
        let kind = nal_type(unit);
        if kind == AUD {
            continue;
        }
        if kind < 32 {
            since_slice.clear();
        } else if since_slice.contains(&unit) {
            continue;
        } else {
            since_slice.push(unit);
        }
        types.push(kind);
        canonical.extend_from_slice(&[0, 0, 0, 1]);
        canonical.extend_from_slice(unit);
    }
    (canonical, types)
}

fn frame_hashes(mkv: &Path) -> Vec<String> {
    let output = run(Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(mkv)
        .args(["-map", "0:v:0", "-f", "framemd5", "-"]));
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.rsplit(',').next().map(|hash| hash.trim().to_owned()))
        .collect()
}

#[test]
fn hlg_becomes_profile84_with_the_source_base_layer() {
    if let Some(reason) = missing_prerequisite() {
        eprintln!("Skipping HLG Profile 8.4 test: {reason}");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let source = synthesize_hlg_mkv(dir.path());

    let conversion = mkvdovi_cmd()
        .current_dir(dir.path())
        .arg(SOURCE)
        .args(["--keep-source", "--verify", "--hwaccel", "none"])
        .output()
        .unwrap();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&conversion.stdout),
        String::from_utf8_lossy(&conversion.stderr)
    );
    assert!(conversion.status.success(), "mkvdovi failed:\n{log}");
    assert!(log.contains("Profile 8.4"), "no Profile 8.4 notice:\n{log}");
    assert!(log.contains("[7/7]"), "HLG should run 7 steps:\n{log}");
    assert!(
        !log.contains("[8/"),
        "step numbering overran the total:\n{log}"
    );
    assert!(source.exists(), "--keep-source must keep the HLG source");
    let output = dir.path().join(OUTPUT);
    assert!(output.exists(), "missing {}", output.display());

    // The container keeps the HLG VUI and signals Profile 8 with an HLG-compatible (ID 4) BL.
    let probe = run(Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .arg("stream=color_transfer:stream_side_data")
        .args(["-of", "default=nw=1"])
        .arg(&output));
    let probe = String::from_utf8_lossy(&probe.stdout);
    for expected in [
        "color_transfer=arib-std-b67",
        "dv_profile=8",
        "dv_bl_signal_compatibility_id=4",
        "rpu_present_flag=1",
        "el_present_flag=0",
    ] {
        assert!(
            probe.lines().any(|line| line.trim() == expected),
            "ffprobe lacks {expected}:\n{probe}"
        );
    }

    // Bitstream-exact base layer: strip the RPU from the output and compare with the source.
    let source_hevc = dir.path().join("source.hevc");
    let output_hevc = dir.path().join("output.hevc");
    let output_bl = dir.path().join("output_bl.hevc");
    extract_annexb(&source, &source_hevc);
    extract_annexb(&output, &output_hevc);
    let output_nals = std::fs::read(&output_hevc).unwrap();
    let rpus = nal_units(&output_nals)
        .into_iter()
        .filter(|unit| nal_type(unit) == RPU)
        .count();
    assert_eq!(rpus.to_string(), FRAMES, "one RPU NAL per frame expected");
    run(Command::new("dovi_tool")
        .arg("remove")
        .arg("-i")
        .arg(&output_hevc)
        .arg("-o")
        .arg(&output_bl));

    let (source_canonical, source_types) = canonical_stream(&std::fs::read(&source_hevc).unwrap());
    let (bl_canonical, bl_types) = canonical_stream(&std::fs::read(&output_bl).unwrap());
    assert_eq!(
        source_types, bl_types,
        "base layer NAL sequence differs from the source"
    );
    assert!(
        source_canonical == bl_canonical,
        "base layer bytes differ from the source ({} vs {} bytes)",
        source_canonical.len(),
        bl_canonical.len()
    );
    let slices = source_types.iter().filter(|&&kind| kind < 32).count();
    assert_eq!(slices.to_string(), FRAMES, "one slice per frame expected");

    // Pixel equivalence of the decoded video, as a second, independent check.
    let source_frames = frame_hashes(&source);
    assert_eq!(source_frames.len().to_string(), FRAMES);
    assert_eq!(source_frames, frame_hashes(&output));
}
