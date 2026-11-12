//! The streams here are assembled byte by byte from the HEVC syntax (ITU-T H.265 7.3.1.2 NAL
//! unit header, 7.3.5 SEI message, D.2.6 user_data_registered_itu_t_t35) and the HDR10+ T.35
//! identity of ST 2094-40 Annex (country 0xB5, provider 0x003C, oriented code 0x0001,
//! application 4, version 1). Expected counts come from how each stream was built.

use std::io::Read;

use super::*;

const TRAIL_R: u8 = 1;
const IDR_W_RADL: u8 = 19;
const VPS: u8 = 32;
const SPS: u8 = 33;
const PPS: u8 = 34;
const AUD: u8 = 35;
const IDENTITY: [u8; 7] = [0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04, 0x01];

/// A NAL unit with a 3-byte start code: two header bytes (`nuh_temporal_id_plus1` = 1), then
/// `body` as it is in the stream (already escaped).
fn nal(kind: u8, layer: u8, body: &[u8]) -> Vec<u8> {
    let mut unit = vec![
        0,
        0,
        1,
        (kind << 1) | (layer >> 5),
        ((layer & 0x1F) << 3) | 1,
    ];
    unit.extend_from_slice(body);
    unit
}

fn slice(kind: u8, first_in_picture: bool) -> Vec<u8> {
    // first_slice_segment_in_pic_flag is the top bit of the first byte after the header.
    nal(
        kind,
        0,
        &[if first_in_picture { 0xAF } else { 0x2F }, 0x91, 0x40],
    )
}

/// `ff_byte`-coded value: 0xFF for every whole 255, then the rest.
fn coded(mut value: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    while value >= 255 {
        bytes.push(0xFF);
        value -= 255;
    }
    bytes.push(value as u8);
    bytes
}

fn message(kind: usize, payload: &[u8]) -> Vec<u8> {
    [coded(kind), coded(payload.len()), payload.to_vec()].concat()
}

fn hdr10plus_with_identity(identity: &[u8]) -> Vec<u8> {
    let mut payload = identity.to_vec();
    payload.extend_from_slice(&[0x40, 0x12, 0x34, 0x56]);
    message(4, &payload)
}

fn hdr10plus() -> Vec<u8> {
    hdr10plus_with_identity(&IDENTITY)
}

/// A user_data_unregistered message (type 5) of `size` bytes.
fn unregistered(size: usize) -> Vec<u8> {
    message(5, &vec![0x5A; size])
}

/// An SEI NAL unit with `messages` and the RBSP trailing bits.
fn sei(kind: u8, layer: u8, messages: &[Vec<u8>]) -> Vec<u8> {
    let mut body = messages.concat();
    body.push(0x80);
    nal(kind, layer, &body)
}

fn prefix(messages: &[Vec<u8>]) -> Vec<u8> {
    sei(PREFIX_SEI, 0, messages)
}

fn parameter_sets() -> Vec<u8> {
    [
        nal(VPS, 0, &[0x0C, 0x01]),
        nal(SPS, 0, &[0x01, 0x60]),
        nal(PPS, 0, &[0xC1, 0x72]),
    ]
    .concat()
}

/// One picture of two slices, with an AUD and, when `payload`, an HDR10+ prefix SEI first.
fn picture(payload: bool) -> Vec<u8> {
    let mut bytes = nal(AUD, 0, &[0x50]);
    if payload {
        bytes.extend(prefix(&[hdr10plus()]));
    }
    bytes.extend(slice(TRAIL_R, true));
    bytes.extend(slice(TRAIL_R, false));
    bytes
}

fn stream(payloads: &[bool]) -> Vec<u8> {
    let mut bytes = parameter_sets();
    for &payload in payloads {
        bytes.extend(picture(payload));
    }
    bytes
}

fn count(bytes: &[u8]) -> Result<Coverage, String> {
    scan(bytes)
}

fn coverage(
    access_units: u64,
    own: u64,
    gaps: u64,
    longest_gap: u64,
    first_gap: Option<u64>,
    ignored_payloads: u64,
) -> Coverage {
    Coverage {
        access_units,
        own,
        gaps,
        longest_gap,
        first_gap,
        ignored_payloads,
    }
}

#[test]
fn full_and_gapped_streams_are_counted_per_picture() {
    let (y, n) = (true, false);
    let cases: [(&[bool], Coverage); 6] = [
        (&[y, y, y, y, y], coverage(5, 5, 0, 0, None, 0)),
        (&[y, y, n, n, y], coverage(5, 3, 1, 2, Some(2), 0)),
        (&[n, n, y, y], coverage(4, 2, 1, 2, Some(0), 0)),
        (&[y, y, n], coverage(3, 2, 1, 1, Some(2), 0)),
        (&[y, n, y, n, n, y], coverage(6, 3, 2, 2, Some(1), 0)),
        (&[n, n, n], coverage(3, 0, 1, 3, Some(0), 0)),
    ];
    for (payloads, expected) in cases {
        assert_eq!(count(&stream(payloads)), Ok(expected), "{payloads:?}");
    }
}

#[test]
fn an_idr_picture_counts_like_any_other() {
    let mut bytes = parameter_sets();
    bytes.extend(prefix(&[hdr10plus()]));
    bytes.extend(slice(IDR_W_RADL, true));
    bytes.extend(picture(false));
    assert_eq!(count(&bytes), Ok(coverage(2, 1, 1, 1, Some(1), 0)));
}

#[test]
fn every_identity_field_must_match() {
    for index in 0..IDENTITY.len() {
        let mut identity = IDENTITY;
        identity[index] ^= 0x01;
        let mut bytes = parameter_sets();
        bytes.extend(prefix(&[hdr10plus_with_identity(&identity)]));
        bytes.extend(slice(TRAIL_R, true));
        assert_eq!(
            count(&bytes).unwrap().own,
            0,
            "identity byte {index} altered"
        );
    }
}

#[test]
fn a_t35_message_shorter_than_the_identity_is_not_hdr10plus() {
    let mut bytes = parameter_sets();
    bytes.extend(prefix(&[message(4, &IDENTITY[..6])]));
    bytes.extend(slice(TRAIL_R, true));
    assert_eq!(count(&bytes).unwrap().own, 0);
    // The identity alone, with no metadata after it, is the shortest message that counts.
    let mut bytes = parameter_sets();
    bytes.extend(prefix(&[message(4, &IDENTITY)]));
    bytes.extend(slice(TRAIL_R, true));
    assert_eq!(count(&bytes).unwrap().own, 1);
}

#[test]
fn another_payload_type_with_the_identity_bytes_is_not_hdr10plus() {
    let mut bytes = parameter_sets();
    bytes.extend(prefix(&[message(
        5,
        &[IDENTITY.to_vec(), vec![0x40]].concat(),
    )]));
    bytes.extend(slice(TRAIL_R, true));
    assert_eq!(count(&bytes).unwrap().own, 0);
}

#[test]
fn a_suffix_sei_payload_belongs_to_no_picture() {
    let mut bytes = parameter_sets();
    bytes.extend(slice(TRAIL_R, true));
    bytes.extend(sei(SUFFIX_SEI, 0, &[hdr10plus()]));
    bytes.extend(picture(false));
    assert_eq!(count(&bytes), Ok(coverage(2, 0, 1, 2, Some(0), 1)));
}

#[test]
fn a_payload_between_the_slices_of_a_picture_is_not_carried_to_the_next() {
    let mut bytes = parameter_sets();
    bytes.extend(slice(TRAIL_R, true));
    bytes.extend(prefix(&[hdr10plus()]));
    bytes.extend(slice(TRAIL_R, false));
    bytes.extend(picture(false));
    assert_eq!(count(&bytes), Ok(coverage(2, 0, 1, 2, Some(0), 1)));
}

#[test]
fn a_payload_after_the_last_picture_is_ignored() {
    let mut bytes = stream(&[true, true]);
    bytes.extend(prefix(&[hdr10plus()]));
    assert_eq!(count(&bytes), Ok(coverage(2, 2, 0, 0, None, 1)));
}

#[test]
fn two_payloads_in_one_picture_count_once() {
    let mut bytes = parameter_sets();
    bytes.extend(prefix(&[hdr10plus()]));
    bytes.extend(prefix(&[hdr10plus()]));
    bytes.extend(slice(TRAIL_R, true));
    bytes.extend(prefix(&[hdr10plus(), hdr10plus()]));
    bytes.extend(slice(TRAIL_R, true));
    assert_eq!(count(&bytes), Ok(coverage(2, 2, 0, 0, None, 0)));
}

#[test]
fn another_message_before_the_hdr10plus_one_is_skipped() {
    let mut bytes = parameter_sets();
    bytes.extend(prefix(&[
        unregistered(20),
        message(4, &[0xB5, 0x00, 0x3C]),
        hdr10plus(),
    ]));
    bytes.extend(slice(TRAIL_R, true));
    assert_eq!(count(&bytes).unwrap().own, 1);
}

#[test]
fn nal_units_of_other_layers_are_ignored() {
    let mut bytes = parameter_sets();
    bytes.extend(sei(PREFIX_SEI, 1, &[hdr10plus()]));
    bytes.extend(slice(TRAIL_R, true));
    let mut layer_one_slice = nal(TRAIL_R, 1, &[0xAF, 0x91, 0x40]);
    bytes.append(&mut layer_one_slice);
    assert_eq!(count(&bytes), Ok(coverage(1, 0, 1, 1, Some(0), 0)));
    // Layer 32 sets only the layer bit in the first header byte.
    let mut bytes = parameter_sets();
    bytes.extend(sei(PREFIX_SEI, 32, &[hdr10plus()]));
    bytes.extend(slice(TRAIL_R, true));
    bytes.extend(nal(TRAIL_R, 32, &[0xAF, 0x91, 0x40]));
    assert_eq!(count(&bytes), Ok(coverage(1, 0, 1, 1, Some(0), 0)));
}

#[test]
fn zero_bytes_after_a_slice_header_leave_the_header_alone() {
    // 00 00 03 right after the first slice byte, then a byte with the top bit clear.
    let mut bytes = parameter_sets();
    bytes.extend(prefix(&[hdr10plus()]));
    bytes.extend(nal(
        TRAIL_R,
        0,
        &[0xAF, 0x00, 0x00, 0x03, 0x01, 0x00, 0x00, 0x03, 0x02],
    ));
    bytes.extend(slice(TRAIL_R, false));
    assert_eq!(count(&bytes), Ok(coverage(1, 1, 0, 0, None, 0)));
}

#[test]
fn a_slice_of_one_byte_after_its_header_counts() {
    let mut bytes = parameter_sets();
    bytes.extend(prefix(&[hdr10plus()]));
    bytes.extend(nal(TRAIL_R, 0, &[0x80]));
    assert_eq!(count(&bytes), Ok(coverage(1, 1, 0, 0, None, 0)));
}

#[test]
fn a_slice_longer_than_the_sei_cap_is_not_kept() {
    let cap = 600;
    let mut body = vec![0xAF];
    body.resize(2000, 0x55);
    let mut bytes = parameter_sets();
    bytes.extend(prefix(&[hdr10plus()]));
    bytes.extend(nal(TRAIL_R, 0, &body));
    assert_eq!(
        scan_with_cap(&bytes[..], cap),
        Ok(coverage(1, 1, 0, 0, None, 0))
    );
}

#[test]
fn an_sei_that_ends_on_another_byte_than_the_trailing_bits_is_not_counted() {
    for last in [0x81, 0x40, 0x01] {
        let mut body = hdr10plus();
        body.push(last);
        let mut bytes = parameter_sets();
        bytes.extend(nal(PREFIX_SEI, 0, &body));
        bytes.extend(slice(TRAIL_R, true));
        assert!(count(&bytes).is_err(), "last byte {last:#04x}");
    }
}

#[test]
fn coded_values_add_255_for_every_ff_byte() {
    for (bytes, value, end) in [
        (&[0x00][..], 0, 1),
        (&[0x2D][..], 45, 1),
        (&[0xFF, 0x2D][..], 300, 2),
        (&[0xFF, 0xFF, 0x00, 0x07][..], 510, 3),
    ] {
        let mut position = 0;
        assert_eq!(read_coded(bytes, &mut position), Ok(value), "{bytes:02x?}");
        assert_eq!(position, end, "{bytes:02x?}");
    }
    let mut position = 0;
    assert!(read_coded(&[0xFF, 0xFF], &mut position).is_err());
}

/// A reader that is interrupted before every read it serves, or that fails.
struct Flaky<'a> {
    bytes: &'a [u8],
    interrupt: bool,
    fail: bool,
}

impl Read for Flaky<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.fail {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
        }
        self.interrupt = !self.interrupt;
        if self.interrupt {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let take = 7.min(buffer.len()).min(self.bytes.len());
        buffer[..take].copy_from_slice(&self.bytes[..take]);
        self.bytes = &self.bytes[take..];
        Ok(take)
    }
}

#[test]
fn an_interrupted_read_is_retried_and_a_failed_one_is_not_counted() {
    let bytes = stream(&[true, false, true]);
    let interrupted = Flaky {
        bytes: &bytes,
        interrupt: false,
        fail: false,
    };
    assert_eq!(scan(interrupted), count(&bytes));
    let failed = Flaky {
        bytes: &bytes,
        interrupt: false,
        fail: true,
    };
    let error = scan(failed).unwrap_err();
    assert!(error.contains("could not be read"), "{error}");
}

#[test]
fn a_second_slice_does_not_start_a_picture() {
    let mut bytes = stream(&[true]);
    for _ in 0..3 {
        bytes.extend(slice(TRAIL_R, false));
    }
    assert_eq!(count(&bytes), Ok(coverage(1, 1, 0, 0, None, 0)));
}

#[test]
fn four_byte_start_codes_and_trailing_zeros_change_nothing() {
    let three = stream(&[true, false, true]);
    // The same units with two trailing zeros each and a zero_byte before every start code.
    let mut four = Vec::new();
    let mut index = 0;
    while index < three.len() {
        if three[index..].starts_with(&[0, 0, 1]) {
            four.extend_from_slice(&[0, 0, 0, 0, 0, 1]);
            index += 3;
        } else {
            four.push(three[index]);
            index += 1;
        }
    }
    assert_eq!(count(&four), count(&three));
    assert_eq!(count(&three).unwrap().own, 2);
}

#[test]
fn message_sizes_count_unescaped_bytes() {
    // An unregistered message whose payload is 00 00 00 00: escaped 00 00 03 00 00 03 ... in
    // the stream, 4 bytes in the RBSP. Then the HDR10+ message.
    let mut body = vec![5, 4, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03];
    body.extend(hdr10plus());
    body.push(0x80);
    let mut bytes = parameter_sets();
    bytes.extend(nal(PREFIX_SEI, 0, &body));
    bytes.extend(slice(TRAIL_R, true));
    assert_eq!(count(&bytes).unwrap().own, 1);
}

#[test]
fn extended_type_and_size_bytes_are_read() {
    let mut bytes = parameter_sets();
    // Type 300 and size 300 (FF 2D each), then an HDR10+ message of 260 bytes (size FF 05).
    let mut long = IDENTITY.to_vec();
    long.resize(260, 0x41);
    bytes.extend(prefix(&[message(300, &[0x33; 300]), message(4, &long)]));
    bytes.extend(slice(TRAIL_R, true));
    assert_eq!(count(&bytes).unwrap().own, 1);
}

#[test]
fn empty_input_and_text_hold_no_picture() {
    assert_eq!(count(b""), Ok(coverage(0, 0, 0, 0, None, 0)));
    assert_eq!(count(b"not a stream"), Ok(coverage(0, 0, 0, 0, None, 0)));
}

#[test]
fn malformed_streams_are_not_counted() {
    let truncated = {
        // The message says 9 bytes; 4 follow before the trailing bits.
        let mut bytes = parameter_sets();
        bytes.extend(nal(PREFIX_SEI, 0, &[4, 9, 0xB5, 0x00, 0x3C, 0x00, 0x80]));
        bytes.extend(slice(TRAIL_R, true));
        bytes
    };
    let without_trailing_bits = {
        let mut bytes = parameter_sets();
        bytes.extend(nal(PREFIX_SEI, 0, &hdr10plus()));
        bytes.extend(slice(TRAIL_R, true));
        bytes
    };
    let header_cut = {
        let mut bytes = parameter_sets();
        bytes.extend(nal(PREFIX_SEI, 0, &[0xFF, 0xFF]));
        bytes.extend(slice(TRAIL_R, true));
        bytes
    };
    let orphan_slice = [parameter_sets(), slice(TRAIL_R, false), picture(true)].concat();
    let short_slice = [parameter_sets(), nal(TRAIL_R, 0, &[])].concat();
    for (name, bytes) in [
        ("truncated", truncated),
        ("without trailing bits", without_trailing_bits),
        ("header cut", header_cut),
        ("orphan slice", orphan_slice),
        ("short slice", short_slice),
    ] {
        assert!(count(&bytes).is_err(), "{name}: {:?}", count(&bytes));
    }
}

/// A reader that returns at most `step` bytes per read.
struct Trickle<'a> {
    bytes: &'a [u8],
    step: usize,
}

impl Read for Trickle<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let take = self.step.min(buffer.len()).min(self.bytes.len());
        buffer[..take].copy_from_slice(&self.bytes[..take]);
        self.bytes = &self.bytes[take..];
        Ok(take)
    }
}

#[test]
fn the_count_does_not_depend_on_where_reads_split_the_stream() {
    let mut bytes = stream(&[true, false, false, true]);
    bytes.extend(slice(TRAIL_R, true));
    bytes.extend(sei(SUFFIX_SEI, 0, &[hdr10plus()]));
    bytes.extend(prefix(&[unregistered(300), hdr10plus()]));
    bytes.extend_from_slice(&[0, 0, 0, 1]);
    bytes.extend(picture(true));
    let expected = count(&bytes).unwrap();
    assert_eq!(expected, coverage(6, 3, 2, 2, Some(1), 1));
    for step in 1..=bytes.len() {
        let reader = Trickle {
            bytes: &bytes,
            step,
        };
        assert_eq!(scan(reader), Ok(expected.clone()), "{step} bytes per read");
    }
}

/// A stream whose one SEI NAL unit is `len` bytes long (header and trailing bits included),
/// with the HDR10+ message after an unregistered message that fills it.
fn stream_with_sei_of(len: usize) -> Vec<u8> {
    // 2 header bytes, the HDR10+ message, the unregistered message's type and size bytes, its
    // payload and the trailing byte.
    let fixed = 2 + hdr10plus().len() + 1;
    let filler = len - fixed;
    let payload = (1..=filler)
        .rev()
        .find(|&size| coded(5).len() + coded(size).len() + size == filler)
        .expect("a filler size fits");
    let mut bytes = parameter_sets();
    let unit = prefix(&[unregistered(payload), hdr10plus()]);
    assert_eq!(unit.len() - 3, len);
    bytes.extend(unit);
    bytes.extend(slice(TRAIL_R, true));
    bytes
}

#[test]
fn an_sei_unit_at_the_cap_is_read_and_one_byte_more_is_not_counted() {
    let cap = 600;
    let at_cap = stream_with_sei_of(cap);
    assert_eq!(scan_with_cap(&at_cap[..], cap).unwrap().own, 1);
    let over = stream_with_sei_of(cap + 1);
    assert!(scan_with_cap(&over[..], cap).is_err());
    // The same unit under the real cap is read: the cap does not come from the content.
    assert_eq!(scan(&over[..]).unwrap().own, 1);
}

fn temp_with_stream(bytes: &[u8]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(HDR10PLUS_HEVC), bytes).unwrap();
    dir
}

fn seal(dir: &Path, version: u32, coverage: &Coverage) {
    let path = dir.join(COVERAGE_FILE);
    let record = serde_json::json!({"version": version, "coverage": coverage});
    fs::write(&path, record.to_string()).unwrap();
    resume::mark_done(&path).unwrap();
}

#[test]
fn a_fresh_count_is_sealed_and_a_sealed_one_is_reused_on_resume() {
    let dir = temp_with_stream(&stream(&[true, false]));
    let scanned = coverage(2, 1, 1, 1, Some(1), 0);
    assert_eq!(count_sealed(dir.path(), true), Ok(scanned.clone()));
    let sealed: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.path().join(COVERAGE_FILE)).unwrap()).unwrap();
    assert_eq!(sealed["version"], 1);
    assert_eq!(sealed["coverage"]["own"], 1);
    assert!(resume::is_complete(&dir.path().join(COVERAGE_FILE)));

    // Numbers the stream cannot give prove that the seal is read, not the stream.
    let planted = coverage(9, 7, 1, 2, Some(3), 0);
    seal(dir.path(), 1, &planted);
    assert_eq!(count_sealed(dir.path(), true), Ok(planted.clone()));
    assert_eq!(
        count_sealed(dir.path(), false),
        Ok(scanned.clone()),
        "no resume: scanned"
    );
}

#[test]
fn a_seal_of_another_version_unreadable_or_without_its_marker_is_scanned_again() {
    let dir = temp_with_stream(&stream(&[true, false]));
    let scanned = coverage(2, 1, 1, 1, Some(1), 0);
    let planted = coverage(9, 7, 1, 2, Some(3), 0);

    seal(dir.path(), 2, &planted);
    assert_eq!(
        count_sealed(dir.path(), true),
        Ok(scanned.clone()),
        "other version"
    );

    let path = dir.path().join(COVERAGE_FILE);
    fs::write(&path, b"{not json").unwrap();
    resume::mark_done(&path).unwrap();
    assert_eq!(
        count_sealed(dir.path(), true),
        Ok(scanned.clone()),
        "bad JSON"
    );

    seal(dir.path(), 1, &planted);
    resume::invalidate(&path).unwrap();
    assert_eq!(count_sealed(dir.path(), true), Ok(scanned), "no marker");
}

#[test]
fn a_stream_that_cannot_be_read_seals_nothing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(count_sealed(dir.path(), true).is_err());
    assert!(!dir.path().join(COVERAGE_FILE).exists());
    // A malformed stream is not sealed either.
    fs::write(
        dir.path().join(HDR10PLUS_HEVC),
        [parameter_sets(), slice(TRAIL_R, false)].concat(),
    )
    .unwrap();
    assert!(count_sealed(dir.path(), true).is_err());
    assert!(!dir.path().join(COVERAGE_FILE).exists());
}

#[test]
fn scene_info_frames_counts_the_entries_without_reading_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hdr10plus_metadata.json");
    fs::write(
        &path,
        r#"{"SceneInfo": [{"a": 1}, {}, {"b": [1, 2]}], "Other": 5}"#,
    )
    .unwrap();
    assert_eq!(scene_info_frames(&path), Some(3));
    fs::write(&path, r#"{"Other": 5}"#).unwrap();
    assert_eq!(scene_info_frames(&path), None);
    assert_eq!(scene_info_frames(&dir.path().join("missing.json")), None);
}

#[test]
fn only_a_trusted_partial_count_under_an_explicit_hybrid_refuses() {
    let full = Ok(coverage(48, 48, 0, 0, None, 0));
    let partial = Ok(coverage(48, 47, 1, 1, Some(20), 0));
    for explicit in [false, true] {
        assert!(matches!(
            judge_extracted(&full, Some(48), explicit),
            Verdict::Full(_)
        ));
        assert!(matches!(
            judge_extracted(&Ok(coverage(48, 0, 1, 48, Some(0), 0)), Some(48), explicit),
            Verdict::NotCounted(_)
        ));
        assert!(matches!(
            judge_extracted(&partial, Some(49), explicit),
            Verdict::NotCounted(_)
        ));
        assert!(matches!(
            judge_extracted(&partial, None, explicit),
            Verdict::NotCounted(_)
        ));
        assert!(matches!(
            judge_extracted(&Err("unreadable".into()), Some(48), explicit),
            Verdict::NotCounted(_)
        ));
    }
    let Verdict::Partial(warning) = judge_extracted(&partial, Some(48), false) else {
        panic!("the default warns");
    };
    assert!(warning.contains("47 of 48 frames"), "{warning}");
    assert!(warning.contains("the other 1 ("), "{warning}");
    assert!(warning.contains("the first at frame 20"), "{warning}");
    let Verdict::Refused(reason) = judge_extracted(&partial, Some(48), true) else {
        panic!("an explicit hybrid refuses");
    };
    assert!(reason.contains("only 47 of 48 frames"), "{reason}");
}

#[test]
fn ignored_payloads_are_named_in_the_messages() {
    let Verdict::Full(message) =
        judge_extracted(&Ok(coverage(4, 4, 0, 0, None, 2)), Some(4), false)
    else {
        panic!("full");
    };
    assert!(message.contains("all 4 frames"), "{message}");
    assert!(message.contains("2 more HDR10+ payload(s)"), "{message}");
    let Verdict::Full(message) =
        judge_extracted(&Ok(coverage(4, 4, 0, 0, None, 0)), Some(4), false)
    else {
        panic!("full");
    };
    assert!(!message.contains("more HDR10+"), "{message}");
}

#[test]
fn unextracted_payloads_warn_or_refuse_an_explicit_hybrid() {
    let log = Path::new("hdr10plus_tool.log");
    let with = Ok(coverage(48, 43, 1, 5, Some(0), 0));
    let Unextracted::Unreadable(warning) = judge_unextracted(&with, false, log) else {
        panic!("the default warns");
    };
    assert!(warning.contains("43 of 48 frames"), "{warning}");
    assert!(warning.contains("hdr10plus_tool.log"), "{warning}");
    let Unextracted::Refused(reason) = judge_unextracted(&with, true, log) else {
        panic!("an explicit hybrid refuses");
    };
    assert!(reason.contains("could not extract"), "{reason}");
    // Nothing was extracted, so `--hdr10plus-l1 hdr10plus` has no metadata to use either.
    let refusal = unextracted_refusal(&reason);
    assert!(
        refusal.starts_with("--hdr10plus-l1 hybrid cannot be used here: 43 of 48 frames"),
        "{refusal}"
    );
    assert!(
        refusal.ends_with(
            "Without --hdr10plus-l1 hybrid the file converts from HDR10 analysis, without the \
             HDR10+ metadata."
        ),
        "{refusal}"
    );
    assert!(!refusal.contains("metadata alone"), "{refusal}");
    for explicit in [false, true] {
        let none = Ok(coverage(48, 0, 1, 48, Some(0), 0));
        assert_eq!(
            judge_unextracted(&none, explicit, log),
            Unextracted::NoPayloads
        );
        let error = Err("unreadable".to_string());
        assert_eq!(
            judge_unextracted(&error, explicit, log),
            Unextracted::NoPayloads
        );
    }
}
