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
