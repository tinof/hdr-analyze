//! A Profile 8 input whose Dolby Vision configuration record names a base layer mkvdovi does not
//! deliver is refused by name before any temp work (ROADMAP P6.3). The fixtures are real Profile 8
//! clips: x265, an 8.1 RPU from `dovi_tool generate`, `dovi_tool inject-rpu`, mkvmerge. mkvmerge
//! derives the record's compatibility ID from the HEVC VUI, so a BT.709-tagged stream gets ID 2
//! (Profile 8.2, SDR base layer) and a PQ-tagged one ID 1 (8.1).
//!
//! Skipped (with a message) when ffmpeg with libx265, ffprobe, mkvmerge, mediainfo or dovi_tool is
//! missing, or when no current `hdr_analyzer_mvp` is built next to mkvdovi.

#[macro_use]
#[path = "support/playback.rs"]
mod playback;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

use self::playback::*;

const FRAMES: usize = 12;
const SDR_TAGS: &str = "colorprim=bt709:transfer=bt709:colormatrix=bt709:range=limited";

/// A Profile 8 clip whose base layer carries these x265 colour tags.
fn p8_clip(dir: &Path, tags: &str) -> PathBuf {
    let raw = encode(
        dir,
        "base",
        STEP,
        FRAMES as u32,
        &format!("bframes=0:{tags}"),
    );
    let config = dir.join("generate.json");
    std::fs::write(
        &config,
        json!({
            "cm_version": "V40",
            "length": FRAMES,
            "profile": "8.1",
            "shots": [{
                "start": 0,
                "duration": FRAMES,
                "metadata_blocks": [{"Level1": {"min_pq": 0, "max_pq": 2800, "avg_pq": 1500}}],
            }],
        })
        .to_string(),
    )
    .unwrap();
    let rpu = dir.join("RPU.bin");
    run(Command::new("dovi_tool")
        .arg("generate")
        .arg("-j")
        .arg(&config)
        .arg("-o")
        .arg(&rpu));
    let injected = dir.join("injected.hevc");
    run(Command::new("dovi_tool")
        .arg("inject-rpu")
        .arg("-i")
        .arg(&raw)
        .arg("--rpu-in")
        .arg(&rpu)
        .arg("-o")
        .arg(&injected));
    mux(dir, SOURCE, &injected, &[])
}

/// The record's `dv_bl_signal_compatibility_id`, read by ffprobe independently of mkvdovi.
fn compatibility_id(mkv: &Path) -> Option<u8> {
    let output = run(Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_streams"])
        .args(["-of", "default=nw=1"])
        .arg(mkv));
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("dv_bl_signal_compatibility_id="))
        .and_then(|id| id.trim().parse().ok())
}

fn temp_dirs(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("mkvdovi_temp_"))
        .collect()
}

#[test]
fn a_profile_8_2_input_is_refused_by_name_before_any_temp_work() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let source = p8_clip(dir.path(), SDR_TAGS);
    assert_eq!(
        compatibility_id(&source),
        Some(2),
        "the fixture must be 8.2"
    );

    let run = mkvdovi_cmd()
        .current_dir(dir.path())
        .args(["--mdfix", "--keep-source", SOURCE])
        .output()
        .unwrap();
    let log = log_of(&run);
    assert!(!run.status.success(), "8.2 must be refused:\n{log}");
    assert!(
        log.contains("Profile 8.2, compatibility ID 2: an SDR base layer"),
        "{log}"
    );
    assert!(temp_dirs(dir.path()).is_empty(), "no temp work:\n{log}");
    assert!(source.exists());
    assert!(!dir.path().join(OUTPUT).exists());
    assert!(!dir.path().join("clip.mdfix.DV.mkv").exists());
}

#[test]
fn a_profile_8_1_input_passes_the_record_check() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let source = p8_clip(dir.path(), PQ_TAGS);
    assert_eq!(
        compatibility_id(&source),
        Some(1),
        "the fixture must be 8.1"
    );

    // Without --mdfix mkvdovi declines a Profile 8 input as already converted: it got past the
    // record check, which would have refused it first.
    let run = mkvdovi_cmd()
        .current_dir(dir.path())
        .args(["--keep-source", SOURCE])
        .output()
        .unwrap();
    let log = log_of(&run);
    assert!(
        log.contains("Profile 8 input is already converted"),
        "{log}"
    );
    assert!(!log.contains("compatibility ID"), "{log}");
    assert!(source.exists());
}

/// A Profile 7 MEL clip (an 8.1 RPU turned into Profile 7 by `dovi_tool editor` mode 1) whose
/// mkvmerge record is rewritten to Profile 8 with compatibility ID 2, its RPUs untouched: the
/// RPU routes it to the MEL path, while the record still describes an SDR base layer. The `dvcC`
/// payload follows its BlockAddIDExtraData header (`41 ED 98`): version 1.0, then `dv_profile`
/// in the top 7 bits of byte 2, and the compatibility ID in the top 4 bits of byte 4.
fn mel_clip_with_a_profile_8_2_record(dir: &Path) -> PathBuf {
    let clip = p8_clip(dir, PQ_TAGS);
    let injected = dir.join("injected.hevc");
    let edit = dir.join("edit.json");
    std::fs::write(&edit, json!({"mode": 1}).to_string()).unwrap();
    let p7 = dir.join("P7_RPU.bin");
    run(Command::new("dovi_tool")
        .arg("editor")
        .arg("-i")
        .arg(dir.join("RPU.bin"))
        .arg("-j")
        .arg(&edit)
        .arg("-o")
        .arg(&p7));
    let mel = dir.join("mel.hevc");
    run(Command::new("dovi_tool")
        .arg("inject-rpu")
        .arg("-i")
        .arg(&injected)
        .arg("--rpu-in")
        .arg(&p7)
        .arg("-o")
        .arg(&mel));
    std::fs::remove_file(&clip).unwrap();
    let clip = mux(dir, SOURCE, &mel, &[]);
    let mut bytes = std::fs::read(&clip).unwrap();
    let at = bytes
        .windows(4)
        .position(|window| window == b"dvcC")
        .expect("mkvmerge writes a dvcC record for Profile 7");
    let payload = at + 4 + 3;
    assert_eq!(&bytes[at + 4..payload], [0x41, 0xED, 0x98], "record layout");
    assert_eq!(bytes[payload + 2] >> 1, 7, "a Profile 7 record");
    bytes[payload + 2] = (8 << 1) | (bytes[payload + 2] & 1);
    bytes[payload + 4] = (2 << 4) | (bytes[payload + 4] & 0x0F);
    std::fs::write(&clip, bytes).unwrap();
    clip
}

#[test]
fn a_profile_8_2_record_over_a_mel_rpu_is_refused_by_name_before_any_temp_work() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let source = mel_clip_with_a_profile_8_2_record(dir.path());
    assert_eq!(
        compatibility_id(&source),
        Some(2),
        "the record must say 8.2"
    );

    let run = mkvdovi_cmd()
        .current_dir(dir.path())
        .args(["--keep-source", SOURCE])
        .output()
        .unwrap();
    let log = log_of(&run);
    assert!(!run.status.success(), "the record must be refused:\n{log}");
    assert!(
        log.contains("the RPU is Dolby Vision Profile 7 MEL"),
        "{log}"
    );
    assert!(
        log.contains("Profile 8.2, compatibility ID 2: an SDR base layer"),
        "{log}"
    );
    assert!(temp_dirs(dir.path()).is_empty(), "no temp work:\n{log}");
    assert!(source.exists());
    assert!(!dir.path().join(OUTPUT).exists());
}

#[test]
fn an_id_2_record_on_a_stream_tagged_hlg_is_refused_as_a_contradiction_not_as_sdr() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    // A BT.709 VUI gives the record ID 2; the Matroska colour element says HLG (18).
    let source = p8_clip(dir.path(), SDR_TAGS);
    std::fs::remove_file(&source).unwrap();
    let source = mux(
        dir.path(),
        SOURCE,
        &dir.path().join("injected.hevc"),
        &["--colour-transfer-characteristics", "0:18"],
    );
    assert_eq!(
        compatibility_id(&source),
        Some(2),
        "the record must say ID 2"
    );

    let run = mkvdovi_cmd()
        .current_dir(dir.path())
        .args(["--mdfix", "--keep-source", SOURCE])
        .output()
        .unwrap();
    let log = log_of(&run);
    assert!(!run.status.success(), "the record must be refused:\n{log}");
    assert!(log.contains("while the stream signals HLG"), "{log}");
    assert!(!log.contains("an SDR base layer"), "{log}");
    assert!(temp_dirs(dir.path()).is_empty(), "no temp work:\n{log}");
}
