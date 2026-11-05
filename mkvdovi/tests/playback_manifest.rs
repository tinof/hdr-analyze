//! `mkvdovi manifest` (ROADMAP P15.3, docs/PLAYBACK_MANIFEST.md) end to end. For each source the
//! manifest is written into a separate work directory, the same source is then converted with
//! `--keep-source`, and the player's path is replayed: the source's packets go through
//! `AuMatcher` in decode order, and the RPU the builder writes for each AU must equal, byte for
//! byte, the RPU NAL that AU carries in the converted file. The two runs analyze independently.
//!
//! Skipped (with a message) when ffmpeg with libx265, ffprobe, mkvmerge, mediainfo or dovi_tool
//! is missing, or when no `hdr_analyzer_mvp` that writes the AU table (`au-table-v1`) is built
//! next to the test's mkvdovi binary (`cargo build -p hdr_analyzer_mvp`). The HDR10+ refusal also
//! needs hdr10plus_tool.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::prelude::*;
use dovi_rpu_builder::manifest::{AuMatcher, AuVerdict, PlaybackManifest, QualityClass};
use dovi_rpu_builder::{HlgComposer, Profile, RpuBuilder};
use serde_json::Value;
use sha2::{Digest, Sha256};

const SOURCE: &str = "clip.mkv";
const OUTPUT: &str = "clip.DV.mkv";
const MANIFEST: &str = "clip.dvmanifest.json";

/// Dark before frame 20, bright from it: two shots with different L1.
const STEP: &str = "nullsrc=s=256x144:r=24,format=yuv420p10le,\
                    geq=lum='if(lt(N\\,20)\\,200\\,700)+mod(X+N\\,32)':cb='512':cr='512'";
/// The open-GOP source of `open_gop_start.rs`: x265 puts RASL pictures before each CRA.
const STEP_40: &str = "nullsrc=s=256x144:r=24,format=yuv420p10le,\
                       geq=lum='if(lt(N\\,40)\\,200\\,700)+mod(X+N\\,32)':cb='512':cr='512'";
/// Letterboxed: black bars of 16 rows at the top and 24 at the bottom, a step at frame 20.
const LETTERBOX: &str = "nullsrc=s=256x144:r=24,format=yuv420p10le,\
                         geq=lum='if(lt(Y\\,16)+gte(Y\\,120)\\,64\\,if(lt(N\\,20)\\,300\\,650)+mod(X+N\\,32))':cb='512':cr='512'";
/// HLG with a moving gradient and a step at frame 20.
const HLG: &str = "nullsrc=s=256x144:r=24,format=yuv420p10le,\
                   geq=lum='if(lt(N\\,20)\\,150\\,600)+mod(X*3+N*20\\,256)':cb='512':cr='512+64*sin(N/6)'";

const PQ_TAGS: &str = "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:range=limited";
const MASTERING: &str = "hdr10=1:\
     master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):\
     max-cll=1000,400";
const HLG_TAGS: &str = "colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc:range=limited";

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

fn log_of(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
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
fn missing_prerequisite(extra_tools: &[(&str, &str)]) -> Option<String> {
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
fn encode(dir: &Path, name: &str, source: &str, frames: u32, x265: &str) -> PathBuf {
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
fn mux(dir: &Path, name: &str, raw: &Path, extra: &[&str]) -> PathBuf {
    let out = dir.join(name);
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&out)
        .args(extra)
        .arg(raw));
    out
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// `(pts, bytes)` of every video packet in decode order, as FFmpeg demuxes them (the bytes the
/// AU table describes).
fn packets(mkv: &Path) -> Vec<(i64, Vec<u8>)> {
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
fn hexdump_bytes(dump: &str) -> Vec<u8> {
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
fn rpu_per_access_unit(mkv: &Path) -> Vec<Vec<u8>> {
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
struct Outcome {
    manifest: PlaybackManifest,
    json: Value,
    log: String,
}

/// Writes the manifest of `source` (in `dir`) into `<dir>/work`, then checks that the run left
/// the source and its directory untouched.
fn write_manifest(dir: &Path, flags: &[&str]) -> Outcome {
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
fn convert(dir: &Path, flags: &[&str]) -> PathBuf {
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
fn assert_rpus_equal_the_conversion(manifest: &PlaybackManifest, source: &Path, converted: &Path) {
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

fn distinct_l1(manifest: &PlaybackManifest) -> usize {
    manifest
        .shots
        .iter()
        .map(|shot| (shot.l1.min_pq, shot.l1.avg_pq, shot.l1.max_pq))
        .collect::<BTreeSet<_>>()
        .len()
}

/// Encodes, muxes as `clip.mkv`, writes the manifest, converts, and compares.
fn check_parity(dir: &Path, source: &Path, flags: &[&str]) -> Outcome {
    assert_eq!(source, dir.join(SOURCE));
    let outcome = write_manifest(dir, flags);
    let converted = convert(dir, flags);
    assert_rpus_equal_the_conversion(&outcome.manifest, source, &converted);
    outcome
}

#[test]
fn pq_with_b_frames_matches_the_conversion_and_rejects_another_encode() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "pq",
        STEP,
        48,
        &format!("keyint=24:min-keyint=24:bframes=3:b-adapt=0:{PQ_TAGS}:{MASTERING}"),
    );
    let source = mux(dir.path(), SOURCE, &raw, &[]);
    let outcome = check_parity(dir.path(), &source, &[]);
    let manifest = &outcome.manifest;
    assert!(matches!(manifest.rpu.profile, Profile::P81 { .. }));
    assert_eq!(manifest.luminance_mapping().unwrap(), "pq");
    assert!(distinct_l1(manifest) >= 2, "{:?}", manifest.shots);
    assert_eq!(manifest.coverage[0].class, QualityClass::Full);
    assert_eq!(outcome.json["l1_policy"], "measured");
    // Decode order differs from presentation order.
    assert!(manifest.aus.pts.windows(2).any(|pair| pair[1] < pair[0]));

    // The candidate step accepts the file the manifest was made for, and nothing else.
    let bytes = std::fs::read(&source).unwrap();
    let chunk = |range: std::ops::Range<usize>| sha256(&bytes[range]);
    let size = bytes.len();
    let head = chunk(0..size.min(1 << 20));
    let tail = chunk(size.saturating_sub(1 << 20)..size);
    manifest.check_source(size as u64, &head, &tail).unwrap();

    // Another encode of the same picture and timing: the candidate step refuses it, and a player
    // that reads it anyway is rejected at the first IRAP.
    let other_raw = encode(
        dir.path(),
        "other",
        STEP,
        48,
        &format!("keyint=24:min-keyint=24:bframes=3:b-adapt=0:crf=12:{PQ_TAGS}:{MASTERING}"),
    );
    let other = mux(dir.path(), "other.mkv", &other_raw, &[]);
    let other_bytes = std::fs::read(&other).unwrap();
    assert!(manifest
        .check_source(
            other_bytes.len() as u64,
            &sha256(&other_bytes),
            &sha256(&other_bytes)
        )
        .is_err());
    let mut matcher = AuMatcher::new(manifest).unwrap();
    let verdicts: Vec<AuVerdict> = packets(&other)
        .iter()
        .map(|(pts, data)| matcher.next(*pts, data, false))
        .collect();
    assert!(
        matches!(verdicts[0], AuVerdict::Rejected(_)),
        "{:?}",
        verdicts[0]
    );
    assert!(!verdicts
        .iter()
        .any(|verdict| matches!(verdict, AuVerdict::Inject { .. })));
}

#[test]
fn hlg_bt2100_matches_the_conversion_and_a_preset_pair_is_not_reused() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "hlg",
        HLG,
        48,
        &format!("keyint=24:min-keyint=24:bframes=2:b-adapt=0:{HLG_TAGS}"),
    );
    let source = mux(dir.path(), SOURCE, &raw, &[]);

    // A pair measured through the preset first: the bt2100 run must not reuse it.
    let preset = write_manifest(dir.path(), &["--hlg-composer", "preset"]);
    assert_eq!(
        preset.manifest.rpu.profile,
        Profile::P84 {
            composer: HlgComposer::Preset
        }
    );

    let outcome = check_parity(dir.path(), &source, &[]);
    let manifest = &outcome.manifest;
    assert!(
        outcome.log.contains("cannot be reused") && outcome.log.contains("re-analyzing"),
        "{}",
        outcome.log
    );
    assert_eq!(
        manifest.rpu.profile,
        Profile::P84 {
            composer: HlgComposer::Bt2100
        }
    );
    assert_eq!(outcome.json["rpu"]["composer"], "bt2100");
    assert_eq!(
        manifest.luminance_mapping().unwrap(),
        dovi84_composer::Composer::Bt2100V1.luminance_mapping()
    );
    assert!(distinct_l1(manifest) >= 2, "{:?}", manifest.shots);
    // A player that writes another composer refuses the manifest.
    manifest.check_composer(HlgComposer::Bt2100).unwrap();
    assert!(manifest.check_composer(HlgComposer::Preset).is_err());
}

#[test]
fn variable_frame_rate_matches_the_conversion() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "vfr",
        STEP,
        48,
        &format!("keyint=24:min-keyint=24:bframes=2:b-adapt=0:{PQ_TAGS}:{MASTERING}"),
    );
    // 24 frames at 24 fps, a 500 ms gap, then 24 frames at 30 fps (timestamps v2).
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
    let source = mux(dir.path(), SOURCE, &raw, &["--timestamps", &timestamps]);
    let outcome = check_parity(dir.path(), &source, &[]);
    let mut pts = outcome.manifest.aus.pts.clone();
    pts.sort_unstable();
    let steps: BTreeSet<i64> = pts.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert!(steps.contains(&42) && steps.contains(&33), "{steps:?}");
}

#[test]
fn non_zero_start_matches_the_conversion() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "late",
        STEP,
        48,
        &format!("keyint=12:min-keyint=12:bframes=2:b-adapt=0:{PQ_TAGS}:{MASTERING}"),
    );
    let source = mux(dir.path(), SOURCE, &raw, &["--sync", "0:2000"]);
    let outcome = check_parity(dir.path(), &source, &[]);
    assert_eq!(outcome.manifest.aus.pts.iter().min(), Some(&2000));
}

#[test]
fn open_gop_cut_matches_the_conversion() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "open",
        STEP_40,
        96,
        &format!("keyint=24:min-keyint=24:open-gop=1:bframes=3:b-adapt=0:{PQ_TAGS}:{MASTERING}"),
    );
    let full = mux(dir.path(), "full.mkv", &raw, &[]);
    let source = dir.path().join(SOURCE);
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&source)
        .args(["--split", "parts:00:00:01-00:00:10"])
        .arg(&full));
    std::fs::remove_file(&full).unwrap();
    let outcome = check_parity(dir.path(), &source, &[]);
    let manifest = &outcome.manifest;
    assert_eq!(manifest.leading_skipped, 3);
    // Shot 0 absorbs the leading pictures, so the step is 3 pictures later than measured.
    assert_eq!(manifest.shots.len(), 2, "{:?}", manifest.shots);
    assert_eq!(manifest.shots[1].start, 40 - 24 + 3);
}

#[test]
fn cm_v29_trims_l5_and_measured_light_levels_match_the_conversion() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    // No MaxCLL / MaxFALL in the stream: the configuration fills them from the measurements.
    let raw = encode(
        dir.path(),
        "box",
        LETTERBOX,
        48,
        &format!(
            "keyint=24:min-keyint=24:bframes=2:b-adapt=0:{PQ_TAGS}:\
             master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)"
        ),
    );
    let source = mux(dir.path(), SOURCE, &raw, &[]);
    for flags in [
        &["--cm-version", "v29"][..],
        &[
            "--trim-targets",
            "100,1000",
            "--content-type",
            "game",
            "--source-primaries",
            "1",
            "--reference-mode",
            "true",
        ],
    ] {
        let outcome = check_parity(dir.path(), &source, flags);
        let manifest = &outcome.manifest;
        let level5 = manifest.rpu.level5.expect("the letterbox gives L5");
        assert!(level5.top > 0 && level5.top != level5.bottom, "{level5:?}");
        assert_ne!(manifest.rpu.level6.max_content_light_level, 1000);
        let cm = &outcome.json["rpu"]["cm"];
        if flags[0] == "--cm-version" {
            assert_eq!(cm["version"], "V29");
        } else {
            assert_eq!(cm["trim_targets_nits"], serde_json::json!([100, 1000]));
            assert_eq!(cm["content_type"], 2);
            assert_eq!(cm["source_primary_index"], 1);
            assert_eq!(cm["reference_mode"], true);
        }
        std::fs::remove_dir_all(dir.path().join("conversion")).unwrap();
    }
}

#[test]
fn a_tampered_pair_is_analyzed_again() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "pq",
        STEP,
        24,
        &format!("keyint=12:min-keyint=12:bframes=2:b-adapt=0:{PQ_TAGS}:{MASTERING}"),
    );
    mux(dir.path(), SOURCE, &raw, &[]);
    let first = write_manifest(dir.path(), &[]);
    let work = dir.path().join("work");
    let sidecar = work.join("clip_measurements.bin.l1.json");
    let table = work.join("clip_measurements.bin.au.json");

    let again = write_manifest(dir.path(), &[]);
    assert!(
        again.log.contains("Reusing the measurements"),
        "{}",
        again.log
    );
    assert_eq!(again.json, first.json);

    // Valid JSON, other bytes: the table's digest no longer names it.
    let mut bytes = std::fs::read(&sidecar).unwrap();
    bytes.push(b'\n');
    std::fs::write(&sidecar, bytes).unwrap();
    let after_sidecar = write_manifest(dir.path(), &[]);
    assert!(
        after_sidecar.log.contains("is bound to sidecar")
            && after_sidecar.log.contains("re-analyzing"),
        "{}",
        after_sidecar.log
    );
    assert_eq!(after_sidecar.json["shots"], first.json["shots"]);

    // A table whose arrays do not fit its count, with the digest still right.
    let mut value = read_json(&table);
    value["aus"]["size"].as_array_mut().unwrap().pop();
    std::fs::write(&table, serde_json::to_vec(&value).unwrap()).unwrap();
    let after_table = write_manifest(dir.path(), &[]);
    assert!(
        after_table.log.contains("do not fit") && after_table.log.contains("re-analyzing"),
        "{}",
        after_table.log
    );
    assert_eq!(after_table.json, first.json);
}

/// Runs `mkvdovi manifest` and expects a refusal that names `expected` and leaves no work dir.
fn assert_refused(dir: &Path, input: &str, path_env: Option<&Path>, expected: &str) {
    let work = dir.join("work");
    let mut command = mkvdovi_cmd();
    command
        .current_dir(dir)
        .args(["manifest", input, "--work-dir"])
        .arg(&work)
        .args(["--hwaccel", "none"]);
    if let Some(path) = path_env {
        command.env("PATH", path);
    }
    let output = command.output().unwrap();
    let log = log_of(&output);
    assert!(!output.status.success(), "{input} must be refused:\n{log}");
    assert!(log.contains(expected), "{log}");
    assert!(
        !work.exists(),
        "a refused input must not reach the analysis"
    );
}

#[test]
fn inputs_outside_version_1_are_refused_before_analysis() {
    require_tools!(("hdr10plus_tool", "--version"));
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "pq",
        STEP,
        24,
        &format!("keyint=12:min-keyint=12:bframes=0:{PQ_TAGS}:{MASTERING}"),
    );

    // Not Matroska: the elementary stream and an MP4.
    assert_refused(dir.path(), "pq.hevc", None, "Matroska input only");
    let mp4 = dir.path().join("pq.mp4");
    run(Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&raw)
        .args(["-c", "copy", "-y"])
        .arg(&mp4));
    assert_refused(dir.path(), "pq.mp4", None, "Matroska input only");

    // HDR10+: one frame of dynamic metadata is enough for the detection.
    let metadata = dir.path().join("hdr10plus.json");
    let frames: Vec<Value> = (0..24)
        .map(|frame| {
            serde_json::json!({
                "BezierCurveData": {"Anchors": [102, 205, 307, 410, 512, 614, 717, 819, 922],
                                    "KneePointX": 0, "KneePointY": 0},
                "LuminanceParameters": {
                    "AverageRGB": 300,
                    "LuminanceDistributions": {
                        "DistributionIndex": [1, 5, 10, 25, 50, 75, 90, 95, 99],
                        "DistributionValues": [0, 10, 100, 200, 300, 400, 500, 600, 7000],
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
    std::fs::write(
        &metadata,
        serde_json::to_vec(&serde_json::json!({
            "JSONInfo": {"HDR10plusProfile": "B", "Version": "1.0"},
            "SceneInfo": frames,
            "SceneInfoSummary": {"SceneFirstFrameIndex": [0], "SceneFrameNumbers": [24]},
            "ToolInfo": {"Tool": "hdr10plus_tool", "Version": "1.7.2"},
        }))
        .unwrap(),
    )
    .unwrap();
    let with_sei = dir.path().join("hdr10plus.hevc");
    run(Command::new("hdr10plus_tool")
        .arg("inject")
        .arg("-i")
        .arg(&raw)
        .arg("-j")
        .arg(&metadata)
        .arg("-o")
        .arg(&with_sei));
    mux(dir.path(), "hdr10plus.mkv", &with_sei, &[]);
    assert_refused(dir.path(), "hdr10plus.mkv", None, "this file is HDR10+");

    // Without dovi_tool the Dolby Vision detection cannot run, so nothing is admitted.
    #[cfg(unix)]
    refused_without_dovi_tool(dir.path(), &raw);
}

#[cfg(unix)]
fn refused_without_dovi_tool(dir: &Path, raw: &Path) {
    let bin = dir.join("bin");
    std::fs::create_dir(&bin).unwrap();
    // `which` too: mkvdovi looks tools up with it.
    for tool in ["ffmpeg", "ffprobe", "mkvmerge", "mediainfo", "which"] {
        let found = Command::new("sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .unwrap();
        let target = String::from_utf8_lossy(&found.stdout).trim().to_owned();
        std::os::unix::fs::symlink(target, bin.join(tool)).unwrap();
    }
    mux(dir, SOURCE, raw, &[]);
    assert_refused(dir, SOURCE, Some(&bin), "needs 'dovi_tool'");
}
