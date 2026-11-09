//! The highlight-pair generator of the development corpus (`scripts/corpus/make_highlight_pair.py`,
//! ROADMAP WS8.1) on a small synthetic clean/grainy pair: the grainy highlights carry the grain,
//! the segments cover all four 2x2 grid phases, the truth file holds the standard's values, and
//! the analyzer's pixel and ST 2094-10 reduced-set peaks match that truth.
//!
//! The expected reduced-set values come from the block occupancy of each patch, computed here
//! from the geometry, not from the generator's samples. Skips when `ffmpeg` (with FFV1) is not
//! available, like `reduced_set.rs`; `python3` is required.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const W: usize = 128;
const H: usize = 96;
const FRAMES: usize = 300;
const X0: usize = 40;
const Y0: usize = 32;
const HIGHLIGHT: u16 = 820;
const CLEAN_LUMA: u16 = 200;
/// The grainy input sits this many codes above the clean one, as a second master can.
const MASTER_OFFSET: u16 = 30;
const GRAIN: u32 = 20;

const ST2084_M1: f64 = 2610.0 / 16384.0;
const ST2084_M2: f64 = (2523.0 / 4096.0) * 128.0;
const ST2084_C1: f64 = 3424.0 / 4096.0;
const ST2084_C2: f64 = (2413.0 / 4096.0) * 32.0;
const ST2084_C3: f64 = (2392.0 / 4096.0) * 32.0;

fn eotf(pq: f64) -> f64 {
    let p = pq.powf(1.0 / ST2084_M2);
    ((p - ST2084_C1).max(0.0) / (ST2084_C2 - ST2084_C3 * p)).powf(1.0 / ST2084_M1)
}

fn inverse_eotf(linear: f64) -> f64 {
    let l = linear.max(0.0).powf(ST2084_M1);
    ((ST2084_C1 + ST2084_C2 * l) / (1.0 + ST2084_C3 * l)).powf(ST2084_M2)
}

/// 12-bit PQ code of a limited-range luma code on neutral chroma.
fn code_of(y_code: u16) -> f64 {
    f64::from(y_code - 64) / 876.0 * 4095.0
}

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok()
}

fn generator() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/corpus/make_highlight_pair.py")
}

/// Raw yuv420p10le frames: flat luma `luma(frame, i)`, slightly off-neutral chroma.
fn write_raw(path: &Path, mut luma: impl FnMut() -> u16) {
    let mut bytes = Vec::with_capacity(FRAMES * W * H * 3);
    for _ in 0..FRAMES {
        for _ in 0..W * H {
            bytes.extend_from_slice(&luma().to_le_bytes());
        }
        for code in [520u16, 505] {
            for _ in 0..W * H / 4 {
                bytes.extend_from_slice(&code.to_le_bytes());
            }
        }
    }
    std::fs::write(path, bytes).expect("write raw input");
}

fn run_generator(dir: &Path, tag: &str) -> (PathBuf, PathBuf, serde_json::Value) {
    let (clean, grainy) = (
        dir.join(format!("{tag}-c.yuv")),
        dir.join(format!("{tag}-g.yuv")),
    );
    let truth = dir.join(format!("{tag}-truth.json"));
    let output = Command::new("python3")
        .arg(generator())
        .args(["--clean", dir.join("clean.yuv").to_str().expect("utf-8")])
        .args(["--grainy", dir.join("grainy.yuv").to_str().expect("utf-8")])
        .args(["--out-clean", clean.to_str().expect("utf-8")])
        .args(["--out-grainy", grainy.to_str().expect("utf-8")])
        .args(["--truth", truth.to_str().expect("utf-8")])
        .args(["--width", &W.to_string(), "--height", &H.to_string()])
        .args(["--x0", &X0.to_string(), "--y0", &Y0.to_string()])
        .output()
        .expect("run python3 (required by this test)");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let truth = serde_json::from_slice(&std::fs::read(truth).expect("truth")).expect("json");
    (clean, grainy, truth)
}

fn encode(raw: &Path) -> PathBuf {
    let out = raw.with_extension("mkv");
    let tags = [
        "-color_primaries",
        "bt2020",
        "-color_trc",
        "smpte2084",
        "-colorspace",
        "bt2020nc",
        "-color_range",
        "tv",
    ];
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo"])
        .args([
            "-pix_fmt",
            "yuv420p10le",
            "-video_size",
            &format!("{W}x{H}"),
        ])
        .args(["-framerate", "60"])
        .args(tags)
        .arg("-i")
        .arg(raw)
        .args(["-c:v", "ffv1"])
        .args(tags)
        .arg(&out)
        .status()
        .expect("spawn ffmpeg");
    assert!(status.success(), "ffmpeg encode failed");
    out
}

fn analyze(clip: &Path) -> (Vec<f64>, Vec<f64>) {
    let stats = clip.with_extension("csv");
    let output: Output = Command::new(env!("CARGO_BIN_EXE_hdr_analyzer_mvp"))
        .arg(clip)
        .arg("-o")
        .arg(clip.with_extension("bin"))
        .args(["--no-crop", "--disable-optimizer", "--peak-source", "max"])
        .args(["--peak-estimator", "st2094-10", "--dump-frame-stats"])
        .arg(&stats)
        .output()
        .expect("run analyzer");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let csv = std::fs::read_to_string(stats).expect("frame stats");
    (column(&csv, "raw_max_pq"), column(&csv, "reduced_max_pq"))
}

/// One CSV column of `--dump-frame-stats`, by header name, as 12-bit codes.
fn column(csv: &str, name: &str) -> Vec<f64> {
    let mut lines = csv.lines();
    let header: Vec<&str> = lines.next().expect("header").split(',').collect();
    let index = header
        .iter()
        .position(|&column| column == name)
        .unwrap_or_else(|| panic!("no column {name} in {header:?}"));
    lines
        .map(|line| {
            line.split(',')
                .nth(index)
                .expect("value")
                .parse::<f64>()
                .expect("number")
                * 4095.0
        })
        .collect()
}

/// Highlight pixels in the fullest 2x2 block of the grid anchored at (0, 0).
fn fullest_block(x: usize, y: usize, size: usize) -> usize {
    let inside =
        |row: usize, col: usize| (y..y + size).contains(&row) && (x..x + size).contains(&col);
    (y / 2..=(y + size - 1) / 2)
        .flat_map(|by| (x / 2..=(x + size - 1) / 2).map(move |bx| (by, bx)))
        .map(|(by, bx)| {
            [(0, 0), (0, 1), (1, 0), (1, 1)]
                .iter()
                .filter(|(dy, dx)| inside(2 * by + dy, 2 * bx + dx))
                .count()
        })
        .max()
        .expect("a block")
}

/// The luma code at (`col`, `row`) of frame `frame` in a raw yuv420p10le stream.
fn luma_at(raw: &[u8], frame: usize, row: usize, col: usize) -> u16 {
    let at = 2 * (frame * W * H * 3 / 2 + row * W + col);
    u16::from_le_bytes([raw[at], raw[at + 1]])
}

fn number(value: &serde_json::Value, key: &str) -> f64 {
    value[key]
        .as_f64()
        .unwrap_or_else(|| panic!("{key} in {value}"))
}

/// Runs the generator on inputs of `frames` frames with extra arguments; returns its output.
fn run_generator_on(dir: &Path, frames: usize, extra: &[&str]) -> Output {
    let frame = vec![0u8; W * H * 3];
    for name in ["short-c.yuv", "short-g.yuv"] {
        std::fs::write(dir.join(name), frame.repeat(frames)).expect("write raw input");
    }
    let path = |name: &str| dir.join(name).to_str().expect("utf-8").to_owned();
    Command::new("python3")
        .arg(generator())
        .args([
            "--clean",
            &path("short-c.yuv"),
            "--grainy",
            &path("short-g.yuv"),
        ])
        .args(["--out-clean", &path("short-oc.yuv")])
        .args(["--out-grainy", &path("short-og.yuv")])
        .args(["--truth", &path("short-truth.json")])
        .args(["--width", &W.to_string(), "--height", &H.to_string()])
        .args(extra)
        .output()
        .expect("run python3 (required by this test)")
}

#[test]
fn generator_refuses_short_inputs_and_negative_offsets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stderr = |output: &Output| String::from_utf8_lossy(&output.stderr).into_owned();
    // One frame short of the last segment: no truth that lists unpainted frames.
    let short = run_generator_on(dir.path(), FRAMES - 1, &["--x0", "40", "--y0", "32"]);
    assert!(!short.status.success(), "a short input must fail");
    assert!(
        stderr(&short).contains("the segments need 300"),
        "{}",
        stderr(&short)
    );
    assert!(!dir.path().join("short-truth.json").exists());
    for (x0, y0) in [("-2", "32"), ("40", "-2")] {
        let negative = run_generator_on(dir.path(), 1, &["--x0", x0, "--y0", y0]);
        assert!(!negative.status.success(), "--x0 {x0} --y0 {y0} must fail");
        assert!(
            stderr(&negative).contains("must not be negative"),
            "{}",
            stderr(&negative)
        );
    }
}

#[test]
fn highlight_pair_carries_grain_and_matches_the_standard() {
    if !have_ffmpeg() {
        eprintln!("Skipping: ffmpeg not found in PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    write_raw(&dir.path().join("clean.yuv"), || CLEAN_LUMA);
    // Uniform grain of +-GRAIN codes from a fixed LCG.
    let mut state = 0x5747_3831u32;
    write_raw(&dir.path().join("grainy.yuv"), || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (state >> 16) % (2 * GRAIN + 1);
        CLEAN_LUMA + MASTER_OFFSET + noise as u16 - GRAIN as u16
    });

    let (clean, grainy, truth) = run_generator(dir.path(), "a");
    let (_, _, again) = run_generator(dir.path(), "b");
    assert_eq!(truth, again, "the generator is deterministic");
    assert_eq!(truth["frames_written"], FRAMES);
    let read = |path: &Path| std::fs::read(path).expect("raw stream");
    let (in_clean, in_grainy) = (
        read(&dir.path().join("clean.yuv")),
        read(&dir.path().join("grainy.yuv")),
    );
    let (out_clean, out_grainy) = (read(&clean), read(&grainy));

    let records = truth["frames"].as_array().expect("frames");
    let mut phases = std::collections::BTreeSet::new();
    let mut grain_frames = 0;
    for record in records {
        let (x, y, size) = (
            number(record, "x") as usize,
            number(record, "y") as usize,
            number(record, "size") as usize,
        );
        if size == 2 {
            phases.insert((x % 2, y % 2));
        }
        // The clean truth is the standard's value for this block occupancy.
        let filled = fullest_block(x, y, size) as f64;
        let expected = inverse_eotf(
            (filled * eotf(code_of(HIGHLIGHT) / 4095.0)
                + (4.0 - filled) * eotf(code_of(CLEAN_LUMA) / 4095.0))
                / 4.0,
        ) * 4095.0;
        let clean_truth = &record["clean"];
        assert!((number(clean_truth, "pixel_pq12") - code_of(HIGHLIGHT)).abs() < 1e-6);
        assert!(
            (number(clean_truth, "st2094_pq12") - expected).abs() < 0.01,
            "{record}: expected {expected}"
        );
        // The grainy highlight carries grain, with the master offset removed.
        let local_mean = number(&record["grainy"], "local_mean");
        assert!(
            (local_mean - f64::from(MASTER_OFFSET)).abs() <= 2.0,
            "{record}"
        );
        // Read back from the written samples: the clean highlight is flat, the grainy one is
        // 820 plus the input's grain, less the local mean, at every pixel.
        let frame = number(record, "frame") as usize;
        let mut codes = Vec::new();
        for row in y..y + size {
            for col in x..x + size {
                let at = |plane: &[u8]| luma_at(plane, frame, row, col);
                assert_eq!(at(&out_clean), HIGHLIGHT, "{record}");
                let want = i32::from(HIGHLIGHT) + i32::from(at(&in_grainy))
                    - i32::from(at(&in_clean))
                    - local_mean as i32;
                assert_eq!(i32::from(at(&out_grainy)), want, "{record} ({col}, {row})");
                codes.push(at(&out_grainy));
            }
        }
        let range = codes.iter().max().expect("codes") - codes.iter().min().expect("codes");
        assert_eq!(number(&record["grainy"], "luma_range"), f64::from(range));
        assert_eq!(number(clean_truth, "luma_range"), 0.0);
        if range > 0 {
            grain_frames += 1;
        }
    }
    assert_eq!(
        phases.len(),
        4,
        "all four 2x2 grid phases occur: {phases:?}"
    );
    assert_eq!(
        grain_frames,
        records.len(),
        "every grainy highlight carries grain"
    );
    // A 2x2 one column and one row off the grid fills a quarter of each block.
    let quarter = records
        .iter()
        .find(|r| r["segment"] == "2x2_odd_xy")
        .expect("2x2_odd_xy");
    assert!(code_of(HIGHLIGHT) - number(&quarter["clean"], "st2094_pq12") > 400.0);

    // The analyzer reads the truth's values on every highlight frame.
    for (raw, name) in [(&clean, "clean"), (&grainy, "grainy")] {
        let (pixel, reduced) = analyze(&encode(raw));
        assert_eq!(pixel.len(), FRAMES);
        for record in records {
            let frame = number(record, "frame") as usize;
            let want = &record[name];
            assert!(
                (pixel[frame] - number(want, "pixel_pq12")).abs() <= 0.5,
                "{name} frame {frame}: pixel peak {} vs {want}",
                pixel[frame]
            );
            assert!(
                (reduced[frame] - number(want, "st2094_pq12")).abs() <= 1.0,
                "{name} frame {frame}: reduced-set peak {} vs {want}",
                reduced[frame]
            );
        }
    }
}
