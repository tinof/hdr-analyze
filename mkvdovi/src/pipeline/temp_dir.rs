//! The per-file temp directory: where it lives, whether a leftover one is resumed, and its
//! creation with the resume fingerprint.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dovi84_composer::Composer;

use crate::progress;
use crate::resume;

/// The temp directory of this input: `mkvdovi_temp_<stem>`, or a leftover legacy one.
pub(super) fn locate_temp_dir(
    dir: &Path,
    temp_dir_name: &str,
    stem: &str,
    resume_enabled: bool,
) -> PathBuf {
    let mut temp_dir = dir.join(temp_dir_name);
    // Pre-rename compat (mkvdolby -> mkvdovi in v0.3.0): resume from a leftover
    // `mkvdolby_temp_*` directory when no new-style one exists. Such directories predate resume
    // fingerprints, so they resume through the missing-fingerprint path below.
    if resume_enabled && !temp_dir.exists() {
        let legacy_temp_dir = dir.join(format!("mkvdolby_temp_{}", stem));
        if legacy_temp_dir.exists() {
            temp_dir = legacy_temp_dir;
        }
    }
    temp_dir
}

/// Decides whether a leftover `temp_dir` is resumed. A discarded directory is removed and
/// replaced by `fresh_temp_dir`. Returns the temp directory to use and whether it is resumed.
pub(super) fn settle_leftover_temp_dir(
    mut temp_dir: PathBuf,
    fresh_temp_dir: PathBuf,
    resume_enabled: bool,
    fingerprint: Option<&resume::Fingerprint>,
    custom_composer: Option<Composer>,
    requested_composer: Composer,
) -> (PathBuf, bool) {
    // A leftover temp dir means a previous run for this file was interrupted. With resume
    // enabled we reuse its completed steps when it was created for this exact input and these
    // settings. A directory with no fingerprint was left by an older mkvdovi, so it resumes with
    // a warning, unless a non-preset HLG composer is selected (its RPU.bin would carry the
    // preset). A directory with a different fingerprint is discarded.
    let mut resuming = resume_enabled && temp_dir.exists();
    if resuming && resume::is_legacy_hlg_dir(&temp_dir) {
        // Only the removed HLG-to-PQ (Profile 8.1) path wrote HLG_to_PQ.mkv. Its PQ base layer and
        // 8.1 RPU do not fit the Profile 8.4 conversion that replaced it, whatever the fingerprint
        // says (v0.4.0 already wrote fingerprints for those directories).
        progress::print_info(&format!(
            "Leftover temp dir '{}' comes from the removed HLG-to-PQ (Profile 8.1) conversion; starting clean.",
            temp_dir.display()
        ));
        let _ = fs::remove_dir_all(&temp_dir);
        temp_dir = fresh_temp_dir.clone();
        resuming = false;
    }
    if resuming {
        let status = fingerprint.map_or(resume::FingerprintStatus::Differs, |current| {
            current.check(&temp_dir)
        });
        match status {
            resume::FingerprintStatus::Matches => {}
            resume::FingerprintStatus::Missing if custom_composer.is_some() => {
                progress::print_warn(&format!(
                    "Leftover temp dir '{}' has no resume fingerprint (created by an older mkvdovi), so its RPU may carry another HLG composer than --hlg-composer {}; starting clean.",
                    temp_dir.display(),
                    requested_composer.cli_name()
                ));
                let _ = fs::remove_dir_all(&temp_dir);
                temp_dir = fresh_temp_dir;
                resuming = false;
            }
            resume::FingerprintStatus::Missing => progress::print_warn(&format!(
                "Leftover temp dir '{}' has no resume fingerprint (created by an older mkvdovi); resuming its completed steps, which may carry metadata from that version. Pass --no-resume to start clean.",
                temp_dir.display()
            )),
            resume::FingerprintStatus::Differs => {
                progress::print_warn(&format!(
                    "Leftover temp dir '{}' was created for a different input, settings, or mkvdovi version; starting clean.",
                    temp_dir.display()
                ));
                let _ = fs::remove_dir_all(&temp_dir);
                temp_dir = fresh_temp_dir;
                resuming = false;
            }
        }
    }
    (temp_dir, resuming)
}

/// Creates (or resets) the temp directory and writes the resume fingerprint into it.
pub(super) fn prepare_temp_dir(
    temp_dir: &Path,
    resuming: bool,
    fingerprint: Option<&resume::Fingerprint>,
) -> Result<()> {
    if temp_dir.exists() && !resuming {
        let _ = fs::remove_dir_all(temp_dir);
    }
    fs::create_dir_all(temp_dir).context("Failed to create temp directory")?;
    if let Some(fingerprint) = fingerprint {
        fingerprint
            .write(temp_dir)
            .context("Failed to write resume fingerprint")?;
    }
    if resuming {
        progress::print_info("Resuming from a previous run — completed steps will be reused.");
    }
    Ok(())
}
