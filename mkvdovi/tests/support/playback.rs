//! Helpers shared by the `mkvdovi manifest` integration tests (`playback_manifest.rs`,
//! `playback_manifest_rpu.rs`): sources, tools, the manifest run, the conversion and the per-AU
//! comparison. Each test crate uses a different subset.

#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::prelude::*;
use dovi_rpu_builder::manifest::{AuMatcher, AuVerdict, PlaybackManifest};
use dovi_rpu_builder::RpuBuilder;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const SOURCE: &str = "clip.mkv";
pub const OUTPUT: &str = "clip.DV.mkv";
pub const MANIFEST: &str = "clip.dvmanifest.json";

/// Dark before frame 20, bright from it: two shots with different L1.
pub const STEP: &str = "nullsrc=s=256x144:r=24,format=yuv420p10le,\
                    geq=lum='if(lt(N\\,20)\\,200\\,700)+mod(X+N\\,32)':cb='512':cr='512'";
/// The open-GOP source of `open_gop_start.rs`: x265 puts RASL pictures before each CRA.
pub const STEP_40: &str = "nullsrc=s=256x144:r=24,format=yuv420p10le,\
                       geq=lum='if(lt(N\\,40)\\,200\\,700)+mod(X+N\\,32)':cb='512':cr='512'";
/// Letterboxed: black bars of 16 rows at the top and 24 at the bottom, a step at frame 20.
pub const LETTERBOX: &str = "nullsrc=s=256x144:r=24,format=yuv420p10le,\
                         geq=lum='if(lt(Y\\,16)+gte(Y\\,120)\\,64\\,if(lt(N\\,20)\\,300\\,650)+mod(X+N\\,32))':cb='512':cr='512'";
/// HLG with a moving gradient and a step at frame 20.
pub const HLG: &str = "nullsrc=s=256x144:r=24,format=yuv420p10le,\
                   geq=lum='if(lt(N\\,20)\\,150\\,600)+mod(X*3+N*20\\,256)':cb='512':cr='512+64*sin(N/6)'";

pub const PQ_TAGS: &str = "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:range=limited";
pub const MASTERING: &str = "hdr10=1:\
     master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):\
     max-cll=1000,400";
pub const HLG_TAGS: &str =
    "colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc:range=limited";

#[allow(deprecated)]
pub fn mkvdovi_cmd() -> Command {
    Command::cargo_bin("mkvdovi").expect("Failed to find mkvdovi binary")
}

pub fn run(command: &mut Command) -> Output {
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

pub fn log_of(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub fn sibling_analyzer() -> Option<PathBuf> {
    #[allow(deprecated)]
    let mkvdovi = assert_cmd::cargo::cargo_bin("mkvdovi");
    mkvdovi
        .parent()
        .map(|dir| dir.join(format!("hdr_analyzer_mvp{}", std::env::consts::EXE_SUFFIX)))
        .filter(|path| path.exists())
}

/// Reason to skip, or `None` when every tool is there.
pub fn missing_prerequisite(extra_tools: &[(&str, &str)]) -> Option<String> {
    for (tool, arg) in [
        ("ffmpeg", "-version"),
        ("ffprobe", "-version"),
        ("mkvmerge", "--version"),
        ("mediainfo", "--version"),
        ("dovi_tool", "--help"),
    ]
    .iter()
    .chain(extra_tools)
    {
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
    let Some(analyzer) = sibling_analyzer() else {
        return Some(
            "hdr_analyzer_mvp is not built next to mkvdovi (cargo build -p hdr_analyzer_mvp)"
                .into(),
        );
    };
    let help = Command::new(analyzer).arg("--help").output().ok()?;
    if !String::from_utf8_lossy(&help.stdout).contains("au-table-v1") {
        return Some(
            "the hdr_analyzer_mvp next to mkvdovi does not write the AU table; rebuild it".into(),
        );
    }
    None
}

macro_rules! require_tools {
    ($($tool:expr),*) => {
        if let Some(reason) = missing_prerequisite(&[$($tool),*]) {
            eprintln!("Skipping playback manifest test: {reason}");
            return;
        }
    };
}

/// A 10-bit Annex B HEVC clip of `frames` frames from `source` with these x265 parameters.
pub fn encode(dir: &Path, name: &str, source: &str, frames: u32, x265: &str) -> PathBuf {
    let raw = dir.join(format!("{name}.hevc"));
    run(Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg(source)
        .args(["-frames:v", &frames.to_string(), "-c:v", "libx265"])
        .args(["-preset", "ultrafast", "-x265-params"])
        .arg(format!(
            "log-level=error:scenecut=0:repeat-headers=1:{x265}"
        ))
        .args(["-pix_fmt", "yuv420p10le", "-f", "hevc", "-y"])
        .arg(&raw));
    raw
}

/// `raw` muxed by mkvmerge into `<dir>/<name>`, with extra mkvmerge options before the input.
pub fn mux(dir: &Path, name: &str, raw: &Path, extra: &[&str]) -> PathBuf {
    let out = dir.join(name);
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&out)
        .args(extra)
        .arg(raw));
    out
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// `(pts, bytes)` of every video packet in decode order, as FFmpeg demuxes them (the bytes the
/// AU table describes).
pub fn packets(mkv: &Path) -> Vec<(i64, Vec<u8>)> {
    let output = run(Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_data"])
        .args(["-show_entries", "packet=pts,data", "-of", "json"])
        .arg(mkv));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    json["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|packet| {
            let pts = packet["pts"].as_i64().expect("every MKV packet has a pts");
            (pts, hexdump_bytes(packet["data"].as_str().unwrap()))
        })
        .collect()
}

/// The bytes of ffprobe's `-show_data` dump: lines of `%08x: ` and 16 bytes as hex pairs (a space
/// after every second byte), padded with spaces, then the ASCII column.
pub fn hexdump_bytes(dump: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in dump.lines().filter(|line| line.len() > 10) {
        let hex: String = line[10..line.len().min(50)]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        for pair in hex.as_bytes().chunks(2) {
            bytes.push(u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap());
        }
    }
    bytes
}

/// NAL units of an Annex B stream (start codes and trailing zero bytes removed).
pub fn nal_units(stream: &[u8]) -> Vec<&[u8]> {
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
        .map(|(n, &start)| {
            let end = starts.get(n + 1).map_or(stream.len(), |&next| next - 3);
            let mut unit = &stream[start..end];
            while let [rest @ .., 0] = unit {
                unit = rest;
            }
            unit
        })
        .filter(|unit| !unit.is_empty())
        .collect()
}

/// The RPU NAL unit (type 62, header included) of every access unit of `mkv`, in decode order.
pub fn rpu_per_access_unit(mkv: &Path) -> Vec<Vec<u8>> {
    let annexb = mkv.with_extension("hevc");
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
        ])
        .args(["-f", "hevc", "-y"])
        .arg(&annexb));
    let stream = std::fs::read(&annexb).unwrap();
    std::fs::remove_file(&annexb).unwrap();
    let mut rpus: Vec<Option<Vec<u8>>> = Vec::new();
    for unit in nal_units(&stream) {
        let kind = (unit[0] >> 1) & 0x3f;
        // A slice with first_slice_segment_in_pic_flag starts a new access unit.
        if kind < 32 && unit.len() > 2 && unit[2] & 0x80 != 0 {
            rpus.push(None);
        } else if kind == 62 {
            let slot = rpus.last_mut().expect("an RPU after a slice");
            assert!(slot.is_none(), "two RPUs in one access unit");
            *slot = Some(unit.to_vec());
        }
    }
    rpus.into_iter()
        .map(|rpu| rpu.expect("every access unit carries an RPU"))
        .collect()
}

/// What one manifest run and one conversion of the same source produced.
pub struct Outcome {
    pub manifest: PlaybackManifest,
    pub json: Value,
    pub log: String,
}

/// Writes the manifest of `source` (in `dir`) into `<dir>/work`, then checks that the run left
/// the source and its directory untouched.
pub fn write_manifest(dir: &Path, flags: &[&str]) -> Outcome {
    let source = dir.join(SOURCE);
    let before = std::fs::read(&source).unwrap();
    let listing = |dir: &Path| -> BTreeSet<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    };
    let mut files_before = listing(dir);
    files_before.remove("work");
    let work = dir.join("work");
    let output = mkvdovi_cmd()
        .current_dir(dir)
        .args(["manifest", SOURCE, "--work-dir"])
        .arg(&work)
        .args(["--hwaccel", "none"])
        .args(flags)
        .output()
        .unwrap();
    let log = log_of(&output);
    assert!(output.status.success(), "mkvdovi manifest failed:\n{log}");
    assert_eq!(
        std::fs::read(&source).unwrap(),
        before,
        "the source changed"
    );
    let mut files_after = listing(dir);
    files_after.remove("work");
    assert_eq!(
        files_after, files_before,
        "the manifest run wrote beside the source"
    );
    let path = work.join(MANIFEST);
    let text = std::fs::read_to_string(&path).unwrap();
    Outcome {
        manifest: PlaybackManifest::parse(&text).unwrap(),
        json: serde_json::from_str(&text).unwrap(),
        log,
    }
}

/// Converts `<dir>/clip.mkv` with `--keep-source` and the same flags, in its own directory so it
/// analyzes on its own, and returns the output path.
pub fn convert(dir: &Path, flags: &[&str]) -> PathBuf {
    let conversion = dir.join("conversion");
    std::fs::create_dir_all(&conversion).unwrap();
    std::fs::copy(dir.join(SOURCE), conversion.join(SOURCE)).unwrap();
    let output = mkvdovi_cmd()
        .current_dir(&conversion)
        .arg(SOURCE)
        .args(["--keep-source", "--hwaccel", "none"])
        .args(flags)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mkvdovi failed:\n{}",
        log_of(&output)
    );
    conversion.join(OUTPUT)
}

/// The player's path over `source` with `manifest`: every AU is injected, and the builder's RPU
/// for it equals the converted file's RPU of the same AU, byte for byte.
pub fn assert_rpus_equal_the_conversion(
    manifest: &PlaybackManifest,
    source: &Path,
    converted: &Path,
) {
    let source_packets = packets(source);
    // Compared by decode index: the conversion copies the bitstream in decode order, but its mux
    // takes the timestamps from the elementary stream, not from the source (ROADMAP E7.3).
    let converted_rpus = rpu_per_access_unit(converted);
    assert_eq!(converted_rpus.len(), source_packets.len());

    let builder = RpuBuilder::new(&manifest.rpu).unwrap();
    let mut matcher = AuMatcher::new(manifest).unwrap();
    let mut shots = HashMap::new();
    for (index, (pts, data)) in source_packets.iter().enumerate() {
        match matcher.next(*pts, data, false) {
            AuVerdict::Inject {
                l1,
                scene_refresh,
                shot,
                class,
            } => {
                assert_eq!(class, manifest.coverage[0].class);
                let rpu = shots
                    .entry(shot)
                    .or_insert_with(|| builder.shot(l1).unwrap())
                    .write(scene_refresh)
                    .unwrap();
                assert!(
                    rpu == converted_rpus[index],
                    "AU {index} (pts {pts}, shot {shot}): the manifest's RPU differs from the conversion's"
                );
            }
            other => panic!("AU {index} (pts {pts}): expected an injection, got {other:?}"),
        }
    }
}

pub fn distinct_l1(manifest: &PlaybackManifest) -> usize {
    manifest
        .shots
        .iter()
        .map(|shot| (shot.l1.min_pq, shot.l1.avg_pq, shot.l1.max_pq))
        .collect::<BTreeSet<_>>()
        .len()
}

/// Encodes, muxes as `clip.mkv`, writes the manifest, converts, and compares.
pub fn check_parity(dir: &Path, source: &Path, flags: &[&str]) -> Outcome {
    assert_eq!(source, dir.join(SOURCE));
    let outcome = write_manifest(dir, flags);
    let converted = convert(dir, flags);
    assert_rpus_equal_the_conversion(&outcome.manifest, source, &converted);
    outcome
}
