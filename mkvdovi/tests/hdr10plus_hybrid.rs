//! `--hdr10plus-l1 hybrid` end to end: an HDR10+ clip whose HDR10+ scenes split the analyzer's
//! scenes. Every delivered RPU frame must carry the L1 of its union piece: the HDR10+ peak of
//! the HDR10+ scene, and the average and minimum of the measured frames of the piece, after the
//! generator's clamps. With `--peak-source max-scl` (MaxSCL is 0 here, as on HDR10+ Profile A
//! discs) every piece takes the peak of its analyzer scene instead. Without the flag nothing is
//! measured.
//!
//! Skipped (with a message) when ffmpeg with libx265, ffprobe, mkvmerge, dovi_tool or
//! hdr10plus_tool is missing, or when no `hdr_analyzer_mvp` is built next to the test's mkvdovi
//! binary (`cargo build -p hdr_analyzer_mvp`). MediaInfo classifying the clip as anything other
//! than HDR10+ is a failure, not a skip.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::prelude::*;
use dolby_vision::rpu::dovi_rpu::DoviRpu;
use dolby_vision::rpu::extension_metadata::blocks::{ExtMetadataBlock, ExtMetadataBlockLevel1};
use dolby_vision::rpu::vdr_dm_data::CmVersion;
use dolby_vision::utils::nits_to_pq_12_bit;
use serde_json::{json, Value};

const SOURCE: &str = "clip.mkv";
const OUTPUT: &str = "clip.DV.mkv";
const FRAMES: u64 = 48;
/// Frame where the picture turns from dark to bright (an analyzer cut).
const STEP_FRAME: u64 = 12;
/// HDR10+ scene starts (6 and 36 fall inside analyzer scenes) and their peaks in nits.
const HDR10PLUS_SCENES: [(u64, u64); 3] = [(0, 1500), (6, 2000), (36, 4000)];

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

/// HDR10+ metadata in `hdr10plus_tool`'s JSON layout: the histogram maximum of each scene is its
/// peak, and MaxSCL is 0 on every frame.
fn hdr10plus_json() -> Value {
    let scene_of = |frame: u64| {
        HDR10PLUS_SCENES
            .iter()
            .rposition(|&(start, _)| start <= frame)
            .unwrap()
    };
    let frames: Vec<Value> = (0..FRAMES)
        .map(|frame| {
            let scene = scene_of(frame);
            let (start, peak_nits) = HDR10PLUS_SCENES[scene];
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
    let starts: Vec<u64> = HDR10PLUS_SCENES.iter().map(|&(start, _)| start).collect();
    let lengths: Vec<u64> = starts
        .iter()
        .zip(starts.iter().skip(1).copied().chain([FRAMES]))
        .map(|(start, end)| end - start)
        .collect();
    json!({
        "JSONInfo": {"HDR10plusProfile": "B", "Version": "1.0"},
        "SceneInfo": frames,
        "SceneInfoSummary": {"SceneFirstFrameIndex": starts, "SceneFrameNumbers": lengths},
        "ToolInfo": {"Tool": "hdr10plus_tool", "Version": "1.7.2"},
    })
}

/// A 10-bit PQ clip, dark before `STEP_FRAME` and bright from it, with the HDR10+ metadata of
/// [`hdr10plus_json`] in its SEI. No B-frames, so decode order is display order.
fn synthesize_hdr10plus_clip(dir: &Path) -> PathBuf {
    let plain = dir.join("plain.hevc");
    run(Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            &format!(
                "nullsrc=s=256x144:r=24,format=yuv420p10le,\
                 geq=lum='if(lt(N\\,{STEP_FRAME})\\,200\\,700)+mod(X+N\\,32)':cb='512':cr='512'"
            ),
            "-frames:v",
            &FRAMES.to_string(),
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-x265-params",
            "log-level=error:keyint=24:min-keyint=24:bframes=0:scenecut=0:\
             colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:range=limited:hdr10=1:\
             master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):\
             max-cll=1000,400:repeat-headers=1",
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
    let clip = dir.join(SOURCE);
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&clip)
        .arg(&with_sei));
    for path in [plain, metadata, with_sei] {
        std::fs::remove_file(path).unwrap();
    }
    clip
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

/// L1 `(min, max, avg)` of the RPU in each access unit, in decode order.
fn l1_per_access_unit(mkv: &Path) -> Vec<(u16, u16, u16)> {
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
    let mut l1: Vec<Option<(u16, u16, u16)>> = Vec::new();
    for unit in nal_units(&stream) {
        let kind = (unit[0] >> 1) & 0x3f;
        // A slice with first_slice_segment_in_pic_flag starts a new access unit.
        if kind < 32 && unit.len() > 2 && unit[2] & 0x80 != 0 {
            l1.push(None);
        } else if kind == 62 {
            let rpu = DoviRpu::parse_unspec62_nalu(unit).expect("parse RPU NAL");
            let block = match rpu.vdr_dm_data.as_ref().and_then(|dm| dm.get_block(1)) {
                Some(ExtMetadataBlock::Level1(block)) => (block.min_pq, block.max_pq, block.avg_pq),
                _ => panic!("RPU without L1"),
            };
            *l1.last_mut().expect("RPU after a slice") = Some(block);
        }
    }
    l1.into_iter()
        .map(|block| block.expect("every access unit carries an RPU"))
        .collect()
}

fn convert(dir: &Path, flags: &[&str]) -> String {
    let conversion = mkvdovi_cmd()
        .current_dir(dir)
        .arg(SOURCE)
        .args([
            "--keep-source",
            "--verify",
            "--hwaccel",
            "none",
            "--no-resume",
        ])
        .args(flags)
        .output()
        .unwrap();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&conversion.stdout),
        String::from_utf8_lossy(&conversion.stderr)
    );
    assert!(conversion.status.success(), "mkvdovi failed:\n{log}");
    assert!(
        log.contains("HDR10+"),
        "MediaInfo must classify the clip as HDR10+:\n{log}"
    );
    log
}

/// Mean of 12-bit codes rounded half away from zero (the pre-registered piece average).
fn mean_code(codes: &[u16]) -> u16 {
    let sum: f64 = codes.iter().map(|&code| f64::from(code)).sum();
    (sum / codes.len() as f64).round() as u16
}

/// The L1 every frame must carry: the union pieces of the analyzer's scenes in `sidecar` and
/// the HDR10+ scenes, with `peak(piece_start, analyzer_scene_max)` as the piece maximum, after
/// the generator's clamps.
fn expected_l1(sidecar: &Value, peak: impl Fn(u64, u16) -> u16) -> Vec<(u16, u16, u16)> {
    let scenes = sidecar["scenes"].as_array().unwrap();
    let field = |name: &str| -> Vec<u16> {
        sidecar["frames"][name]
            .as_array()
            .unwrap()
            .iter()
            .map(|code| code.as_u64().unwrap() as u16)
            .collect()
    };
    let (avg, min) = (field("avg_max_rgb_pq_12bit"), field("min_pq_12bit"));
    let mut cuts: Vec<u64> = scenes
        .iter()
        .map(|s| s["start"].as_u64().unwrap())
        .collect();
    cuts.extend(HDR10PLUS_SCENES.iter().map(|&(start, _)| start));
    cuts.sort_unstable();
    cuts.dedup();
    let mut frames = Vec::new();
    for (index, &start) in cuts.iter().enumerate() {
        let end = cuts.get(index + 1).copied().unwrap_or(FRAMES);
        let range = start as usize..end as usize;
        let scene_max = scenes
            .iter()
            .rfind(|s| s["start"].as_u64().unwrap() <= start)
            .unwrap()["max_pq_12bit"]
            .as_u64()
            .unwrap() as u16;
        let block = ExtMetadataBlockLevel1::from_stats_cm_version(
            *min[range.clone()].iter().min().unwrap(),
            peak(start, scene_max),
            mean_code(&avg[range.clone()]),
            CmVersion::V29,
        );
        frames.extend(range.map(|_| (block.min_pq, block.max_pq, block.avg_pq)));
    }
    frames
}

#[test]
fn hybrid_l1_takes_the_hdr10plus_peak_and_the_measured_average_per_union_piece() {
    if let Some(reason) = missing_prerequisite() {
        eprintln!("Skipping HDR10+ hybrid test: {reason}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    synthesize_hdr10plus_clip(dir.path());
    let measurements = dir.path().join("clip_measurements.bin");
    let sidecar_path = dir.path().join("clip_measurements.bin.l1.json");
    let output = dir.path().join(OUTPUT);

    // Without the flag nothing is measured: L1 comes from the HDR10+ metadata.
    let log = convert(dir.path(), &[]);
    assert!(!log.contains("hybrid"), "{log}");
    assert!(!measurements.exists());
    std::fs::remove_file(&output).unwrap();

    let log = convert(dir.path(), &["--hdr10plus-l1", "hybrid"]);
    assert!(log.contains("HDR10+ hybrid L1:"), "{log}");
    assert!(log.contains("pixel fallback on 0"), "{log}");
    let sidecar: Value = serde_json::from_slice(&std::fs::read(&sidecar_path).unwrap()).unwrap();
    let scene_starts: Vec<u64> = sidecar["scenes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["start"].as_u64().unwrap())
        .collect();
    assert!(scene_starts.contains(&STEP_FRAME), "{scene_starts:?}");
    for (start, _) in &HDR10PLUS_SCENES[1..] {
        assert!(
            !scene_starts.contains(start),
            "the test needs HDR10+ start {start} inside an analyzer scene: {scene_starts:?}"
        );
    }
    let pieces = scene_starts.len() + HDR10PLUS_SCENES.len() - 1;
    assert!(
        log.contains(&format!("HDR10+ hybrid L1: {pieces} pieces")),
        "{log}"
    );
    let hdr10plus_peak = |start: u64, _| {
        let (_, nits) = HDR10PLUS_SCENES
            .iter()
            .rfind(|&&(scene_start, _)| scene_start <= start)
            .unwrap();
        nits_to_pq_12_bit(*nits as f64)
    };
    let delivered = l1_per_access_unit(&output);
    assert_eq!(delivered.len() as u64, FRAMES);
    assert_eq!(delivered, expected_l1(&sidecar, hdr10plus_peak));
    // The dark frames before the step keep their own (floored) average; the HDR10+ scene
    // starting at 6 changes only the peak.
    assert_eq!(delivered[5].1, nits_to_pq_12_bit(1500.0));
    assert_eq!(delivered[6].1, nits_to_pq_12_bit(2000.0));
    assert_eq!(delivered[5].2, delivered[6].2);
    std::fs::remove_file(&output).unwrap();

    // MaxSCL is 0 on every frame: every piece falls back to its analyzer scene's peak, visibly.
    // The measurements of the previous run are reused.
    let log = convert(
        dir.path(),
        &["--hdr10plus-l1", "hybrid", "--peak-source", "max-scl"],
    );
    assert!(log.contains("Using existing measurements file."), "{log}");
    assert!(log.contains(&format!("zero {pieces}")), "{log}");
    assert!(log.contains("HDR10+ peak is 0"), "{log}");
    let delivered = l1_per_access_unit(&output);
    assert_eq!(delivered, expected_l1(&sidecar, |_, scene_max| scene_max));
}
