//! Synthetic ground-truth accuracy test.
//!
//! Builds a lossless (FFV1) PQ clip whose peak luminance is known by construction,
//! runs the analyzer on it, and asserts the measured per-frame peak matches the
//! constructed value to within a quarter of one 12-bit PQ code (lossless codec,
//! so the only slack needed is the measurement format's internal quantization).
//!
//! Skips when `ffmpeg` (with FFV1 support) is not available, mirroring the
//! environment-dependent skips in mkvdovi's integration tests.

use std::io::Write;
use std::process::{Command, Stdio};

const ST2084_Y_MAX: f64 = 10000.0;
const ST2084_M1: f64 = 2610.0 / 16384.0;
const ST2084_M2: f64 = (2523.0 / 4096.0) * 128.0;
const ST2084_C1: f64 = 3424.0 / 4096.0;
const ST2084_C2: f64 = (2413.0 / 4096.0) * 32.0;
const ST2084_C3: f64 = (2392.0 / 4096.0) * 32.0;

fn nits_to_pq(nits: f64) -> f64 {
    let y = (nits / ST2084_Y_MAX).max(0.0);
    ((ST2084_C1 + ST2084_C2 * y.powf(ST2084_M1)) / (1.0 + ST2084_C3 * y.powf(ST2084_M1)))
        .powf(ST2084_M2)
}

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok()
}

const W: usize = 320;
const H: usize = 180;
const FRAMES: usize = 48;

/// Encode a solid-luma yuv420p10le clip (limited range, PQ-tagged) losslessly.
fn encode_clip(dir: &std::path::Path, y_code: u16) -> std::path::PathBuf {
    let out = dir.join(format!("pq_{y_code}.mkv"));
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p10le",
            "-video_size",
            &format!("{W}x{H}"),
            "-framerate",
            "24",
            "-i",
            "-",
            "-c:v",
            "ffv1",
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "smpte2084",
            "-colorspace",
            "bt2020nc",
            "-color_range",
            "tv",
        ])
        .arg(&out)
        .stdin(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn ffmpeg");

    let mut frame = Vec::with_capacity(W * H * 3);
    for _ in 0..(W * H) {
        frame.extend_from_slice(&y_code.to_le_bytes());
    }
    for _ in 0..(W * H / 2) {
        frame.extend_from_slice(&512u16.to_le_bytes());
    }
    {
        let stdin = child.stdin.as_mut().expect("ffmpeg stdin");
        for _ in 0..FRAMES {
            stdin.write_all(&frame).expect("write frame");
        }
    }
    let status = child.wait().expect("wait ffmpeg");
    assert!(status.success(), "ffmpeg encode failed");
    out
}

#[test]
fn peak_matches_constructed_value() {
    if !have_ffmpeg() {
        eprintln!("Skipping: ffmpeg not found in PATH");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");

    for target_nits in [100.0_f64, 1000.0, 4000.0] {
        // Constructed ground truth: limited-range 10-bit code for the target PQ level.
        let pq = nits_to_pq(target_nits);
        let y_code = (pq * 876.0 + 64.0).round() as u16;
        let expected_pq = f64::from(y_code - 64) / 876.0;

        let clip = encode_clip(dir.path(), y_code);
        let bin = dir.path().join(format!("m_{target_nits}.bin"));

        let status = Command::new(env!("CARGO_BIN_EXE_hdr_analyzer_mvp"))
            .arg(&clip)
            .arg("-o")
            .arg(&bin)
            .args(["--peak-source", "max", "--disable-optimizer", "--no-crop"])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .expect("run analyzer");
        assert!(
            status.success(),
            "analyzer failed on {target_nits} nits clip"
        );

        let data = std::fs::read(&bin).expect("read measurements");
        let m =
            madvr_parse::MadVRMeasurements::parse_measurements(&data).expect("parse measurements");
        assert_eq!(m.frames.len(), FRAMES);

        // Quarter of one 12-bit PQ code: admits the measurement format's internal
        // quantization while still failing on any real pixel-level deviation.
        let tolerance = 0.25 / 4095.0;
        for (i, f) in m.frames.iter().enumerate() {
            let err = (f.peak_pq_2020 - expected_pq).abs();
            assert!(
                err < tolerance,
                "{target_nits} nits clip, frame {i}: peak_pq {} != constructed {} (|err| {err})",
                f.peak_pq_2020,
                expected_pq
            );
        }
    }
}
