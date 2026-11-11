//! HDR10+ payload coverage end to end (ROADMAP P9.4): `hdr10plus_tool` gives a frame without
//! its own HDR10+ payload the previous frame's, so mkvdovi counts the payloads in the stream.
//! A clip whose every frame carries one converts with the count and no warning. A clip with a
//! gap converts with a warning under the default and `--hdr10plus-l1 hdr10plus`, and an explicit
//! `--hdr10plus-l1 hybrid` refuses it before any analysis. A gap at the start, which makes
//! `hdr10plus_tool extract` fail, is named the same way instead of "no dynamic metadata".
//!
//! The gaps are cut by removing the HDR10+ SEI NAL units of chosen access units from a stream
//! `hdr10plus_tool inject` wrote, so the expected counts come from the cut, not from mkvdovi.
//!
//! Skipped (with a message) when ffmpeg with libx265, ffprobe, mkvmerge, dovi_tool or
//! hdr10plus_tool is missing, or when no `hdr_analyzer_mvp` is built next to the test's mkvdovi
//! binary (`cargo build -p hdr_analyzer_mvp`). MediaInfo classifying a clip as anything other
//! than HDR10+ is a failure, not a skip.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::prelude::*;
use serde_json::{json, Value};

const SOURCE: &str = "clip.mkv";
const OUTPUT: &str = "clip.DV.mkv";
const MEASUREMENTS: &str = "clip_measurements.bin";
const TEMP_DIR: &str = "mkvdovi_temp_clip";
const COVERAGE_FILE: &str = "hdr10plus_coverage.json";
const FRAMES: u64 = 48;
const PREFIX_SEI: u8 = 39;
/// ST 2094-40 in a T.35 message: country 0xB5, provider 0x003C, oriented code 1, application 4,
/// version 1.
const HDR10PLUS_IDENTITY: [u8; 7] = [0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04, 0x01];

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

fn sibling_analyzer() -> Option<PathBuf> {
    #[allow(deprecated)]
    let mkvdovi = assert_cmd::cargo::cargo_bin("mkvdovi");
    mkvdovi
        .parent()
        .map(|dir| dir.join(format!("hdr_analyzer_mvp{}", std::env::consts::EXE_SUFFIX)))
        .filter(|path| path.exists())
}

/// Reason to skip, or `None` when every tool is there.
fn missing_prerequisite() -> Option<String> {
    for (tool, arg) in [
        ("ffmpeg", "-version"),
        ("ffprobe", "-version"),
        ("mkvmerge", "--version"),
        ("dovi_tool", "--help"),
        ("hdr10plus_tool", "--version"),
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
    if sibling_analyzer().is_none() {
        return Some(
            "hdr_analyzer_mvp is not built next to mkvdovi (cargo build -p hdr_analyzer_mvp)"
                .into(),
        );
    }
    None
}

/// HDR10+ metadata in `hdr10plus_tool`'s JSON layout: two scenes, a different peak each.
fn hdr10plus_json() -> Value {
    let frames: Vec<Value> = (0..FRAMES)
        .map(|frame| {
            let (scene, start, peak_nits) = if frame < 24 {
                (0, 0, 1000)
            } else {
                (1, 24, 2000)
            };
            json!({
                "BezierCurveData": {"Anchors": [102, 205, 307, 410, 512, 614, 717, 819, 922],
                                    "KneePointX": 0, "KneePointY": 0},
                "LuminanceParameters": {
                    "AverageRGB": 300,
                    "LuminanceDistributions": {
                        "DistributionIndex": [1, 5, 10, 25, 50, 75, 90, 95, 99],
                        "DistributionValues": [0, 10, 100, 200, 300, 400, 500, 600, peak_nits * 10],
                    },
                    "MaxScl": [0, 0, 0],
                },
                "NumberOfWindows": 1,
                "TargetedSystemDisplayMaximumLuminance": 400,
                "SceneFrameIndex": frame - start,
                "SceneId": scene,
                "SequenceFrameIndex": frame,
            })
        })
        .collect();
    json!({
        "JSONInfo": {"HDR10plusProfile": "B", "Version": "1.0"},
        "SceneInfo": frames,
        "SceneInfoSummary": {"SceneFirstFrameIndex": [0, 24], "SceneFrameNumbers": [24, 24]},
        "ToolInfo": {"Tool": "hdr10plus_tool", "Version": "1.7.2"},
    })
}

/// A 10-bit PQ stream of [`FRAMES`] frames with B-frames (decode order is not display order),
/// two slices per picture, and HDR10+ in the SEI of every frame.
fn hdr10plus_stream(dir: &Path) -> Vec<u8> {
    let plain = dir.join("plain.hevc");
    run(Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "nullsrc=s=256x144:r=24,format=yuv420p10le,geq=lum='300+mod(X+3*N\\,256)':cb='512':cr='512'",
            "-frames:v",
            &FRAMES.to_string(),
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-x265-params",
            "log-level=error:keyint=24:min-keyint=24:bframes=3:b-pyramid=1:b-adapt=0:scenecut=0:\
             slices=2:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:range=limited:\
             hdr10=1:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)\
             L(10000000,1):max-cll=1000,400:repeat-headers=1",
            "-pix_fmt",
            "yuv420p10le",
            "-f",
            "hevc",
            "-y",
        ])
        .arg(&plain));
    let metadata = dir.join("hdr10plus.json");
    std::fs::write(&metadata, serde_json::to_vec(&hdr10plus_json()).unwrap()).unwrap();
    let with_sei = dir.join("hdr10plus.hevc");
    run(Command::new("hdr10plus_tool")
        .arg("inject")
        .arg("-i")
        .arg(&plain)
        .arg("-j")
        .arg(&metadata)
        .arg("-o")
        .arg(&with_sei));
    let stream = std::fs::read(&with_sei).unwrap();
    for path in [plain, metadata, with_sei] {
        std::fs::remove_file(path).unwrap();
    }
    stream
}

/// Start offsets (of the start code) of the NAL units of an Annex B stream, with their ends.
fn nal_spans(stream: &[u8]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 <= stream.len() {
        if stream[index..index + 3] == [0, 0, 1] {
            starts.push(index);
            index += 3;
        } else {
            index += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| (start, starts.get(i + 1).copied().unwrap_or(stream.len())))
        .collect()
}

/// `stream` without the HDR10+ prefix SEI NAL units of the access units (decode order) in
/// `cut`. Asserts that each of them had exactly one, as `hdr10plus_tool inject` writes it.
fn without_payloads(stream: &[u8], cut: RangeInclusive<u64>) -> Vec<u8> {
    let mut kept = Vec::with_capacity(stream.len());
    // Access unit of the next slice: a prefix SEI precedes the first slice of its picture.
    let mut next_access_unit = 0u64;
    let mut removed = Vec::new();
    for (start, end) in nal_spans(stream) {
        let unit = &stream[start + 3..end];
        let kind = (unit[0] >> 1) & 0x3F;
        let is_first_slice = kind <= 21 && unit.len() > 2 && unit[2] & 0x80 != 0;
        let is_hdr10plus = kind == PREFIX_SEI
            && unit
                .windows(HDR10PLUS_IDENTITY.len())
                .any(|window| window == HDR10PLUS_IDENTITY);
        if is_hdr10plus && cut.contains(&next_access_unit) {
            removed.push(next_access_unit);
            continue;
        }
        if is_first_slice {
            next_access_unit += 1;
        }
        kept.extend_from_slice(&stream[start..end]);
    }
    assert_eq!(next_access_unit, FRAMES, "access units in the stream");
    let expected: Vec<u64> = cut.collect();
    assert_eq!(
        removed, expected,
        "one HDR10+ SEI removed per access unit of the cut"
    );
    kept
}

/// `<dir>/clip.mkv` from an Annex B stream.
fn mux(dir: &Path, stream: &[u8]) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let annexb = dir.join("clip.hevc");
    std::fs::write(&annexb, stream).unwrap();
    let clip = dir.join(SOURCE);
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&clip)
        .args(["--default-duration", "0:24fps"])
        .arg(&annexb));
    std::fs::remove_file(annexb).unwrap();
    clip
}

/// Runs `mkvdovi` on `<dir>/clip.mkv` with `--no-resume`; the status and the log.
fn convert(dir: &Path, flags: &[&str]) -> (bool, String) {
    convert_resuming(dir, &[&["--no-resume"], flags].concat())
}

/// Runs `mkvdovi` on `<dir>/clip.mkv`, resuming its temp directory; the status and the log.
fn convert_resuming(dir: &Path, flags: &[&str]) -> (bool, String) {
    let conversion = mkvdovi_cmd()
        .current_dir(dir)
        .arg(SOURCE)
        .args(["--keep-source", "--hwaccel", "none"])
        .args(flags)
        .output()
        .unwrap();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&conversion.stdout),
        String::from_utf8_lossy(&conversion.stderr)
    );
    assert!(
        log.contains("HDR10+"),
        "MediaInfo must classify the clip as HDR10+:\n{log}"
    );
    (conversion.status.success(), log)
}

fn clean(dir: &Path) {
    for name in [OUTPUT, MEASUREMENTS] {
        let _ = std::fs::remove_file(dir.join(name));
    }
    let _ = std::fs::remove_dir_all(dir.join(TEMP_DIR));
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.to_string_lossy().contains("_measurements") {
            let _ = std::fs::remove_file(path);
        }
    }
}

const PARTIAL_WARNING: &str = "28 of 48 frames carry their own";
const HYBRID_RAN: &str = "HDR10+ hybrid L1:";

#[test]
fn full_coverage_converts_with_the_count_and_no_warning() {
    if let Some(reason) = missing_prerequisite() {
        eprintln!("Skipping HDR10+ coverage test: {reason}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let clip = dir.path().join("full");
    mux(&clip, &hdr10plus_stream(dir.path()));
    let (success, log) = convert(&clip, &[]);
    assert!(success, "mkvdovi failed:\n{log}");
    assert!(
        log.contains("HDR10+ payloads: all 48 frames carry their own."),
        "{log}"
    );
    assert!(!log.contains("frames carry their own;"), "{log}");
    assert!(!log.contains("coverage was not counted"), "{log}");
    assert!(log.contains(HYBRID_RAN), "{log}");
    assert!(clip.join(OUTPUT).exists());
}

#[test]
fn a_coverage_gap_warns_under_the_default_and_the_metadata_mode_and_refuses_an_explicit_hybrid() {
    if let Some(reason) = missing_prerequisite() {
        eprintln!("Skipping HDR10+ coverage test: {reason}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let clip = dir.path().join("gap");
    mux(
        &clip,
        &without_payloads(&hdr10plus_stream(dir.path()), 10..=29),
    );

    let (success, log) = convert(&clip, &["--hdr10plus-l1", "hybrid"]);
    assert!(!success, "an explicit hybrid must refuse:\n{log}");
    assert!(
        log.contains("--hdr10plus-l1 hybrid cannot be used here: only 28 of 48 frames"),
        "{log}"
    );
    assert!(
        log.contains("1 gap(s) in decode order, the longest 20 frame(s), the first at frame 10"),
        "{log}"
    );
    assert!(!clip.join(OUTPUT).exists(), "{log}");
    assert!(
        !clip.join(MEASUREMENTS).exists(),
        "the analyzer ran:\n{log}"
    );
    let sealed: Value = serde_json::from_slice(
        &std::fs::read(clip.join(TEMP_DIR).join(COVERAGE_FILE)).expect("the count stays sealed"),
    )
    .unwrap();
    assert_eq!(sealed["coverage"]["access_units"], 48);
    assert_eq!(sealed["coverage"]["own"], 28);

    // A fresh extraction withdraws a sealed count: plant "all 48" over the sealed one and
    // withdraw the stream's marker, so the resumed run extracts the stream again.
    let temp = clip.join(TEMP_DIR);
    let mut planted = sealed.clone();
    planted["coverage"]["own"] = json!(48);
    planted["coverage"]["gaps"] = json!(0);
    planted["coverage"]["longest_gap"] = json!(0);
    planted["coverage"]["first_gap"] = Value::Null;
    std::fs::write(temp.join(COVERAGE_FILE), planted.to_string()).unwrap();
    std::fs::remove_file(temp.join("video.hevc.done")).unwrap();
    let (success, log) = convert_resuming(&clip, &["--hdr10plus-l1", "hybrid"]);
    assert!(!success, "the planted count must not be reused:\n{log}");
    assert!(log.contains("only 28 of 48 frames"), "{log}");
    clean(&clip);

    let (success, log) = convert(&clip, &[]);
    assert!(success, "the default converts:\n{log}");
    assert!(log.contains(PARTIAL_WARNING), "{log}");
    assert!(log.contains("the other 20"), "{log}");
    assert!(
        log.contains(HYBRID_RAN),
        "the default stays on the hybrid:\n{log}"
    );
    assert!(clip.join(OUTPUT).exists());
    clean(&clip);

    let (success, log) = convert(&clip, &["--hdr10plus-l1", "hdr10plus"]);
    assert!(success, "the metadata mode converts:\n{log}");
    assert!(log.contains(PARTIAL_WARNING), "{log}");
    assert!(!log.contains(HYBRID_RAN), "{log}");
    assert!(clip.join(OUTPUT).exists());
}

#[test]
fn a_coverage_gap_at_the_start_is_named_instead_of_no_dynamic_metadata() {
    if let Some(reason) = missing_prerequisite() {
        eprintln!("Skipping HDR10+ coverage test: {reason}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let clip = dir.path().join("start-gap");
    mux(
        &clip,
        &without_payloads(&hdr10plus_stream(dir.path()), 0..=4),
    );
    const UNEXTRACTED: &str =
        "43 of 48 frames carry their own HDR10+ payload, but hdr10plus_tool could not extract them";

    let (success, log) = convert(&clip, &["--hdr10plus-l1", "hybrid"]);
    assert!(!success, "an explicit hybrid must refuse:\n{log}");
    assert!(
        log.contains("--hdr10plus-l1 hybrid cannot be used here"),
        "{log}"
    );
    assert!(log.contains(UNEXTRACTED), "{log}");
    assert!(
        log.contains("Without --hdr10plus-l1 hybrid the file converts from HDR10 analysis"),
        "the refusal names the fallback that works:\n{log}"
    );
    assert!(
        !log.contains("to use the HDR10+ metadata alone"),
        "nothing was extracted, so the metadata mode cannot use it:\n{log}"
    );
    assert!(
        !clip.join(MEASUREMENTS).exists(),
        "the analyzer ran:\n{log}"
    );
    assert!(!clip.join(OUTPUT).exists(), "{log}");
    clean(&clip);

    let (success, log) = convert(&clip, &[]);
    assert!(success, "the default converts from HDR10 analysis:\n{log}");
    assert!(log.contains(UNEXTRACTED), "{log}");
    assert!(!log.contains("no dynamic metadata found"), "{log}");
    assert!(clip.join(OUTPUT).exists());
}
