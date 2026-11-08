use std::path::PathBuf;

use clap::Parser;

use super::*;
use crate::rpu_guard::fixtures::{rpu_bin, Rpu, MMR, POLY};

/// An `RPU.bin` of `count` RPUs as dovi_tool writes it.
fn rpus(count: usize) -> Vec<u8> {
    let payload = Rpu::p81(POLY, MMR, MMR).payload();
    rpu_bin(&vec![payload; count])
}

#[test]
fn a_successful_extraction_counts_the_rpus_of_its_file() {
    for count in [1, 2, 48] {
        assert_eq!(
            count_of_extraction(true, "", Some(&rpus(count))),
            Some(count)
        );
    }
    assert_eq!(count_of_extraction(true, "", Some(&[])), Some(0));
}

#[test]
fn an_extraction_that_finds_no_rpu_counts_zero_and_any_other_failure_counts_nothing() {
    let no_rpu = "Running command: ...\nError: No RPU was found in input file\n";
    assert_eq!(count_of_extraction(false, no_rpu, None), Some(0));
    // A file left by an earlier run does not count when dovi_tool failed.
    assert_eq!(count_of_extraction(false, no_rpu, Some(&rpus(3))), Some(0));
    let other = "Error: Invalid input file\n";
    assert_eq!(count_of_extraction(false, other, None), None);
    assert_eq!(count_of_extraction(false, other, Some(&rpus(3))), None);
    // A success without a file is no count either.
    assert_eq!(count_of_extraction(true, "", None), None);
}

#[test]
fn the_timing_checks_frame_count_is_used_without_reading_the_output() {
    let missing = PathBuf::from("/nonexistent/mkvdovi-e72/out.DV.mkv");
    assert_eq!(
        video_frames(&missing, Delivery::Checked { video_frames: 48 }),
        Ok(48)
    );
    // Without it the output is read, and a file nothing can read gives no count.
    assert!(video_frames(&missing, Delivery::Unchecked).is_err());
}

/// A check that cannot count fails closed: the output and its mux sentinel are removed, so the
/// next run muxes and counts again.
#[test]
fn an_output_whose_frames_cannot_be_counted_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (temp, output) = (dir.path().join("temp"), dir.path().join("out.DV.mkv"));
    fs::create_dir(&temp).unwrap();
    fs::write(&output, b"not a Matroska file").unwrap();
    fs::write(temp.join("mux.done"), b"").unwrap();
    fs::write(temp.join(RPU_FILE), rpus(48)).unwrap();
    let args = Args::try_parse_from(["mkvdovi"]).unwrap();
    assert!(!check(&output, &temp, Delivery::Unchecked, &args));
    assert!(!output.exists());
    assert!(!temp.join("mux.done").exists());
}
