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
#[path = "support/conversion.rs"]
mod conversion;

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use self::conversion::*;
use self::playback::*;

const MKVEXTRACT: (&str, &str) = ("mkvextract", "--version");

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

/// The HEVC NAL unit type of a NAL unit.
fn nal_type(unit: &[u8]) -> u8 {
    (unit[0] >> 1) & 0x3f
}

/// The output's video packets carry the source's pts, decode index for decode index. Each one,
/// without its access unit delimiter (type 35) and RPU (type 62), carries the source packet's NAL
/// units unchanged and in order, the same slices, and adds only copies of the source packet's
/// units: FFmpeg's base-layer extraction repeats the parameter sets of an IRAP picture, at the
/// start of the access unit (FFmpeg 9) or just before its slice (FFmpeg 6.1, CI).
fn assert_timing_and_slices_kept(source: &Path, output: &Path) {
    let (source_packets, output_packets) = (packets(source), packets(output));
    assert_eq!(output_packets.len(), source_packets.len());
    for (index, ((source_pts, source_data), (output_pts, output_data))) in
        source_packets.iter().zip(&output_packets).enumerate()
    {
        assert_eq!(output_pts, source_pts, "pts of packet {index}");
        let kept: Vec<&[u8]> = packet_nal_units(output_data)
            .into_iter()
            .filter(|unit| !matches!(nal_type(unit), 35 | 62))
            .collect();
        let original = packet_nal_units(source_data);
        let mut rest = kept.iter();
        assert!(
            original
                .iter()
                .all(|unit| rest.any(|carried| carried == unit)),
            "packet {index}: the source's NAL units are not carried unchanged and in order"
        );
        assert!(
            kept.iter().all(|unit| original.contains(unit)),
            "packet {index}: the output adds a NAL unit that the source packet does not carry"
        );
        let slices = |units: &[&[u8]]| -> Vec<Vec<u8>> {
            units
                .iter()
                .filter(|unit| nal_type(unit) < 32)
                .map(|unit| unit.to_vec())
                .collect()
        };
        assert_eq!(
            slices(&kept),
            slices(&original),
            "packet {index}: the slices differ"
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

#[cfg(unix)]
#[test]
fn a_source_whose_timing_cannot_be_read_is_converted_and_kept() {
    use std::os::unix::fs::PermissionsExt;

    require_tools!(MKVEXTRACT);
    let dir = tempfile::tempdir().unwrap();
    let source = pq_source(dir.path(), &[]);
    // An ffprobe that fails the video packet query of the timing check and runs the real one
    // for every other query, ahead of it on PATH.
    let real = run(Command::new("sh").arg("-c").arg("command -v ffprobe"));
    let real = String::from_utf8_lossy(&real.stdout).trim().to_owned();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let shim = bin.join("ffprobe");
    std::fs::write(
        &shim,
        format!("#!/bin/sh\ncase \"$*\" in *packet=pts,duration*) exit 1 ;; esac\nexec '{real}' \"$@\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // No --keep-source: the unchecked timing alone keeps this PQ source.
    let (run, copy, output) = convert_copy(dir.path(), &source, &[], Some(&path));
    let log = log_of(&run);
    assert!(run.status.success(), "mkvdovi failed:\n{log}");
    assert!(
        log.contains("The source's video timestamps could not be read"),
        "{log}"
    );
    assert!(
        log.contains("Keeping source file: the output's video timing was not checked"),
        "{log}"
    );
    assert!(copy.exists(), "the source must be kept");
    assert!(output.exists(), "the unchecked output is delivered");
    assert!(!log.contains("Deleting source file"), "{log}");
}
