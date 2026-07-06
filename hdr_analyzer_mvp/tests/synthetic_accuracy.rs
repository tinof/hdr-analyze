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

/// Encode a solid-color yuv420p10le clip (limited range, PQ-tagged) losslessly.
fn encode_clip(
    dir: &std::path::Path,
    y_code: u16,
    cb_code: u16,
    cr_code: u16,
) -> std::path::PathBuf {
    let out = dir.join(format!("pq_{y_code}_{cb_code}_{cr_code}.mkv"));
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
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "smpte2084",
            "-colorspace",
            "bt2020nc",
            "-color_range",
            "tv",
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
    for _ in 0..(W * H / 4) {
        frame.extend_from_slice(&cb_code.to_le_bytes());
    }
    for _ in 0..(W * H / 4) {
        frame.extend_from_slice(&cr_code.to_le_bytes());
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

        let clip = encode_clip(dir.path(), y_code, 512, 512);
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

#[test]
fn saturated_peak_matches_constructed_max_rgb_and_luma() {
    if !have_ffmpeg() {
        eprintln!("Skipping: ffmpeg not found in PATH");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");

    // Construct a saturated non-linear BT.2020 RGB signal, then invert the
    // BT.2020 NCL matrix into limited-range 10-bit YCbCr codes.
    let (red, green, blue) = (0.62_f64, 0.18_f64, 0.08_f64);
    let y = 0.2627 * red + 0.6780 * green + 0.0593 * blue;
    let cb = (blue - y) / 1.8814;
    let cr = (red - y) / 1.4746;
    let y_code = (y * 876.0 + 64.0).round() as u16;
    let cb_code = (cb * 896.0 + 512.0).round() as u16;
    let cr_code = (cr * 896.0 + 512.0).round() as u16;

    // Ground truth is reconstructed from the quantized codes actually encoded,
    // so the assertion measures analyzer error rather than fixture rounding.
    let quantized_y = (f64::from(y_code) - 64.0) / 876.0;
    let quantized_cb = (f64::from(cb_code) - 512.0) / 896.0;
    let quantized_cr = (f64::from(cr_code) - 512.0) / 896.0;
    let quantized_red = quantized_y + 1.4746 * quantized_cr;
    let quantized_blue = quantized_y + 1.8814 * quantized_cb;
    let quantized_green = (quantized_y - 0.2627 * quantized_red - 0.0593 * quantized_blue) / 0.6780;
    let expected_max_rgb = quantized_red
        .max(quantized_green)
        .max(quantized_blue)
        .clamp(0.0, 1.0);
    let expected_luma = quantized_y.clamp(0.0, 1.0);
    assert!(expected_max_rgb > expected_luma);

    let clip = encode_clip(dir.path(), y_code, cb_code, cr_code);
    let decoded = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&clip)
        .args([
            "-frames:v",
            "1",
            "-pix_fmt",
            "yuv420p10le",
            "-f",
            "rawvideo",
            "-",
        ])
        .output()
        .expect("decode synthetic clip");
    let read_code = |offset: usize| {
        u16::from_le_bytes([decoded.stdout[offset], decoded.stdout[offset + 1]]) & 0x03ff
    };
    assert_eq!(
        (read_code(0), read_code(W * H * 2), read_code(W * H * 5 / 2)),
        (y_code, cb_code, cr_code),
        "FFV1 must preserve the constructed YCbCr codes"
    );
    let tolerance = 0.25 / 4095.0;

    for (label, domain, expected) in [
        ("default", None, expected_max_rgb),
        ("max-rgb", Some("max-rgb"), expected_max_rgb),
        ("luma", Some("luma"), expected_luma),
    ] {
        let bin = dir.path().join(format!("saturated_{label}.bin"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_hdr_analyzer_mvp"));
        command
            .arg(&clip)
            .arg("-o")
            .arg(&bin)
            .args(["--disable-optimizer", "--no-crop"]);
        if let Some(domain) = domain {
            command.args(["--peak-source", "max", "--peak-domain", domain]);
        }
        let status = command
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .expect("run analyzer");
        assert!(
            status.success(),
            "analyzer failed for {label} configuration"
        );

        let data = std::fs::read(&bin).expect("read measurements");
        let measurements =
            madvr_parse::MadVRMeasurements::parse_measurements(&data).expect("parse measurements");
        assert_eq!(measurements.frames.len(), FRAMES);
        for (i, frame) in measurements.frames.iter().enumerate() {
            let err = (frame.peak_pq_2020 - expected).abs();
            assert!(
                err < tolerance,
                "{label} frame {i}: peak_pq {} != constructed {} (|err| {err})",
                frame.peak_pq_2020,
                expected
            );
        }
    }
}

#[test]
fn crop_probe_ignores_black_and_full_frame_lead_in() {
    if !have_ffmpeg() {
        eprintln!("Skipping: ffmpeg not found in PATH");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let clip = dir.path().join("crop_lead_in.mkv");
    let encode = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=160x90:r=24:d=0.5",
            "-f",
            "lavfi",
            "-i",
            "color=c=gray:s=160x90:r=24:d=0.5",
            "-f",
            "lavfi",
            "-i",
            "color=c=white:s=160x66:r=24:d=7",
            "-filter_complex",
            "[0:v]format=yuv420p10le[b];[1:v]format=yuv420p10le[l];[2:v]pad=160:90:0:12:black,format=yuv420p10le[m];[b][l][m]concat=n=3:v=1:a=0[out]",
            "-map",
            "[out]",
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-g",
            "1",
        ])
        .arg(&clip)
        .status()
        .expect("encode crop fixture");
    assert!(encode.success(), "ffmpeg crop fixture encode failed");

    let run = |label: &str, extra_args: &[&str]| {
        let output_path = dir.path().join(format!("{label}.bin"));
        let output = Command::new(env!("CARGO_BIN_EXE_hdr_analyzer_mvp"))
            .arg(&clip)
            .arg("-o")
            .arg(output_path)
            .args(["--disable-optimizer", "--hist-bin-ema-beta", "0"])
            .args(extra_args)
            .output()
            .expect("run analyzer on crop fixture");
        assert!(
            output.status.success(),
            "analyzer {label} run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("analyzer output is UTF-8")
    };

    let probed = run("probed", &[]);
    assert!(probed.contains("Committed active video area: 160x66 at offset (0, 12)"));

    let fallback = run("fallback", &["--crop-probes", "0"]);
    assert!(fallback.contains("Fallback active video area: 160x90 at offset (0, 0)"));

    let no_crop = run("no_crop", &["--no-crop"]);
    assert!(no_crop.contains("Crop disabled: using full frame 160x90"));
    assert!(!no_crop.contains("Probing active video area"));
}
