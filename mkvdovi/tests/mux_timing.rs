//! The source's video timing in the converted file (ROADMAP E7.3). A variable-rate source and a
//! source whose video starts at 2000 ms are converted, and the output's video packets must carry
//! the source's pts, decode index for decode index, with the source's slices unchanged. A source
//! that is not Dolby Vision is deleted only after that check passes: a mux that loses the timing
//! fails the run and keeps the source. The expected times come from the source itself, read by
//! ffprobe and by mkvextract.
//!
//! Skipped (with a message) when ffmpeg with libx265, ffprobe, mkvmerge, mkvextract, mediainfo
//! or dovi_tool is missing, or when no current `hdr_analyzer_mvp` is built next to mkvdovi.

#[macro_use]
#[path = "support/playback.rs"]
mod playback;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

use self::playback::*;

const MKVEXTRACT: (&str, &str) = ("mkvextract", "--version");

/// A PQ source of 48 frames with B-frames, muxed by mkvmerge with `timing` (options for the
/// video file) and a FLAC audio track that starts at 0.
fn pq_source(dir: &Path, timing: &[&str]) -> PathBuf {
    let raw = encode(
        dir,
        "pq",
        STEP,
        48,
        &format!("keyint=24:min-keyint=24:bframes=2:b-adapt=0:{PQ_TAGS}:{MASTERING}"),
    );
    with_audio(dir, &raw, timing)
}

/// `<dir>/clip.mkv`: `video` (Annex B) muxed with `timing`, plus four seconds of FLAC.
fn with_audio(dir: &Path, video: &Path, timing: &[&str]) -> PathBuf {
    let audio = dir.join("audio.flac");
    run(Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args(["-i", "sine=frequency=440:duration=4", "-c:a", "flac", "-y"])
        .arg(&audio));
    let source = dir.join(SOURCE);
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&source)
        .args(timing)
        .arg(video)
        .arg(&audio));
    source
}

/// 24 frames at 24 fps, a 500 ms gap, then 24 frames at 30 fps (timestamps v2 for mkvmerge).
fn vfr_timestamps(dir: &Path) -> String {
    let mut stamps = String::from("# timestamp format v2\n");
    for frame in 0..48u32 {
        let ms = if frame < 24 {
            f64::from(frame) * 1000.0 / 24.0
        } else {
            1500.0 + f64::from(frame - 24) * 1000.0 / 30.0
        };
        stamps.push_str(&format!("{ms:.3}\n"));
    }
    let path = dir.join("v2.txt");
    std::fs::write(&path, stamps).unwrap();
    format!("0:{}", path.display())
}

/// Converts a copy of `source` in `<dir>/run` with `flags` and `path` (PATH) when given;
/// returns the run's output, the copy and the converted file's path.
fn convert_copy(
    dir: &Path,
    source: &Path,
    flags: &[&str],
    path: Option<&str>,
) -> (Output, PathBuf, PathBuf) {
    let work = dir.join("run");
    std::fs::create_dir_all(&work).unwrap();
    let copy = work.join(SOURCE);
    std::fs::copy(source, &copy).unwrap();
    let mut command = mkvdovi_cmd();
    command
        .current_dir(&work)
        .arg(SOURCE)
        .args(["--hwaccel", "none"])
        .args(flags);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    (command.output().unwrap(), copy, work.join(OUTPUT))
}

/// The NAL units of a Matroska packet (4-byte lengths, as mkvmerge writes HEVC).
fn packet_nal_units(packet: &[u8]) -> Vec<&[u8]> {
    let mut units = Vec::new();
    let mut rest = packet;
    while rest.len() >= 4 {
        let length = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        units.push(&rest[4..4 + length]);
        rest = &rest[4 + length..];
    }
    assert!(rest.is_empty(), "a packet ends inside a NAL unit length");
    units
}

/// The output's video packets carry the source's pts, decode index for decode index, and each
/// one, without its RPU NAL unit (type 62), ends with the source packet's NAL units unchanged
/// (FFmpeg's base-layer extraction repeats the parameter sets before an IRAP picture).
fn assert_timing_and_slices_kept(source: &Path, output: &Path) {
    let (source_packets, output_packets) = (packets(source), packets(output));
    assert_eq!(output_packets.len(), source_packets.len());
    for (index, ((source_pts, source_data), (output_pts, output_data))) in
        source_packets.iter().zip(&output_packets).enumerate()
    {
        assert_eq!(output_pts, source_pts, "pts of packet {index}");
        let kept: Vec<&[u8]> = packet_nal_units(output_data)
            .into_iter()
            .filter(|unit| (unit[0] >> 1) & 0x3f != 62)
            .collect();
        let original = packet_nal_units(source_data);
        assert!(
            kept.ends_with(&original),
            "packet {index}: the source's NAL units are not carried unchanged"
        );
    }
    // The same times as mkvextract reads them (frames and the end of the last one), and the
    // audio's first timestamp.
    let (source_times, output_times) = (mkvextract_times(source), mkvextract_times(output));
    assert_eq!(output_times.len(), source_times.len());
    for (source, output) in source_times.iter().zip(&output_times) {
        assert!(
            (source - output).abs() <= 1.0,
            "{output} ms, the source's {source} ms"
        );
    }
    assert_eq!(first_audio_pts(output), first_audio_pts(source));
}

/// The v2 timestamps of track 0 as mkvextract writes them, in milliseconds.
fn mkvextract_times(mkv: &Path) -> Vec<f64> {
    let file = mkv.with_extension("v2.txt");
    run(Command::new("mkvextract")
        .arg(mkv)
        .arg("timestamps_v2")
        .arg(format!("0:{}", file.display())));
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::remove_file(&file).unwrap();
    text.lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| line.trim().parse().unwrap())
        .collect()
}

fn first_audio_pts(mkv: &Path) -> i64 {
    let output = run(Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-read_intervals",
            "%+#1",
        ])
        .args(["-show_entries", "packet=pts", "-of", "json"])
        .arg(mkv));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    json["packets"][0]["pts"].as_i64().unwrap()
}

/// The video track's default duration (ns) as mkvmerge reports it.
fn video_default_duration(mkv: &Path) -> Option<u64> {
    let output = run(Command::new("mkvmerge").arg("-J").arg(mkv));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    json["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|track| track["type"] == "video")
        .and_then(|track| track["properties"]["default_duration"].as_u64())
}

#[test]
fn variable_frame_rate_source_keeps_its_frame_times_and_slices() {
    require_tools!(MKVEXTRACT);
    let dir = tempfile::tempdir().unwrap();
    let stamps = vfr_timestamps(dir.path());
    let source = pq_source(dir.path(), &["--timestamps", &stamps]);
    let (run, copy, output) = convert_copy(dir.path(), &source, &["--keep-source"], None);
    let log = log_of(&run);
    assert!(run.status.success(), "mkvdovi failed:\n{log}");
    assert!(copy.exists());
    assert_timing_and_slices_kept(&source, &output);
    assert!(log.contains("variable frame rate"), "{log}");
}

#[test]
fn late_video_start_is_kept_with_the_default_duration_and_the_source_is_deleted() {
    require_tools!(MKVEXTRACT);
    let dir = tempfile::tempdir().unwrap();
    let source = pq_source(dir.path(), &["--sync", "0:2000"]);
    // No --keep-source: the check passes, so the source is deleted.
    let (run, copy, output) = convert_copy(dir.path(), &source, &[], None);
    let log = log_of(&run);
    assert!(run.status.success(), "mkvdovi failed:\n{log}");
    assert!(log.contains("Deleting source file"), "{log}");
    assert!(
        !copy.exists(),
        "the source must be deleted after a passing check"
    );
    assert_timing_and_slices_kept(&source, &output);
    assert_eq!(packets(&output)[0].0, 2000);
    // --sync, not a timestamps file: mkvmerge keeps the source's default duration.
    assert!(video_default_duration(&source).is_some());
    assert_eq!(
        video_default_duration(&output),
        video_default_duration(&source)
    );
}

/// A Profile 7 MEL clip of 48 frames from 2000 ms: a generated 8.1 RPU turned MEL by
/// `dovi_tool editor` mode 1, injected, muxed by mkvmerge (as `metadata/tests.rs`
/// `profile7_clip`).
fn profile7_mel_source(dir: &Path) -> PathBuf {
    let raw = encode(
        dir,
        "p7",
        STEP,
        48,
        &format!("keyint=24:min-keyint=24:bframes=2:b-adapt=0:{PQ_TAGS}:{MASTERING}"),
    );
    let config = dir.join("generate.json");
    std::fs::write(
        &config,
        json!({"cm_version": "V40", "length": 48, "profile": "8.1"}).to_string(),
    )
    .unwrap();
    let (rpu, p7, edit) = (
        dir.join("RPU.bin"),
        dir.join("P7_RPU.bin"),
        dir.join("edit.json"),
    );
    run(Command::new("dovi_tool")
        .arg("generate")
        .arg("-j")
        .arg(&config)
        .arg("-o")
        .arg(&rpu));
    std::fs::write(&edit, json!({"mode": 1}).to_string()).unwrap();
    run(Command::new("dovi_tool")
        .arg("editor")
        .arg("-i")
        .arg(&rpu)
        .arg("-j")
        .arg(&edit)
        .arg("-o")
        .arg(&p7));
    let injected = dir.join("injected.hevc");
    run(Command::new("dovi_tool")
        .arg("inject-rpu")
        .arg("-i")
        .arg(&raw)
        .arg("--rpu-in")
        .arg(&p7)
        .arg("-o")
        .arg(&injected));
    with_audio(dir, &injected, &["--sync", "0:2000"])
}

#[test]
fn profile7_mel_fast_path_keeps_a_late_video_start() {
    require_tools!(MKVEXTRACT);
    let dir = tempfile::tempdir().unwrap();
    let source = profile7_mel_source(dir.path());
    let (run, copy, output) = convert_copy(dir.path(), &source, &[], None);
    let log = log_of(&run);
    assert!(run.status.success(), "mkvdovi failed:\n{log}");
    assert!(log.contains("without rebuilding metadata"), "{log}");
    assert!(copy.exists(), "a Dolby Vision source is kept");
    let (source_packets, output_packets) = (packets(&source), packets(&output));
    assert_eq!(output_packets.len(), source_packets.len());
    for (index, (source, output)) in source_packets.iter().zip(&output_packets).enumerate() {
        assert_eq!(output.0, source.0, "pts of packet {index}");
    }
    assert_eq!(output_packets[0].0, 2000);
}

#[cfg(unix)]
#[test]
fn a_mux_that_loses_the_timing_fails_the_run_and_keeps_the_source() {
    use std::os::unix::fs::PermissionsExt;

    require_tools!(MKVEXTRACT);
    let dir = tempfile::tempdir().unwrap();
    let source = pq_source(dir.path(), &["--sync", "0:2000"]);
    // An mkvmerge that drops --sync and --timestamps with their values, as the mux did before
    // E7.3, ahead of the real one on PATH.
    let real = run(Command::new("sh").arg("-c").arg("command -v mkvmerge"));
    let real = String::from_utf8_lossy(&real.stdout).trim().to_owned();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let shim = bin.join("mkvmerge");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nn=$#\ndrop=0\nfor arg in \"$@\"; do\n  if [ \"$drop\" = 1 ]; then drop=0\n  \
             elif [ \"$arg\" = --sync ] || [ \"$arg\" = --timestamps ]; then drop=1\n  \
             else set -- \"$@\" \"$arg\"; fi\ndone\nshift \"$n\"\nexec '{real}' \"$@\"\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let (run, copy, output) = convert_copy(dir.path(), &source, &[], Some(&path));
    let log = log_of(&run);
    assert!(!run.status.success(), "the run must fail:\n{log}");
    assert!(
        log.contains("video starts at 0 ms, the source's at 2000 ms"),
        "{log}"
    );
    assert!(
        log.contains("muxing again with the source's timestamps"),
        "{log}"
    );
    assert!(
        log.contains("The output was removed and the source kept"),
        "{log}"
    );
    assert!(copy.exists(), "the source must be kept");
    assert!(!output.exists(), "the mistimed output must be removed");
    assert!(!log.contains("Deleting source file"), "{log}");
}
