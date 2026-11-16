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

#[macro_use]
#[path = "support/playback.rs"]
mod playback;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use dovi_rpu_builder::manifest::{AuMatcher, AuVerdict, QualityClass};
use dovi_rpu_builder::{HlgComposer, Profile};
use serde_json::Value;

use self::playback::*;

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

/// ROADMAP P15.5, as decided in docs/PLAYBACK_MANIFEST.md: an FFmpeg-cut open-GOP file is
/// refused with the mkvmerge remux as the remedy, its conversion works, and the remux gets a
/// manifest whose RPUs equal that conversion's, AU for AU.
#[test]
fn ffmpeg_open_gop_cut_is_refused_and_its_mkvmerge_remux_matches_the_conversion() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let cut = ffmpeg_open_gop_cut(dir.path());
    let cut_packets = packets_with_optional_pts(&cut);
    let without_pts = cut_packets.iter().filter(|(pts, _)| pts.is_none()).count();
    assert_eq!(
        without_pts, 1,
        "the fixture no longer gives its RASL picture a negative timestamp"
    );

    // Refused after the measurement: the .bin and the sidecar stay, no AU table, no manifest.
    let work = dir.path().join("work");
    let output = mkvdovi_cmd()
        .current_dir(dir.path())
        .args(["manifest", SOURCE, "--work-dir"])
        .arg(&work)
        .args(["--hwaccel", "none"])
        .output()
        .unwrap();
    let log = log_of(&output);
    assert!(!output.status.success(), "the cut must be refused:\n{log}");
    assert!(log.contains("video packet 1 has no pts"), "{log}");
    assert!(
        log.contains("Remux the file with `mkvmerge -o <new>.mkv <input>`"),
        "{log}"
    );
    assert!(!work.join(MANIFEST).exists(), "a refusal wrote a manifest");
    assert!(!work.join("clip_measurements.bin.au.json").exists());
    let measurements = std::fs::metadata(work.join("clip_measurements.bin")).unwrap();
    assert!(measurements.len() > 0);
    let sidecar = read_json(&work.join("clip_measurements.bin.l1.json"));
    assert_eq!(sidecar["source"]["leading_skipped_frames"], 1);
    assert_eq!(sidecar["source"]["stream_frames"], cut_packets.len());

    let converted = convert(dir.path(), &[]);

    let remux = mkvmerge_remux(&cut, &dir.path().join("remux"));
    // `packets` also asserts that every packet of the remux has a pts.
    let remux_payloads: Vec<Vec<u8>> = packets(&remux).into_iter().map(|(_, d)| d).collect();
    let cut_payloads: Vec<Vec<u8>> = cut_packets.into_iter().map(|(_, d)| d).collect();
    assert!(
        remux_payloads == cut_payloads,
        "the remux must keep every packet, in decode order"
    );
    let outcome = write_manifest(&dir.path().join("remux"), &[]);
    assert_eq!(outcome.manifest.leading_skipped, 1);
    assert_rpus_equal_the_conversion(&outcome.manifest, &remux, &converted);
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
