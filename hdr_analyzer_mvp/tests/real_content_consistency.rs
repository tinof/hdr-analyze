//! Real-content consistency test, gated on an operator-supplied sample.
//!
//! Runs the analyzer twice on the same real video — default max-RGB peak domain and
//! `--peak-domain luma` — and asserts cross-run invariants that must hold for any
//! genuine HDR10 content: identical frame counts, peaks inside the PQ signal range,
//! the max-RGB peak dominating the luma peak, and more than one detected scene.
//!
//! Skips (with a note) unless `HDR_ANALYZE_REAL_SAMPLE` points at an existing video,
//! mirroring the environment-dependent skips in mkvdovi's integration tests.
//!
//! Optionally, when `HDR_ANALYZE_REFERENCE_CSV` (frame,min_pq,max_pq,avg_pq in 12-bit
//! PQ codes) and `HDR_ANALYZE_SHOTLIST` (0-based shot starts, optional trailing
//! sentinel equal to the frame count) are also set, the per-shot signed peak bias
//! against the reference must stay within 25 codes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn real_sample() -> Option<PathBuf> {
    let Ok(value) = std::env::var("HDR_ANALYZE_REAL_SAMPLE") else {
        eprintln!("Skipping: HDR_ANALYZE_REAL_SAMPLE is not set");
        return None;
    };
    let path = PathBuf::from(value);
    if !path.exists() {
        eprintln!(
            "Skipping: HDR_ANALYZE_REAL_SAMPLE does not exist: {}",
            path.display()
        );
        return None;
    }
    Some(path)
}

fn run_analyzer(sample: &Path, bin: &Path, extra_args: &[&str]) -> madvr_parse::MadVRMeasurements {
    let status = Command::new(env!("CARGO_BIN_EXE_hdr_analyzer_mvp"))
        .arg(sample)
        .arg("-o")
        .arg(bin)
        .args([
            "--peak-source",
            "max",
            "--header-peak-source",
            "max",
            "--disable-optimizer",
            "--no-crop",
        ])
        .args(extra_args)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .expect("run analyzer on real sample");
    assert!(status.success(), "analyzer failed on {}", sample.display());

    let data = std::fs::read(bin).expect("read measurements");
    madvr_parse::MadVRMeasurements::parse_measurements(&data).expect("parse measurements")
}

/// Parse a shotlist into per-shot ranges; panics on invalid input (test-only helper).
fn parse_shotlist(path: &Path, frame_count: usize) -> Vec<std::ops::Range<usize>> {
    let text = std::fs::read_to_string(path).expect("read shotlist");
    let mut starts: Vec<usize> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.parse().expect("shotlist frame number"))
        .collect();
    if starts.last() == Some(&frame_count) {
        starts.pop();
    }
    assert!(!starts.is_empty(), "shotlist contains no shot starts");
    assert_eq!(starts[0], 0, "shotlist must start at frame 0");
    assert!(
        starts.windows(2).all(|pair| pair[0] < pair[1]),
        "shotlist starts must be strictly increasing"
    );
    assert!(
        starts[starts.len() - 1] < frame_count,
        "shotlist start beyond frame count"
    );
    let mut ranges = Vec::with_capacity(starts.len());
    for (index, &start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(frame_count);
        ranges.push(start..end);
    }
    ranges
}

fn reference_max_pq(path: &Path) -> Vec<f64> {
    let text = std::fs::read_to_string(path).expect("read reference CSV");
    text.lines()
        .enumerate()
        .filter(|(i, line)| !(*i == 0 && line.starts_with("frame")) && !line.trim().is_empty())
        .map(|(_, line)| {
            let cols: Vec<&str> = line.split(',').collect();
            assert!(cols.len() >= 4, "reference CSV needs 4 columns");
            cols[2].trim().parse::<f64>().expect("max_pq column")
        })
        .collect()
}

#[test]
fn real_content_domains_are_consistent() {
    let Some(sample) = real_sample() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");

    let maxrgb_bin = dir.path().join("real_maxrgb.bin");
    let maxrgb = run_analyzer(&sample, &maxrgb_bin, &[]);

    let luma_bin = dir.path().join("real_luma.bin");
    let luma = run_analyzer(&sample, &luma_bin, &["--peak-domain", "luma"]);

    assert_eq!(
        maxrgb.frames.len(),
        luma.frames.len(),
        "both domains must measure the same frame count"
    );
    assert!(!maxrgb.frames.is_empty(), "sample produced no frames");

    let tolerance = 0.25 / 4095.0;
    for (i, (m, l)) in maxrgb.frames.iter().zip(&luma.frames).enumerate() {
        for (domain, peak) in [("max-rgb", m.peak_pq_2020), ("luma", l.peak_pq_2020)] {
            assert!(
                (0.0..=1.0).contains(&peak),
                "frame {i}: {domain} peak {peak} outside [0, 1]"
            );
        }
        assert!(
            m.peak_pq_2020 >= l.peak_pq_2020 - tolerance,
            "frame {i}: max-RGB peak {} below luma peak {}",
            m.peak_pq_2020,
            l.peak_pq_2020
        );
    }

    assert!(
        maxrgb.scenes.len() > 1,
        "real content must produce more than one scene (got {})",
        maxrgb.scenes.len()
    );

    let (Ok(reference_csv), Ok(shotlist)) = (
        std::env::var("HDR_ANALYZE_REFERENCE_CSV"),
        std::env::var("HDR_ANALYZE_SHOTLIST"),
    ) else {
        eprintln!("Reference bias check skipped: HDR_ANALYZE_REFERENCE_CSV / HDR_ANALYZE_SHOTLIST not set");
        return;
    };

    let reference = reference_max_pq(Path::new(&reference_csv));
    assert_eq!(
        reference.len(),
        maxrgb.frames.len(),
        "reference CSV frame count must match the sample"
    );
    let shots = parse_shotlist(Path::new(&shotlist), reference.len());

    let ours: Vec<f64> = maxrgb
        .frames
        .iter()
        .map(|frame| frame.peak_pq_2020 * 4095.0)
        .collect();
    let shot_max = |series: &[f64], range: &std::ops::Range<usize>| {
        series[range.clone()]
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
    };
    let bias = shots
        .iter()
        .map(|range| shot_max(&ours, range) - shot_max(&reference, range))
        .sum::<f64>()
        / shots.len() as f64;
    eprintln!(
        "per-shot signed peak bias vs reference: {bias:+.1} codes over {} shots",
        shots.len()
    );
    assert!(
        bias.abs() <= 25.0,
        "per-shot |peak bias| {:.1} codes exceeds 25 vs {}",
        bias.abs(),
        reference_csv
    );
}
