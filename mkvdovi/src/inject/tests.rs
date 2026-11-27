use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use hevc_parser::hevc::{Frame, NALUnit};

use super::frame_index::{rpu_index, Entry, FrameIndex};
use super::interleave::Interleaver;
use super::source::{parse_stream, FillReader};
use super::*;

mod equivalence;

/// `mkvdovi/tests/fixtures/inject/`, written by its `make_fixtures.sh` with dovi_tool.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/inject")
        .join(name)
}

const FIXTURE_FRAMES: usize = 36;

fn frame(decoded: u64, presentation: u64, frame_type: u64) -> Frame {
    Frame {
        decoded_number: decoded,
        presentation_number: presentation,
        frame_type,
        ..Frame::default()
    }
}

#[test]
fn the_frame_index_maps_decoded_numbers_to_presentation_and_type() {
    let index = FrameIndex::from_frames(&[frame(1, 0, 0), frame(0, 1, 2), frame(2, 2, 1)]).unwrap();
    assert_eq!(index.len(), 3);
    let entry = |presentation, frame_type| {
        Some(Entry {
            presentation,
            frame_type,
        })
    };
    assert_eq!(index.get(0), entry(1, 2));
    assert_eq!(index.get(1), entry(0, 0));
    assert_eq!(index.get(2), entry(2, 1));
    assert_eq!(index.get(3), None);
}

#[test]
fn the_frame_index_refuses_a_duplicate_a_gap_and_an_out_of_range_number() {
    for (case, frames) in [
        ("duplicate decoded", vec![frame(0, 0, 2), frame(0, 1, 0)]),
        (
            "duplicate presentation",
            vec![frame(0, 1, 2), frame(1, 1, 0)],
        ),
        ("decoded gap", vec![frame(0, 0, 2), frame(2, 1, 0)]),
        ("presentation gap", vec![frame(0, 0, 2), frame(1, 2, 0)]),
        ("out of range", vec![frame(0, 0, 2), frame(u64::MAX, 1, 0)]),
    ] {
        assert!(FrameIndex::from_frames(&frames).is_err(), "{case}");
    }
    assert_eq!(FrameIndex::from_frames(&[]).unwrap().len(), 0);
}

#[test]
fn the_rpu_of_a_picture_is_its_own_else_the_last_only_when_the_lengths_differ() {
    // (presentation, RPUs, mismatched) -> RPU, by dovi_tool's
    // `rpus.get(p).or_else(|| mismatched.then(|| rpus.last()).flatten())`.
    for (presentation, rpus, mismatched, expected) in [
        (0, 3, false, Some(0)),
        (2, 3, false, Some(2)),
        (2, 3, true, Some(2)),
        (3, 3, true, Some(2)),
        (9, 3, true, Some(2)),
        (3, 3, false, None),
        (0, 0, true, None),
    ] {
        assert_eq!(
            rpu_index(presentation, rpus, mismatched),
            expected,
            "presentation {presentation}, {rpus} RPUs, mismatched {mismatched}"
        );
    }
}

/// HEVC NAL unit types (ITU-T H.265 table 7-1).
const TRAIL_R: u8 = 1;
const VPS: u8 = 32;
const SPS: u8 = 33;
const PPS: u8 = 34;
const AUD: u8 = 35;
const EOS: u8 = 36;
const EOB: u8 = 37;
const SUFFIX_SEI: u8 = 40;
const UNSPEC62: u8 = 62;

/// Synthetic parser output: NAL units laid out in one chunk, three-byte start codes.
#[derive(Default)]
struct Events {
    chunk: Vec<u8>,
    nals: Vec<NALUnit>,
}

impl Events {
    /// A NAL unit of `nal_type` in access unit `decoded`, with a payload that names it.
    fn nal(mut self, nal_type: u8, decoded: u64, payload: &[u8]) -> Self {
        self.chunk.extend_from_slice(&[0, 0, 1]);
        let start = self.chunk.len();
        self.chunk.extend_from_slice(&nal_bytes(nal_type, payload));
        self.nals.push(NALUnit {
            start,
            end: self.chunk.len(),
            nal_type,
            decoded_frame_index: decoded,
            ..NALUnit::default()
        });
        self
    }

    /// The interleaver's output for these events with `index`, `rpus` and `stream_frames`.
    fn interleave(
        &self,
        index: &FrameIndex,
        rpus: &[Vec<u8>],
        mismatched: bool,
        stream_frames: usize,
    ) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut interleaver = Interleaver::new(index, rpus, mismatched, &mut out);
        for nal in &self.nals {
            interleaver.push(nal, &self.chunk)?;
        }
        interleaver.finish(stream_frames)?;
        Ok(out)
    }
}

/// A NAL unit: its two-byte header (layer 0, temporal id 0) and `payload`.
fn nal_bytes(nal_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![nal_type << 1, 1];
    bytes.extend_from_slice(payload);
    bytes
}

/// What dovi_tool writes for one NAL unit: a four-byte start code and the unit.
fn written(nal: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0, 0, 0, 1];
    bytes.extend_from_slice(nal);
    bytes
}

/// The AUD of an I, P or B picture: header `46 01`, pic_type 0, 1 or 2, the stop bit
/// (ITU-T H.265 7.3.2.5).
const AUD_I: [u8; 3] = [0x46, 0x01, 0x10];
const AUD_P: [u8; 3] = [0x46, 0x01, 0x30];
const AUD_B: [u8; 3] = [0x46, 0x01, 0x50];

/// Two pictures: decoded 0 is presented second (a P picture), decoded 1 first (a B picture).
fn two_pictures() -> FrameIndex {
    FrameIndex::from_frames(&[frame(0, 1, 1), frame(1, 0, 0)]).unwrap()
}

fn rpus(names: &[&[u8]]) -> Vec<Vec<u8>> {
    names.iter().map(|name| nal_bytes(UNSPEC62, name)).collect()
}

#[test]
fn each_access_unit_gets_an_aud_first_and_its_presentation_rpu_last() {
    let events = Events::default()
        .nal(AUD, 0, &[0x30])
        .nal(VPS, 0, b"vps")
        .nal(SPS, 0, b"sps")
        .nal(PPS, 0, b"pps")
        .nal(TRAIL_R, 0, b"slice 0")
        .nal(SUFFIX_SEI, 0, b"hash 0")
        .nal(EOS, 0, b"")
        // An AUD that opens the next access unit is dropped, not taken for its start.
        .nal(AUD, 1, &[0x50])
        .nal(TRAIL_R, 1, b"slice 1")
        .nal(UNSPEC62, 1, b"old rpu")
        .nal(EOB, 1, b"");
    let rpus = rpus(&[b"rpu 0", b"rpu 1"]);
    let out = events.interleave(&two_pictures(), &rpus, false, 2).unwrap();

    let mut expected = written(&AUD_P);
    for (nal_type, payload) in [
        (VPS, &b"vps"[..]),
        (SPS, b"sps"),
        (PPS, b"pps"),
        (TRAIL_R, b"slice 0"),
        (SUFFIX_SEI, b"hash 0"),
        (EOS, b""),
    ] {
        expected.extend(written(&nal_bytes(nal_type, payload)));
    }
    // Decoded 0 is presentation 1: the second RPU, after EOS.
    expected.extend(written(&nal_bytes(UNSPEC62, b"rpu 1")));
    expected.extend(written(&AUD_B));
    expected.extend(written(&nal_bytes(TRAIL_R, b"slice 1")));
    expected.extend(written(&nal_bytes(EOB, b"")));
    expected.extend(written(&nal_bytes(UNSPEC62, b"rpu 0")));
    assert_eq!(out, expected);
}

#[test]
fn the_aud_follows_the_first_slice_type_of_i_p_and_b_pictures() {
    let index = FrameIndex::from_frames(&[frame(0, 0, 2), frame(1, 1, 1), frame(2, 2, 0)]).unwrap();
    let events = Events::default()
        .nal(TRAIL_R, 0, b"i")
        .nal(TRAIL_R, 1, b"p")
        .nal(TRAIL_R, 2, b"b");
    let out = events
        .interleave(&index, &rpus(&[b"0", b"1", b"2"]), false, 3)
        .unwrap();
    let mut expected = Vec::new();
    for (aud, slice, rpu) in [
        (AUD_I, b"i", b"0"),
        (AUD_P, b"p", b"1"),
        (AUD_B, b"b", b"2"),
    ] {
        expected.extend(written(&aud));
        expected.extend(written(&nal_bytes(TRAIL_R, slice)));
        expected.extend(written(&nal_bytes(UNSPEC62, rpu)));
    }
    assert_eq!(out, expected);
}

#[test]
fn nal_units_after_the_last_picture_are_dropped() {
    // A parameter set after the last access unit opens an access unit with no picture.
    let events = Events::default()
        .nal(TRAIL_R, 0, b"slice 0")
        .nal(TRAIL_R, 1, b"slice 1")
        .nal(VPS, 2, b"trailing vps");
    let out = events
        .interleave(&two_pictures(), &rpus(&[b"a", b"b"]), false, 2)
        .unwrap();
    let mut expected = written(&AUD_P);
    expected.extend(written(&nal_bytes(TRAIL_R, b"slice 0")));
    expected.extend(written(&nal_bytes(UNSPEC62, b"b")));
    expected.extend(written(&AUD_B));
    expected.extend(written(&nal_bytes(TRAIL_R, b"slice 1")));
    expected.extend(written(&nal_bytes(UNSPEC62, b"a")));
    assert_eq!(out, expected);
}

#[test]
fn an_access_unit_outside_the_frame_index_is_an_error() {
    let events = Events::default()
        .nal(TRAIL_R, 0, b"slice 0")
        .nal(TRAIL_R, 5, b"unknown")
        .nal(TRAIL_R, 6, b"next");
    let error = events
        .interleave(&two_pictures(), &rpus(&[b"a", b"b"]), false, 2)
        .unwrap_err();
    assert!(error.to_string().contains("decoded frame 5"), "{error:#}");
}

#[test]
fn a_missing_rpu_is_an_error_unless_the_lengths_differ() {
    let events = Events::default()
        .nal(TRAIL_R, 0, b"slice 0")
        .nal(TRAIL_R, 1, b"slice 1");
    let one = rpus(&[b"only"]);
    let error = events
        .interleave(&two_pictures(), &one, false, 2)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("No RPU found for presentation frame 1"),
        "{error:#}"
    );
    // Mismatched: decoded 0 (presentation 1) repeats the last RPU.
    let out = events.interleave(&two_pictures(), &one, true, 2).unwrap();
    let rpu = written(&nal_bytes(UNSPEC62, b"only"));
    assert_eq!(out.windows(rpu.len()).filter(|w| *w == rpu).count(), 2);
}

/// Records every NAL unit the parser hands over: its type, access unit and bytes.
#[derive(Debug, Default)]
struct Recorder {
    nals: Vec<(u8, u64, Vec<u8>)>,
    frames: usize,
    input: PathBuf,
}

impl IoProcessor for Recorder {
    fn input(&self) -> &PathBuf {
        &self.input
    }

    fn update_progress(&mut self, _delta: u64) {}

    fn process_nals(&mut self, _parser: &HevcParser, nals: &[NALUnit], chunk: &[u8]) -> Result<()> {
        self.nals.extend(nals.iter().map(|nal| {
            (
                nal.nal_type,
                nal.decoded_frame_index,
                chunk[nal.start..nal.end].to_vec(),
            )
        }));
        Ok(())
    }

    fn finalize(&mut self, parser: &HevcParser) -> Result<()> {
        self.frames = parser.ordered_frames().len();
        Ok(())
    }
}

/// A reader that hands out the next of `sizes` bytes per read (the last size repeats), and
/// `Interrupted` before every read when `interrupts`.
struct Scripted {
    data: io::Cursor<Vec<u8>>,
    sizes: Vec<usize>,
    reads: usize,
    interrupts: bool,
    interrupted: bool,
    fail_after: Option<u64>,
}

impl Scripted {
    fn new(data: Vec<u8>, sizes: Vec<usize>) -> Self {
        Self {
            data: io::Cursor::new(data),
            sizes,
            reads: 0,
            interrupts: false,
            interrupted: false,
            fail_after: None,
        }
    }
}

impl Read for Scripted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.interrupts && !self.interrupted {
            self.interrupted = true;
            return Err(io::ErrorKind::Interrupted.into());
        }
        self.interrupted = false;
        if self
            .fail_after
            .is_some_and(|limit| self.data.position() >= limit)
        {
            return Err(io::Error::other("the pipe broke"));
        }
        let size = self.sizes[self.reads.min(self.sizes.len() - 1)];
        self.reads += 1;
        let size = size.min(buf.len());
        self.data.read(&mut buf[..size])
    }
}

fn record(reader: impl Read, chunk_size: usize) -> Result<Recorder> {
    let mut recorder = Recorder::default();
    parse_stream(&mut FillReader::new(reader), &mut recorder, chunk_size)?;
    Ok(recorder)
}

#[test]
fn short_reads_give_the_nal_units_of_one_read() {
    let clip = fs::read(fixture("clip.hevc")).unwrap();
    let whole = record(io::Cursor::new(clip.clone()), 4096).unwrap();
    assert_eq!(whole.frames, FIXTURE_FRAMES);
    for sizes in [vec![1], vec![7], vec![4095, 1, 3000], vec![65536]] {
        let short = record(Scripted::new(clip.clone(), sizes.clone()), 4096).unwrap();
        assert_eq!(short.nals, whole.nals, "reads of {sizes:?}");
        assert_eq!(short.frames, FIXTURE_FRAMES);
    }
}

#[test]
fn a_start_code_split_across_reads_is_found() {
    let clip = fs::read(fixture("clip.hevc")).unwrap();
    let whole = record(io::Cursor::new(clip.clone()), 4096).unwrap();
    // Every read of the first 600 bytes ends inside a start code: after `00 00` of `00 00 01`.
    let mut cuts = Vec::new();
    let mut last = 0;
    for at in (1..600).filter(|&at| clip[at..].starts_with(&[0, 0, 1])) {
        cuts.push(at + 2 - last);
        last = at + 2;
    }
    assert!(cuts.len() >= 5, "the clip starts with several NAL units");
    cuts.push(4096);
    assert_eq!(
        record(Scripted::new(clip, cuts), 4096).unwrap().nals,
        whole.nals
    );
}

#[test]
fn an_interrupted_read_is_retried() {
    let clip = fs::read(fixture("clip.hevc")).unwrap();
    let whole = record(io::Cursor::new(clip.clone()), 4096).unwrap();
    let mut reader = Scripted::new(clip, vec![1000]);
    reader.interrupts = true;
    assert_eq!(record(reader, 4096).unwrap().nals, whole.nals);
}

#[test]
fn a_read_error_fails_the_pass_although_the_parser_takes_it_for_the_end() {
    let clip = fs::read(fixture("clip.hevc")).unwrap();
    let mut reader = Scripted::new(clip, vec![1000]);
    reader.fail_after = Some(5000);
    let error = record(reader, 4096).unwrap_err();
    assert!(error.to_string().contains("the pipe broke"), "{error:#}");
}

/// [`inject`] of the fixture clip read from `source` with the fixture `RPU.bin`.
fn inject_fixture(
    source: &mut Source,
    chunk_size: usize,
    refuse: bool,
) -> (Result<Outcome>, Vec<u8>) {
    inject_with(
        source,
        &Rpus::prepare(&fixture("RPU.bin")).unwrap(),
        chunk_size,
        refuse,
        &mut |_| {},
    )
}

fn inject_with(
    source: &mut Source,
    rpus: &Rpus,
    chunk_size: usize,
    refuse_mismatch: bool,
    announce: &mut dyn FnMut(Pass),
) -> (Result<Outcome>, Vec<u8>) {
    let mut out = Vec::new();
    let outcome = inject(
        Request {
            source,
            rpus,
            refuse_mismatch,
            chunk_size,
            size_hint: None,
            stall_secs: 0,
            announce,
        },
        &mut out,
    );
    (outcome, out)
}

#[test]
fn the_fixture_clip_equals_dovi_tools_injection_at_every_chunk_size() {
    let golden = fs::read(fixture("BL_RPU.hevc")).unwrap();
    for chunk_size in [CHUNK_SIZE, 4096, 257] {
        let (outcome, out) =
            inject_fixture(&mut Source::RawFile(fixture("clip.hevc")), chunk_size, true);
        let Outcome::Injected(report) = outcome.unwrap() else {
            panic!("chunk {chunk_size}: not injected");
        };
        assert_eq!(report.frames, FIXTURE_FRAMES);
        assert_eq!(report.rpus, FIXTURE_FRAMES);
        assert_eq!(report.bytes_written, golden.len() as u64);
        assert_eq!(
            report.bytes_read,
            fs::metadata(fixture("clip.hevc")).unwrap().len()
        );
        assert!(
            out == golden,
            "chunk {chunk_size}: output differs from dovi_tool's"
        );
    }
}

/// The fixture clip written to stdout by `sh -c <script>` (`$CLIP` is its path), as ffmpeg would.
#[cfg(unix)]
fn piped(dir: &Path, script: &str) -> Source {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(script)
        .env("CLIP", fixture("clip.hevc"));
    Source::Ffmpeg {
        command,
        logs: [dir.join("pass1.log"), dir.join("pass2.log")],
    }
}

#[cfg(unix)]
#[test]
fn a_piped_stream_equals_dovi_tools_injection_of_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let (outcome, out) = inject_fixture(&mut piped(dir.path(), "cat \"$CLIP\""), CHUNK_SIZE, true);
    assert!(matches!(outcome.unwrap(), Outcome::Injected(_)));
    assert!(out == fs::read(fixture("BL_RPU.hevc")).unwrap());
}

#[cfg(unix)]
#[test]
fn a_reader_that_fails_after_its_stream_fails_the_injection() {
    let dir = tempfile::tempdir().unwrap();
    let mut source = piped(dir.path(), "cat \"$CLIP\"; exit 3");
    let error = inject_fixture(&mut source, CHUNK_SIZE, true).0.unwrap_err();
    assert!(
        format!("{error:#}").contains("exited with exit status: 3"),
        "{error:#}"
    );
    assert!(format!("{error:#}").contains("pass1.log"), "{error:#}");
}

#[cfg(unix)]
#[test]
fn a_reader_that_stops_early_fails_the_injection_without_hanging() {
    // The parser refuses the first slice (it names a PPS the stream never sent), and the
    // reader never stops writing: it must be ended, not waited for on a full pipe.
    let dir = tempfile::tempdir().unwrap();
    let mut source = piped(
        dir.path(),
        "while :; do printf '\\000\\000\\001\\002\\001\\300'; done",
    );
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(inject_fixture(&mut source, CHUNK_SIZE, true).0.is_err());
    });
    let failed = receiver.recv_timeout(std::time::Duration::from_secs(60));
    assert_eq!(failed, Ok(true));
}

#[test]
fn a_stream_that_changes_between_the_passes_fails_the_injection() {
    let dir = tempfile::tempdir().unwrap();
    let clip = dir.path().join("clip.hevc");
    fs::copy(fixture("clip.hevc"), &clip).unwrap();
    let mut changed = fs::read(&clip).unwrap();
    // A byte of the last SEI payload: the parser does not read it, only the hash sees it.
    let at = changed.len() - 3;
    assert!(changed[at] > 1, "a byte that stays clear of start codes");
    changed[at] ^= 0x80;
    let mut announce = |pass| {
        if pass == Pass::Write {
            fs::write(&clip, &changed).unwrap();
        }
    };
    let rpus = Rpus::prepare(&fixture("RPU.bin")).unwrap();
    let (outcome, _) = inject_with(
        &mut Source::RawFile(clip.clone()),
        &rpus,
        CHUNK_SIZE,
        true,
        &mut announce,
    );
    let error = outcome.unwrap_err();
    assert!(
        error.to_string().contains("changed between the two passes"),
        "{error:#}"
    );
}

/// The fixture `RPU.bin` with its first `keep` RPUs only.
fn first_rpus(dir: &Path, keep: usize) -> Rpus {
    let bytes = fs::read(fixture("RPU.bin")).unwrap();
    let starts: Vec<usize> = (0..bytes.len() - 3)
        .filter(|&at| bytes[at..].starts_with(&[0, 0, 0, 1]))
        .collect();
    assert_eq!(starts.len(), FIXTURE_FRAMES);
    let path = dir.join("RPU.bin");
    fs::write(&path, &bytes[..starts[keep]]).unwrap();
    let rpus = Rpus::prepare(&path).unwrap();
    assert_eq!(rpus.len(), keep);
    rpus
}

#[test]
fn a_length_mismatch_is_refused_before_anything_is_written_only_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let rpus = first_rpus(dir.path(), 30);
    let mut announced = Vec::new();
    let (outcome, out) = inject_with(
        &mut Source::RawFile(fixture("clip.hevc")),
        &rpus,
        CHUNK_SIZE,
        true,
        &mut |pass| announced.push(pass),
    );
    assert_eq!(
        outcome.unwrap(),
        Outcome::LengthMismatch {
            frames: FIXTURE_FRAMES,
            rpus: 30
        }
    );
    assert!(out.is_empty());
    assert_eq!(announced, [Pass::Index]);

    let (outcome, out) = inject_with(
        &mut Source::RawFile(fixture("clip.hevc")),
        &rpus,
        CHUNK_SIZE,
        false,
        &mut |_| {},
    );
    let Outcome::Injected(report) = outcome.unwrap() else {
        panic!("a metadata RPU of another length is injected");
    };
    assert_eq!((report.frames, report.rpus), (FIXTURE_FRAMES, 30));
    assert_eq!(report.bytes_written, out.len() as u64);
}

#[test]
fn an_rpu_file_the_guard_refuses_cannot_be_prepared() {
    let error = Rpus::prepare(&fixture("../linear_interp_RPU.bin")).unwrap_err();
    assert!(rpu_guard::is_unparseable(&error), "{error:#}");
}

/// Compares what is written with a file instead of keeping it: the first differing byte and
/// whether the lengths are equal.
struct CompareWriter {
    reference: io::BufReader<fs::File>,
    position: u64,
    first_difference: Option<u64>,
    reference_ended: bool,
    scratch: Vec<u8>,
}

impl Write for CompareWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.scratch.resize(data.len(), 0);
        let mut got = 0;
        while got < data.len() {
            match self.reference.read(&mut self.scratch[got..])? {
                0 => break,
                n => got += n,
            }
        }
        if self.first_difference.is_none() {
            let differs = (0..got).find(|&i| self.scratch[i] != data[i]);
            if let Some(i) = differs {
                self.first_difference = Some(self.position + i as u64);
            } else if got < data.len() {
                self.first_difference = Some(self.position + got as u64);
                self.reference_ended = true;
            }
        }
        self.position += data.len() as u64;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The episode gate (ROADMAP E20.1): one `RPU.bin` through this injector must give dovi_tool's
/// `BL_RPU.hevc` byte for byte. `MKVDOVI_INJECT_SOURCE` is a `.hevc` (read as a file) or a
/// container (read through ffmpeg's pipe, as the conversion does). Compares while streaming,
/// so nothing large is written, and prints each pass's time.
#[test]
#[ignore = "needs MKVDOVI_INJECT_SOURCE, MKVDOVI_INJECT_RPU and MKVDOVI_INJECT_REFERENCE"]
fn real_stream_matches_reference() {
    let var = |name: &str| {
        PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} is not set")))
    };
    let (input, rpu, reference) = (
        var("MKVDOVI_INJECT_SOURCE"),
        var("MKVDOVI_INJECT_RPU"),
        var("MKVDOVI_INJECT_REFERENCE"),
    );
    let dir = tempfile::tempdir().unwrap();
    let mut source = if input.extension().is_some_and(|ext| ext == "hevc") {
        Source::RawFile(input.clone())
    } else {
        let mut command = Command::new("ffmpeg");
        command
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(&input)
            .args(["-map", "0:v:0", "-c:v", "copy", "-f", "hevc", "-"]);
        Source::Ffmpeg {
            command,
            logs: [dir.path().join("pass1.log"), dir.path().join("pass2.log")],
        }
    };
    let started = Instant::now();
    let rpus = Rpus::prepare(&rpu).unwrap();
    let prepare_secs = started.elapsed().as_secs_f64();
    let mut compare = CompareWriter {
        reference: io::BufReader::with_capacity(1 << 20, fs::File::open(&reference).unwrap()),
        position: 0,
        first_difference: None,
        reference_ended: false,
        scratch: Vec::new(),
    };
    let outcome = inject(
        Request {
            source: &mut source,
            rpus: &rpus,
            refuse_mismatch: false,
            chunk_size: CHUNK_SIZE,
            size_hint: fs::metadata(&input).ok().map(|m| m.len()),
            stall_secs: 0,
            announce: &mut |_| {},
        },
        &mut compare,
    )
    .unwrap();
    let Outcome::Injected(report) = outcome else {
        unreachable!("mismatches are not refused here");
    };
    let reference_len = fs::metadata(&reference).unwrap().len();
    println!(
        "real_stream_matches_reference: {} frames, {} RPUs; RPU parse {prepare_secs:.1} s, pass 1 {:.1} s, pass 2 {:.1} s; read {} bytes, wrote {} bytes, reference {reference_len} bytes",
        report.frames, report.rpus, report.index_secs, report.write_secs, report.bytes_read, report.bytes_written
    );
    assert_eq!(
        compare.first_difference, None,
        "first differing byte (reference ended: {})",
        compare.reference_ended
    );
    assert_eq!(report.bytes_written, reference_len);
    assert_eq!(compare.position, reference_len);
}

/// A processor that panics on the first NAL units it gets, as the pinned parser can on a
/// malformed header in a build that unwinds.
#[derive(Default)]
struct Panicker {
    input: PathBuf,
}

impl IoProcessor for Panicker {
    fn input(&self) -> &PathBuf {
        &self.input
    }

    fn update_progress(&mut self, _delta: u64) {}

    fn process_nals(&mut self, _: &HevcParser, _: &[NALUnit], _: &[u8]) -> Result<()> {
        panic!("parser panic");
    }

    fn finalize(&mut self, _: &HevcParser) -> Result<()> {
        Ok(())
    }
}

#[test]
fn a_panic_while_parsing_unwinds_instead_of_waiting_for_the_watcher() {
    // Before the fix the progress watcher never saw `done`, so the scope that joins it, and the
    // whole pass, waited forever.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(|| {
            let display = Display {
                label: "test",
                total: None,
                stall_secs: 0,
            };
            let mut source = Source::RawFile(fixture("clip.hevc"));
            let _ = run_pass(&mut source, 0, &mut Panicker::default(), 4096, &display);
        });
        let _ = tx.send(result.is_err());
    });
    let panicked = rx.recv_timeout(std::time::Duration::from_secs(20));
    assert_eq!(panicked, Ok(true), "the pass must unwind, not hang");
}

/// Hands out `step` bytes per read and records the progress counter it sees at each read.
struct Trickle<'a> {
    left: usize,
    step: usize,
    progress: &'a std::sync::atomic::AtomicU64,
    seen: Vec<u64>,
}

impl Read for Trickle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.seen
            .push(self.progress.load(std::sync::atomic::Ordering::Relaxed));
        let n = self.step.min(self.left).min(buf.len());
        buf[..n].fill(0);
        self.left -= n;
        Ok(n)
    }
}

#[test]
fn progress_counts_every_inner_read_of_a_slow_pipe() {
    // A pipe that trickles 10 bytes per read into a 100-byte chunk: the bar and the stall
    // warning see the bytes as they come, not only once the chunk is full.
    let progress = std::sync::atomic::AtomicU64::new(0);
    let mut trickle = Trickle {
        left: 100,
        step: 10,
        progress: &progress,
        seen: Vec::new(),
    };
    let mut reader = FillReader::new(&mut trickle).counting_into(&progress);
    let mut chunk = [0u8; 100];
    assert_eq!(reader.read(&mut chunk).unwrap(), 100);
    drop(reader);
    assert_eq!(trickle.seen, (0..10).map(|k| k * 10).collect::<Vec<u64>>());
    assert_eq!(progress.load(std::sync::atomic::Ordering::Relaxed), 100);
}
