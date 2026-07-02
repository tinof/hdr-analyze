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

/// Sentinel path for a completed artifact: `<artifact>.done`.
fn marker_path(artifact: &Path) -> PathBuf {
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
    fn complete_with_content_and_marker() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join("BL_RPU.hevc");
        fs::write(&artifact, b"payload").unwrap();
        mark_done(&artifact).unwrap();

        assert!(is_complete(&artifact));
        assert!(marker_path(&artifact).exists());
    }
}
