//! The in-process injector against `dovi_tool inject-rpu` 2.3.4 (ROADMAP E20.1). Each case
//! encodes a stream with x265 (through ffmpeg), gives one `RPU.bin` from `dovi_tool generate`
//! to both injectors and needs the same bytes: once from the raw file, once from ffmpeg's pipe
//! out of an MKV that mkvmerge muxed, against dovi_tool on the file ffmpeg extracts with the
//! same arguments. The expected bytes are always dovi_tool's. The injector API is called
//! directly, so no fallback can stand in for it.
//!
//! Skipped (with a message) when ffmpeg with libx265, mkvmerge, dovi_tool or hdr10plus_tool is
//! missing; CI has them (`.github/actions/setup-test-tools`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;

use super::*;

/// The reason to skip, or `None` when every tool is there.
fn missing_tool() -> Option<String> {
    for (tool, arg) in [
        ("ffmpeg", "-version"),
        ("mkvmerge", "--version"),
        ("dovi_tool", "--version"),
        ("hdr10plus_tool", "--version"),
    ] {
        if Command::new(tool).arg(arg).output().is_err() {
            return Some(format!("{tool} not found in PATH"));
        }
    }
    let encoders = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()?;
    if !String::from_utf8_lossy(&encoders.stdout).contains("libx265") {
        return Some("ffmpeg has no libx265 encoder".into());
    }
    None
}

macro_rules! require_tools {
    ($test:expr) => {
        if let Some(reason) = missing_tool() {
            println!("Skipping {}: {reason}", $test);
            return;
        }
    };
}

fn run(command: &mut Command) -> Output {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to start {command:?}: {error}"));
    assert!(
        output.status.success(),
        "{command:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

const PQ: &str = "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:range=limited";
/// Moving detail, so that pictures differ and B-frames are used.
const PATTERN: &str = "nullsrc=s=256x144:r=24,format=yuv420p10le,\
                       geq=lum='200+mod(X*2+N*7\\,512)':cb='512+mod(Y+N\\,64)':cr='512'";

/// `<dir>/<name>.hevc`: `frames` pictures of `source` from libx265 with these parameters.
fn encode(dir: &Path, name: &str, source: &str, frames: u32, x265: &str) -> PathBuf {
    let raw = dir.join(format!("{name}.hevc"));
    run(Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            source,
        ])
        .args(["-frames:v", &frames.to_string(), "-c:v", "libx265"])
        .args(["-preset", "ultrafast", "-x265-params"])
        .arg(format!("log-level=error:scenecut=0:{x265}"))
        .args(["-pix_fmt", "yuv420p10le", "-f", "hevc", "-y"])
        .arg(&raw));
    raw
}

/// The NAL units of an Annex B stream (from the byte after each `00 00 01` to the next).
fn nal_units(stream: &[u8]) -> Vec<&[u8]> {
    let starts: Vec<usize> = (0..stream.len().saturating_sub(2))
        .filter(|&at| stream[at..].starts_with(&[0, 0, 1]))
        .map(|at| at + 3)
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).map_or(stream.len(), |next| next - 3);
            let mut unit = &stream[start..end];
            while let [rest @ .., 0] = unit {
                unit = rest;
            }
            unit
        })
        .collect()
}

fn nal_type(unit: &[u8]) -> u8 {
    (unit[0] >> 1) & 0x3f
}

/// The pictures of `stream`, counted without the parser under test: base-layer VCL NAL units
/// (types 0-9 and 16-21) whose `first_slice_segment_in_pic_flag` is set.
fn pictures(stream: &[u8]) -> usize {
    nal_units(stream)
        .iter()
        .filter(|unit| unit.len() > 2 && matches!(nal_type(unit), 0..=9 | 16..=21))
        .filter(|unit| unit[0] & 1 == 0 && unit[1] >> 3 == 0 && unit[2] & 0x80 != 0)
        .count()
}

/// The VCL NAL unit types in use (ITU-T H.265 table 7-1).
const VCL: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 16, 17, 18, 19, 20, 21];

fn count_type(stream: &[u8], types: &[u8]) -> usize {
    nal_units(stream)
        .iter()
        .filter(|unit| !unit.is_empty() && types.contains(&nal_type(unit)))
        .count()
}

/// `<dir>/<name>`: `length` RPUs from `dovi_tool generate`, each with its own L1, so an RPU in
/// the wrong picture shows.
fn generate_rpu(dir: &Path, name: &str, length: usize, profile: &str) -> PathBuf {
    let shots: Vec<_> = (0..length)
        .map(|frame| {
            json!({"start": frame, "duration": 1, "metadata_blocks": [{"Level1": {
                "min_pq": 0, "max_pq": 2100 + 7 * (frame % 100), "avg_pq": 900 + 3 * (frame % 100)
            }}]})
        })
        .collect();
    let config = dir.join(format!("{name}.json"));
    fs::write(
        &config,
        json!({"cm_version": "V40", "length": length, "profile": profile, "shots": shots})
            .to_string(),
    )
    .unwrap();
    let rpu = dir.join(name);
    run(Command::new("dovi_tool")
        .arg("generate")
        .arg("-j")
        .arg(&config)
        .arg("-o")
        .arg(&rpu));
    rpu
}

/// dovi_tool's injection of `rpu` into the raw stream `bl`, and whether it reported mismatched
/// lengths.
fn dovi_tool_inject(dir: &Path, bl: &Path, rpu: &Path) -> (Vec<u8>, bool) {
    let out = dir.join("dovi_tool_BL_RPU.hevc");
    let output = run(Command::new("dovi_tool")
        .arg("inject-rpu")
        .arg("-i")
        .arg(bl)
        .arg("--rpu-in")
        .arg(rpu)
        .arg("-o")
        .arg(&out));
    let mismatched = String::from_utf8_lossy(&output.stdout).contains("mismatched lengths");
    let bytes = fs::read(&out).unwrap();
    fs::remove_file(out).unwrap();
    (bytes, mismatched)
}

/// The in-process injection of `rpu` into `source`, mismatches not refused.
fn internal(source: &mut Source, rpu: &Path, chunk_size: usize) -> (Report, Vec<u8>) {
    let rpus = Rpus::prepare(rpu).unwrap();
    let (outcome, out) = inject_with(source, &rpus, chunk_size, false, &mut |_| {});
    let Outcome::Injected(report) = outcome.unwrap() else {
        unreachable!("mismatches are not refused here");
    };
    (report, out)
}

/// The ffmpeg arguments of the conversion's base layer (`pipeline::deliver`), writing to
/// `output` (`-` = stdout).
fn ffmpeg_copy(mkv: &Path, bsf: Option<&str>, output: &Path) -> Command {
    let mut command = Command::new("ffmpeg");
    command
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(mkv)
        .args(["-map", "0:v:0", "-c:v", "copy"]);
    if let Some(bsf) = bsf {
        command.args(["-bsf:v", bsf]);
    }
    command.args(["-f", "hevc"]);
    if output != Path::new("-") {
        command.arg("-y");
    }
    command.arg(output);
    command
}

/// `actual` equals dovi_tool's `expected`, else the case and the first differing byte.
fn assert_same(case: &str, actual: &[u8], expected: &[u8]) {
    if actual != expected {
        let at = actual
            .iter()
            .zip(expected)
            .position(|(a, b)| a != b)
            .unwrap_or(actual.len().min(expected.len()));
        panic!(
            "{case}: differs from dovi_tool at byte {at} ({} bytes, dovi_tool {})",
            actual.len(),
            expected.len()
        );
    }
}

/// Runs both injectors on `raw` with RPUs of each of `lengths` (`None`: one per picture), from
/// the raw file (two chunk sizes) and through ffmpeg from an MKV of it, with `bsf` on ffmpeg.
fn check(
    dir: &Path,
    case: &str,
    raw: &Path,
    lengths: &[Option<usize>],
    profile: &str,
    bsf: Option<&str>,
) {
    let stream = fs::read(raw).unwrap();
    let frames = pictures(&stream);
    assert!(frames > 0, "{case}: no pictures");
    let mkv = dir.join(format!("{case}.mkv"));
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&mkv)
        .arg(raw));
    let extracted = dir.join(format!("{case}.BL.hevc"));
    run(&mut ffmpeg_copy(&mkv, bsf, &extracted));
    assert_eq!(
        pictures(&fs::read(&extracted).unwrap()),
        frames,
        "{case}: the MKV keeps every picture"
    );
    for &length in lengths {
        let length = length.unwrap_or(frames);
        let rpu = generate_rpu(dir, &format!("{case}-{length}.RPU.bin"), length, profile);
        let label = format!("{case}, {length} RPUs for {frames} pictures");

        let (expected, mismatched) = dovi_tool_inject(dir, raw, &rpu);
        assert_eq!(
            mismatched,
            length != frames,
            "{label}: dovi_tool's own count"
        );
        for chunk_size in [CHUNK_SIZE, 4096] {
            let (report, actual) =
                internal(&mut Source::RawFile(raw.to_path_buf()), &rpu, chunk_size);
            assert_same(
                &format!("{label}, raw file, chunk {chunk_size}"),
                &actual,
                &expected,
            );
            assert_eq!((report.frames, report.rpus), (frames, length), "{label}");
        }

        let (expected, _) = dovi_tool_inject(dir, &extracted, &rpu);
        let mut source = Source::Ffmpeg {
            command: ffmpeg_copy(&mkv, bsf, Path::new("-")),
            logs: [dir.join("pass1.log"), dir.join("pass2.log")],
        };
        let (_, actual) = internal(&mut source, &rpu, CHUNK_SIZE);
        assert_same(
            &format!("{label}, ffmpeg pipe from MKV"),
            &actual,
            &expected,
        );
    }
}

/// One RPU per picture: the plain case.
const EQUAL: &[Option<usize>] = &[None];

#[test]
fn a_closed_gop_with_b_frames_equals_dovi_tool() {
    require_tools!("a_closed_gop_with_b_frames_equals_dovi_tool");
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "closed",
        PATTERN,
        48,
        &format!("keyint=24:min-keyint=24:open-gop=0:bframes=3:b-adapt=0:{PQ}"),
    );
    check(dir.path(), "closed", &raw, EQUAL, "8.1", None);
}

/// An open GOP: a CRA every 12 pictures with RASL pictures before it in display order.
fn open_gop(dir: &Path) -> PathBuf {
    let raw = encode(
        dir,
        "open",
        PATTERN,
        48,
        &format!("keyint=12:min-keyint=12:open-gop=1:bframes=3:b-adapt=0:repeat-headers=1:{PQ}"),
    );
    let stream = fs::read(&raw).unwrap();
    assert!(
        count_type(&stream, &[21]) > 0,
        "the stream has CRA pictures"
    );
    assert!(
        count_type(&stream, &[8, 9]) > 0,
        "the stream has RASL pictures"
    );
    raw
}

#[test]
fn an_open_gop_with_rasl_pictures_and_rpus_of_every_length_equals_dovi_tool() {
    require_tools!("an_open_gop_with_rasl_pictures_and_rpus_of_every_length_equals_dovi_tool");
    let dir = tempfile::tempdir().unwrap();
    let raw = open_gop(dir.path());
    let frames = pictures(&fs::read(&raw).unwrap());
    check(
        dir.path(),
        "open",
        &raw,
        &[None, Some(frames - 7), Some(frames + 7)],
        "8.1",
        None,
    );
}

#[test]
fn a_cut_that_starts_at_a_cra_with_its_rasl_pictures_equals_dovi_tool() {
    require_tools!("a_cut_that_starts_at_a_cra_with_its_rasl_pictures_equals_dovi_tool");
    let dir = tempfile::tempdir().unwrap();
    let stream = fs::read(open_gop(dir.path())).unwrap();
    // The cut starts at the VPS before the second CRA: its first pictures in decode order are
    // the CRA and RASL pictures that reference the GOP the cut removed.
    let units = nal_units(&stream);
    let offset = |unit: &[u8]| unit.as_ptr() as usize - stream.as_ptr() as usize;
    let second_cra = units
        .iter()
        .filter(|unit| nal_type(unit) == 21 && unit[2] & 0x80 != 0)
        .nth(1)
        .expect("a second CRA");
    let vps = units
        .iter()
        .rev()
        .find(|unit| nal_type(unit) == 32 && offset(unit) < offset(second_cra))
        .expect("parameter sets before the CRA");
    let cut = &stream[offset(vps) - 4..];
    assert_eq!(&cut[..4], &[0, 0, 0, 1]);
    let after_cra = &cut[offset(second_cra) - offset(vps) + 4..];
    let next_vcl = nal_units(after_cra)
        .into_iter()
        .find(|unit| matches!(nal_type(unit), 0..=9 | 16..=21))
        .unwrap();
    assert!(
        matches!(nal_type(next_vcl), 8 | 9),
        "a RASL picture follows the CRA"
    );
    let raw = dir.path().join("cut.hevc");
    fs::write(&raw, cut).unwrap();
    check(dir.path(), "cut", &raw, EQUAL, "8.1", None);
}

#[test]
fn a_picture_order_count_that_wraps_equals_dovi_tool() {
    require_tools!("a_picture_order_count_that_wraps_equals_dovi_tool");
    let dir = tempfile::tempdir().unwrap();
    // x265 raises log2-max-poc-lsb to what the GOP needs (6 bits here), and an open GOP never
    // resets the count after the first IDR: 150 pictures wrap it twice.
    let raw = encode(
        dir.path(),
        "poc",
        PATTERN,
        150,
        &format!("keyint=48:min-keyint=48:open-gop=1:log2-max-poc-lsb=4:bframes=2:b-adapt=0:{PQ}"),
    );
    let trace = Command::new("ffmpeg")
        .args(["-hide_banner", "-i"])
        .arg(&raw)
        .args(["-c", "copy", "-bsf:v", "trace_headers", "-f", "null", "-"])
        .output()
        .unwrap();
    let trace = String::from_utf8_lossy(&trace.stderr);
    let lsb_bits = trace
        .lines()
        .find(|line| line.contains("log2_max_pic_order_cnt_lsb_minus4"))
        .and_then(|line| line.split_whitespace().last()?.parse::<u32>().ok())
        .expect("the SPS in the trace")
        + 4;
    let stream = fs::read(&raw).unwrap();
    let idrs = nal_units(&stream)
        .iter()
        .filter(|unit| matches!(nal_type(unit), 19 | 20) && unit[2] & 0x80 != 0)
        .count();
    assert_eq!(idrs, 1, "only the first picture resets the count");
    assert!(
        pictures(&stream) > 1 << lsb_bits,
        "{lsb_bits}-bit POC LSB must wrap"
    );
    check(dir.path(), "poc", &raw, EQUAL, "8.1", None);
}

#[test]
fn idr_resets_and_two_concatenated_streams_equal_dovi_tool() {
    require_tools!("idr_resets_and_two_concatenated_streams_equal_dovi_tool");
    let dir = tempfile::tempdir().unwrap();
    let params = format!("keyint=5:min-keyint=5:open-gop=0:bframes=2:b-adapt=0:{PQ}");
    let first = fs::read(encode(dir.path(), "a", PATTERN, 23, &params)).unwrap();
    let second = fs::read(encode(
        dir.path(),
        "b",
        "nullsrc=s=256x144:r=24,format=yuv420p10le,geq=lum='600-mod(Y*3+N*5\\,400)':cb='512':cr='540'",
        17,
        &params,
    ))
    .unwrap();
    let raw = dir.path().join("concat.hevc");
    fs::write(&raw, [first, second].concat()).unwrap();
    check(dir.path(), "concat", &raw, EQUAL, "8.1", None);
}

#[test]
fn existing_auds_and_repeated_headers_equal_dovi_tool() {
    require_tools!("existing_auds_and_repeated_headers_equal_dovi_tool");
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "aud",
        PATTERN,
        36,
        &format!("keyint=12:min-keyint=12:bframes=3:b-adapt=0:aud=1:repeat-headers=1:{PQ}"),
    );
    let stream = fs::read(&raw).unwrap();
    assert_eq!(
        count_type(&stream, &[35]),
        pictures(&stream),
        "one AUD per access unit"
    );
    assert!(count_type(&stream, &[32]) > 1, "the VPS is repeated");
    check(dir.path(), "aud", &raw, EQUAL, "8.1", None);
}

#[test]
fn four_slices_and_a_suffix_sei_per_picture_equal_dovi_tool() {
    require_tools!("four_slices_and_a_suffix_sei_per_picture_equal_dovi_tool");
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "slices",
        PATTERN,
        30,
        &format!("keyint=15:min-keyint=15:bframes=2:b-adapt=0:slices=4:hash=1:{PQ}"),
    );
    let stream = fs::read(&raw).unwrap();
    let frames = pictures(&stream);
    assert_eq!(
        count_type(&stream, &VCL),
        4 * frames,
        "four slices per picture"
    );
    assert_eq!(
        count_type(&stream, &[40]),
        frames,
        "a suffix SEI per picture"
    );
    check(dir.path(), "slices", &raw, EQUAL, "8.1", None);
}

#[test]
fn end_of_sequence_end_of_bitstream_and_mixed_start_codes_equal_dovi_tool() {
    require_tools!("end_of_sequence_end_of_bitstream_and_mixed_start_codes_equal_dovi_tool");
    let dir = tempfile::tempdir().unwrap();
    let params = format!("keyint=12:min-keyint=12:open-gop=0:bframes=2:b-adapt=0:{PQ}");
    let first = fs::read(encode(dir.path(), "a", PATTERN, 20, &params)).unwrap();
    let mut second = fs::read(encode(dir.path(), "b", PATTERN, 16, &params)).unwrap();
    // The second stream starts with a three-byte start code, its EOS with three bytes and the
    // EOB with four; x265 already mixes both lengths inside each stream.
    assert_eq!(&second[..4], &[0, 0, 0, 1]);
    second.remove(0);
    let eos = [0, 0, 1, 36 << 1, 1];
    let eob = [0, 0, 0, 1, 37 << 1, 1];
    let spliced = [&first[..], &eos, &second, &eob].concat();
    assert_eq!(count_type(&spliced, &[36]), 1);
    assert_eq!(count_type(&spliced, &[37]), 1);
    let raw = dir.path().join("spliced.hevc");
    fs::write(&raw, spliced).unwrap();
    check(dir.path(), "spliced", &raw, EQUAL, "8.1", None);
}

#[test]
fn an_input_that_already_carries_rpus_equals_dovi_tool() {
    require_tools!("an_input_that_already_carries_rpus_equals_dovi_tool");
    let dir = tempfile::tempdir().unwrap();
    let plain = encode(
        dir.path(),
        "plain",
        PATTERN,
        30,
        &format!("keyint=15:min-keyint=15:bframes=2:b-adapt=0:{PQ}"),
    );
    let frames = pictures(&fs::read(&plain).unwrap());
    let old = generate_rpu(dir.path(), "old.RPU.bin", frames + 3, "8.1");
    let (with_rpus, _) = dovi_tool_inject(dir.path(), &plain, &old);
    assert_eq!(
        count_type(&with_rpus, &[62]),
        frames,
        "the input carries an RPU per picture"
    );
    let raw = dir.path().join("with_rpus.hevc");
    fs::write(&raw, with_rpus).unwrap();
    check(dir.path(), "with_rpus", &raw, EQUAL, "8.1", None);
}

#[test]
fn hdr10plus_seis_are_kept_as_dovi_tool_keeps_them() {
    require_tools!("hdr10plus_seis_are_kept_as_dovi_tool_keeps_them");
    let dir = tempfile::tempdir().unwrap();
    let plain = encode(
        dir.path(),
        "plain",
        PATTERN,
        30,
        &format!("keyint=15:min-keyint=15:bframes=3:b-adapt=0:{PQ}:hdr10=1:max-cll=1000,400"),
    );
    let frames = pictures(&fs::read(&plain).unwrap());
    let scenes: Vec<_> = (0..frames)
        .map(|frame| {
            json!({
                "BezierCurveData": {"Anchors": [102, 205, 307, 410, 512, 614, 717, 819, 922],
                                    "KneePointX": 0, "KneePointY": 0},
                "LuminanceParameters": {
                    "AverageRGB": 300 + frame,
                    "LuminanceDistributions": {
                        "DistributionIndex": [1, 5, 10, 25, 50, 75, 90, 95, 99],
                        "DistributionValues": [0, 10, 100, 200, 300, 400, 500, 600, 10000],
                    },
                    "MaxScl": [0, 0, 0],
                },
                "NumberOfWindows": 1,
                "TargetedSystemDisplayMaximumLuminance": 400,
                "SceneFrameIndex": frame,
                "SceneId": 0,
                "SequenceFrameIndex": frame,
            })
        })
        .collect();
    let metadata = dir.path().join("hdr10plus.json");
    fs::write(
        &metadata,
        json!({
            "JSONInfo": {"HDR10plusProfile": "B", "Version": "1.0"},
            "SceneInfo": scenes,
            "SceneInfoSummary": {"SceneFirstFrameIndex": [0], "SceneFrameNumbers": [frames]},
            "ToolInfo": {"Tool": "hdr10plus_tool", "Version": "1.7.2"},
        })
        .to_string(),
    )
    .unwrap();
    let raw = dir.path().join("hdr10plus.hevc");
    run(Command::new("hdr10plus_tool")
        .arg("inject")
        .arg("-i")
        .arg(&plain)
        .arg("-j")
        .arg(&metadata)
        .arg("-o")
        .arg(&raw));
    let prefix_seis = |path: &Path| count_type(&fs::read(path).unwrap(), &[39]);
    assert_eq!(
        prefix_seis(&raw),
        prefix_seis(&plain) + frames,
        "an HDR10+ SEI per picture"
    );
    check(dir.path(), "hdr10plus", &raw, EQUAL, "8.1", None);
}

#[test]
fn hlg_with_the_vui_edit_equals_dovi_tool() {
    require_tools!("hlg_with_the_vui_edit_equals_dovi_tool");
    let dir = tempfile::tempdir().unwrap();
    // HLG tagged only in the container: the bitstream says BT.709, and the conversion's ffmpeg
    // rewrites the VUI (`pipeline::deliver`).
    let raw = encode(
        dir.path(),
        "hlg",
        PATTERN,
        30,
        "keyint=15:min-keyint=15:bframes=2:b-adapt=0:colorprim=bt2020:transfer=bt709:colormatrix=bt2020nc:range=limited",
    );
    let edit = "hevc_metadata=colour_primaries=9:transfer_characteristics=18:matrix_coefficients=9";
    let mkv = dir.path().join("probe.mkv");
    run(Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&mkv)
        .arg(&raw));
    let (plain, edited) = (
        dir.path().join("plain.hevc"),
        dir.path().join("edited.hevc"),
    );
    run(&mut ffmpeg_copy(&mkv, None, &plain));
    run(&mut ffmpeg_copy(&mkv, Some(edit), &edited));
    assert_ne!(
        fs::read(plain).unwrap(),
        fs::read(edited).unwrap(),
        "the edit changes the stream"
    );
    check(dir.path(), "hlg", &raw, EQUAL, "8.4", Some(edit));
}

#[test]
fn a_stream_of_several_chunks_equals_dovi_tool() {
    require_tools!("a_stream_of_several_chunks_equals_dovi_tool");
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "large",
        "testsrc2=s=640x360:r=24,format=yuv420p10le",
        72,
        &format!("keyint=24:min-keyint=24:bframes=3:b-adapt=0:crf=12:{PQ}"),
    );
    let size = fs::metadata(&raw).unwrap().len();
    assert!(
        size > 300_000,
        "{size} bytes: more than three chunks of 100,000"
    );
    check(dir.path(), "large", &raw, EQUAL, "8.1", None);
}
