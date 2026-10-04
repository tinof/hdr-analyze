//! Replace the composer of a generated Profile 8.4 `RPU.bin`, leaving everything else intact.
//!
//! `dovi_tool generate` always writes the preset composer. [`rewrite_rpu_file`] parses its
//! output with the `dolby_vision` crate, checks that the crate reproduces the file byte for
//! byte, swaps `rpu_data_mapping` on every frame and writes the result atomically.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use dolby_vision::rpu::dovi_rpu::DoviRpu;
use dolby_vision::rpu::utils::parse_rpu_file;

use crate::{mappings_equal, Composer, COEFFICIENT_LOG2_DENOM};

/// Start code before each RPU in an `RPU.bin`, as `dovi_tool` writes it
/// (`GenerateConfig::write_rpus`).
const RPU_START_CODE: [u8; 4] = [0, 0, 0, 1];

/// Serialize RPUs in the `RPU.bin` layout: the start code, then the HEVC unspec62 NAL
/// without its 2-byte NAL header.
fn encode_rpu_file(rpus: &[DoviRpu]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (index, rpu) in rpus.iter().enumerate() {
        let nal = rpu
            .write_hevc_unspec62_nalu()
            .with_context(|| format!("encoding RPU frame {index}"))?;
        out.extend_from_slice(&RPU_START_CODE);
        out.extend_from_slice(&nal[2..]);
    }
    Ok(out)
}

/// Header conditions under which a composer can be installed: fixed-point coefficients with
/// the denominator the composers were quantized to, and a mapping signalled on this frame.
fn check_header(rpu: &DoviRpu, index: usize) -> Result<()> {
    let header = &rpu.header;
    ensure!(
        header.coefficient_data_type == 0,
        "RPU frame {index}: coefficient_data_type {} (expected fixed point, 0)",
        header.coefficient_data_type
    );
    ensure!(
        u32::try_from(header.coefficient_log2_denom).ok() == Some(COEFFICIENT_LOG2_DENOM)
            && u32::try_from(header.coefficient_log2_denom_length).ok()
                == Some(COEFFICIENT_LOG2_DENOM),
        "RPU frame {index}: coefficient_log2_denom {} / length {} (expected {COEFFICIENT_LOG2_DENOM})",
        header.coefficient_log2_denom,
        header.coefficient_log2_denom_length
    );
    ensure!(
        !header.use_prev_vdr_rpu_flag,
        "RPU frame {index}: use_prev_vdr_rpu_flag is set, so the frame carries no mapping"
    );
    ensure!(
        rpu.rpu_data_mapping.is_some(),
        "RPU frame {index}: no rpu_data_mapping"
    );
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".composer.tmp");
    path.with_file_name(name)
}

/// Install `composer` into every frame of the `RPU.bin` at `path`, in place.
///
/// Refuses (and leaves the file untouched) when the file does not survive an unchanged
/// round trip through the `dolby_vision` crate byte for byte, or when a frame's header does
/// not allow the swap (see [`check_header`]). The new file is written next to the old one,
/// re-parsed and checked (frame count, mapping on every frame, CRC through the parser), and
/// only then renamed over `path`. Returns the number of frames.
pub fn rewrite_rpu_file(path: &Path, composer: Composer) -> Result<usize> {
    let original =
        fs::read(path).with_context(|| format!("reading RPU file {}", path.display()))?;
    let mut rpus =
        parse_rpu_file(path).with_context(|| format!("parsing RPU file {}", path.display()))?;
    ensure!(!rpus.is_empty(), "{} contains no RPUs", path.display());
    let preset = Composer::Preset.rpu_data_mapping();
    for (index, rpu) in rpus.iter().enumerate() {
        check_header(rpu, index)?;
        // Only ever swap the preset `dovi_tool generate` writes, so no other mapping field
        // (partitions, colour space, NLQ) is silently replaced.
        ensure!(
            mappings_equal(
                rpu.rpu_data_mapping
                    .as_ref()
                    .expect("checked by check_header"),
                &preset
            ),
            "RPU frame {index} does not carry the preset composer ({})",
            path.display()
        );
    }
    if encode_rpu_file(&rpus)? != original {
        bail!(
            "{} does not survive an unchanged round trip through the dolby_vision crate byte for \
             byte; refusing to rewrite its composer",
            path.display()
        );
    }

    let mapping = composer.rpu_data_mapping();
    for rpu in &mut rpus {
        rpu.rpu_data_mapping = Some(mapping.clone());
        rpu.modified = true;
    }
    let encoded = encode_rpu_file(&rpus)?;

    let temporary = temporary_path(path);
    fs::write(&temporary, &encoded).with_context(|| format!("writing {}", temporary.display()))?;
    let checked = check_rpu_file(&temporary, composer).and_then(|frames| {
        ensure!(
            frames == rpus.len(),
            "rewritten RPU has {frames} frames, expected {}",
            rpus.len()
        );
        Ok(frames)
    });
    if let Err(error) = checked {
        let _ = fs::remove_file(&temporary);
        return Err(error.context(format!("checking rewritten {}", temporary.display())));
    }
    fs::rename(&temporary, path)
        .with_context(|| format!("replacing {} with the rewritten RPU", path.display()))?;
    Ok(rpus.len())
}

/// Check that every frame of the `RPU.bin` at `path` carries `composer`'s mapping. Returns
/// the number of frames.
pub fn check_rpu_file(path: &Path, composer: Composer) -> Result<usize> {
    let rpus =
        parse_rpu_file(path).with_context(|| format!("parsing RPU file {}", path.display()))?;
    check_rpus(&rpus, composer).with_context(|| path.display().to_string())
}

/// Check that every frame of already parsed `rpus` carries `composer`'s mapping. Returns the
/// number of frames.
pub fn check_rpus(rpus: &[DoviRpu], composer: Composer) -> Result<usize> {
    ensure!(!rpus.is_empty(), "the RPU contains no frames");
    let expected = composer.rpu_data_mapping();
    for (index, rpu) in rpus.iter().enumerate() {
        check_header(rpu, index)?;
        let mapping = rpu
            .rpu_data_mapping
            .as_ref()
            .expect("checked by check_header");
        ensure!(
            mappings_equal(mapping, &expected),
            "RPU frame {index} does not carry the {} composer",
            composer.luminance_mapping()
        );
    }
    Ok(rpus.len())
}

#[cfg(test)]
mod tests {
    use dolby_vision::rpu::generate::{GenerateConfig, GenerateProfile, VideoShot};

    use super::*;

    fn generated_rpu_file(dir: &Path, frames: usize) -> PathBuf {
        let config = GenerateConfig {
            profile: GenerateProfile::Profile84,
            length: frames,
            shots: vec![VideoShot {
                start: 0,
                duration: frames,
                ..Default::default()
            }],
            ..Default::default()
        };
        let path = dir.join("RPU.bin");
        config.write_rpus(&path).unwrap();
        path
    }

    #[test]
    fn preset_rpu_carries_the_preset_composer() {
        let dir = tempfile::tempdir().unwrap();
        let path = generated_rpu_file(dir.path(), 3);
        assert_eq!(check_rpu_file(&path, Composer::Preset).unwrap(), 3);
        assert!(check_rpu_file(&path, Composer::Bt2100V1).is_err());
    }

    #[test]
    fn rewrite_installs_the_composer_on_every_frame_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = generated_rpu_file(dir.path(), 5);
        let before = parse_rpu_file(&path).unwrap();

        assert_eq!(rewrite_rpu_file(&path, Composer::Bt2100V1).unwrap(), 5);
        assert_eq!(check_rpu_file(&path, Composer::Bt2100V1).unwrap(), 5);
        assert!(!temporary_path(&path).exists());

        let after = parse_rpu_file(&path).unwrap();
        for (old, new) in before.iter().zip(&after) {
            assert_eq!(
                format!("{:?}", old.vdr_dm_data),
                format!("{:?}", new.vdr_dm_data)
            );
            assert_eq!(format!("{:?}", old.header), format!("{:?}", new.header));
        }

        // A file that no longer carries the preset is refused and left untouched.
        let rewritten = fs::read(&path).unwrap();
        let error = rewrite_rpu_file(&path, Composer::Preset).unwrap_err();
        assert!(
            format!("{error:#}").contains("preset composer"),
            "{error:#}"
        );
        assert_eq!(fs::read(&path).unwrap(), rewritten);
    }

    #[test]
    fn installing_the_preset_reproduces_the_generated_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = generated_rpu_file(dir.path(), 4);
        let original = fs::read(&path).unwrap();
        rewrite_rpu_file(&path, Composer::Preset).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn rewrite_refuses_a_file_that_does_not_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = generated_rpu_file(dir.path(), 2);
        let mut bytes = fs::read(&path).unwrap();
        // `parse_rpu_file` splits on 4-byte start codes only, so frame 0 (now behind a 3-byte
        // start code) is silently dropped; the identity check must catch the lost frame.
        bytes.remove(0);
        fs::write(&path, &bytes).unwrap();
        assert_eq!(parse_rpu_file(&path).unwrap().len(), 1);
        let error = rewrite_rpu_file(&path, Composer::Bt2100V1).unwrap_err();
        assert!(format!("{error:#}").contains("round trip"), "{error:#}");
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}
