//! `--au-table` on real Matroska files (ROADMAP P15.1, `docs/PLAYBACK_MANIFEST.md`).
//!
//! The oracle for every timestamp and size is `mkvinfo --summary`, which reads the container
//! without FFmpeg. Fixtures are muxed with `--compression 0:none`, so stored and reconstructed
//! frame bytes are the same. Skipped (with a message) when ffmpeg with libx265 or mkvtoolnix is
//! missing; CI installs both.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::prelude::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[allow(deprecated)]
fn analyzer_cmd() -> Command {
    Command::cargo_bin("hdr_analyzer_mvp").expect("Failed to find hdr_analyzer_mvp binary")
}

fn run(command: &mut Command) -> Output {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to start {command:?}: {error}"));
    assert!(
        output.status.success(),
        "{command:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn missing_prerequisite() -> Option<String> {
    for (tool, arg) in [
        ("ffmpeg", "-version"),
        ("mkvmerge", "--version"),
        ("mkvinfo", "--version"),
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

macro_rules! require_tools {
    () => {
        if let Some(reason) = missing_prerequisite() {
            eprintln!("Skipping AU table test: {reason}");
            return;
        }
    };
}

const PATTERN: &str = "nullsrc=s=128x72:r=24,format=yuv420p10le,\
                       geq=lum='200+mod(X*3+N*7\\,400)':cb='512':cr='512'";
/// The source of `mkvdovi/tests/open_gop_start.rs`: with it x265 puts RASL pictures before each
/// CRA in display order (the plain pattern above gets none).
const STEP: &str = "nullsrc=s=256x144:r=24,format=yuv420p10le,\
                    geq=lum='if(lt(N\\,40)\\,200\\,700)+mod(X+N\\,32)':cb='512':cr='512'";

/// A 10-bit PQ-tagged Annex B HEVC clip of `frames` frames with the given x265 GOP settings.
fn encode(dir: &Path, name: &str, frames: u32, gop: &str) -> PathBuf {
    encode_source(dir, name, PATTERN, frames, gop)
}

fn encode_source(dir: &Path, name: &str, source: &str, frames: u32, gop: &str) -> PathBuf {
    let raw = dir.join(format!("{name}.hevc"));
    run(Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            source,
            "-frames:v",
            &frames.to_string(),
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-x265-params",
            &format!(
                "log-level=error:{gop}:scenecut=0:colorprim=bt2020:transfer=smpte2084:\
                 colormatrix=bt2020nc:range=limited:repeat-headers=1"
            ),
            "-pix_fmt",
            "yuv420p10le",
            "-f",
            "hevc",
            "-y",
        ])
        .arg(&raw));
    raw
}

fn mux(dir: &Path, name: &str, inputs: &[&Path], extra: &[&str]) -> PathBuf {
    let out = dir.join(format!("{name}.mkv"));
    let mut command = Command::new("mkvmerge");
    command.arg("-q").arg("-o").arg(&out).args(extra);
    for input in inputs {
        command.args(["--compression", "0:none"]).arg(input);
    }
    run(&mut command);
    out
}

struct Run {
    output: Output,
    bin: PathBuf,
}

impl Run {
    fn sidecar(&self) -> PathBuf {
        PathBuf::from(format!("{}.l1.json", self.bin.display()))
    }
    fn table_path(&self) -> PathBuf {
        PathBuf::from(format!("{}.au.json", self.bin.display()))
    }
    fn table(&self) -> Value {
        read_json(&self.table_path())
    }
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }
}

fn analyze(input: &Path, flags: &[&str]) -> Run {
    let bin = input.with_extension("bin");
    let output = analyzer_cmd()
        .arg(input)
        .args(["--no-crop", "-o"])
        .arg(&bin)
        .args(flags)
        .output()
        .unwrap();
    Run { output, bin }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `(pts in ms, size, keyframe)` per frame in file order, from `mkvinfo --summary`.
fn mkvinfo_frames(file: &Path) -> Vec<(i64, u64, bool)> {
    let output = run(Command::new("mkvinfo").arg("--summary").arg(file));
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains(" frame, track "))
        .map(|line| {
            let field = |key: &str| {
                line.split(", ")
                    .find_map(|part| part.strip_prefix(key))
                    .unwrap_or_else(|| panic!("no {key} in {line}"))
                    .to_owned()
            };
            let stamp = field("timestamp ");
            let mut parts = stamp.split([':', '.']);
            let mut next = || parts.next().unwrap().parse::<i64>().unwrap();
            let (h, m, s, ns) = (next(), next(), next(), next());
            let pts = ((h * 60 + m) * 60 + s) * 1000 + ns / 1_000_000;
            let size = field("size ").parse().unwrap();
            (pts, size, line.starts_with("I frame"))
        })
        .collect()
}

/// The table's pts in decode order.
fn table_pts(table: &Value) -> Vec<i64> {
    let mut pts = vec![table["aus"]["pts_first"].as_i64().unwrap()];
    for delta in table["aus"]["pts_delta"].as_array().unwrap() {
        pts.push(pts.last().unwrap() + delta.as_i64().unwrap());
    }
    pts
}

/// Every field the oracle can check: pts, sizes, keyframes, time base, identity, sidecar digest.
fn assert_table_matches_file(run: &Run, file: &Path) {
    assert!(run.output.status.success(), "{}", run.stderr());
    let table = run.table();
    let frames = mkvinfo_frames(file);
    assert_eq!(table["version"], 1);
    assert_eq!(table["track"]["time_base"], serde_json::json!([1, 1000]));
    assert_eq!(
        table["aus"]["count"].as_u64().unwrap() as usize,
        frames.len()
    );
    let pts: Vec<i64> = frames.iter().map(|frame| frame.0).collect();
    assert_eq!(table_pts(&table), pts);
    let sizes: Vec<u64> = frames.iter().map(|frame| frame.1).collect();
    let table_sizes: Vec<u64> = table["aus"]["size"]
        .as_array()
        .unwrap()
        .iter()
        .map(|size| size.as_u64().unwrap())
        .collect();
    assert_eq!(table_sizes, sizes);
    let keyframes: Vec<u64> = (0..frames.len() as u64)
        .filter(|&index| frames[index as usize].2)
        .collect();
    let iraps: Vec<u64> = table["aus"]["irap_crc32"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry[0].as_u64().unwrap())
        .collect();
    assert_eq!(iraps, keyframes);

    let bytes = std::fs::read(file).unwrap();
    let chunk = bytes.len().min(1 << 20);
    assert_eq!(table["source"]["size_bytes"], bytes.len() as u64);
    assert_eq!(table["source"]["head_sha256"], sha256_hex(&bytes[..chunk]));
    assert_eq!(
        table["source"]["tail_sha256"],
        sha256_hex(&bytes[bytes.len() - chunk..])
    );
    let sidecar = std::fs::read(run.sidecar()).unwrap();
    assert_eq!(table["sidecar_sha256"], sha256_hex(&sidecar));
    assert_eq!(
        table["leading_skipped"],
        read_json(&run.sidecar())["source"]["leading_skipped_frames"]
    );
}

#[test]
fn cfr_with_b_frames_matches_the_container() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "cfr",
        48,
        "keyint=24:min-keyint=24:bframes=3:b-adapt=0",
    );
    let file = mux(dir.path(), "cfr", &[&raw], &[]);
    let run = analyze(&file, &["--au-table"]);
    assert_table_matches_file(&run, &file);
    // B-frames: decode order is not pts order.
    let pts = table_pts(&run.table());
    assert!(pts.windows(2).any(|pair| pair[1] < pair[0]), "{pts:?}");
}

#[test]
fn variable_frame_rate_matches_the_container() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "vfr",
        48,
        "keyint=24:min-keyint=24:bframes=2:b-adapt=0",
    );
    // 24 frames at 24 fps, then a 500 ms gap, then 24 frames at 30 fps (timestamps v2).
    let mut stamps = String::from("# timestamp format v2\n");
    for frame in 0..48u32 {
        let ms = if frame < 24 {
            f64::from(frame) * 1000.0 / 24.0
        } else {
            1500.0 + f64::from(frame - 24) * 1000.0 / 30.0
        };
        stamps.push_str(&format!("{ms:.3}\n"));
    }
    let stamps_path = dir.path().join("v2.txt");
    std::fs::write(&stamps_path, stamps).unwrap();
    let timestamps = format!("0:{}", stamps_path.display());
    let file = mux(dir.path(), "vfr", &[&raw], &["--timestamps", &timestamps]);
    let run = analyze(&file, &["--au-table"]);
    assert_table_matches_file(&run, &file);
    let mut pts = table_pts(&run.table());
    pts.sort_unstable();
    let steps: std::collections::BTreeSet<i64> =
        pts.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert!(steps.contains(&42) && steps.contains(&33), "{steps:?}");
}

#[test]
fn non_zero_start_keeps_absolute_timestamps() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "late",
        24,
        "keyint=12:min-keyint=12:bframes=2:b-adapt=0",
    );
    let file = mux(dir.path(), "late", &[&raw], &["--sync", "0:2000"]);
    let run = analyze(&file, &["--au-table"]);
    assert_table_matches_file(&run, &file);
    assert_eq!(table_pts(&run.table()).into_iter().min(), Some(2000));
}

#[test]
fn open_gop_cut_counts_its_leading_pictures() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode_source(
        dir.path(),
        "open",
        STEP,
        96,
        "keyint=24:min-keyint=24:open-gop=1:bframes=3:b-adapt=0",
    );
    let full = mux(dir.path(), "full", &[&raw], &[]);
    let file = dir.path().join("cut.mkv");
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&file)
        .args(["--split", "parts:00:00:01-00:00:10"])
        .arg(&full));
    let run = analyze(&file, &["--au-table"]);
    assert_table_matches_file(&run, &file);
    assert!(run.table()["leading_skipped"].as_u64().unwrap() > 0);
}

#[test]
fn two_video_tracks_are_refused_before_analysis() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(dir.path(), "one", 4, "keyint=1");
    let file = mux(dir.path(), "two", &[&raw, &raw], &[]);
    let run = analyze(&file, &["--au-table"]);
    assert!(!run.output.status.success());
    assert!(
        run.stderr().contains("exactly one video track"),
        "{}",
        run.stderr()
    );
    assert!(!run.bin.exists(), "no analysis may run");
}

#[test]
fn table_only_writes_the_same_table_and_no_measurements() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "only",
        24,
        "keyint=12:min-keyint=12:bframes=2:b-adapt=0",
    );
    let file = mux(dir.path(), "only", &[&raw], &[]);
    let full = analyze(&file, &["--au-table"]);
    assert_table_matches_file(&full, &file);
    let full_table = full.table();
    std::fs::remove_file(&full.bin).unwrap();
    std::fs::remove_file(full.sidecar()).unwrap();

    let only = analyze(&file, &["--au-table-only"]);
    assert!(only.output.status.success(), "{}", only.stderr());
    assert!(!only.bin.exists() && !only.sidecar().exists());
    let table = only.table();
    assert!(table["sidecar_sha256"].is_null());
    assert_eq!(table["aus"], full_table["aus"]);
    assert_eq!(table["source"], full_table["source"]);
    assert_eq!(table["track"], full_table["track"]);
}

#[test]
fn a_table_from_an_earlier_run_is_removed() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(dir.path(), "stale", 4, "keyint=1");
    let file = mux(dir.path(), "stale", &[&raw], &[]);
    let stale = file.with_extension("bin.au.json");
    std::fs::write(&stale, "{\"version\":1}").unwrap();
    let run = analyze(&file, &[]);
    assert!(run.output.status.success(), "{}", run.stderr());
    assert_eq!(run.table_path(), stale);
    assert!(!stale.exists());
}

// --- A hand-built Matroska file, for the one layout mkvmerge never writes: laced video. ---

fn vint_size(size: usize) -> Vec<u8> {
    let mut bytes = vec![0x01];
    bytes.extend_from_slice(&(size as u64).to_be_bytes()[1..]);
    bytes
}

fn element(id: &[u8], body: &[u8]) -> Vec<u8> {
    let mut bytes = id.to_vec();
    bytes.extend(vint_size(body.len()));
    bytes.extend_from_slice(body);
    bytes
}

fn uint(id: &[u8], value: u64) -> Vec<u8> {
    element(id, &value.to_be_bytes())
}

/// Splits an Annex B stream with in-band parameter sets on every VPS into length-prefixed AUs
/// (the clip is intra-only, so each AU starts with its VPS).
fn annexb_access_units(stream: &[u8]) -> Vec<Vec<u8>> {
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
    let mut units = Vec::new();
    for (position, &start) in starts.iter().enumerate() {
        let mut end = starts
            .get(position + 1)
            .map_or(stream.len(), |next| next - 3);
        while end > start && stream[end - 1] == 0 {
            end -= 1;
        }
        units.push(&stream[start..end]);
    }
    let mut aus: Vec<Vec<u8>> = Vec::new();
    for nal in units {
        if (nal[0] >> 1) & 0x3F == 32 || aus.is_empty() {
            aus.push(Vec::new());
        }
        let au = aus.last_mut().unwrap();
        au.extend((nal.len() as u32).to_be_bytes());
        au.extend_from_slice(nal);
    }
    aus
}

/// `laced`: the first two AUs share one Xiph-laced SimpleBlock.
fn handmade_mkv(aus: &[Vec<u8>], laced: bool) -> Vec<u8> {
    let ebml = element(
        &[0x1A, 0x45, 0xDF, 0xA3],
        &[
            uint(&[0x42, 0x86], 1),
            uint(&[0x42, 0xF7], 1),
            uint(&[0x42, 0xF2], 4),
            uint(&[0x42, 0xF3], 8),
            element(&[0x42, 0x82], b"matroska"),
            uint(&[0x42, 0x87], 4),
            uint(&[0x42, 0x85], 2),
        ]
        .concat(),
    );
    let info = element(
        &[0x15, 0x49, 0xA9, 0x66],
        &[
            uint(&[0x2A, 0xD7, 0xB1], 1_000_000),
            element(&[0x4D, 0x80], b"test"),
            element(&[0x57, 0x41], b"test"),
        ]
        .concat(),
    );
    // hvcC with no parameter-set arrays (they are in-band), 4-byte NAL lengths.
    let hvcc = [
        1, 0x02, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 0x5D, 0xF0, 0, 0xFC, 0xFD, 0xFA, 0xFA, 0, 0,
        0x0F, 0,
    ];
    let track = element(
        &[0xAE],
        &[
            uint(&[0xD7], 1),
            uint(&[0x73, 0xC5], 1),
            uint(&[0x83], 1),
            element(&[0x86], b"V_MPEGH/ISO/HEVC"),
            element(&[0x63, 0xA2], &hvcc),
            uint(&[0x23, 0xE3, 0x83], 41_666_667),
            element(&[0xE0], &[uint(&[0xB0], 128), uint(&[0xBA], 72)].concat()),
        ]
        .concat(),
    );
    let tracks = element(&[0x16, 0x54, 0xAE, 0x6B], &track);
    let block = |frames: &[&Vec<u8>], timecode: i16| {
        let mut body = vec![0x81];
        body.extend(timecode.to_be_bytes());
        if frames.len() == 1 {
            body.push(0x80);
        } else {
            body.push(0x80 | 0x02);
            body.push((frames.len() - 1) as u8);
            for frame in &frames[..frames.len() - 1] {
                let mut size = frame.len();
                while size >= 255 {
                    body.push(255);
                    size -= 255;
                }
                body.push(size as u8);
            }
        }
        for frame in frames {
            body.extend_from_slice(frame);
        }
        element(&[0xA3], &body)
    };
    let mut cluster = uint(&[0xE7], 0);
    let mut index = 0;
    while index < aus.len() {
        let count = if laced && index == 0 { 2 } else { 1 };
        let frames: Vec<&Vec<u8>> = aus[index..index + count].iter().collect();
        cluster.extend(block(&frames, (index * 42) as i16));
        index += count;
    }
    let segment = element(
        &[0x18, 0x53, 0x80, 0x67],
        &[info, tracks, element(&[0x1F, 0x43, 0xB6, 0x75], &cluster)].concat(),
    );
    [ebml, segment].concat()
}

#[test]
fn laced_video_is_refused_but_measured() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(dir.path(), "intra", 6, "keyint=1");
    let aus = annexb_access_units(&std::fs::read(&raw).unwrap());
    assert_eq!(aus.len(), 6);

    // The control: the same writer without lacing gives a valid table.
    let plain = dir.path().join("plain.mkv");
    std::fs::write(&plain, handmade_mkv(&aus, false)).unwrap();
    let run_plain = analyze(&plain, &["--au-table"]);
    assert_table_matches_file(&run_plain, &plain);

    let laced = dir.path().join("laced.mkv");
    std::fs::write(&laced, handmade_mkv(&aus, true)).unwrap();
    let run_laced = analyze(&laced, &["--au-table"]);
    assert!(!run_laced.output.status.success());
    assert!(
        run_laced.stderr().contains("laced video"),
        "{}",
        run_laced.stderr()
    );
    assert!(run_laced.bin.exists() && run_laced.sidecar().exists());
    assert!(!run_laced.table_path().exists());
}
