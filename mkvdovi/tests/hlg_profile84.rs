//! End-to-end HLG -> Dolby Vision Profile 8.4: a synthetic HLG MKV is converted without a
//! re-encode, and the output's base layer must be the source bitstream. Also covers the default
//! `bt2100` composer, `--hlg-composer preset`, and the refusal of HLG colorimetry Profile 8.4
//! cannot describe.
//!
//! Skipped (with a message) when a required tool is missing, or when the workspace-built
//! `hdr_analyzer_mvp` next to the test's mkvdovi binary is missing or predates the Dolby Vision
//! 8.4 HLG mapping with the spec 4:2:0 decode (its `--help` must name the composers' mapping
//! names, which mkvdovi probes too). mkvdovi prefers that sibling over
//! PATH, so build it first with `cargo build -p hdr_analyzer_mvp` (or run
//! `cargo test --workspace`). The refusal cases stop before analysis and need no analyzer.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::prelude::*;
use dovi84_composer::Composer;

const FRAMES: &str = "48";
const SOURCE: &str = "hlg_sample.mkv";
const OUTPUT: &str = "hlg_sample.DV.mkv";
/// Colour tags of the synthetic source: what Profile 8.4 requires.
const CONFORMING_COLOUR: &str = "colormatrix=bt2020nc:range=limited";

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

/// Reason to skip, or `None` when every external tool this test (and mkvdovi) needs is available.
fn missing_tools() -> Option<String> {
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
    None
}

/// `--help` of the analyzer mkvdovi will run, or the reason to skip.
fn analyzer_help() -> Result<String, String> {
    // Same resolution as `pipeline::analyzer_executable`: the sibling of mkvdovi wins.
    #[allow(deprecated)]
    let mkvdovi = assert_cmd::cargo::cargo_bin("mkvdovi");
    let analyzer = mkvdovi
        .parent()
        .map(|dir| dir.join(format!("hdr_analyzer_mvp{}", std::env::consts::EXE_SUFFIX)))
        .filter(|path| path.exists());
    let Some(analyzer) = analyzer else {
        return Err(
            "hdr_analyzer_mvp is not built next to mkvdovi (cargo build -p hdr_analyzer_mvp)"
                .into(),
        );
    };
    let help = Command::new(&analyzer)
        .arg("--help")
        .output()
        .map_err(|error| format!("{} --help failed: {error}", analyzer.display()))?;
    let help = String::from_utf8_lossy(&help.stdout).into_owned();
    if help.contains("--hlg-peak-nits") {
        return Err(format!(
            "{} predates the Dolby Vision 8.4 HLG mapping; rebuild it",
            analyzer.display()
        ));
    }
    if let Some(composer) = Composer::ALL
        .into_iter()
        .find(|composer| !help.contains(composer.luminance_mapping()))
    {
        return Err(format!(
            "{} predates the spec 4:2:0 HLG decode (its --help does not name {}); rebuild it",
            analyzer.display(),
            composer.luminance_mapping()
        ));
    }
    Ok(help)
}

/// Reason to skip, or `None` when the tools and an analyzer with the 8.4 HLG mapping are there.
fn missing_prerequisite() -> Option<String> {
    missing_tools().or_else(|| analyzer_help().err())
}

/// Short 10-bit HLG clip with a moving gradient, muxed by mkvmerge like a real source. `colour`
/// holds the x265 matrix and range tags (BT.2020 primaries and HLG transfer are always set).
fn synthesize_hlg_mkv(dir: &Path, colour: &str) -> PathBuf {
    let raw = dir.join("synthetic.hevc");
    run(Command::new("ffmpeg")
        .args([
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
            &format!("log-level=error:colorprim=bt2020:transfer=arib-std-b67:{colour}"),
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
    let source = synthesize_hlg_mkv(dir.path(), CONFORMING_COLOUR);

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

/// Run mkvdovi on the synthetic source in `dir` with `extra` arguments; returns success and the
/// combined log.
fn convert(dir: &Path, extra: &[&str]) -> (bool, String) {
    let conversion = mkvdovi_cmd()
        .current_dir(dir)
        .arg(SOURCE)
        .args(["--keep-source", "--hwaccel", "none"])
        .args(extra)
        .output()
        .unwrap();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&conversion.stdout),
        String::from_utf8_lossy(&conversion.stderr)
    );
    (conversion.status.success(), log)
}

/// Convert with `extra` arguments and check that `composer` was measured, written into every
/// RPU frame and confirmed by `--verify`.
fn assert_composer_end_to_end(composer: Composer, extra: &[&str]) {
    if let Some(reason) = missing_prerequisite() {
        eprintln!(
            "Skipping HLG {} composer test: {reason}",
            composer.cli_name()
        );
        return;
    }
    if !analyzer_help().is_ok_and(|help| help.contains("--hlg-composer")) {
        eprintln!(
            "Skipping HLG {} composer test: the hdr_analyzer_mvp next to mkvdovi does not list --hlg-composer; rebuild it (cargo build -p hdr_analyzer_mvp)",
            composer.cli_name()
        );
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    synthesize_hlg_mkv(dir.path(), CONFORMING_COLOUR);
    let mut args = vec!["--verify"];
    args.extend_from_slice(extra);
    let (success, log) = convert(dir.path(), &args);
    assert!(success, "mkvdovi failed:\n{log}");
    assert!(log.contains("Profile 8.4"), "no Profile 8.4 notice:\n{log}");
    // Only a non-preset composer is installed; the preset RPU stays as dovi_tool wrote it.
    let installed = log.contains(&format!(
        "Installed the {} HLG composer ({}) on {FRAMES} RPU frames",
        composer.cli_name(),
        composer.luminance_mapping()
    ));
    assert_eq!(
        installed,
        composer != Composer::Preset,
        "composer install notice:\n{log}"
    );
    assert!(
        log.contains(&format!(
            "all {FRAMES} RPU frames carry {}",
            composer.cli_name()
        )),
        "--verify did not check the composer:\n{log}"
    );
    let output = dir.path().join(OUTPUT);
    assert!(output.exists(), "missing {}", output.display());

    // The sidecar names the composer the analyzer measured through.
    let sidecar_path = dir.path().join("hlg_sample_measurements.bin.l1.json");
    let sidecar: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&sidecar_path).unwrap()).unwrap();
    assert_eq!(
        sidecar["analysis"]["luminance_mapping"],
        composer.luminance_mapping(),
        "{}",
        sidecar_path.display()
    );

    // Every frame of the muxed RPU carries the composer, independently of --verify.
    let output_hevc = dir.path().join("output.hevc");
    let rpu = dir.path().join("output_rpu.bin");
    extract_annexb(&output, &output_hevc);
    run(Command::new("dovi_tool")
        .arg("extract-rpu")
        .arg("-i")
        .arg(&output_hevc)
        .arg("-o")
        .arg(&rpu));
    let frames = dovi84_composer::check_rpu_file(&rpu, composer).unwrap();
    assert_eq!(frames.to_string(), FRAMES);
    for other in Composer::ALL.into_iter().filter(|&other| other != composer) {
        assert!(dovi84_composer::check_rpu_file(&rpu, other).is_err());
    }
}

/// `downscale` and `sample_rate` the analyzer recorded in the source's sidecar.
fn sidecar_sampling(dir: &Path) -> (u64, u64) {
    let sidecar_path = dir.join("hlg_sample_measurements.bin.l1.json");
    let sidecar: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&sidecar_path).unwrap()).unwrap();
    let analysis = &sidecar["analysis"];
    (
        analysis["downscale"].as_u64().unwrap(),
        analysis["sample_rate"].as_u64().unwrap(),
    )
}

#[test]
fn accurate_reanalyzes_coarser_measurements() {
    if let Some(reason) = missing_prerequisite() {
        eprintln!("Skipping coarse-measurement re-analysis test: {reason}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    synthesize_hlg_mkv(dir.path(), CONFORMING_COLOUR);

    let (success, log) = convert(dir.path(), &["--analysis-quality", "balanced"]);
    assert!(success, "balanced conversion failed:\n{log}");
    assert_eq!(sidecar_sampling(dir.path()), (2, 1));
    std::fs::remove_file(dir.path().join(OUTPUT)).unwrap();

    // Accurate re-analyzes the half-resolution measurements instead of reusing them with a warning.
    let (success, log) = convert(dir.path(), &["--analysis-quality", "accurate"]);
    assert!(success, "accurate conversion failed:\n{log}");
    assert!(
        log.contains("are not reused: they were analyzed at downscale 2 / sample-rate 1"),
        "no re-analysis notice:\n{log}"
    );
    assert!(!log.contains("Using existing measurements file"), "{log}");
    assert_eq!(sidecar_sampling(dir.path()), (1, 1));
}

/// The default for a non-DV input: a successful conversion deletes the source and its temp dir
/// (the cleanup at the end of `convert_file`, not `finish_success`, which only the MEL fast path
/// calls).
#[test]
fn successful_hlg_conversion_deletes_the_source_and_the_temp_dir() {
    if let Some(reason) = missing_prerequisite() {
        eprintln!("Skipping HLG source deletion test: {reason}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = synthesize_hlg_mkv(dir.path(), CONFORMING_COLOUR);

    let conversion = mkvdovi_cmd()
        .current_dir(dir.path())
        .arg(SOURCE)
        .args(["--hwaccel", "none"])
        .output()
        .unwrap();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&conversion.stdout),
        String::from_utf8_lossy(&conversion.stderr)
    );
    assert!(conversion.status.success(), "mkvdovi failed:\n{log}");
    assert!(dir.path().join(OUTPUT).exists(), "no output:\n{log}");
    assert!(log.contains("Deleting source file"), "{log}");
    assert!(
        !source.exists(),
        "the source must be deleted without --keep-source"
    );
    let temp_dirs: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("mkvdovi_temp_"))
        .collect();
    assert!(temp_dirs.is_empty(), "temp dir left behind: {temp_dirs:?}");
}

#[test]
fn hlg_composer_defaults_to_bt2100_on_every_frame() {
    assert_composer_end_to_end(Composer::Bt2100V1, &[]);
}

#[test]
fn hlg_composer_preset_keeps_the_dovi_tool_composer() {
    assert_composer_end_to_end(Composer::Preset, &["--hlg-composer", "preset"]);
}

/// A conversion of an HLG source tagged with `colour` must be refused before any temp work, and
/// the refusal must name `expected`.
fn assert_refused(colour: &str, expected: &str) {
    if let Some(reason) = missing_tools() {
        eprintln!("Skipping HLG colorimetry refusal test: {reason}");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let source = synthesize_hlg_mkv(dir.path(), colour);
    let (success, log) = convert(dir.path(), &[]);
    assert!(!success, "mkvdovi accepted {colour}:\n{log}");
    assert!(
        log.contains("Profile 8.4 RPU cannot describe"),
        "no refusal:\n{log}"
    );
    assert!(log.contains(expected), "refusal lacks {expected}:\n{log}");
    assert!(source.exists(), "a refused source must be kept");
    assert!(!dir.path().join(OUTPUT).exists());
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("mkvdovi_temp_") || name.contains("measurements"))
        .collect();
    assert!(leftovers.is_empty(), "refusal left {leftovers:?}");
}

#[test]
fn full_range_hlg_is_refused_before_any_work() {
    assert_refused(
        "colormatrix=bt2020nc:range=full",
        "ffprobe color_range = pc",
    );
}

#[test]
fn bt709_matrix_hlg_is_refused_before_any_work() {
    assert_refused(
        "colormatrix=bt709:range=limited",
        "ffprobe color_space = bt709",
    );
}
