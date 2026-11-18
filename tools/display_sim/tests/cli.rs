//! display_sim end to end. The dumps are written here from the documented `pqhist` v1 layout
//! (docs/FORMAT_COMPATIBILITY.md), not by the analyzer; the expected displayed nits come from the
//! standalone libplacebo oracle (`harness6.c` row K, as in `src/sim_tests.rs`): reference L1 max
//! 2800, avg 1500 codes on a 600-nit display (a no-op, clipped at 600.007385 nits), candidate
//! L1 max 3200, avg 1500 (the spline compresses).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Bins 1000, 2000, 2500, 2900 and 3500 with their pixel counts.
const PIXELS: [(usize, u32); 5] = [(1000, 3), (2000, 2), (2500, 1), (2900, 1), (3500, 1)];
const REFERENCE_NITS: [f64; 5] = [4.74743128, 82.0855789, 269.51532, 600.007385, 600.007385];
const CANDIDATE_NITS: [f64; 5] = [3.84731364, 64.256958, 179.448563, 367.066376, 600.007385];
/// Displayed nits of 100 nits: reference (no-op), candidate.
const AT_100: (f64, f64) = (99.9997253, 76.7752991);

/// The 64-byte header, field by field: max-RGB, PQ, sample rate 1, downscale 1.
fn header(frames: u64, leading: u64) -> Vec<u8> {
    let mut bytes = b"PQHIST".to_vec();
    bytes.extend_from_slice(&1u16.to_le_bytes());
    for word in [64u32, 4096, 4, 0, 0, 1, 1, 0] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(&frames.to_le_bytes());
    bytes.extend_from_slice(&leading.to_le_bytes());
    bytes.extend_from_slice(&[0; 8]);
    assert_eq!(bytes.len(), 64);
    bytes
}

fn dump_bytes(frames: usize, leading: u64) -> Vec<u8> {
    let mut bytes = header(frames as u64, leading);
    let mut record = vec![0u32; 4096];
    for (bin, count) in PIXELS {
        record[bin] = count;
    }
    for _ in 0..frames {
        bytes.extend(record.iter().flat_map(|count| count.to_le_bytes()));
    }
    bytes
}

fn write(dir: &Path, name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

/// An L1 CSV with labels from `first`, one row per `(max, avg)`.
fn csv(first: u64, rows: &[(&str, &str)]) -> String {
    let mut text = String::from("frame,min_pq,max_pq,avg_pq\n");
    for (index, (max, avg)) in rows.iter().enumerate() {
        text.push_str(&format!("{},10,{max},{avg}\n", first + index as u64));
    }
    text
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_display_sim"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run display_sim")
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "display_sim failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn refused(output: &Output, message: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "accepted; expected '{message}'");
    assert!(
        stderr.contains(message),
        "expected '{message}' in:\n{stderr}"
    );
}

/// The percentage after `label` in a report line.
fn percent_after(text: &str, label: &str) -> f64 {
    let start = text
        .find(label)
        .unwrap_or_else(|| panic!("no '{label}' in:\n{text}"))
        + label.len();
    let rest = text[start..].trim_start();
    rest[..rest.find('%').expect("a percentage")]
        .parse()
        .expect("a number")
}

fn light(nits: &[f64; 5]) -> f64 {
    PIXELS
        .iter()
        .zip(nits)
        .map(|(&(_, count), nits)| f64::from(count) * nits)
        .sum()
}

#[test]
fn two_frames_report_light_clipping_and_steps_after_the_limits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "dump.pqhist", dump_bytes(2, 0));
    // Integer codes in the reference, decimals in the candidate.
    write(
        root,
        "reference.csv",
        csv(0, &[("2800", "1500"), ("2800", "1500")]),
    );
    write(
        root,
        "candidate.csv",
        csv(0, &[("2800.0", "1500.00"), ("3200.000", "1500")]),
    );
    let output = run(
        root,
        &[
            "--dump",
            "dump.pqhist",
            "--reference",
            "reference.csv",
            "--candidate",
            "candidate.csv",
            "--curves",
            "spline",
            "--panels",
            "600",
            "--frames-csv",
            "frames.csv",
        ],
    );
    let text = stdout(&output);
    assert!(text.starts_with("display_sim (ROADMAP WS7.1)"), "{text}");
    assert!(text.contains("Limits: open curves are not a Dolby display"));
    assert!(text.contains("libplacebo: linked") && text.contains("pinned 7.387.0"));

    let (reference, candidate) = (light(&REFERENCE_NITS), light(&CANDIDATE_NITS));
    let run_change = (reference + candidate) / (2.0 * reference) - 1.0;
    let got = percent_after(&text, "displayed light, histogram-weighted:");
    assert!(
        (got - run_change * 100.0).abs() < 0.01,
        "{got} vs {run_change}"
    );
    assert!(
        text.contains("clipped pixels: reference 25.000%, candidate 18.750%"),
        "{text}"
    );
    assert!(
        text.contains("on 2 reference and 1 candidate frames"),
        "{text}"
    );
    assert!(
        text.contains("(1 frame pairs): reference 0 (")
            && text.contains("candidate 3 (")
            && text.contains("added by the candidate 3"),
        "{text}"
    );

    let frames = std::fs::read_to_string(root.join("frames.csv")).unwrap();
    let mut lines = frames.lines();
    assert!(lines
        .next()
        .unwrap()
        .starts_with("# display_sim (ROADMAP WS7.1)"));
    let table: Vec<&str> = lines.skip_while(|line| line.starts_with('#')).collect();
    assert_eq!(table.len(), 3, "{frames}");
    let header: Vec<&str> = table[0].split(',').collect();
    let column = |name: &str| header.iter().position(|&c| c == name).unwrap();
    let rows: Vec<Vec<&str>> = table[1..]
        .iter()
        .map(|row| row.split(',').collect())
        .collect();
    let change_100: f64 = rows[1][column("change_100_pct")].parse().unwrap();
    assert!((change_100 - (AT_100.1 / AT_100.0 - 1.0) * 100.0).abs() < 0.01);
    assert_eq!(rows[0][column("step_candidate_100_pct")], "");
    let step: f64 = rows[1][column("step_candidate_100_pct")].parse().unwrap();
    assert!(
        (step - change_100).abs() < 0.01,
        "frame 0 displays as the reference"
    );
    assert_eq!(rows[1][column("peak_bin")], "3500");
    assert_eq!(rows[1][column("candidate_max_pq")], "3200");
}

#[test]
fn leading_pictures_and_shot_lists_line_up() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // 4 measured frames after 2 leading pictures: stream frames 0..6.
    write(root, "dump.pqhist", dump_bytes(4, 2));
    let same = ("2800", "1500");
    let compressed = ("3200", "1500");
    // A whole-stream reference (rows 0 and 1 describe the leading pictures) and an l1_diff
    // style candidate from stream frame 2; the candidate changes at the second shot.
    write(
        root,
        "reference.csv",
        csv(0, &[("4000", "0"), ("4000", "0"), same, same, same, same]),
    );
    write(
        root,
        "candidate.csv",
        csv(2, &[same, same, compressed, compressed]),
    );
    write(root, "shots.txt", "0\n4\n6\n");
    let args = [
        "--dump",
        "dump.pqhist",
        "--reference",
        "reference.csv",
        "--candidate",
        "candidate.csv",
        "--curves",
        "spline",
        "--panels",
        "600",
    ];
    let text = stdout(&run(
        root,
        &[&args[..], &["--per-shot", "shots.txt"]].concat(),
    ));
    assert!(text.contains("4 frames after 2 leading pictures"), "{text}");
    assert!(
        text.contains("2 rows before the first measured frame skipped"),
        "{text}"
    );
    // Stream frames 4 and 5 are measured frames 2 and 3, the second shot; the step into it is
    // left out.
    assert!(text.contains("(2 frame pairs): reference 0 (") && text.contains("candidate 0 ("));
    let shot_rows: Vec<&str> = text
        .lines()
        .filter(|line| line.trim_start().starts_with("0-1") || line.trim_start().starts_with("2-3"))
        .collect();
    assert_eq!(shot_rows.len(), 2, "{text}");
    assert!(shot_rows[0].contains("+0.00%"), "{}", shot_rows[0]);
    let without_shots = stdout(&run(root, &args));
    assert!(
        without_shots.contains("(3 frame pairs)") && without_shots.contains("candidate 3 ("),
        "{without_shots}"
    );

    // A reference in decoded coordinates (from 0, one row per measured frame) cannot be
    // paired with a candidate in stream coordinates, nor used with a shot list.
    write(root, "decoded.csv", csv(0, &[same, same, same, same]));
    refused(
        &run(
            root,
            &[
                "--dump",
                "dump.pqhist",
                "--reference",
                "decoded.csv",
                "--candidate",
                "candidate.csv",
            ],
        ),
        "cannot both describe",
    );
    refused(
        &run(
            root,
            &[
                "--dump",
                "dump.pqhist",
                "--reference",
                "decoded.csv",
                "--candidate",
                "decoded.csv",
                "--per-shot",
                "shots.txt",
            ],
        ),
        "--per-shot cannot",
    );
}

#[test]
fn malformed_inputs_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "l1.csv",
        csv(0, &[("2800", "1500"), ("2800", "1500")]),
    );
    let base = |dump: &str, reference: &str| {
        run(
            root,
            &[
                "--dump",
                dump,
                "--reference",
                reference,
                "--candidate",
                "l1.csv",
            ],
        )
    };
    let good = dump_bytes(2, 0);
    let mut unfinished = good.clone();
    unfinished[40..48].copy_from_slice(&u64::MAX.to_le_bytes());
    write(root, "unfinished.pqhist", unfinished);
    refused(&base("unfinished.pqhist", "l1.csv"), "did not finish");
    write(root, "short.pqhist", &good[..good.len() - 1]);
    refused(&base("short.pqhist", "l1.csv"), "2 frames need 32832");
    let mut magic = good.clone();
    magic[0] = b'X';
    write(root, "magic.pqhist", magic);
    refused(&base("magic.pqhist", "l1.csv"), "no PQHIST magic");
    let mut version = good.clone();
    version[6] = 2;
    write(root, "version.pqhist", version);
    refused(
        &base("version.pqhist", "l1.csv"),
        "version 2 is not supported",
    );

    write(root, "dump.pqhist", &good);
    write(root, "one.csv", csv(0, &[("2800", "1500")]));
    refused(&base("dump.pqhist", "one.csv"), "frame count mismatch");
    write(
        root,
        "range.csv",
        csv(0, &[("2800", "1500"), ("4096", "1500")]),
    );
    refused(
        &base("dump.pqhist", "range.csv"),
        "max_pq 4096 is not a 12-bit PQ code",
    );
    stdout(&base("dump.pqhist", "l1.csv"));
}

#[test]
fn invalid_numbers_are_refused_before_libplacebo_runs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "dump.pqhist", dump_bytes(1, 0));
    write(root, "l1.csv", csv(0, &[("2800", "1500")]));
    let with = |extra: &[&str]| {
        let base = [
            "--dump",
            "dump.pqhist",
            "--reference",
            "l1.csv",
            "--candidate",
            "l1.csv",
        ];
        run(root, &[&base[..], extra].concat())
    };
    refused(&with(&["--panels", "0"]), "panel peak 0 nits");
    refused(&with(&["--panels", "600,20000"]), "panel peak 20000 nits");
    refused(&with(&["--source-black=-1"]), "source black -1 nits");
    refused(
        &with(&["--panel-black", "600", "--panels", "600"]),
        "panel black 600 nits",
    );
    refused(&with(&["--step-threshold=-0.1"]), "--step-threshold -0.1");
    refused(&with(&["--step-threshold", "NaN"]), "--step-threshold NaN");
    refused(&with(&["--curves", "hable"]), "invalid value 'hable'");
}
