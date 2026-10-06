//! l1_diff on a synthetic open-GOP run: a measurement file of `DECODED` frames whose v5 sidecar
//! records `LEADING` undecodable pictures in front, scored against references in stream and in
//! decoded-frame coordinates.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use madvr_parse::{MadVRFrame, MadVRHeader, MadVRMeasurements, MadVRScene};
use serde_json::json;

const DECODED: usize = 10;
const LEADING: usize = 2;
const STREAM: usize = DECODED + LEADING;
/// Our scene starts, in decoded frames.
const OUR_SCENES: [usize; 3] = [0, 3, 7];

/// The `.bin` stores a peak as a multiple of 1/64000, so build peaks from such multiples: the
/// reference then carries exactly what we read back.
fn peak_pq(frame: usize) -> f64 {
    (20000 + 1000 * frame) as f64 / 64000.0
}

fn peak_code(frame: usize) -> f64 {
    peak_pq(frame) * 4095.0
}

fn min_code(frame: usize) -> u16 {
    10 + frame as u16
}

fn avg_code(frame: usize) -> u16 {
    500 + 10 * frame as u16
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write `ours.bin` and its sidecar; `source` is the sidecar's source block.
fn write_run(dir: &Path, version: u32, source: serde_json::Value) -> PathBuf {
    let mut lum_histogram = vec![0.0; 256];
    lum_histogram[128] = 100.0;
    let frames = (0..DECODED)
        .map(|frame| MadVRFrame {
            peak_pq_2020: peak_pq(frame),
            lum_histogram: lum_histogram.clone(),
            hue_histogram: Some(vec![0.0; 31]),
            ..Default::default()
        })
        .collect();
    let scenes = OUR_SCENES
        .iter()
        .enumerate()
        .map(|(index, &start)| MadVRScene {
            start: start as u32,
            end: OUR_SCENES.get(index + 1).map_or(DECODED, |&next| next) as u32 - 1,
            peak_nits: 1000,
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let measurements = MadVRMeasurements {
        header: MadVRHeader {
            version: 5,
            header_size: 32,
            scene_count: scenes.len() as u32,
            frame_count: DECODED as u32,
            flags: 2,
            maxcll: 1000,
            ..Default::default()
        },
        scenes,
        frames,
    };
    let bin = dir.join("ours.bin");
    fs::write(&bin, measurements.write_measurements().unwrap()).unwrap();
    let sidecar = json!({
        "version": version,
        "min_percentile": 0.01,
        "source": source,
        "frames": {
            "min_pq_12bit": (0..DECODED).map(min_code).collect::<Vec<_>>(),
            "avg_luma_pq_12bit": (0..DECODED).map(avg_code).collect::<Vec<_>>(),
            "avg_max_rgb_pq_12bit": (0..DECODED).map(avg_code).collect::<Vec<_>>(),
        },
    });
    fs::write(dir.join("ours.bin.l1.json"), sidecar.to_string()).unwrap();
    bin
}

fn v5_source(stream: usize, leading: usize) -> serde_json::Value {
    json!({ "file_name": "cut.mkv", "stream_frames": stream, "leading_skipped_frames": leading })
}

/// A reference CSV: one row per label; `ours` gives our frame for a label, rows without one
/// carry values no frame of ours has.
fn write_reference(
    path: &Path,
    labels: std::ops::Range<usize>,
    ours: impl Fn(usize) -> Option<usize>,
) {
    let mut csv = String::from("frame,min_pq,max_pq,avg_pq\n");
    for label in labels {
        let row = match ours(label) {
            Some(frame) => format!(
                "{label},{},{},{}\n",
                min_code(frame),
                peak_code(frame),
                avg_code(frame)
            ),
            None => format!("{label},4000,4000,4000\n"),
        };
        csv.push_str(&row);
    }
    fs::write(path, csv).unwrap();
}

fn stream_to_ours(label: usize) -> Option<usize> {
    label.checked_sub(LEADING)
}

fn l1_diff(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_l1_diff"))
        .args(args)
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

const ZERO_LIMITS: [&str; 14] = [
    "--max-peak-bias",
    "0.01",
    "--max-peak-error",
    "0.01",
    "--max-min-bias",
    "0",
    "--max-min-error",
    "0",
    "--max-avg-bias",
    "0",
    "--max-avg-error",
    "0",
    "--max-scene-mismatches",
    "0",
];

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn a_stream_reference_scores_after_the_leading_pictures() {
    let dir = scratch("stream_reference");
    let bin = write_run(&dir, 5, v5_source(STREAM, LEADING));
    let reference = dir.join("reference.csv");
    write_reference(&reference, 0..STREAM, stream_to_ours);
    // Stream shot list with the dovi_tool sentinel; cuts without the first line and sentinel.
    let shots = dir.join("shots.txt");
    let stream_starts: Vec<usize> = OUR_SCENES.iter().map(|start| start + LEADING).collect();
    fs::write(
        &shots,
        format!("0\n{}\n{}\n{STREAM}\n", stream_starts[1], stream_starts[2]),
    )
    .unwrap();
    let cuts = dir.join("cuts.txt");
    fs::write(
        &cuts,
        format!("{}\n{}\n", stream_starts[1], stream_starts[2]),
    )
    .unwrap();
    let csv = dir.join("deltas.csv");

    let mut args = vec![
        "--ours",
        p(&bin),
        "--reference",
        p(&reference),
        "--scenes",
        p(&cuts),
        "--csv",
        p(&csv),
    ];
    args.extend(ZERO_LIMITS);
    let output = l1_diff(&args);
    assert!(output.status.success(), "{}", text(&output));
    let out = text(&output);
    assert!(out.contains("ours:      "), "{out}");
    assert!(out.contains("(12 frames)"), "{out}");
    assert!(
        out.contains("10 decoded + 2 leading = 12 stream; reference in stream coordinates"),
        "{out}"
    );
    assert!(
        out.contains("2 reference frames describe undecodable leading pictures"),
        "{out}"
    );
    assert!(out.contains("reference cuts matched by ours: 2/2"), "{out}");

    let deltas = fs::read_to_string(&csv).unwrap();
    let labels: Vec<&str> = deltas
        .lines()
        .skip(1)
        .map(|line| line.split(',').next().unwrap())
        .collect();
    assert_eq!(labels.len(), DECODED);
    assert_eq!(labels[0], "2");
    assert_eq!(labels[DECODED - 1], "11");

    let mut args = vec![
        "--ours",
        p(&bin),
        "--reference",
        p(&reference),
        "--per-shot",
        p(&shots),
    ];
    args.extend(&ZERO_LIMITS[..12]);
    let output = l1_diff(&args);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("per-shot (3 shots)"),
        "{}",
        text(&output)
    );
}

#[test]
fn an_export_is_labelled_with_stream_frames_and_reads_back() {
    let dir = scratch("export");
    let bin = write_run(&dir, 5, v5_source(STREAM, LEADING));
    let export = dir.join("export.csv");
    let output = l1_diff(&["--ours", p(&bin), "--export-reference", p(&export)]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("labelled from stream frame 2"),
        "{}",
        text(&output)
    );
    let csv = fs::read_to_string(&export).unwrap();
    let first = csv.lines().nth(1).unwrap();
    assert!(first.starts_with("2,"), "{first}");
    assert_eq!(csv.lines().count(), DECODED + 1);

    let cuts = dir.join("cuts.txt");
    fs::write(
        &cuts,
        format!("{}\n{}\n", OUR_SCENES[1] + LEADING, OUR_SCENES[2] + LEADING),
    )
    .unwrap();
    let mut args = vec![
        "--ours",
        p(&bin),
        "--reference",
        p(&export),
        "--scenes",
        p(&cuts),
    ];
    args.extend(ZERO_LIMITS);
    let output = l1_diff(&args);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("reference cuts matched by ours: 2/2"),
        "{}",
        text(&output)
    );
}

#[test]
fn an_export_without_leading_pictures_is_unchanged() {
    let dir = scratch("export_closed_gop");
    let bin = write_run(&dir, 5, v5_source(DECODED, 0));
    let export = dir.join("export.csv");
    let output = l1_diff(&["--ours", p(&bin), "--export-reference", p(&export)]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("Reference CSV written to {} (10 frames)", p(&export))
    );
    assert!(fs::read_to_string(&export)
        .unwrap()
        .lines()
        .nth(1)
        .unwrap()
        .starts_with("0,"));
}

#[test]
fn a_decoded_frame_reference_keeps_its_coordinates_but_refuses_lists() {
    let dir = scratch("decoded_reference");
    let bin = write_run(&dir, 5, v5_source(STREAM, LEADING));
    let reference = dir.join("reference.csv");
    write_reference(&reference, 0..DECODED, Some);
    let mut args = vec!["--ours", p(&bin), "--reference", p(&reference)];
    args.extend(&ZERO_LIMITS[..12]);
    let output = l1_diff(&args);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("decoded-frame coordinates"),
        "{}",
        text(&output)
    );
    assert!(text(&output).contains("warning:"), "{}", text(&output));

    let cuts = dir.join("cuts.txt");
    fs::write(&cuts, "3\n7\n").unwrap();
    let output = l1_diff(&[
        "--ours",
        p(&bin),
        "--reference",
        p(&reference),
        "--scenes",
        p(&cuts),
    ]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("re-export the reference"),
        "{}",
        text(&output)
    );
}

#[test]
fn cuts_beyond_the_stream_are_refused() {
    for (leading, name) in [(LEADING, "cuts_beyond_open"), (0, "cuts_beyond_closed")] {
        let dir = scratch(name);
        let bin = write_run(&dir, 5, v5_source(DECODED + leading, leading));
        let reference = dir.join("reference.csv");
        write_reference(&reference, 0..DECODED + leading, |label| {
            label.checked_sub(leading)
        });
        let cuts = dir.join("cuts.txt");
        fs::write(&cuts, format!("5\n{}\n", DECODED + leading + 1)).unwrap();
        let output = l1_diff(&[
            "--ours",
            p(&bin),
            "--reference",
            p(&reference),
            "--scenes",
            p(&cuts),
        ]);
        assert!(
            !output.status.success(),
            "leading {leading}: {}",
            text(&output)
        );
        assert!(
            text(&output).contains("outside the reference's frames"),
            "{}",
            text(&output)
        );
    }
}

#[test]
fn count_differences_and_inconsistent_sidecars_are_refused() {
    let dir = scratch("refused");
    let bin = write_run(&dir, 5, v5_source(STREAM, LEADING));
    let reference = dir.join("reference.csv");
    write_reference(&reference, 0..STREAM + 1, stream_to_ours);
    let output = l1_diff(&["--ours", p(&bin), "--reference", p(&reference)]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("frame count mismatch"),
        "{}",
        text(&output)
    );

    // A sidecar whose stream count does not fit the measurement file, for scoring and export.
    let bin = write_run(&dir, 5, v5_source(STREAM + 1, LEADING));
    write_reference(&reference, 0..STREAM, stream_to_ours);
    let output = l1_diff(&["--ours", p(&bin), "--reference", p(&reference)]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("does not describe"),
        "{}",
        text(&output)
    );
    let export = dir.join("export.csv");
    let output = l1_diff(&["--ours", p(&bin), "--export-reference", p(&export)]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("does not describe"),
        "{}",
        text(&output)
    );

    // A v5 sidecar without the stream fields.
    let bin = write_run(&dir, 5, json!({ "file_name": "cut.mkv" }));
    let output = l1_diff(&["--ours", p(&bin), "--reference", p(&reference)]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("does not record"),
        "{}",
        text(&output)
    );
}

#[test]
fn an_older_sidecar_with_a_stream_reference_gets_the_hint() {
    let dir = scratch("hint");
    let bin = write_run(&dir, 4, json!({ "file_name": "cut.mkv" }));
    let reference = dir.join("reference.csv");
    write_reference(&reference, 0..STREAM, stream_to_ours);
    let output = l1_diff(&["--ours", p(&bin), "--reference", p(&reference)]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("writes sidecar v5"),
        "{}",
        text(&output)
    );
}
