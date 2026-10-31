//! `--hdr10plus-l1 hybrid` end to end: an HDR10+ clip whose HDR10+ scenes split the analyzer's
//! scenes. Every delivered RPU frame must carry the L1 of its union piece: the HDR10+ peak of
//! the HDR10+ scene, and the average and minimum of the measured frames of the piece, after the
//! generator's clamps. With `--peak-source max-scl` (MaxSCL is 0 here, as on HDR10+ Profile A
//! discs) every piece takes the measured peak of its own frames instead. With `--hdr10plus-l1
//! hdr10plus` nothing is measured. An explicit hybrid refuses an analyzer that cannot record the
//! per-frame peak before it runs; without the flag such a file takes the metadata path, visibly.
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

/// What `dovi_tool generate` writes for any lower L1 maximum (the 100-nit floor).
const L1_MAX_FLOOR: u16 = 2081;
const SOURCE: &str = "clip.mkv";
const OUTPUT: &str = "clip.DV.mkv";
const FRAMES: u64 = 48;
/// Frame where the picture turns from dark to bright (an analyzer cut).
const STEP_FRAME: u64 = 12;
/// The HDR10+ scene start inside the dark analyzer scene. A small bright block starts here, so
/// the measured peak of the piece before it is far below the peak of its analyzer scene.
const HDR10PLUS_START: u64 = 6;
/// HDR10+ scene starts (6 and 36 fall inside analyzer scenes) and their peaks in nits.
const HDR10PLUS_SCENES: [(u64, u64); 3] = [(0, 1500), (HDR10PLUS_START, 2000), (36, 4000)];

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
                 geq=lum='if(lt(N\\,{STEP_FRAME})\\,if(gte(N\\,{HDR10PLUS_START})*between(X\\,96\\,111)*between(Y\\,48\\,63)\\,600\\,200)\\,700)+mod(X+N\\,32)':cb='512':cr='512'"
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

/// The Annex B video stream of `mkv`, RPU NAL units included.
fn video_stream(mkv: &Path) -> Vec<u8> {
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
    std::fs::read(&annexb).unwrap()
}

/// L1 `(min, max, avg)` of the RPU in each access unit, in decode order.
fn l1_per_access_unit(mkv: &Path) -> Vec<(u16, u16, u16)> {
    let stream = video_stream(mkv);
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
    convert_with(mkvdovi_cmd(), dir, flags)
}

/// Runs `mkvdovi` with the options every conversion here shares; the status and the log.
fn try_convert(mut mkvdovi: Command, dir: &Path, flags: &[&str]) -> (bool, String) {
    let conversion = mkvdovi
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
    (conversion.status.success(), log)
}

fn convert_with(mkvdovi: Command, dir: &Path, flags: &[&str]) -> String {
    let (success, log) = try_convert(mkvdovi, dir, flags);
    assert!(success, "mkvdovi failed:\n{log}");
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
/// the HDR10+ scenes, with `peak(piece_start, piece_max)` as the piece maximum, where `piece_max`
/// is the largest frame peak in the piece, after the generator's clamps.
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
    let frame_peaks = field("max_pq_12bit");
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
        let piece_max = *frame_peaks[range.clone()].iter().max().unwrap();
        let block = ExtMetadataBlockLevel1::from_stats_cm_version(
            *min[range.clone()].iter().min().unwrap(),
            peak(start, piece_max),
            mean_code(&avg[range.clone()]),
            CmVersion::V29,
        );
        frames.extend(range.map(|_| (block.min_pq, block.max_pq, block.avg_pq)));
    }
    frames
}

/// A copy of mkvdovi with a fake `hdr_analyzer_mvp` beside it, which mkvdovi prefers over any
/// other analyzer. The fake prints `help` for `--help`; any analysis run leaves `analyzed` and
/// fails. Written before the slow clip synthesis, so that no test's fork holds these files open
/// for writing when they are executed.
#[allow(deprecated)]
#[cfg(unix)]
fn mkvdovi_with_fake_analyzer(dir: &Path, help: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir(dir).unwrap();
    let mkvdovi = dir.join("mkvdovi");
    std::fs::copy(assert_cmd::cargo::cargo_bin("mkvdovi"), &mkvdovi).unwrap();
    let analyzed = dir.join("analyzed");
    let analyzer = dir.join("hdr_analyzer_mvp");
    std::fs::write(
        &analyzer,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n\
             --help) echo '{help}' ;;\n\
             --version) echo 'hdr_analyzer_mvp 0.0.0' ;;\n\
             *) : > '{}'; exit 1 ;;\n\
             esac\n",
            analyzed.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&analyzer, std::fs::Permissions::from_mode(0o755)).unwrap();
    (mkvdovi, analyzed)
}

/// [`mkvdovi_with_fake_analyzer`] whose analyzer answers `--help` with the capability once, then
/// can no longer be started, so the analysis fails at the spawn and not with an exit status.
#[cfg(unix)]
fn mkvdovi_with_analyzer_that_stops_starting(dir: &Path) -> PathBuf {
    let (mkvdovi, _) = mkvdovi_with_fake_analyzer(dir, "Capabilities: l1-sidecar-frame-peak");
    let analyzer = dir.join("hdr_analyzer_mvp");
    let script = std::fs::read_to_string(&analyzer).unwrap();
    let script = script.replace(
        "--help) echo 'Capabilities: l1-sidecar-frame-peak' ;;",
        "--help) echo 'Capabilities: l1-sidecar-frame-peak'; chmod -x \"$0\" ;;",
    );
    std::fs::write(&analyzer, script).unwrap();
    mkvdovi
}

/// [`mkvdovi_with_fake_analyzer`] with an analyzer that cannot be started at all, which wins over
/// any analyzer on `PATH`, as a missing one would not.
#[cfg(unix)]
fn mkvdovi_with_unstartable_analyzer(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let (mkvdovi, _) = mkvdovi_with_fake_analyzer(dir, "Capabilities: l1-sidecar-frame-peak");
    let plain = std::fs::Permissions::from_mode(0o644);
    std::fs::set_permissions(dir.join("hdr_analyzer_mvp"), plain).unwrap();
    mkvdovi
}

#[cfg(unix)]
#[test]
fn hybrid_refuses_an_analyzer_without_the_frame_peak_before_analysis() {
    if let Some(reason) = missing_prerequisite() {
        eprintln!("Skipping HDR10+ hybrid test: {reason}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (stale, stale_analyzed) =
        mkvdovi_with_fake_analyzer(&dir.path().join("stale"), "Usage: hdr_analyzer_mvp");
    let (current, current_analyzed) = mkvdovi_with_fake_analyzer(
        &dir.path().join("current"),
        "Capabilities: l1-sidecar-frame-peak",
    );
    let clip = dir.path().join("clip");
    std::fs::create_dir(&clip).unwrap();
    synthesize_hdr10plus_clip(&clip);
    let hybrid = |mkvdovi: &Path| {
        let conversion = Command::new(mkvdovi)
            .current_dir(&clip)
            .arg(SOURCE)
            .args(["--keep-source", "--hwaccel", "none", "--no-resume"])
            .args(["--hdr10plus-l1", "hybrid"])
            .output()
            .unwrap();
        format!(
            "{}{}",
            String::from_utf8_lossy(&conversion.stdout),
            String::from_utf8_lossy(&conversion.stderr)
        )
    };
    const REFUSAL: &str = "rebuild or update hdr_analyzer_mvp";

    let log = hybrid(&stale);
    assert!(log.contains(REFUSAL), "{log}");
    assert!(!stale_analyzed.exists(), "the analyzer ran:\n{log}");
    assert!(!clip.join("clip_measurements.bin").exists(), "{log}");

    let log = hybrid(&current);
    assert!(!log.contains(REFUSAL), "{log}");
    assert!(
        current_analyzed.exists(),
        "the analyzer did not run:\n{log}"
    );
}

#[cfg(unix)]
#[test]
fn without_the_flag_a_file_that_cannot_take_the_hybrid_converts_from_the_metadata() {
    if let Some(reason) = missing_prerequisite() {
        eprintln!("Skipping HDR10+ hybrid test: {reason}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (stale, stale_analyzed) =
        mkvdovi_with_fake_analyzer(&dir.path().join("stale"), "Usage: hdr_analyzer_mvp");
    let (failing, failing_analyzed) = mkvdovi_with_fake_analyzer(
        &dir.path().join("failing"),
        "Capabilities: l1-sidecar-frame-peak",
    );
    let unstartable = mkvdovi_with_unstartable_analyzer(&dir.path().join("unstartable"));
    let vanishing = mkvdovi_with_analyzer_that_stops_starting(&dir.path().join("vanishing"));
    let vanishing_too = mkvdovi_with_analyzer_that_stops_starting(&dir.path().join("vanishing2"));
    let clip = dir.path().join("clip");
    std::fs::create_dir(&clip).unwrap();
    synthesize_hdr10plus_clip(&clip);
    let output = clip.join(OUTPUT);
    let measurements = clip.join("clip_measurements.bin");

    convert(&clip, &["--hdr10plus-l1", "hdr10plus"]);
    let metadata_stream = video_stream(&output);
    std::fs::remove_file(&output).unwrap();

    // Each of these runs delivers the stream of the explicit metadata path, and says so.
    let assert_metadata_path = |log: &str, reason: &str| {
        assert!(log.contains(reason), "{log}");
        assert!(
            log.contains("L1 comes from the HDR10+ metadata alone"),
            "{log}"
        );
        assert!(video_stream(&output) == metadata_stream, "{log}");
        std::fs::remove_file(&output).unwrap();
    };

    let log = convert_with(Command::new(&stale), &clip, &[]);
    assert_metadata_path(&log, "does not record the per-frame peak");
    assert!(!stale_analyzed.exists(), "the analyzer ran:\n{log}");

    let log = convert(&clip, &["--analysis-quality", "fast"]);
    assert_metadata_path(&log, "analyzes one frame in 3");
    assert!(!measurements.exists(), "{log}");

    let log = convert(&clip, &["--legacy-madvr-l1"]);
    assert_metadata_path(&log, "--legacy-madvr-l1");
    assert!(!measurements.exists(), "{log}");

    let log = convert_with(Command::new(&failing), &clip, &[]);
    assert_metadata_path(&log, "the analysis failed");
    assert!(
        failing_analyzed.exists(),
        "the analyzer did not run:\n{log}"
    );

    let log = convert_with(Command::new(&unstartable), &clip, &[]);
    assert_metadata_path(&log, "hdr_analyzer_mvp is missing");

    // The analysis itself cannot start, which is an error and not a failed run.
    let log = convert_with(Command::new(&vanishing), &clip, &[]);
    assert_metadata_path(&log, "the analysis failed (Failed to spawn command");

    // The same conditions refuse an explicit hybrid.
    let hybrid = ["--hdr10plus-l1", "hybrid"];
    for (mkvdovi, extra, reason) in [
        (
            Command::new(&stale),
            &[][..],
            "rebuild or update hdr_analyzer_mvp",
        ),
        (Command::new(&failing), &[][..], "Failed to process"),
        (
            Command::new(&unstartable),
            &[][..],
            "rebuild or update hdr_analyzer_mvp",
        ),
        (Command::new(&vanishing_too), &[][..], "Failed to spawn command"),
        (
            mkvdovi_cmd(),
            &["--analysis-quality", "fast"][..],
            "analyzes one frame in 3; use balanced or accurate, or convert with --hdr10plus-l1 hdr10plus",
        ),
    ] {
        let flags: Vec<&str> = hybrid.iter().chain(extra).copied().collect();
        let (success, log) = try_convert(mkvdovi, &clip, &flags);
        assert!(!success, "an explicit hybrid must refuse:\n{log}");
        assert!(!log.contains("panicked"), "{log}");
        assert!(log.contains(reason), "{log}");
        assert!(!output.exists(), "{log}");
    }
    let (success, log) = try_convert(
        mkvdovi_cmd(),
        &clip,
        &["--hdr10plus-l1", "hybrid", "--legacy-madvr-l1"],
    );
    assert!(!success, "{log}");
    assert!(log.contains("use one or the other"), "{log}");
}

/// `dovi_tool` that stops every `generate`, which leaves the temp directory of an interrupted
/// run, and passes anything else on. Returns its directory for the front of `PATH`.
#[cfg(unix)]
fn dovi_tool_without_generate(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let real = run(Command::new("sh").args(["-c", "command -v dovi_tool"]));
    let real = String::from_utf8(real.stdout).unwrap();
    let shim = dir.join("dovi_tool");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\ncase \" $* \" in *' generate '*) exit 1 ;; esac\nexec '{}' \"$@\"\n",
            real.trim()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir.to_path_buf()
}

/// The settings fingerprinted by a run that `dovi_tool` stopped. The temp directory goes with it.
#[cfg(unix)]
fn fingerprinted_settings(mut mkvdovi: Command, clip: &Path, shim: &Path) -> String {
    let path = format!(
        "{}:{}",
        shim.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = mkvdovi
        .current_dir(clip)
        .env("PATH", path)
        .arg(SOURCE)
        .args(["--keep-source", "--hwaccel", "none"])
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "dovi_tool generate must stop the run"
    );
    let temp = clip.join("mkvdovi_temp_clip");
    let fingerprint: Value =
        serde_json::from_slice(&std::fs::read(temp.join("resume.json")).unwrap()).unwrap();
    std::fs::remove_dir_all(temp).unwrap();
    fingerprint.to_string()
}

#[cfg(unix)]
#[test]
fn the_fingerprint_follows_the_path_the_file_takes_not_the_flag() {
    if let Some(reason) = missing_prerequisite() {
        eprintln!("Skipping HDR10+ hybrid test: {reason}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (stale, _) =
        mkvdovi_with_fake_analyzer(&dir.path().join("stale"), "Usage: hdr_analyzer_mvp");
    let shim = dir.path().join("shim");
    std::fs::create_dir(&shim).unwrap();
    let shim = dovi_tool_without_generate(&shim);
    let clip = dir.path().join("clip");
    std::fs::create_dir(&clip).unwrap();
    synthesize_hdr10plus_clip(&clip);
    const TOKEN: &str = "hdr10plus_l1=hybrid";

    let settings = fingerprinted_settings(Command::new(&stale), &clip, &shim);
    assert!(!settings.contains(TOKEN), "{settings}");
    let settings = fingerprinted_settings(mkvdovi_cmd(), &clip, &shim);
    assert!(settings.contains(TOKEN), "{settings}");
}

#[test]
fn the_default_is_the_hybrid_with_the_fresh_analysis_of_an_explicit_one() {
    if let Some(reason) = missing_prerequisite() {
        eprintln!("Skipping HDR10+ hybrid test: {reason}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    synthesize_hdr10plus_clip(dir.path());
    let output = dir.path().join(OUTPUT);
    let measurements = dir.path().join("clip_measurements.bin");
    let sidecar = dir.path().join("clip_measurements.bin.l1.json");

    let log = convert(dir.path(), &["--hdr10plus-l1", "hybrid"]);
    assert!(log.contains("HDR10+ hybrid L1:"), "{log}");
    let explicit = video_stream(&output);
    std::fs::remove_file(&output).unwrap();
    std::fs::remove_file(&measurements).unwrap();
    std::fs::remove_file(&sidecar).unwrap();

    let log = convert(dir.path(), &[]);
    assert!(log.contains("HDR10+ hybrid L1:"), "{log}");
    assert!(log.contains("--hdr10plus-l1 hdr10plus skips this"), "{log}");
    assert!(!log.contains("metadata alone"), "{log}");
    assert!(measurements.exists(), "{log}");
    assert!(video_stream(&output) == explicit, "{log}");
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

    // Under hdr10plus nothing is measured: L1 comes from the HDR10+ metadata.
    let log = convert(dir.path(), &["--hdr10plus-l1", "hdr10plus"]);
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
    let frame_peaks: Vec<u64> = sidecar["frames"]["max_pq_12bit"]
        .as_array()
        .unwrap()
        .iter()
        .map(|code| code.as_u64().unwrap())
        .collect();
    assert_eq!(frame_peaks.len() as u64, FRAMES);
    for scene in sidecar["scenes"].as_array().unwrap() {
        let start = scene["start"].as_u64().unwrap() as usize;
        let end = scene["end"].as_u64().unwrap() as usize;
        assert_eq!(
            frame_peaks[start..=end].iter().max(),
            Some(&scene["max_pq_12bit"].as_u64().unwrap()),
            "the scene peak is the maximum of its frames: {scene}"
        );
    }
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

    // MaxSCL is 0 on every frame: every piece falls back to its own measured peak, visibly. The
    // measurements of the previous run are reused.
    let log = convert(
        dir.path(),
        &["--hdr10plus-l1", "hybrid", "--peak-source", "max-scl"],
    );
    assert!(log.contains("Using existing measurements file."), "{log}");
    assert!(log.contains(&format!("zero {pieces}")), "{log}");
    assert!(log.contains("HDR10+ peak is 0"), "{log}");
    let delivered = l1_per_access_unit(&output);
    assert_eq!(delivered, expected_l1(&sidecar, |_, piece_max| piece_max));
    // Frames 0-5 hold no bright block, so their peak is the generator's floor; the next piece of
    // the same analyzer scene keeps the block's peak.
    assert_eq!(delivered[5].1, L1_MAX_FLOOR);
    assert!(delivered[6].1 > 2200, "{:?}", delivered[6]);
}
