//! The authored CM v4.0 trim blocks of the muxed output's RPU (ROADMAP E17.1).
//!
//! A Dolby Vision input passes its L8 trims and L10 custom target displays through (the Profile 7
//! MEL fast path), so `--verify` checks them on every frame: no two L8 blocks of a frame may
//! name the same `target_display_index`, no two L10 blocks either, and every L8 target must be a
//! preset display or an L10 block of the same frame. `dolby_vision` 3.4.0 checks none of these
//! when it parses: `CmV40DmData::validate` only counts blocks per level, and the parser pushes
//! every block it reads without looking for a duplicate.
//!
//! Not checked here: whether an L8 block's optional fields agree with its length. The crate
//! covers it at parse time. It reads the fields a length implies (`ExtMetadataBlockLevel8::parse`
//! reads mid contrast above 10 bytes, clip trim above 12, the saturation vector above 13, the
//! hue vector above 19) and requires every bit after them to be zero
//! (`ExtMetadataBlock::validate_and_read_remaining`), so a parsed block carries exactly the
//! fields of its length. A length outside 10, 12, 13, 19 and 25 aborts the crate
//! (`unreachable!` in `required_bits`), which is why the output RPU is parsed through
//! [`crate::rpu_guard`]. Re-check both when `dolby_vision` is bumped.

use colored::Colorize;
use dolby_vision::rpu::dovi_rpu::DoviRpu;
use dolby_vision::rpu::extension_metadata::blocks::level10::PRESET_TARGET_DISPLAYS;
use dolby_vision::rpu::extension_metadata::blocks::ExtMetadataBlock;
use std::path::Path;

use super::MAX_LISTED_MISMATCHES;
use crate::rpu_guard;

/// One inconsistency in the CM v4.0 blocks of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Problem {
    /// Two or more L8 blocks trim for the same target display.
    DuplicateL8 { target: u8 },
    /// Two or more L10 blocks define the same custom target display.
    DuplicateL10 { target: u8 },
    /// An L8 block's target is neither a preset display nor an L10 block of the frame.
    UnresolvedL8 { target: u8 },
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::DuplicateL8 { target } => {
                write!(f, "more than one L8 block for target display {target}")
            }
            Self::DuplicateL10 { target } => {
                write!(f, "more than one L10 block for target display {target}")
            }
            Self::UnresolvedL8 { target } => write!(
                f,
                "an L8 block for target display {target}, which is neither a preset display nor \
                 an L10 block of the frame"
            ),
        }
    }
}

/// A problem with the frames (0-based, in RPU file order) that carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Finding {
    pub problem: Problem,
    pub first_frame: usize,
    pub frames: usize,
}

/// Whether an L8 `target_display_index` names a preset display, one that needs no L10 block:
/// the crate's preset list (`PRESET_TARGET_DISPLAYS`, the indices an L10 block may not take)
/// and 24 and 25, which `ExtMetadataBlockLevel8::trim_target_nits` maps to 300 nits (owner
/// decision 2026-11-15).
fn is_preset(target: u8) -> bool {
    PRESET_TARGET_DISPLAYS.contains(&target) || matches!(target, 24 | 25)
}

/// The problems of one frame's CM v4.0 blocks, each problem once. A frame without CM v4.0
/// metadata has none.
pub(super) fn frame_problems(rpu: &DoviRpu) -> Vec<Problem> {
    let Some(blocks) = rpu
        .vdr_dm_data
        .as_ref()
        .and_then(|dm| dm.metadata_blocks(8))
    else {
        return Vec::new();
    };
    let l8: Vec<u8> = blocks
        .iter()
        .filter_map(|block| match block {
            ExtMetadataBlock::Level8(b) => Some(b.target_display_index),
            _ => None,
        })
        .collect();
    let l10: Vec<u8> = blocks
        .iter()
        .filter_map(|block| match block {
            ExtMetadataBlock::Level10(b) => Some(b.target_display_index),
            _ => None,
        })
        .collect();
    let mut problems = Vec::new();
    let mut push = |problem| {
        if !problems.contains(&problem) {
            problems.push(problem);
        }
    };
    for (i, &target) in l8.iter().enumerate() {
        if l8[..i].contains(&target) {
            push(Problem::DuplicateL8 { target });
        }
        if !is_preset(target) && !l10.contains(&target) {
            push(Problem::UnresolvedL8 { target });
        }
    }
    for (i, &target) in l10.iter().enumerate() {
        if l10[..i].contains(&target) {
            push(Problem::DuplicateL10 { target });
        }
    }
    problems
}

/// Every problem of `rpus`, in the order it first appears, with its first frame and its count
/// of frames.
pub(super) fn check(rpus: &[DoviRpu]) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    for (frame, rpu) in rpus.iter().enumerate() {
        for problem in frame_problems(rpu) {
            match findings.iter_mut().find(|f| f.problem == problem) {
                Some(finding) => finding.frames += 1,
                None => findings.push(Finding {
                    problem,
                    first_frame: frame,
                    frames: 1,
                }),
            }
        }
    }
    findings
}

/// Checks the authored blocks of the RPU extracted from the output and prints the result.
/// `parsed` is the RPU already parsed by the caller (a generated RPU); without it the file at
/// `rpu_path` is parsed through [`rpu_guard`]. Returns `false` only on a problem: an RPU the
/// crate cannot parse, on any frame, skips the checks for the whole RPU with a warning. Any
/// parse error skips, not only a guard refusal: a passed-through RPU can carry L253, which
/// `dolby_vision` 3.4.0 refuses with a plain error ("Unknown metadata block found",
/// `cmv40.rs:56-64`), and failing on it would fail valid Profile 7 MEL outputs. A malformed
/// RPU still fails `--verify` through the `dovi_tool` checks.
pub(super) fn verify(rpu_path: &Path, parsed: Option<&[DoviRpu]>) -> bool {
    let owned;
    let rpus = match parsed {
        Some(rpus) => rpus,
        None => match rpu_guard::parse_source_rpu_file(rpu_path) {
            Ok(rpus) => {
                owned = rpus;
                &owned
            }
            Err(error) => {
                println!(
                    "{}",
                    format!(
                        "Cannot parse the output RPU ({error:#}); skipping the L8 and L10 checks."
                    )
                    .yellow()
                );
                return true;
            }
        },
    };
    let findings = check(rpus);
    if findings.is_empty() {
        println!(
            "L8 and L10 blocks: consistent on all {} frames.",
            rpus.len()
        );
        return true;
    }
    for finding in findings.iter().take(MAX_LISTED_MISMATCHES) {
        let more = match finding.frames {
            1 => String::new(),
            n => format!(" (and {} more frames)", n - 1),
        };
        println!(
            "{}",
            format!(
                "RPU frame {}{more}: {}.",
                finding.first_frame, finding.problem
            )
            .red()
        );
    }
    if findings.len() > MAX_LISTED_MISMATCHES {
        println!(
            "{}",
            format!(
                "... and {} more L8/L10 problems.",
                findings.len() - MAX_LISTED_MISMATCHES
            )
            .red()
        );
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use dolby_vision::rpu::extension_metadata::blocks::{
        ExtMetadataBlockLevel1, ExtMetadataBlockLevel10, ExtMetadataBlockLevel8,
    };
    use dolby_vision::rpu::generate::{GenerateConfig, VideoShot};

    use crate::rpu_guard::fixtures::{linear_interp_rpu_bin, rpu_bin};

    fn l8(target: u8, length: u64) -> ExtMetadataBlock {
        ExtMetadataBlock::Level8(ExtMetadataBlockLevel8 {
            length,
            target_display_index: target,
            ..Default::default()
        })
    }

    /// A custom target display of 600 nits: the short form with a primaries index, or the long
    /// form with explicit primaries (BT.2020 in 0.00002 units).
    fn l10(target: u8, long: bool) -> ExtMetadataBlock {
        let mut block = ExtMetadataBlockLevel10 {
            target_display_index: target,
            target_max_pq: 2851,
            target_min_pq: 62,
            target_primary_index: 2,
            ..Default::default()
        };
        if long {
            block.length = 21;
            block.target_primary_index = 255;
            block.target_primary_red_x = 35400;
            block.target_primary_red_y = 14600;
            block.target_primary_green_x = 8500;
            block.target_primary_green_y = 39850;
            block.target_primary_blue_x = 6550;
            block.target_primary_blue_y = 2300;
            block.target_primary_white_x = 15635;
            block.target_primary_white_y = 16450;
        }
        ExtMetadataBlock::Level10(block)
    }

    /// The blocks of a well-formed authored frame: L8 trims for the 100-nit default (1), the
    /// 300-nit presets 24 and 25 and two custom displays, each custom one with its L10 block,
    /// and every L8 length the crate knows.
    fn authored() -> Vec<ExtMetadataBlock> {
        vec![
            l8(1, 25),
            l8(24, 10),
            l8(25, 12),
            l8(3, 13),
            l8(20, 19),
            l10(3, false),
            l10(20, true),
        ]
    }

    /// CM v4.0 Profile 8.1 RPUs, one per frame, each with the given extra blocks added as they
    /// stand (the crate's `add_metadata_block` keeps duplicates), then written as `dovi_tool
    /// extract-rpu` writes an `RPU.bin`, for the guarded parse `--verify` runs on it.
    fn round_trip(frames: &[Vec<ExtMetadataBlock>]) -> (tempfile::TempDir, std::path::PathBuf) {
        let config = GenerateConfig {
            length: frames.len(),
            shots: vec![VideoShot {
                start: 0,
                duration: frames.len(),
                metadata_blocks: vec![ExtMetadataBlock::Level1(ExtMetadataBlockLevel1::new(
                    0, 2081, 1000,
                ))],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut payloads = Vec::new();
        for (mut rpu, blocks) in config.generate_rpu_list().unwrap().into_iter().zip(frames) {
            let dm = rpu.vdr_dm_data.as_mut().unwrap();
            for block in blocks {
                dm.add_metadata_block(block.clone()).unwrap();
            }
            payloads.push(rpu.write_rpu().unwrap());
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("RPU.bin");
        std::fs::write(&path, rpu_bin(&payloads)).unwrap();
        (dir, path)
    }

    fn parsed(frames: &[Vec<ExtMetadataBlock>]) -> Vec<DoviRpu> {
        let (_dir, path) = round_trip(frames);
        rpu_guard::parse_source_rpu_file(&path).unwrap()
    }

    fn targets(rpu: &DoviRpu, level: u8) -> Vec<u8> {
        rpu.vdr_dm_data
            .as_ref()
            .unwrap()
            .metadata_blocks(8)
            .unwrap()
            .iter()
            .filter_map(|block| match block {
                ExtMetadataBlock::Level8(b) if level == 8 => Some(b.target_display_index),
                ExtMetadataBlock::Level10(b) if level == 10 => Some(b.target_display_index),
                _ => None,
            })
            .collect()
    }

    fn finding(problem: Problem, first_frame: usize, frames: usize) -> Finding {
        Finding {
            problem,
            first_frame,
            frames,
        }
    }

    #[test]
    fn an_authored_rpu_and_a_generated_one_pass() {
        let (_dir, path) = round_trip(&[authored(), authored(), Vec::new()]);
        let rpus = rpu_guard::parse_source_rpu_file(&path).unwrap();
        assert_eq!(targets(&rpus[0], 8), [1, 3, 20, 24, 25]);
        assert_eq!(targets(&rpus[0], 10), [3, 20]);
        assert_eq!(check(&rpus), []);
        assert!(verify(&path, None));
        assert!(verify(&path, Some(&rpus)));
    }

    #[test]
    fn a_duplicate_l8_target_fails() {
        // The crate writes at most five L8 blocks: drop the 25 trim for the duplicate.
        let mut duplicated = authored();
        duplicated.remove(2);
        duplicated.push(l8(24, 25));
        let (_dir, path) = round_trip(&[authored(), duplicated]);
        let rpus = rpu_guard::parse_source_rpu_file(&path).unwrap();
        // The crate parses both blocks: the check is not covered at parse time.
        assert_eq!(targets(&rpus[1], 8), [1, 3, 20, 24, 24]);
        assert_eq!(
            check(&rpus),
            [finding(Problem::DuplicateL8 { target: 24 }, 1, 1)]
        );
        assert!(!verify(&path, None));
    }

    #[test]
    fn a_duplicate_l10_target_fails() {
        let mut duplicated = authored();
        duplicated.push(l10(3, true));
        let (_dir, path) = round_trip(&[duplicated, authored()]);
        let rpus = rpu_guard::parse_source_rpu_file(&path).unwrap();
        assert_eq!(targets(&rpus[0], 10), [3, 3, 20]);
        assert_eq!(
            check(&rpus),
            [finding(Problem::DuplicateL10 { target: 3 }, 0, 1)]
        );
        assert!(!verify(&path, None));
    }

    #[test]
    fn an_l8_target_needs_a_preset_or_an_l10_block_of_its_frame() {
        // 1, 24 and 25 are presets; 20 is not, and passes only with its L10 block.
        assert_eq!(
            check(&parsed(&[vec![l8(1, 10), l8(24, 10), l8(25, 10)]])),
            []
        );
        assert_eq!(check(&parsed(&[vec![l8(20, 10), l10(20, false)]])), []);
        assert_eq!(
            check(&parsed(&[vec![l8(20, 10)]])),
            [finding(Problem::UnresolvedL8 { target: 20 }, 0, 1)]
        );
        // The L10 block of another frame does not resolve the target.
        assert_eq!(
            check(&parsed(&[
                vec![l8(20, 10), l10(20, false)],
                vec![l8(20, 10)],
                vec![l8(20, 10), l10(3, false)],
            ])),
            [finding(Problem::UnresolvedL8 { target: 20 }, 1, 2)]
        );
    }

    #[test]
    fn findings_are_counted_per_problem_across_frames() {
        let mut both = authored();
        both.remove(2);
        both.push(l8(1, 10));
        both.push(l10(20, false));
        let rpus = parsed(&[authored(), both.clone(), both, vec![l8(5, 10)]]);
        assert_eq!(
            check(&rpus),
            [
                finding(Problem::DuplicateL8 { target: 1 }, 1, 2),
                finding(Problem::DuplicateL10 { target: 20 }, 1, 2),
                finding(Problem::UnresolvedL8 { target: 5 }, 3, 1),
            ]
        );
    }

    /// A frame after frame 0 that the crate refuses (a broken CRC) must fail the whole parse,
    /// so that the checks are skipped with a warning; `parse_rpu_file` would keep only the RPUs
    /// before it and the later frames would go unchecked.
    #[test]
    fn a_frame_after_the_first_that_does_not_parse_fails_the_whole_parse() {
        let (_dir, path) = round_trip(&[vec![], vec![], vec![l8(1, 10), l8(1, 10)]]);
        let bytes = std::fs::read(&path).unwrap();
        let starts: Vec<usize> = bytes
            .windows(4)
            .enumerate()
            .filter(|(_, window)| *window == [0, 0, 0, 1])
            .map(|(at, _)| at)
            .collect();
        assert_eq!(starts.len(), 3);
        // Flip a bit of frame 1's CRC: the byte before its trailing 0x80.
        let mut broken = bytes.clone();
        broken[starts[2] - 2] ^= 0x01;
        assert!(DoviRpu::parse_unspec62_nalu(&broken[starts[1] + 4..starts[2]]).is_err());
        assert!(DoviRpu::parse_unspec62_nalu(&broken[starts[2] + 4..]).is_ok());
        let broken_path = path.with_file_name("broken.bin");
        std::fs::write(&broken_path, &broken).unwrap();
        assert!(
            rpu_guard::parse_source_rpu_file(&broken_path).is_err(),
            "a parse that drops frames 1 and 2 hides frame 2's duplicate L8"
        );
        assert!(verify(&broken_path, None), "skipped with a warning");
        assert_eq!(rpu_guard::parse_source_rpu_file(&path).unwrap().len(), 3);
        assert!(!verify(&path, None), "frame 2's duplicate L8 fails");
    }

    #[test]
    fn an_rpu_the_crate_cannot_parse_skips_the_checks() {
        let dir = tempfile::tempdir().unwrap();
        // A crate limit: the guard refuses the RPU before the crate would abort.
        let limit = dir.path().join("limit.bin");
        std::fs::write(&limit, linear_interp_rpu_bin()).unwrap();
        assert!(rpu_guard::is_unparseable(
            &rpu_guard::parse_source_rpu_file(&limit).unwrap_err()
        ));
        assert!(verify(&limit, None));
        // An RPU the crate refuses with an error: one byte short of its CRC.
        let (_rpu_dir, path) = round_trip(&[vec![l8(20, 10)]]);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(bytes.len() - 2);
        bytes.push(0x80);
        let refused = dir.path().join("refused.bin");
        std::fs::write(&refused, bytes).unwrap();
        assert!(rpu_guard::parse_source_rpu_file(&refused).is_err());
        assert!(verify(&refused, None));
    }
}
