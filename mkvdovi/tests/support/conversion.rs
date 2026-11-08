//! Sources and the conversion run shared by the integration tests of the output checks of every
//! mux (`mux_timing.rs`, `rpu_count.rs`). Declared after `support/playback.rs`, whose helpers
//! it uses.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;

use super::playback::*;

/// A PQ source of 48 frames with B-frames, muxed by mkvmerge with `timing` (options for the
/// video file) and a FLAC audio track that starts at 0.
pub fn pq_source(dir: &Path, timing: &[&str]) -> PathBuf {
    let raw = encode(
        dir,
        "pq",
        STEP,
        48,
        &format!("keyint=24:min-keyint=24:bframes=2:b-adapt=0:{PQ_TAGS}:{MASTERING}"),
    );
    with_audio(dir, &raw, timing)
}

/// `<dir>/clip.mkv`: `video` (Annex B) muxed with `timing`, plus four seconds of FLAC.
pub fn with_audio(dir: &Path, video: &Path, timing: &[&str]) -> PathBuf {
    let audio = dir.join("audio.flac");
    run(Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args(["-i", "sine=frequency=440:duration=4", "-c:a", "flac", "-y"])
        .arg(&audio));
    let source = dir.join(SOURCE);
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&source)
        .args(timing)
        .arg(video)
        .arg(&audio));
    source
}

/// Converts a copy of `source` in `<dir>/run` with `flags` and `path` (PATH) when given;
/// returns the run's output, the copy and the converted file's path.
pub fn convert_copy(
    dir: &Path,
    source: &Path,
    flags: &[&str],
    path: Option<&str>,
) -> (Output, PathBuf, PathBuf) {
    let work = dir.join("run");
    std::fs::create_dir_all(&work).unwrap();
    let copy = work.join(SOURCE);
    std::fs::copy(source, &copy).unwrap();
    let mut command = mkvdovi_cmd();
    command
        .current_dir(&work)
        .arg(SOURCE)
        .args(["--hwaccel", "none"])
        .args(flags);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    (command.output().unwrap(), copy, work.join(OUTPUT))
}

/// A Profile 7 MEL clip of 48 frames from 2000 ms: a generated 8.1 RPU turned MEL by
/// `dovi_tool editor` mode 1, injected, muxed by mkvmerge (as `metadata/tests.rs`
/// `profile7_clip`).
pub fn profile7_mel_source(dir: &Path) -> PathBuf {
    let raw = encode(
        dir,
        "p7",
        STEP,
        48,
        &format!("keyint=24:min-keyint=24:bframes=2:b-adapt=0:{PQ_TAGS}:{MASTERING}"),
    );
    let config = dir.join("generate.json");
    std::fs::write(
        &config,
        json!({"cm_version": "V40", "length": 48, "profile": "8.1"}).to_string(),
    )
    .unwrap();
    let (rpu, p7, edit) = (
        dir.join("RPU.bin"),
        dir.join("P7_RPU.bin"),
        dir.join("edit.json"),
    );
    run(Command::new("dovi_tool")
        .arg("generate")
        .arg("-j")
        .arg(&config)
        .arg("-o")
        .arg(&rpu));
    std::fs::write(&edit, json!({"mode": 1}).to_string()).unwrap();
    run(Command::new("dovi_tool")
        .arg("editor")
        .arg("-i")
        .arg(&rpu)
        .arg("-j")
        .arg(&edit)
        .arg("-o")
        .arg(&p7));
    let injected = dir.join("injected.hevc");
    run(Command::new("dovi_tool")
        .arg("inject-rpu")
        .arg("-i")
        .arg(&raw)
        .arg("--rpu-in")
        .arg(&p7)
        .arg("-o")
        .arg(&injected));
    with_audio(dir, &injected, &["--sync", "0:2000"])
}
