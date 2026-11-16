//! `--peak-estimator st2094-10` end to end (ROADMAP WS1.5): the SMPTE ST 2094-10 reduced-set
//! peak per frame on lossless synthetic clips, with expected values computed here from the
//! standard's definition (2x2 block means in linear light), and the inputs it refuses.
//!
//! Skips when `ffmpeg` (with FFV1) is not available, like `synthetic_accuracy.rs`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const W: usize = 320;
const H: usize = 180;
const FRAMES: usize = 48;

const ST2084_M1: f64 = 2610.0 / 16384.0;
const ST2084_M2: f64 = (2523.0 / 4096.0) * 128.0;
const ST2084_C1: f64 = 3424.0 / 4096.0;
const ST2084_C2: f64 = (2413.0 / 4096.0) * 32.0;
const ST2084_C3: f64 = (2392.0 / 4096.0) * 32.0;

/// ST 2084 EOTF: normalized PQ to linear light / 10000 nits.
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

/// Encode one luma plane per frame on neutral chroma as lossless yuv420p10le, tagged `trc`.
fn encode(dir: &Path, label: &str, planes: &[Vec<u16>], trc: &str) -> PathBuf {
    let out = dir.join(format!("{label}.mkv"));
    let size = format!("{W}x{H}");
    let tags = [
        "-color_primaries",
        "bt2020",
        "-color_trc",
        trc,
        "-colorspace",
        "bt2020nc",
        "-color_range",
        "tv",
    ];
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo"])
        .args([
            "-pix_fmt",
            "yuv420p10le",
            "-video_size",
            &size,
            "-framerate",
            "24",
        ])
        .args(tags)
        .args(["-i", "-", "-c:v", "ffv1"])
        .args(tags)
        .arg(&out)
        .stdin(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn ffmpeg");
    {
        let stdin = child.stdin.as_mut().expect("ffmpeg stdin");
        let chroma = vec![512u16; W * H / 2];
        for plane in planes {
            assert_eq!(plane.len(), W * H);
            let bytes: Vec<u8> = plane
                .iter()
                .chain(&chroma)
                .flat_map(|code| code.to_le_bytes())
                .collect();
            stdin.write_all(&bytes).expect("write frame");
        }
    }
    assert!(
        child.wait().expect("wait ffmpeg").success(),
        "ffmpeg encode failed"
    );
    out
}

fn analyze(clip: &Path, bin: &Path, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hdr_analyzer_mvp"))
        .arg(clip)
        .arg("-o")
        .arg(bin)
        .args(["--no-crop", "--disable-optimizer", "--peak-source", "max"])
        .args(flags)
        .output()
        .expect("run analyzer")
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

/// A 2x2 specular of luma `peak` at (`x0`, `y0`) on a flat background of luma `background`.
fn picture(background: u16, specular: Option<(usize, usize, u16)>) -> Vec<u16> {
    let mut plane = vec![background; W * H];
    if let Some((x0, y0, peak)) = specular {
        for y in y0..y0 + 2 {
            for x in x0..x0 + 2 {
                plane[y * W + x] = peak;
            }
        }
    }
    plane
}

/// The frozen synthetic cases of the WS1 rules: a small specular on or off the block grid, a
/// one-frame flash, a highlight on the first and on the last frame, a 3-frame specular and a
/// fade. Each frame's reduced-set peak must be within 1 code of the standard's value, and the
/// sidecar must carry it as the frame peak with `peak_estimator` "st2094-10".
#[test]
fn reduced_set_peak_matches_the_standard_on_the_frozen_cases() {
    if !have_ffmpeg() {
        eprintln!("Skipping: ffmpeg not found in PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    const PEAK: u16 = 820; // 3534 codes, the spike's synthetic highlight
                           // (frame, specular position): x odd means off the grid by one column, so two blocks each
                           // hold half of the specular; (160, 90) is on the grid.
    let on_grid = |frame: usize| match frame {
        0 | 20 | 47 => Some((161, 90)),
        30..=32 => Some((160, 90)),
        _ => None,
    };
    // Background: flat, then a fade from frame 36 up to the last frame.
    let background = |frame: usize| {
        if frame < 36 {
            200
        } else {
            200 + 30 * (frame as u16 - 35)
        }
    };
    let planes: Vec<Vec<u16>> = (0..FRAMES)
        .map(|frame| picture(background(frame), on_grid(frame).map(|(x, y)| (x, y, PEAK))))
        .collect();
    let clip = encode(dir.path(), "frozen", &planes, "smpte2084");
    let bin = dir.path().join("frozen.bin");
    let stats = dir.path().join("frozen.csv");
    let output = analyze(
        &clip,
        &bin,
        &[
            "--peak-estimator",
            "st2094-10",
            "--dump-frame-stats",
            stats.to_str().expect("utf-8"),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let csv = std::fs::read_to_string(&stats).expect("frame stats");
    let (selected, reduced, raw) = (
        column(&csv, "selected_pq"),
        column(&csv, "reduced_max_pq"),
        column(&csv, "raw_max_pq"),
    );
    assert_eq!(reduced.len(), FRAMES);
    for frame in 0..FRAMES {
        let floor = eotf(code_of(background(frame)) / 4095.0);
        let expected = match on_grid(frame) {
            None => code_of(background(frame)),
            Some((x, _)) if x % 2 == 0 => code_of(PEAK),
            Some(_) => inverse_eotf((eotf(code_of(PEAK) / 4095.0) + floor) / 2.0) * 4095.0,
        };
        assert!(
            (reduced[frame] - expected).abs() <= 1.0,
            "frame {frame}: reduced-set peak {} vs the standard's {expected}",
            reduced[frame]
        );
        assert_eq!(
            selected[frame], reduced[frame],
            "frame {frame}: selected peak"
        );
        assert!(
            reduced[frame] <= raw[frame] + 0.5,
            "frame {frame}: above the pixel peak"
        );
    }
    // The off-grid flash reads about 300 codes below its pixel peak; the raw peak does not.
    assert!((raw[20] - code_of(PEAK)).abs() < 0.5);
    assert!(
        (250.0..350.0).contains(&(code_of(PEAK) - reduced[20])),
        "{}",
        reduced[20]
    );

    let sidecar: serde_json::Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("frozen.bin.l1.json")).expect("sidecar"),
    )
    .expect("parse sidecar");
    assert_eq!(sidecar["peak_estimator"], "st2094-10");
    let frame_peaks: Vec<f64> = sidecar["frames"]["max_pq_12bit"]
        .as_array()
        .expect("frame peaks")
        .iter()
        .map(|value| value.as_f64().expect("code"))
        .collect();
    for (frame, (&stored, &measured)) in frame_peaks.iter().zip(&reduced).enumerate() {
        assert_eq!(
            stored,
            measured.round(),
            "frame {frame}: sidecar frame peak"
        );
    }
}

/// The default estimator is unchanged on the same clip: the raw pixel peak, and the reduced-set
/// columns stay zero.
#[test]
fn default_estimator_ignores_the_reduced_set() {
    if !have_ffmpeg() {
        eprintln!("Skipping: ffmpeg not found in PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let planes = vec![picture(200, Some((161, 90, 820))); FRAMES];
    let clip = encode(dir.path(), "default", &planes, "smpte2084");
    let stats = dir.path().join("default.csv");
    let output = analyze(
        &clip,
        &dir.path().join("default.bin"),
        &["--dump-frame-stats", stats.to_str().expect("utf-8")],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let csv = std::fs::read_to_string(&stats).expect("frame stats");
    assert!(column(&csv, "selected_pq")
        .iter()
        .all(|&code| (code - code_of(820)).abs() < 0.5));
    assert!(column(&csv, "reduced_max_pq")
        .iter()
        .all(|&code| code == 0.0));
    assert!(column(&csv, "reduced_min_pq")
        .iter()
        .all(|&code| code == 0.0));
}

/// Inputs the reduced set is not defined for yet are refused before analysis, with the reason;
/// the same clips analyze with the default estimator.
#[test]
fn reduced_set_refuses_hlg_luma_domain_and_downscale() {
    if !have_ffmpeg() {
        eprintln!("Skipping: ffmpeg not found in PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let planes = vec![picture(400, None); FRAMES];
    let pq = encode(dir.path(), "pq", &planes, "smpte2084");
    let hlg = encode(dir.path(), "hlg", &planes, "arib-std-b67");
    let cases: [(&Path, &[&str], &str); 3] = [
        (&hlg, &[], "PQ input only"),
        (&pq, &["--peak-domain", "luma"], "--peak-domain max-rgb"),
        (&pq, &["--downscale", "2"], "--downscale 1"),
    ];
    for (index, (clip, flags, reason)) in cases.into_iter().enumerate() {
        let bin = dir.path().join(format!("refused{index}.bin"));
        let mut refused_flags = vec!["--peak-estimator", "st2094-10"];
        refused_flags.extend_from_slice(flags);
        let output = analyze(clip, &bin, &refused_flags);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{flags:?}: must be refused");
        assert!(stderr.contains(reason), "{flags:?}: {stderr}");
        assert!(
            !bin.exists(),
            "{flags:?}: refused before writing measurements"
        );
        let accepted = analyze(
            clip,
            &dir.path().join(format!("accepted{index}.bin")),
            flags,
        );
        assert!(
            accepted.status.success(),
            "{flags:?} with the default estimator: {}",
            String::from_utf8_lossy(&accepted.stderr)
        );
    }
}
