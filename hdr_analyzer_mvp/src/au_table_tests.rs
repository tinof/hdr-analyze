use super::*;

const TRAIL_R: u8 = 1;
const RASL_N: u8 = 8;
const IDR_W_RADL: u8 = 19;
const CRA: u8 = 21;
const EOS: u8 = 37;

/// One length-prefixed (4-byte) NAL unit of the given type, with a payload byte.
fn packet(kind: u8, payload: u8) -> Vec<u8> {
    vec![0, 0, 0, 3, kind << 1, 1, payload]
}

fn recorder() -> AuRecorder {
    AuRecorder::from_parts(
        NalFraming::LengthPrefixed(4),
        AuSource {
            file_name: "in.mkv".into(),
            size_bytes: 0,
            head_sha256: String::new(),
            tail_sha256: String::new(),
        },
        AuTrack {
            container: "matroska".into(),
            codec: "hevc".into(),
            width: 16,
            height: 16,
            codec_private_sha256: String::new(),
            time_base: [1, 1000],
        },
    )
}

fn scene() -> SceneSettings {
    SceneSettings {
        detector: "global".into(),
        metric: "hist".into(),
        threshold: 0.3,
        min_length: 24,
    }
}

/// Feeds `(kind, pts)` packets at increasing positions.
fn feed(recorder: &mut AuRecorder, packets: &[(u8, i64)]) {
    for (index, &(kind, pts)) in packets.iter().enumerate() {
        recorder.observe_packet(
            Some(pts),
            100 + index as i64 * 10,
            &packet(kind, index as u8),
        );
    }
}

fn finish(
    mut rec: AuRecorder,
    output_pts: &[Option<i64>],
    sidecar_sha256: Option<String>,
) -> Result<AuTable> {
    rec.set_output_pts(output_pts.to_vec());
    rec.finish(sidecar_sha256, scene())
}

fn outputs(pts: &[i64]) -> Vec<Option<i64>> {
    pts.iter().copied().map(Some).collect()
}

#[test]
fn b_frames_table_records_decode_order_and_irap_crc() {
    let mut rec = recorder();
    // I0 P3 B1 B2 in decode order; output is pts order.
    feed(
        &mut rec,
        &[(IDR_W_RADL, 0), (TRAIL_R, 3), (TRAIL_R, 1), (TRAIL_R, 2)],
    );
    let table = finish(rec, &outputs(&[0, 1, 2, 3]), Some("ab".into())).unwrap();
    assert_eq!(table.aus.count, 4);
    assert_eq!(table.aus.pts_first, 0);
    assert_eq!(table.aus.pts_delta, vec![3, -2, 1]);
    assert_eq!(table.aus.size, vec![7; 4]);
    assert!(table.aus.non_picture.is_empty());
    assert_eq!(
        table.aus.irap_crc32,
        vec![(0, CRC32.checksum(&packet(IDR_W_RADL, 0)))]
    );
    assert_eq!(table.leading_skipped, 0);
    assert_eq!(table.sidecar_sha256.as_deref(), Some("ab"));
}

#[test]
fn crc_is_iso_hdlc() {
    // The CRC-32 check value, the same as java.util.zip.CRC32 gives.
    assert_eq!(CRC32.checksum(b"123456789"), 0xCBF4_3926);
}

#[test]
fn non_picture_packets_are_listed_and_kept_out_of_presentation_order() {
    let mut rec = recorder();
    feed(&mut rec, &[(IDR_W_RADL, 0), (TRAIL_R, 1), (EOS, 1)]);
    let table = finish(rec, &outputs(&[0, 1]), None).unwrap();
    assert_eq!(table.aus.count, 3);
    assert_eq!(table.aus.non_picture, vec![2]);
}

#[test]
fn rasl_pictures_of_the_first_cra_are_the_lowest_pts_and_skipped() {
    let mut rec = recorder();
    // CRA at pts 3 with two RASL pictures (pts 1, 2) that the decoder drops.
    feed(
        &mut rec,
        &[(CRA, 3), (RASL_N, 1), (RASL_N, 2), (TRAIL_R, 4)],
    );
    let table = finish(rec, &outputs(&[3, 4]), None).unwrap();
    assert_eq!(table.leading_skipped, 2);
}

#[test]
fn output_that_is_not_pts_order_is_refused() {
    let mut rec = recorder();
    feed(&mut rec, &[(IDR_W_RADL, 0), (TRAIL_R, 2), (TRAIL_R, 1)]);
    let error = finish(rec, &outputs(&[0, 2, 1]), None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("presentation order is not pts order"),
        "{error}"
    );
}

#[test]
fn output_count_mismatch_is_refused() {
    let mut rec = recorder();
    feed(&mut rec, &[(IDR_W_RADL, 0), (TRAIL_R, 1)]);
    let error = finish(rec, &outputs(&[0]), None).unwrap_err();
    assert!(
        error.to_string().contains("decoder output 1 frames"),
        "{error}"
    );
}

#[test]
fn output_frame_without_pts_is_refused() {
    let mut rec = recorder();
    feed(&mut rec, &[(IDR_W_RADL, 0)]);
    let error = finish(rec, &[None], None).unwrap_err();
    assert!(error.to_string().contains("has no pts"), "{error}");
}

#[test]
fn duplicate_picture_pts_is_refused() {
    let mut rec = recorder();
    feed(&mut rec, &[(IDR_W_RADL, 0), (TRAIL_R, 1), (TRAIL_R, 1)]);
    let error = finish(rec, &outputs(&[0, 1, 1]), None).unwrap_err();
    assert!(error.to_string().contains("same pts 1"), "{error}");
}

#[test]
fn packet_without_pts_is_refused() {
    let mut rec = recorder();
    rec.observe_packet(Some(0), 10, &packet(IDR_W_RADL, 0));
    rec.observe_packet(None, 20, &packet(TRAIL_R, 1));
    let error = finish(rec, &outputs(&[0]), None).unwrap_err();
    assert!(
        error.to_string().contains("video packet 1 has no pts"),
        "{error}"
    );
}

#[test]
fn packet_that_does_not_advance_is_refused_as_laced() {
    let mut rec = recorder();
    rec.observe_packet(Some(0), 10, &packet(IDR_W_RADL, 0));
    rec.observe_packet(Some(1), 10, &packet(TRAIL_R, 1));
    let error = finish(rec, &outputs(&[0, 1]), None).unwrap_err();
    assert!(error.to_string().contains("laced video"), "{error}");
}

#[test]
fn unknown_position_is_refused() {
    let mut rec = recorder();
    rec.observe_packet(Some(0), -1, &packet(IDR_W_RADL, 0));
    let error = finish(rec, &outputs(&[0]), None).unwrap_err();
    assert!(
        error.to_string().contains("no container position"),
        "{error}"
    );
}

#[test]
fn source_identity_hashes_both_ends() {
    let dir = tempfile::tempdir().unwrap();
    let small = dir.path().join("small.mkv");
    std::fs::write(&small, b"abc").unwrap();
    let id = source_identity(&small).unwrap();
    assert_eq!(id.size_bytes, 3);
    assert_eq!(id.head_sha256, hex_sha256(b"abc"));
    assert_eq!(id.tail_sha256, hex_sha256(b"abc"));

    let big = dir.path().join("big.mkv");
    let bytes: Vec<u8> = (0..(3 * CHUNK_BYTES as usize))
        .map(|i| (i % 251) as u8)
        .collect();
    std::fs::write(&big, &bytes).unwrap();
    let id = source_identity(&big).unwrap();
    let chunk = CHUNK_BYTES as usize;
    assert_eq!(id.head_sha256, hex_sha256(&bytes[..chunk]));
    assert_eq!(id.tail_sha256, hex_sha256(&bytes[bytes.len() - chunk..]));
    assert_eq!(id.file_name, "big.mkv");
}

#[test]
fn write_is_atomic_and_stale_tables_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let path = au_table_path(&dir.path().join("x_measurements.bin"));
    assert!(path
        .to_string_lossy()
        .ends_with("x_measurements.bin.au.json"));
    remove_stale_au_table(&path).unwrap();
    let mut rec = recorder();
    feed(&mut rec, &[(IDR_W_RADL, 0)]);
    let table = finish(rec, &outputs(&[0]), None).unwrap();
    write_au_table(&path, &table).unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["aus"]["irap_crc32"][0][0], 0);
    assert!(json["sidecar_sha256"].is_null());
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    assert!(!PathBuf::from(temp).exists());
    remove_stale_au_table(&path).unwrap();
    assert!(!path.exists());
}
