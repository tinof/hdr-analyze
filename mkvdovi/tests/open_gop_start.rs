//! A source cut at a CRA picture (open GOP) starts with RASL pictures that no decoder outputs.
//! `dovi_tool inject-rpu` still gives RPU `n` to the picture with presentation number `n`, and
//! those pictures come first, so the measured L1 must move back by their count. The test checks
//! the delivered association without `dovi_tool`'s own parser: the RPU of every access unit (in
//! decode order) is matched to the picture the decoder displays from it.
//!
//! Skipped (with a message) when ffmpeg with libx265, ffprobe, mkvmerge or dovi_tool is missing,
//! or when no `hdr_analyzer_mvp` that records undecodable leading pictures (sidecar v5) is built
//! next to the test's mkvdovi binary (`cargo build -p hdr_analyzer_mvp`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::prelude::*;
use dolby_vision::rpu::dovi_rpu::DoviRpu;
use dolby_vision::rpu::extension_metadata::blocks::ExtMetadataBlock;
use serde_json::Value;

const SOURCE: &str = "cut.mkv";
const OUTPUT: &str = "cut.DV.mkv";
/// Source frame where the picture turns from dark to bright.
const STEP_FRAME: u64 = 40;
/// The cut starts at the CRA of source frame 24 (keyint 24).
const CUT_FRAME: u64 = 24;

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

/// A 10-bit PQ clip with open GOPs (keyint 24, three B-frames before each CRA in display
/// order), dark before `STEP_FRAME` and bright from it, cut by mkvmerge at the CRA of
/// `CUT_FRAME`: the cut keeps that CRA's three RASL pictures.
fn synthesize_open_gop_cut(dir: &Path) -> PathBuf {
    let raw = dir.join("full.hevc");
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
            "96",
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-x265-params",
            "log-level=error:keyint=24:min-keyint=24:open-gop=1:bframes=3:b-adapt=0:scenecut=0:\
             colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:range=limited:hdr10=1:\
             master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):\
             max-cll=1000,400:repeat-headers=1",
            "-pix_fmt",
            "yuv420p10le",
            "-f",
            "hevc",
            "-y",
        ])
        .arg(&raw));
    let full = dir.join("full.mkv");
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&full)
        .arg(&raw));
    let cut = dir.join(SOURCE);
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&cut)
        .args(["--split", "parts:00:00:01-00:00:10"])
        .arg(&full));
    for path in [raw, full] {
        std::fs::remove_file(path).unwrap();
    }
    cut
}

fn ffprobe_json(path: &Path, entries: &str) -> Value {
    let output = run(Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            entries,
        ])
        .args(["-of", "json"])
        .arg(path));
    serde_json::from_slice(&output.stdout).unwrap()
}

fn pts_list(json: &Value, key: &str) -> Vec<i64> {
    json[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            item["pts"]
                .as_i64()
                .expect("every picture has a pts in MKV")
        })
        .collect()
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

/// L1 max of the RPU in each access unit, in decode order.
fn l1_max_per_access_unit(mkv: &Path) -> Vec<u16> {
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
    let mut maxima: Vec<Option<u16>> = Vec::new();
    for unit in nal_units(&stream) {
        let kind = (unit[0] >> 1) & 0x3f;
        // A slice with first_slice_segment_in_pic_flag starts a new access unit.
        if kind < 32 && unit.len() > 2 && unit[2] & 0x80 != 0 {
            maxima.push(None);
        } else if kind == 62 {
            let rpu = DoviRpu::parse_unspec62_nalu(unit).expect("parse RPU NAL");
            let level1 = match rpu.vdr_dm_data.as_ref().and_then(|dm| dm.get_block(1)) {
                Some(ExtMetadataBlock::Level1(level1)) => level1.max_pq,
                _ => panic!("RPU without L1"),
            };
            *maxima.last_mut().expect("RPU after a slice") = Some(level1);
        }
    }
    maxima
        .into_iter()
        .map(|max| max.expect("every access unit carries an RPU"))
        .collect()
}

#[test]
fn an_open_gop_cut_keeps_its_l1_on_the_displayed_pictures() {
    if let Some(reason) = missing_prerequisite() {
        eprintln!("Skipping open-GOP cut test: {reason}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = synthesize_open_gop_cut(dir.path());

    // The measurements the conversion will use, taken by the same analyzer.
    let measured = dir.path().join("measured.bin");
    run(Command::new(sibling_analyzer().unwrap())
        .arg(&source)
        .arg("-o")
        .arg(&measured)
        .args(["--no-crop", "--hwaccel", "none"]));
    let sidecar: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("measured.bin.l1.json")).unwrap())
            .unwrap();
    if sidecar["version"].as_u64() < Some(5) {
        eprintln!(
            "Skipping open-GOP cut test: the analyzer writes sidecar v{}",
            sidecar["version"]
        );
        return;
    }
    assert_eq!(sidecar["source"]["leading_skipped_frames"], 3);
    assert_eq!(sidecar["source"]["stream_frames"], 75);
    let scenes = sidecar["scenes"].as_array().unwrap();
    assert_eq!(scenes.len(), 2, "{scenes:?}");
    // Measured frame 0 is source frame 24, so the step at source frame 40 is measured frame 16.
    assert_eq!(scenes[1]["start"], STEP_FRAME - CUT_FRAME);
    std::fs::remove_file(&measured).unwrap();
    std::fs::remove_file(dir.path().join("measured.bin.l1.json")).unwrap();

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
    assert!(log.contains("3 undecodable leading pictures"), "{log}");

    // Display order from the decoder, decode order from the demuxer; MKV gives every picture a
    // distinct pts, so pts maps a displayed frame to its access unit.
    let output = dir.path().join(OUTPUT);
    let packets = pts_list(&ffprobe_json(&output, "packet=pts"), "packets");
    let displayed = pts_list(&ffprobe_json(&output, "frame=pts"), "frames");
    let access_unit: HashMap<i64, usize> = packets
        .iter()
        .enumerate()
        .map(|(index, &pts)| (pts, index))
        .collect();
    let maxima = l1_max_per_access_unit(&output);
    assert_eq!(maxima.len(), packets.len());
    assert_eq!(displayed.len(), 72);

    // `--verify` already compared every RPU frame with the measured L1; here the dark scene's L1
    // must end exactly at the last dark displayed picture.
    let l1_of = |frame: usize| maxima[access_unit[&displayed[frame]]];
    let (dark, bright) = (l1_of(0), l1_of(displayed.len() - 1));
    assert!(dark < bright, "dark L1 max {dark}, bright {bright}");
    for (frame, pts) in displayed.iter().enumerate() {
        let expected = if (frame as u64) < STEP_FRAME - CUT_FRAME {
            dark
        } else {
            bright
        };
        assert_eq!(
            maxima[access_unit[pts]], expected,
            "displayed frame {frame} carries the wrong L1"
        );
    }
}

#[test]
fn a_stream_that_does_not_start_at_a_random_access_picture_is_refused() {
    if let Some(reason) = missing_prerequisite() {
        eprintln!("Skipping mid-GOP start test: {reason}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let cut = synthesize_open_gop_cut(dir.path());
    // Drop the CRA: the stream now starts with its RASL pictures and the decoder loses more
    // pictures than the leading ones, which nothing can line up with the video.
    let broken = dir.path().join("broken.mkv");
    run(Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&cut)
        .args(["-c", "copy", "-bsf:v", "noise=drop=eq(n\\,0)", "-y"])
        .arg(&broken));

    let conversion = mkvdovi_cmd()
        .current_dir(dir.path())
        .arg("broken.mkv")
        .args(["--hwaccel", "none"])
        .output()
        .unwrap();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&conversion.stdout),
        String::from_utf8_lossy(&conversion.stderr)
    );
    assert!(!conversion.status.success(), "mkvdovi accepted it:\n{log}");
    assert!(
        log.contains("does not start with a random access picture"),
        "{log}"
    );
    assert!(broken.exists(), "the source must be kept");
    assert!(!dir.path().join("broken.DV.mkv").exists());
}
