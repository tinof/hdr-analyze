use super::*;

/// The header of `docs/FORMAT_COMPATIBILITY.md` "PQ histogram dump", written out byte by byte
/// for a luma, HLG, `--sample-rate 2`, `--downscale 4` run of 3 frames after 5 leading pictures.
/// `tools/display_sim/src/hist_tests.rs` reads the same literal.
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

fn header() -> DumpHeader {
    DumpHeader {
        peak_domain: PeakDomain::Luma,
        hlg: true,
        sample_rate: 2,
        downscale: 4,
    }
}

fn histogram(seed: u64) -> Vec<u64> {
    (0..BINS as u64).map(|bin| (bin * 7 + seed) % 13).collect()
}

fn record_bytes(histogram: &[u64]) -> Vec<u8> {
    histogram
        .iter()
        .flat_map(|&count| (count as u32).to_le_bytes())
        .collect()
}

#[test]
fn finished_dump_matches_the_documented_layout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dump.pqhist");
    let mut writer = PqHistWriter::create(&path, &header()).unwrap();
    writer.write_frame(&histogram(1)).unwrap();
    writer.repeat_last().unwrap();
    writer.write_frame(&histogram(2)).unwrap();
    assert_eq!(writer.finish(5).unwrap(), 3);

    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 64 + 3 * 16_384);
    assert_eq!(bytes[..64], FINISHED_HEADER);
    let records: Vec<&[u8]> = bytes[64..].chunks(16_384).collect();
    assert_eq!(records[0], record_bytes(&histogram(1)));
    assert_eq!(
        records[1], records[0],
        "repeat_last copies the previous record"
    );
    assert_eq!(records[2], record_bytes(&histogram(2)));
}

#[test]
fn an_unfinished_dump_carries_the_sentinel_count() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dump.pqhist");
    let mut writer = PqHistWriter::create(
        &path,
        &DumpHeader {
            peak_domain: PeakDomain::MaxRgb,
            hlg: false,
            sample_rate: 1,
            downscale: 1,
        },
    )
    .unwrap();
    writer.write_frame(&histogram(3)).unwrap();
    // Dropped without finish, as when the analysis fails: the writer flushes, the count stays.
    drop(writer);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 64 + 16_384);
    assert_eq!(bytes[40..48], [0xff; 8]);
    assert_eq!(bytes[48..56], [0; 8]);
    assert_eq!(bytes[20..28], [0, 0, 0, 0, 0, 0, 0, 0], "max-RGB, PQ");
    assert_eq!(
        bytes[28..36],
        [1, 0, 0, 0, 1, 0, 0, 0],
        "sample rate, downscale"
    );
}

#[test]
fn malformed_frames_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = PqHistWriter::create(&dir.path().join("dump"), &header()).unwrap();
    let error = writer.repeat_last().unwrap_err().to_string();
    assert!(error.contains("before the first measured frame"), "{error}");
    let error = writer.write_frame(&[0; 4095]).unwrap_err().to_string();
    assert!(error.contains("4095 bins"), "{error}");
    let mut too_many = vec![0; BINS];
    too_many[17] = u64::from(u32::MAX) + 1;
    let error = format!("{:#}", writer.write_frame(&too_many).unwrap_err());
    assert!(error.contains("bin 17"), "{error}");
    let mut largest = vec![0; BINS];
    largest[17] = u64::from(u32::MAX);
    writer.write_frame(&largest).unwrap();
    assert_eq!(writer.finish(0).unwrap(), 1);
}

#[test]
fn normalized_paths_resolve_relative_missing_and_symlinked_names() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("real")).unwrap();
    std::fs::write(root.join("real/out.bin"), b"x").unwrap();
    let norm = |path: &Path| normalized_path(path).unwrap();

    let target = root.join("real/out.bin");
    assert_eq!(norm(&target), target);
    assert_eq!(norm(&root.join("real/./out.bin")), target);
    assert_eq!(norm(&root.join("missing/../real/out.bin")), target);
    assert_eq!(
        norm(&root.join("real/new/../next.bin")),
        root.join("real/next.bin")
    );
    assert_eq!(norm(&root.join("real/a/b")), root.join("real/a/b"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        assert_eq!(norm(&root.join("link/out.bin")), target);
        assert_eq!(norm(&root.join("link/new.bin")), root.join("real/new.bin"));
    }
    // Relative paths are read against the working directory.
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    assert_eq!(norm(Path::new("not-there.bin")), cwd.join("not-there.bin"));
    assert_eq!(
        norm(Path::new("./not-there.bin")),
        cwd.join("not-there.bin")
    );
    assert_eq!(
        norm(Path::new("missing/../not-there.bin")),
        cwd.join("not-there.bin")
    );
}

#[test]
fn a_dump_path_naming_another_file_of_the_run_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let input = root.join("input.mkv");
    std::fs::write(&input, b"video").unwrap();
    let output = root.join("out.bin");
    let others = [
        ("input", input.as_path()),
        ("measurement file", output.as_path()),
    ];

    let error = check_dump_path(&root.join("./out.bin"), &others)
        .unwrap_err()
        .to_string();
    assert!(error.contains("is the measurement file"), "{error}");
    let error = check_dump_path(&input, &others).unwrap_err().to_string();
    assert!(error.contains("is the input"), "{error}");
    check_dump_path(&root.join("out.pqhist"), &others).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&input, root.join("alias.mkv")).unwrap();
        let error = check_dump_path(&root.join("alias.mkv"), &others)
            .unwrap_err()
            .to_string();
        assert!(error.contains("is the input"), "{error}");
        std::fs::hard_link(&input, root.join("hard.mkv")).unwrap();
        let error = check_dump_path(&root.join("hard.mkv"), &others)
            .unwrap_err()
            .to_string();
        assert!(error.contains("is the input"), "{error}");
    }
}
