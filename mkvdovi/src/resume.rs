//! Resume support for interrupted conversions.
//!
//! Each long pipeline step writes a sibling completion sentinel (`<artifact>.done`) *after*
//! its output is fully written and validated. On a later run, [`is_complete`] only treats an
//! artifact as reusable when both the artifact and its sentinel exist — so a file that was
//! truncated by a killed process (it exists but has no sentinel) is correctly regenerated.
//!
//! Sentinels live inside the per-file temp directory, so the normal end-of-run cleanup
//! (`remove_dir_all`) removes them along with the artifacts.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

/// File inside the temp directory that binds its artifacts to one input and one set of settings.
pub const FINGERPRINT_FILE: &str = "resume.json";

/// Identity of the input and the artifact-affecting settings a temp directory was created for.
/// A leftover directory is only resumed when its fingerprint equals the current one, so a
/// replaced input, a different preset, or a new mkvdovi version never reuses stale artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub input_name: String,
    pub input_size: u64,
    pub input_mtime_secs: u64,
    pub mkvdovi_version: String,
    pub settings: String,
}

impl Fingerprint {
    pub fn for_input(input: &Path, settings: String) -> std::io::Result<Self> {
        let metadata = fs::metadata(input)?;
        let input_mtime_secs = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |elapsed| elapsed.as_secs());
        Ok(Self {
            input_name: input
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            input_size: metadata.len(),
            input_mtime_secs,
            mkvdovi_version: env!("CARGO_PKG_VERSION").to_owned(),
            settings,
        })
    }

    pub fn write(&self, temp_dir: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        fs::write(temp_dir.join(FINGERPRINT_FILE), json)
    }

    /// True when `temp_dir` holds a fingerprint equal to this one. A missing or unreadable
    /// fingerprint (e.g. a directory from an older mkvdovi) never matches.
    pub fn matches(&self, temp_dir: &Path) -> bool {
        fs::read(temp_dir.join(FINGERPRINT_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Fingerprint>(&bytes).ok())
            .is_some_and(|stored| stored == *self)
    }
}

/// Sentinel path for a completed artifact: `<artifact>.done`.
pub fn marker_path(artifact: &Path) -> PathBuf {
    let mut name = artifact.as_os_str().to_owned();
    name.push(".done");
    PathBuf::from(name)
}

/// Mark `artifact` as fully written. Call only after the producing step succeeds and the
/// output has been validated (exists and non-empty).
pub fn mark_done(artifact: &Path) -> std::io::Result<()> {
    fs::write(marker_path(artifact), b"")
}

/// True when `artifact` exists, is non-empty, and has a completion sentinel — i.e. it was
/// produced by a step that ran to completion and is safe to reuse.
pub fn is_complete(artifact: &Path) -> bool {
    fs::metadata(artifact).map(|m| m.len() > 0).unwrap_or(false) && marker_path(artifact).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn incomplete_without_marker_or_content() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join("BL.hevc");

        // Missing entirely.
        assert!(!is_complete(&artifact));

        // Exists with content but no sentinel (e.g. killed mid-write) -> not reusable.
        let mut f = fs::File::create(&artifact).unwrap();
        f.write_all(b"some data").unwrap();
        drop(f);
        assert!(!is_complete(&artifact));

        // Sentinel present but artifact empty -> not reusable.
        let empty = dir.path().join("empty.hevc");
        fs::File::create(&empty).unwrap();
        mark_done(&empty).unwrap();
        assert!(!is_complete(&empty));
    }

    #[test]
    fn fingerprint_matches_only_the_same_input_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.mkv");
        fs::write(&input, b"source").unwrap();
        let temp = dir.path().join("mkvdovi_temp_input");
        fs::create_dir(&temp).unwrap();

        let fingerprint = Fingerprint::for_input(&input, "quality=Accurate".into()).unwrap();
        assert!(!fingerprint.matches(&temp), "no fingerprint written yet");
        fingerprint.write(&temp).unwrap();
        assert!(fingerprint.matches(&temp));

        let other_settings = Fingerprint::for_input(&input, "quality=Fast".into()).unwrap();
        assert!(!other_settings.matches(&temp));

        fs::write(&input, b"replaced source").unwrap();
        let replaced = Fingerprint::for_input(&input, "quality=Accurate".into()).unwrap();
        assert!(!replaced.matches(&temp));
    }

    #[test]
    fn complete_with_content_and_marker() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join("BL_RPU.hevc");
        fs::write(&artifact, b"payload").unwrap();
        mark_done(&artifact).unwrap();

        assert!(is_complete(&artifact));
        assert!(marker_path(&artifact).exists());
    }
}
