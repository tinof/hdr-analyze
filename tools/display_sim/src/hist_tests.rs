use super::*;

/// The header of `docs/FORMAT_COMPATIBILITY.md` "PQ histogram dump", byte by byte: luma, HLG,
/// `--sample-rate 2`, `--downscale 4`, 3 frames after 5 leading pictures. The analyzer's
/// `hdr_analyzer_mvp/src/pq_hist_dump_tests.rs` writes the same literal.
const FINISHED_HEADER: [u8; HEADER_BYTES] = [
    b'P', b'Q', b'H', b'I', b'S', b'T', 1, 0, // magic, version 1
    64, 0, 0, 0, // header bytes
    0, 16, 0, 0, // 4096 bins
    4, 0, 0, 0, // bytes per count
    1, 0, 0, 0, // peak domain: luma
    1, 0, 0, 0, // transfer: HLG
    2, 0, 0, 0, // sample rate
    4, 0, 0, 0, // downscale
    0, 0, 0, 0, // reserved
    3, 0, 0, 0, 0, 0, 0, 0, // frame count
    5, 0, 0, 0, 0, 0, 0, 0, // leading skipped frames
    0, 0, 0, 0, 0, 0, 0, 0, // reserved
];
const THREE_FRAMES: u64 = 64 + 3 * 16_384;

#[test]
fn the_documented_header_reads_back() {
    assert_eq!(
        parse_header(&FINISHED_HEADER, THREE_FRAMES).unwrap(),
        Header {
            peak_domain: PeakDomain::Luma,
            hlg: true,
            sample_rate: 2,
            downscale: 4,
            frames: 3,
            leading_skipped: 5,
        }
    );
    let mut pq_max_rgb = FINISHED_HEADER;
    pq_max_rgb[20] = 0;
    pq_max_rgb[24] = 0;
    let header = parse_header(&pq_max_rgb, THREE_FRAMES).unwrap();
    assert_eq!(
        (header.peak_domain, header.hlg),
        (PeakDomain::MaxRgb, false)
    );
}

#[test]
fn malformed_headers_are_refused() {
    let refused = |offset: usize, value: u8, file_len: u64, message: &str| {
        let mut bytes = FINISHED_HEADER;
        bytes[offset] = value;
        let error = parse_header(&bytes, file_len).unwrap_err().to_string();
        assert!(error.contains(message), "byte {offset} = {value}: {error}");
    };
    refused(0, b'X', THREE_FRAMES, "no PQHIST magic");
    refused(6, 2, THREE_FRAMES, "version 2 is not supported");
    refused(8, 63, THREE_FRAMES, "header size is 63");
    refused(13, 15, THREE_FRAMES, "bin count is 3840");
    refused(16, 2, THREE_FRAMES, "bytes per count is 2");
    refused(20, 2, THREE_FRAMES, "peak domain 2 is unknown");
    refused(24, 2, THREE_FRAMES, "transfer 2 is unknown");
    refused(28, 0, THREE_FRAMES, "sample rate is 0");
    refused(32, 0, THREE_FRAMES, "downscale is 0");
    refused(36, 1, THREE_FRAMES, "reserved header fields");
    refused(63, 1, THREE_FRAMES, "reserved header fields");
    refused(40, 0, 64, "holds no frames");
    // One byte short, one byte long.
    refused(40, 3, THREE_FRAMES - 1, "need 49216");
    refused(40, 3, THREE_FRAMES + 1, "need 49216");

    let mut unfinished = FINISHED_HEADER;
    unfinished[40..48].copy_from_slice(&u64::MAX.to_le_bytes());
    let error = parse_header(&unfinished, THREE_FRAMES)
        .unwrap_err()
        .to_string();
    assert!(error.contains("did not finish"), "{error}");
    let mut huge = FINISHED_HEADER;
    huge[40..48].copy_from_slice(&(u64::MAX - 1).to_le_bytes());
    let error = parse_header(&huge, THREE_FRAMES).unwrap_err().to_string();
    assert!(error.contains("overflows"), "{error}");
}

#[test]
fn records_stream_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dump.pqhist");
    let mut bytes = FINISHED_HEADER.to_vec();
    for frame in 0..3u32 {
        for bin in 0..BINS as u32 {
            bytes.extend_from_slice(&(bin * 3 + frame).to_le_bytes());
        }
    }
    std::fs::write(&path, &bytes).unwrap();
    let mut dump = Dump::open(&path).unwrap();
    assert_eq!(dump.header.frames, 3);
    let mut counts = [0u32; BINS];
    for frame in 0..3u32 {
        assert!(dump.next_frame(&mut counts).unwrap());
        assert_eq!((counts[0], counts[4095]), (frame, 4095 * 3 + frame));
    }
    assert!(!dump.next_frame(&mut counts).unwrap());

    std::fs::write(&path, &bytes[..40]).unwrap();
    let error = format!("{:#}", Dump::open(&path).err().unwrap());
    assert!(error.contains("shorter than a pqhist header"), "{error}");
}
