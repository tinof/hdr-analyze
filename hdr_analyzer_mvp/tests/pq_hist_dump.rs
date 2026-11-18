//! `--dump-pq-hist` end to end (ROADMAP WS7.1): the `pqhist` v1 dump of lossless synthetic clips.
//! Every record is checked against a histogram computed here from the encoded planes, with the
//! BT.2020 non-constant-luminance inverse written from Kr and Kb (not the analyzer's constants),
//! on codes chosen away from bin ties; plus the header fields, `--sample-rate` repeats, and the
//! paths the flag refuses. The `.bin` and the sidecar must not change with the flag.
//!
//! Skips when `ffmpeg` (with FFV1) is not available, like `reduced_set.rs`; the open-GOP case
//! also needs libx265 and mkvmerge, like `au_table.rs`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const W: usize = 64;
const H: usize = 36;
const FRAMES: usize = 6;
const RECORD: usize = 4096 * 4;
/// Constant chroma of the coloured clip: red-ish (Cr above, Cb below neutral).
const CB: u16 = 470;
const CR: u16 = 600;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok()
}

macro_rules! require_ffmpeg {
    () => {
        if !have_ffmpeg() {
            eprintln!("Skipping: ffmpeg not found in PATH");
            return;
        }
    };
}

/// The peak-domain value of a pixel, normalized PQ, from BT.2020 NCL: R' = Y' + 2(1 - Kr) Cr,
/// B' = Y' + 2(1 - Kb) Cb, G' = (Y' - Kr R' - Kb B') / (1 - Kr - Kb), clipped to [0, 1].
fn peak_value(y: u16, cb: u16, cr: u16, luma: bool) -> f64 {
    let (kr, kb) = (0.2627, 0.0593);
    let y = (f64::from(y) - 64.0) / 876.0;
    let value = if luma {
        y
    } else {
        let cb = (f64::from(cb) - 512.0) / 896.0;
        let cr = (f64::from(cr) - 512.0) / 896.0;
        let r = y + 2.0 * (1.0 - kr) * cr;
        let b = y + 2.0 * (1.0 - kb) * cb;
        let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
        r.max(g).max(b)
    };
    value.clamp(0.0, 1.0)
}

/// The bin of a peak-domain value; `None` within 0.01 of a rounding tie, where f32 and f64
/// could disagree.
fn bin_of(value: f64) -> Option<usize> {
    let scaled = value * 4095.0;
    ((scaled.fract() - 0.5).abs() > 0.01).then(|| scaled.round() as usize)
}

/// Luma codes whose luma and max-RGB (on the clip's chroma) are both away from bin ties.
fn usable_codes() -> Vec<u16> {
    (64..=940)
        .step_by(11)
        .filter(|&y| {
            bin_of(peak_value(y, CB, CR, true)).is_some()
                && bin_of(peak_value(y, CB, CR, false)).is_some()
        })
        .collect()
}

/// Luma plane of frame `k`: every frame a different arrangement and mix of the usable codes.
fn luma_plane(codes: &[u16], k: usize) -> Vec<u16> {
    (0..W * H)
        .map(|i| {
            let (x, y) = (i % W, i / W);
            codes[(x * 7 + y * 13 + k * 5) % (codes.len() - k)]
        })
        .collect()
}

/// The expected record of a frame.
fn expected_histogram(plane: &[u16], luma: bool) -> Vec<u32> {
    let mut histogram = vec![0u32; 4096];
    for &y in plane {
        histogram[bin_of(peak_value(y, CB, CR, luma)).expect("usable code")] += 1;
    }
    histogram
}

/// Encode the luma planes on constant chroma (`cb`, `cr`) as lossless yuv420p10le FFV1.
fn encode(path: &Path, planes: &[Vec<u16>], cb: u16, cr: u16, trc: &str) {
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
        .args(["-pix_fmt", "yuv420p10le", "-video_size", &size])
        .args(["-framerate", "24"])
        .args(tags)
        .args(["-i", "-", "-c:v", "ffv1"])
        .args(tags)
        .arg(path)
        .stdin(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn ffmpeg");
    {
        let stdin = child.stdin.as_mut().expect("ffmpeg stdin");
        let chroma: Vec<u16> = [cb, cr]
            .iter()
            .flat_map(|&code| std::iter::repeat_n(code, W * H / 4))
            .collect();
        for plane in planes {
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
        "ffmpeg failed"
    );
}

/// The coloured clip and its luma planes.
fn coloured_clip(dir: &Path) -> (PathBuf, Vec<Vec<u16>>) {
    let codes = usable_codes();
    assert!(codes.len() > 40, "{} usable codes", codes.len());
    let planes: Vec<Vec<u16>> = (0..FRAMES).map(|k| luma_plane(&codes, k)).collect();
    let clip = dir.join("coloured.mkv");
    encode(&clip, &planes, CB, CR, "smpte2084");
    (clip, planes)
}

fn analyze_in(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hdr_analyzer_mvp"))
        .current_dir(cwd)
        .args(["--no-crop", "--disable-optimizer"])
        .args(args)
        .output()
        .expect("run analyzer")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "analyzer failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Dump {
    header: Vec<u8>,
    records: Vec<Vec<u32>>,
}

impl Dump {
    fn read(path: &Path) -> Self {
        let bytes = std::fs::read(path).expect("read dump");
        assert!(bytes.len() >= 64, "dump shorter than its header");
        assert_eq!((bytes.len() - 64) % RECORD, 0, "partial record");
        let records = bytes[64..]
            .chunks(RECORD)
            .map(|record| {
                record
                    .chunks(4)
                    .map(|count| u32::from_le_bytes(count.try_into().unwrap()))
                    .collect()
            })
            .collect();
        Self {
            header: bytes[..64].to_vec(),
            records,
        }
    }

    fn u32_at(&self, offset: usize) -> u32 {
        u32::from_le_bytes(self.header[offset..offset + 4].try_into().unwrap())
    }

    fn u64_at(&self, offset: usize) -> u64 {
        u64::from_le_bytes(self.header[offset..offset + 8].try_into().unwrap())
    }

    /// Peak domain, transfer, sample rate, downscale, frame count, leading skipped frames.
    fn fields(&self) -> (u32, u32, u32, u32, u64, u64) {
        assert_eq!(&self.header[..8], b"PQHIST\x01\x00");
        assert_eq!(
            (self.u32_at(8), self.u32_at(12), self.u32_at(16)),
            (64, 4096, 4)
        );
        assert_eq!(self.u32_at(36), 0, "reserved word");
        assert_eq!(self.header[56..64], [0; 8], "reserved bytes");
        (
            self.u32_at(20),
            self.u32_at(24),
            self.u32_at(28),
            self.u32_at(32),
            self.u64_at(40),
            self.u64_at(48),
        )
    }
}

fn first_difference(actual: &[u32], expected: &[u32]) -> Option<(usize, u32, u32)> {
    actual
        .iter()
        .zip(expected)
        .enumerate()
        .find(|(_, (a, e))| a != e)
        .map(|(bin, (&a, &e))| (bin, a, e))
}

#[test]
fn pq_hist_records_are_each_frames_max_rgb_histogram_and_change_nothing_else() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let (clip, planes) = coloured_clip(dir.path());
    for name in ["plain", "dumped"] {
        std::fs::create_dir(dir.path().join(name)).unwrap();
    }
    let clip_arg = clip.to_str().unwrap();
    assert_success(&analyze_in(dir.path(), &[clip_arg, "-o", "plain/out.bin"]));
    assert_success(&analyze_in(
        dir.path(),
        &[
            clip_arg,
            "-o",
            "dumped/out.bin",
            "--dump-pq-hist",
            "out.pqhist",
        ],
    ));

    let dump = Dump::read(&dir.path().join("out.pqhist"));
    assert_eq!(dump.fields(), (0, 0, 1, 1, FRAMES as u64, 0));
    assert_eq!(dump.records.len(), FRAMES);
    for (frame, (record, plane)) in dump.records.iter().zip(&planes).enumerate() {
        let expected = expected_histogram(plane, false);
        assert_eq!(
            first_difference(record, &expected),
            None,
            "frame {frame}: (bin, dumped, expected)"
        );
        assert_eq!(
            record.iter().map(|&c| u64::from(c)).sum::<u64>(),
            (W * H) as u64
        );
        // Max-RGB differs from luma on this chroma, so a luma histogram would fail above.
        assert_ne!(expected, expected_histogram(plane, true));
    }
    for file in ["out.bin", "out.bin.l1.json"] {
        assert!(
            std::fs::read(dir.path().join("plain").join(file)).unwrap()
                == std::fs::read(dir.path().join("dumped").join(file)).unwrap(),
            "{file} changed with --dump-pq-hist"
        );
    }
}

#[test]
fn pq_hist_luma_domain_with_sample_rate_repeats_the_measured_record() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let (clip, planes) = coloured_clip(dir.path());
    let output = analyze_in(
        dir.path(),
        &[
            clip.to_str().unwrap(),
            "-o",
            "out.bin",
            "--peak-domain",
            "luma",
            "--sample-rate",
            "2",
            "--dump-pq-hist",
            "out.pqhist",
        ],
    );
    assert_success(&output);
    let dump = Dump::read(&dir.path().join("out.pqhist"));
    assert_eq!(dump.fields(), (1, 0, 2, 1, FRAMES as u64, 0));
    for frame in 0..FRAMES {
        // Frames 1, 3 and 5 are skipped and repeat the frame before.
        let measured = frame - frame % 2;
        let expected = expected_histogram(&planes[measured], true);
        assert_eq!(
            first_difference(&dump.records[frame], &expected),
            None,
            "frame {frame}: (bin, dumped, expected)"
        );
    }
    assert_ne!(dump.records[0], dump.records[2]);
}

#[test]
fn pq_hist_hlg_and_downscale_are_recorded_in_the_header() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let planes: Vec<Vec<u16>> = (0..3)
        .map(|k| luma_plane(&(64..=940).step_by(7).collect::<Vec<u16>>(), k))
        .collect();
    let clip = dir.path().join("hlg.mkv");
    encode(&clip, &planes, 512, 512, "arib-std-b67");
    let output = analyze_in(
        dir.path(),
        &[
            clip.to_str().unwrap(),
            "-o",
            "out.bin",
            "--transfer",
            "hlg",
            "--downscale",
            "2",
            "--dump-pq-hist",
            "out.pqhist",
        ],
    );
    assert_success(&output);
    let dump = Dump::read(&dir.path().join("out.pqhist"));
    assert_eq!(dump.fields(), (0, 1, 1, 2, 3, 0));
    for record in &dump.records {
        let total: u64 = record.iter().map(|&c| u64::from(c)).sum();
        assert_eq!(total, ((W / 2) * (H / 2)) as u64);
    }
}

#[test]
fn pq_hist_an_untagged_input_is_measured_as_pq_and_recorded_as_transfer_0() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let codes = usable_codes();
    let planes: Vec<Vec<u16>> = (0..2).map(|k| luma_plane(&codes, k)).collect();
    let clip = dir.path().join("untagged.mkv");
    encode(&clip, &planes, CB, CR, "unspecified");
    let output = analyze_in(
        dir.path(),
        &[
            clip.to_str().unwrap(),
            "-o",
            "out.bin",
            "--dump-pq-hist",
            "out.pqhist",
        ],
    );
    assert_success(&output);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Transfer function unspecified"),
        "the clip must take the untagged path"
    );
    let dump = Dump::read(&dir.path().join("out.pqhist"));
    assert_eq!(dump.fields(), (0, 0, 1, 1, 2, 0));
    for (record, plane) in dump.records.iter().zip(&planes) {
        assert_eq!(
            first_difference(record, &expected_histogram(plane, false)),
            None
        );
    }
}

#[test]
fn pq_hist_a_dump_path_naming_another_file_of_the_run_is_refused_before_anything_is_written() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let planes = vec![luma_plane(&usable_codes(), 0)];
    let clip = root.join("clip.mkv");
    encode(&clip, &planes, CB, CR, "smpte2084");
    let clip_bytes = std::fs::read(&clip).unwrap();
    // Files of an earlier run that a refused run must leave alone.
    std::fs::write(root.join("out.bin"), b"earlier bin").unwrap();
    std::fs::write(root.join("out.bin.au.json"), b"earlier table").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("out.bin"), root.join("alias.bin")).unwrap();

    let refused = |args: &[&str], what: &str| {
        let output = analyze_in(root, args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?} was accepted");
        assert!(
            stderr.contains("--dump-pq-hist") && stderr.contains(what),
            "{args:?}: {stderr}"
        );
    };
    let base = ["clip.mkv", "-o", "out.bin", "--dump-pq-hist"];
    let with = |dump: &'static str| [&base[..], &[dump]].concat();
    refused(&with("out.bin"), "the measurement file");
    refused(&with("./out.bin"), "the measurement file");
    refused(&with("missing/../out.bin"), "the measurement file");
    #[cfg(unix)]
    refused(&with("alias.bin"), "the measurement file");
    refused(&with("out.bin.l1.json"), "the L1 sidecar");
    refused(&with("out.bin.au.json"), "the AU table path");
    refused(&with("clip.mkv"), "the input");
    refused(
        &[
            &with("./stats.csv")[..],
            &["--dump-frame-stats", "stats.csv"],
        ]
        .concat(),
        "the --dump-frame-stats CSV",
    );
    // Without -o the measurement file is <stem>_measurements.bin in the working directory.
    refused(
        &["clip.mkv", "--dump-pq-hist", "clip_measurements.bin"],
        "the measurement file",
    );

    assert_eq!(std::fs::read(&clip).unwrap(), clip_bytes);
    assert_eq!(std::fs::read(root.join("out.bin")).unwrap(), b"earlier bin");
    assert_eq!(
        std::fs::read(root.join("out.bin.au.json")).unwrap(),
        b"earlier table"
    );
    for absent in ["out.bin.l1.json", "stats.csv", "clip_measurements.bin"] {
        assert!(!root.join(absent).exists(), "{absent} was written");
    }
    // A path of its own is accepted.
    assert_success(&analyze_in(root, &with("out.pqhist")));
    assert_eq!(Dump::read(&root.join("out.pqhist")).records.len(), 1);
}

/// ffmpeg with libx265 and mkvmerge, for the open-GOP cut.
fn missing_open_gop_tools() -> Option<String> {
    if Command::new("mkvmerge").arg("--version").output().is_err() {
        return Some("mkvmerge not found in PATH".into());
    }
    let encoders = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()?;
    (!String::from_utf8_lossy(&encoders.stdout).contains("libx265"))
        .then(|| "ffmpeg has no libx265 encoder".into())
}

fn run_ok(command: &mut Command) {
    let output = command.output().expect("start tool");
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn pq_hist_an_open_gop_cut_records_its_leading_pictures() {
    require_ffmpeg!();
    if let Some(reason) = missing_open_gop_tools() {
        eprintln!("Skipping open-GOP dump test: {reason}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("open.hevc");
    // As `au_table.rs` `open_gop_cut_counts_its_leading_pictures`: x265 puts RASL pictures
    // before each CRA, and the mkvmerge cut starts at one.
    run_ok(Command::new("ffmpeg").args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "nullsrc=s=256x144:r=24,format=yuv420p10le,\
         geq=lum='if(lt(N\\,40)\\,200\\,700)+mod(X+N\\,32)':cb='512':cr='512'",
        "-frames:v",
        "96",
        "-c:v",
        "libx265",
        "-preset",
        "ultrafast",
        "-x265-params",
        "log-level=error:keyint=24:min-keyint=24:open-gop=1:bframes=3:b-adapt=0:scenecut=0:\
         colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:range=limited:repeat-headers=1",
        "-pix_fmt",
        "yuv420p10le",
        "-f",
        "hevc",
        "-y",
        raw.to_str().unwrap(),
    ]));
    let full = dir.path().join("full.mkv");
    run_ok(
        Command::new("mkvmerge")
            .arg("-q")
            .arg("-o")
            .arg(&full)
            .args(["--compression", "0:none"])
            .arg(&raw),
    );
    let cut = dir.path().join("cut.mkv");
    run_ok(
        Command::new("mkvmerge")
            .arg("-q")
            .arg("-o")
            .arg(&cut)
            .args(["--split", "parts:00:00:01-00:00:10"])
            .arg(&full),
    );
    let output = analyze_in(
        dir.path(),
        &[
            cut.to_str().unwrap(),
            "-o",
            "cut.bin",
            "--dump-pq-hist",
            "cut.pqhist",
        ],
    );
    assert_success(&output);
    let sidecar: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("cut.bin.l1.json")).unwrap())
            .unwrap();
    let leading = sidecar["source"]["leading_skipped_frames"]
        .as_u64()
        .unwrap();
    let frames = sidecar["frames"]["min_pq_12bit"].as_array().unwrap().len();
    assert!(leading > 0, "the cut has no leading pictures");
    let dump = Dump::read(&dir.path().join("cut.pqhist"));
    let (_, _, _, _, count, dump_leading) = dump.fields();
    assert_eq!((count, dump_leading), (frames as u64, leading));
    assert_eq!(dump.records.len(), frames);
}
