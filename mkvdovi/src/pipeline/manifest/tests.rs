use std::collections::HashMap;
use std::path::Path;

use clap::Parser;
use dovi84_composer::Composer;
use dovi_rpu_builder::manifest::QualityClass;
use dovi_rpu_builder::{HlgComposer, Profile};
use serde_json::{json, Value};

use super::au_table::{check_structure, AuTable, Pair};
use super::build::{analysis, manifest, quality_class, Inputs};
use super::identity::{
    hex, is_matroska_header, is_same_file, sha256_hex, FileIdentity, CHUNK_BYTES,
};
use super::{check_sampling, write_manifest, Paths};
use crate::cli::Args;
use crate::metadata::{CmV40Config, L1Sidecar};
use crate::pipeline::config::Settled;

const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// A sidecar as the analyzer writes it (version 6, default settings), with `leading` RASL
/// pictures and two scenes over `measured` frames.
fn sidecar_json(leading: u64, measured: u64) -> Value {
    json!({
        "version": 6,
        "analyzer_version": "0.8.0",
        "source": {"file_name": "clip.mkv", "size_bytes": 1000, "width": 256, "height": 144,
                   "transfer_function": "PQ (SMPTE 2084)", "stream_frames": leading + measured,
                   "leading_skipped_frames": leading},
        "analysis": {"downscale": 1, "sample_rate": 1, "gpu": false, "no_crop": false,
                     "luminance_mapping": "pq"},
        "crop_space": "full",
        "min_percentile": 0.1, "denoise_mode": "off", "peak_domain": "max-rgb",
        "peak_estimator": "max", "peak_source": "max", "peak_percentile": 99.99,
        "crop": {"x": 0, "y": 0, "width": 256, "height": 144},
        "scenes": [
            {"start": 0, "end": 9, "min_pq_12bit": 10, "avg_luma_pq_12bit": 700,
             "avg_max_rgb_pq_12bit": 700, "max_pq_12bit": 800},
            {"start": 10, "end": measured - 1, "min_pq_12bit": 20, "avg_luma_pq_12bit": 3000,
             "avg_max_rgb_pq_12bit": 3000, "max_pq_12bit": 3100},
        ],
    })
}

fn sidecar(leading: u64, measured: u64) -> L1Sidecar {
    serde_json::from_value(sidecar_json(leading, measured)).unwrap()
}

/// An AU table for `count` AUs: IRAPs every 8 AUs, decode order = presentation order.
fn table_json(sidecar_bytes: &[u8], count: u64, leading: u64) -> Value {
    json!({
        "version": 1,
        "analyzer_version": "0.8.0",
        "sidecar_sha256": sha256_hex(sidecar_bytes),
        "source": {"file_name": "clip.mkv", "size_bytes": 1000,
                   "head_sha256": DIGEST_A, "tail_sha256": DIGEST_A},
        "track": {"container": "matroska", "codec": "hevc", "width": 256, "height": 144,
                  "codec_private_sha256": DIGEST_A, "time_base": [1, 1000]},
        "aus": {"count": count, "pts_first": 0, "pts_delta": vec![42; count as usize - 1],
                "size": vec![100; count as usize], "non_picture": [],
                "irap_crc32": (0..count).step_by(8).map(|k| json!([k, 7])).collect::<Vec<_>>()},
        "leading_skipped": leading,
        "scene": {"detector": "global", "metric": "hist", "threshold": 3.0, "min_length": 12},
    })
}

fn pair_from(table: &Value, sidecar_bytes: &[u8]) -> Result<Pair, String> {
    Pair::from_bytes(
        &serde_json::to_vec(table).unwrap(),
        sidecar_bytes,
        serde_json::from_slice(sidecar_bytes).unwrap(),
    )
}

fn good_pair(leading: u64, measured: u64) -> Pair {
    let bytes = serde_json::to_vec(&sidecar_json(leading, measured)).unwrap();
    pair_from(&table_json(&bytes, leading + measured, leading), &bytes).unwrap()
}

#[test]
fn a_bound_well_formed_table_is_accepted() {
    let pair = good_pair(0, 24);
    assert_eq!(pair.table.aus.picture_count(), 24);
    assert_eq!(pair.table.aus.pts()[..3], [0, 42, 84]);
    pair.check_sidecar(&sidecar(0, 24)).unwrap();
}

#[test]
fn a_table_bound_to_other_sidecar_bytes_is_refused() {
    let bytes = serde_json::to_vec(&sidecar_json(0, 24)).unwrap();
    let table = table_json(&bytes, 24, 0);
    let mut other = bytes.clone();
    other.push(b'\n');
    let error = pair_from(&table, &other).unwrap_err();
    assert!(error.contains("is bound to sidecar"), "{error}");

    let mut unbound = table;
    unbound["sidecar_sha256"] = Value::Null;
    let error = pair_from(&unbound, &bytes).unwrap_err();
    assert!(error.contains("--au-table-only"), "{error}");
}

#[test]
fn only_table_version_1_is_read() {
    let bytes = serde_json::to_vec(&sidecar_json(0, 24)).unwrap();
    for (version, expected) in [
        (json!(2), "version 2"),
        (Value::Null, "no version"),
        (json!("1"), "no version"),
    ] {
        let mut table = table_json(&bytes, 24, 0);
        table["version"] = version;
        let error = pair_from(&table, &bytes).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
    let error = Pair::from_bytes(b"not json", &bytes, Value::Null).unwrap_err();
    assert!(error.contains("not JSON"), "{error}");
}

#[test]
fn every_structure_rule_refuses_a_broken_table() {
    let bytes = serde_json::to_vec(&sidecar_json(0, 24)).unwrap();
    let cases: [(&str, fn(&mut Value), &str); 11] = [
        (
            "delta",
            |t| drop(t["aus"]["pts_delta"].as_array_mut().unwrap().pop()),
            "do not fit",
        ),
        (
            "size",
            |t| t["aus"]["size"].as_array_mut().unwrap().push(json!(1)),
            "do not fit",
        ),
        ("count", |t| t["aus"]["count"] = json!(0), "no AUs"),
        (
            "unsorted",
            |t| t["aus"]["non_picture"] = json!([5, 3]),
            "not strictly rising",
        ),
        (
            "beyond",
            |t| t["aus"]["non_picture"] = json!([24]),
            "beyond 24",
        ),
        (
            "irap on non-picture",
            |t| t["aus"]["non_picture"] = json!([8]),
            "non-picture AU 8",
        ),
        ("no irap", |t| t["aus"]["irap_crc32"] = json!([]), "no IRAP"),
        (
            "duplicate pts",
            |t| t["aus"]["pts_delta"][3] = json!(0),
            "two AUs",
        ),
        (
            "leading",
            |t| t["leading_skipped"] = json!(25),
            "leading_skipped",
        ),
        (
            "time base",
            |t| t["track"]["time_base"] = json!([1, 0]),
            "time base",
        ),
        (
            "codec",
            |t| t["track"]["codec"] = json!("avc"),
            "matroska/hevc",
        ),
    ];
    for (name, break_it, expected) in cases {
        let mut table = table_json(&bytes, 24, 0);
        break_it(&mut table);
        let error = pair_from(&table, &bytes).unwrap_err();
        assert!(error.contains(expected), "{name}: {error}");
    }
    // The accepting side of the rules above.
    let mut table = table_json(&bytes, 24, 0);
    table["aus"]["non_picture"] = json!([3, 5]);
    let parsed: AuTable = serde_json::from_value(table).unwrap();
    check_structure(&parsed).unwrap();
}

#[test]
fn the_table_must_describe_the_input_as_it_is_now() {
    let pair = good_pair(0, 24);
    let a = <[u8; 32]>::try_from(vec![0xaa; 32]).unwrap();
    let identity = FileIdentity {
        size_bytes: 1000,
        head: a,
        tail: a,
    };
    pair.check_source(Path::new("/media/clip.mkv"), &identity)
        .unwrap();
    let error = pair
        .check_source(Path::new("/media/other.mkv"), &identity)
        .unwrap_err();
    assert!(error.contains("not 'other.mkv'"), "{error}");
    for changed in [
        FileIdentity {
            size_bytes: 1001,
            ..identity
        },
        FileIdentity {
            head: [0xab; 32],
            ..identity
        },
        FileIdentity {
            tail: [0; 32],
            ..identity
        },
    ] {
        let error = pair
            .check_source(Path::new("clip.mkv"), &changed)
            .unwrap_err();
        assert!(error.contains("chunk digests"), "{error}");
    }
}

#[test]
fn the_table_and_the_sidecar_must_count_the_same_pictures() {
    let pair = good_pair(3, 24);
    pair.check_sidecar(&sidecar(3, 24)).unwrap();
    let error = pair.check_sidecar(&sidecar(2, 25)).unwrap_err();
    assert!(error.contains("leading skipped"), "{error}");
    let error = pair.check_sidecar(&sidecar(3, 23)).unwrap_err();
    assert!(error.contains("pictures"), "{error}");
}

#[test]
fn file_identity_matches_an_independent_digest() {
    let dir = tempfile::tempdir().unwrap();
    // 1.5 MiB, so the head and tail chunks overlap but differ. Expected values: Python hashlib.
    let data: Vec<u8> = (0..3usize << 19)
        .map(|i| ((i * 7 + i / 251) % 256) as u8)
        .collect();
    let path = dir.path().join("long.mkv");
    std::fs::write(&path, &data).unwrap();
    let identity = FileIdentity::of(&path).unwrap();
    assert_eq!(identity.size_bytes, 1_572_864);
    assert_eq!(
        hex(&identity.head),
        "07f4465ef6fe98070beaf8d8d01454b5d11f6cd4ff86a139d92cd031b46ddfdc"
    );
    assert_eq!(
        hex(&identity.tail),
        "1ff7d6857e1d4ca09245e1209d0d27f4863116d3e544766cfd84f02ca9fd7704"
    );
    assert_eq!(
        identity.lookup_key(),
        "548dbb7d44ea1f8edf47f3722a04e661618b0d9644aa1b36e81c918f56d4a994"
    );

    // Below one chunk both digests cover the whole file.
    let short = dir.path().join("short.mkv");
    std::fs::write(&short, b"abc").unwrap();
    let identity = FileIdentity::of(&short).unwrap();
    let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    assert_eq!(
        (hex(&identity.head), hex(&identity.tail)),
        (abc.into(), abc.into())
    );
    assert!(CHUNK_BYTES > 3);
}

#[test]
fn lookup_key_hashes_the_size_and_the_raw_digests() {
    // sha256(b"head"), sha256(b"tail") and the key, from Python hashlib.
    let digest = |hex: &str| -> [u8; 32] {
        (0..32)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
            .collect::<Vec<_>>()
            .try_into()
            .unwrap()
    };
    let identity = FileIdentity {
        size_bytes: 123_456_789,
        head: digest("9f2e6d33a3717ee826353a404ba4618d1aeeb6879ad7936bce8ed5f46814924d"),
        tail: digest("0c62f876ef1dea830de9f32c2f4b46dd6d74d50d15896e09ef5a2fcd4ac7e1d7"),
    };
    assert_eq!(
        identity.lookup_key(),
        "ee5b3868ca6a75fe7f6abbfd4ec3eb61fc06478b88bb1c4ccf27620acc728f98"
    );
}

#[test]
fn only_a_matroska_doc_type_is_matroska() {
    let header = |doc_type: &[u8]| {
        let mut bytes = vec![0x1A, 0x45, 0xDF, 0xA3, 0xA3, 0x42, 0x86, 0x81, 0x01];
        bytes.extend([0x42, 0x82, 0x80 | doc_type.len() as u8]);
        bytes.extend_from_slice(doc_type);
        bytes
    };
    assert!(is_matroska_header(&header(b"matroska")));
    assert!(!is_matroska_header(&header(b"webm")));
    assert!(!is_matroska_header(&header(b"matroskax")));
    let mut no_magic = header(b"matroska");
    no_magic[0] = 0;
    assert!(!is_matroska_header(&no_magic));
    let truncated = &header(b"matroska")[..14];
    assert!(!is_matroska_header(truncated));
    assert!(!is_matroska_header(b"\0\0\0\x18ftypisom"));
}

#[cfg(unix)]
#[test]
fn every_name_of_the_input_counts_as_the_input() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("clip.mkv");
    std::fs::write(&input, b"x").unwrap();
    let link = dir.path().join("link.json");
    std::os::unix::fs::symlink(&input, &link).unwrap();
    let hard = dir.path().join("hard.json");
    std::fs::hard_link(&input, &hard).unwrap();
    let dotted = dir.path().join("sub").join("..").join("clip.mkv");
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    for alias in [&input, &link, &hard, &dotted] {
        assert!(is_same_file(alias, &input), "{}", alias.display());
    }
    let other = dir.path().join("other.json");
    assert!(!is_same_file(&other, &input));
    std::fs::write(&other, b"x").unwrap();
    assert!(!is_same_file(&other, &input));
}

#[test]
fn paths_default_to_the_input_directory_and_refuse_the_input() {
    let paths = Paths::new("/media/movies/clip.mkv", None, None);
    assert_eq!(paths.work_dir, Path::new("/media/movies"));
    assert_eq!(
        paths.measurements,
        Path::new("/media/movies/clip_measurements.bin")
    );
    assert_eq!(
        paths.sidecar,
        Path::new("/media/movies/clip_measurements.bin.l1.json")
    );
    assert_eq!(
        paths.au_table,
        Path::new("/media/movies/clip_measurements.bin.au.json")
    );
    assert_eq!(paths.out, Path::new("/media/movies/clip.dvmanifest.json"));
    assert_eq!(
        paths.out_tmp,
        Path::new("/media/movies/clip.dvmanifest.json.tmp")
    );
    let paths = Paths::new("clip.mkv", Some(Path::new("/w")), Some(Path::new("m.json")));
    assert_eq!(paths.work_dir, Path::new("/w"));
    assert_eq!(paths.measurements, Path::new("/w/clip_measurements.bin"));
    assert_eq!(paths.log, Path::new("/w/clip.manifest-analyzer.log"));
    assert_eq!(paths.out, Path::new("m.json"));
    assert_eq!(Paths::new("clip.mkv", None, None).work_dir, Path::new("."));

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("clip.mkv");
    std::fs::write(&input, b"x").unwrap();
    let input_file = input.to_str().unwrap();
    assert_eq!(Paths::new(input_file, None, None).aliasing(&input), None);
    let paths = Paths::new(input_file, None, Some(&input));
    assert_eq!(paths.aliasing(&input), Some(input.as_path()));
    // The temp file of `--out x` is `x.tmp`: an input of that name is refused too.
    let tmp_named = dir.path().join("m.json.tmp");
    std::fs::write(&tmp_named, b"x").unwrap();
    let paths = Paths::new(
        tmp_named.to_str().unwrap(),
        None,
        Some(&dir.path().join("m.json")),
    );
    assert_eq!(paths.aliasing(&tmp_named), Some(tmp_named.as_path()));
}

/// The import's table (`--au-table-only`, no sidecar) gets the version, structure and source
/// checks of a bound one.
#[test]
fn an_unbound_table_is_read_with_the_checks_of_a_bound_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.rpu-import.au.json");
    let mut table = table_json(b"{}", 24, 0);
    table["sidecar_sha256"] = Value::Null;
    std::fs::write(&path, serde_json::to_vec(&table).unwrap()).unwrap();
    let read = AuTable::read_unbound(&path).unwrap();
    assert_eq!(read.aus.picture_count(), 24);
    assert_eq!(read.sidecar_sha256, None);

    let input = dir.path().join("clip.mkv");
    std::fs::write(&input, vec![0u8; 1000]).unwrap();
    let identity = FileIdentity::of(&input).unwrap();
    // The fixture's digests are not this file's.
    let error = read.check_source(&input, &identity).unwrap_err();
    assert!(error.contains("size or chunk digests differ"), "{error}");
    let other = dir.path().join("other.mkv");
    std::fs::write(&other, vec![0u8; 1000]).unwrap();
    let error = read.check_source(&other, &identity).unwrap_err();
    assert!(
        error.contains("describes 'clip.mkv', not 'other.mkv'"),
        "{error}"
    );
    let mut matching = table.clone();
    matching["source"]["head_sha256"] = json!(hex(&identity.head));
    matching["source"]["tail_sha256"] = json!(hex(&identity.tail));
    std::fs::write(&path, serde_json::to_vec(&matching).unwrap()).unwrap();
    AuTable::read_unbound(&path)
        .unwrap()
        .check_source(&input, &identity)
        .unwrap();

    let refused = |table: &Value, needle: &str| {
        std::fs::write(&path, serde_json::to_vec(table).unwrap()).unwrap();
        let error = AuTable::read_unbound(&path).unwrap_err();
        assert!(error.contains(needle), "'{needle}' not in: {error}");
    };
    let mut later = table.clone();
    later["version"] = json!(2);
    refused(&later, "the AU table is version 2");
    let mut empty = table.clone();
    empty["aus"]["count"] = json!(0);
    refused(&empty, "the AU table has no AUs");
    // A structure the matcher would refuse: two AUs with one pts.
    let mut twins = table;
    twins["aus"]["pts_delta"][3] = json!(0);
    refused(&twins, "have pts");
}

#[test]
fn an_rpu_import_writes_its_own_au_table_and_never_its_rpu_file() {
    let paths = Paths::for_rpu_import("clip.mkv", Some(Path::new("/w")), None);
    assert_eq!(paths.au_table, Path::new("/w/clip.rpu-import.au.json"));
    assert_eq!(
        paths.au_table_tmp,
        Path::new("/w/clip.rpu-import.au.json.tmp")
    );
    assert_eq!(paths.out, Path::new("/w/clip.dvmanifest.json"));
    assert_eq!(paths.log, Path::new("/w/clip.manifest-analyzer.log"));
    // A measured pair beside it is not one of the written paths.
    assert!(!paths
        .written()
        .contains(&Path::new("/w/clip_measurements.bin.au.json")));

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("clip.mkv");
    let rpu = dir.path().join("RPU.bin");
    std::fs::write(&input, b"x").unwrap();
    std::fs::write(&rpu, b"y").unwrap();
    let input_file = input.to_str().unwrap();
    assert_eq!(
        Paths::for_rpu_import(input_file, None, None).aliasing(&rpu),
        None
    );
    let paths = Paths::for_rpu_import(input_file, None, Some(&rpu));
    assert_eq!(paths.aliasing(&rpu), Some(rpu.as_path()));
    let paths = Paths::for_rpu_import(input_file, None, Some(&dir.path().join("RPU")));
    assert_eq!(paths.aliasing(&rpu), None);
    std::fs::rename(&rpu, dir.path().join("RPU.tmp")).unwrap();
    assert_eq!(
        paths.aliasing(&dir.path().join("RPU.tmp")),
        Some(dir.path().join("RPU.tmp").as_path()),
        "the manifest's temp file"
    );
}

#[test]
fn an_analyzer_temp_file_that_is_the_input_is_refused() {
    // The analyzer writes the sidecar and the AU table through `<path>.tmp` with
    // `File::create` (hdr_analyzer_mvp/src/l1_sidecar.rs, au_table.rs), which truncates.
    for temp_name in [
        "clip_measurements.bin.l1.json.tmp",
        "clip_measurements.bin.au.json.tmp",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("clip.mkv");
        std::fs::write(&input, b"x").unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let link = work.join(temp_name);
        std::fs::hard_link(&input, &link).unwrap();
        let paths = Paths::new(input.to_str().unwrap(), Some(&work), None);
        assert_eq!(paths.aliasing(&input), Some(link.as_path()), "{temp_name}");
    }
}

#[test]
fn the_default_analysis_is_full_and_any_other_setting_approximate() {
    let pair = good_pair(0, 24);
    let full = analysis(&pair.sidecar, &pair.table.scene).unwrap();
    assert_eq!(quality_class(&full), QualityClass::Full);
    assert_eq!(full["scene"]["metric"], "hist");
    assert_eq!(full["crop"]["width"], 256);
    assert_eq!(full["luminance_mapping"], "pq");

    let changes: [(&str, Value); 10] = [
        ("downscale", json!(2)),
        ("sample_rate", json!(3)),
        ("no_crop", json!(true)),
        ("peak_domain", json!("luma")),
        ("peak_estimator", json!("robust")),
        ("peak_source", json!("histogram99")),
        ("peak_percentile", json!(99.9)),
        ("min_percentile", json!(1.0)),
        ("denoise_mode", json!("median3")),
        (
            "scene",
            json!({"detector": "global", "metric": "hist", "threshold": 2.0, "min_length": 12}),
        ),
    ];
    for (key, value) in changes {
        let mut changed = full.clone();
        changed.insert(key.into(), value);
        assert_eq!(quality_class(&changed), QualityClass::Approximate, "{key}");
        let mut missing = full.clone();
        missing.remove(key);
        assert_eq!(
            quality_class(&missing),
            QualityClass::Approximate,
            "missing {key}"
        );
    }
    for key in ["detector", "metric", "min_length"] {
        let mut changed = full.clone();
        changed["scene"][key] = json!("other");
        assert_eq!(
            quality_class(&changed),
            QualityClass::Approximate,
            "scene {key}"
        );
    }
    assert!(analysis(&json!({"crop": {}}), &json!({})).is_err());
}

#[test]
fn a_pair_is_reused_only_at_the_sampling_the_quality_asks_for() {
    let args =
        |quality: &str| Args::try_parse_from(["mkvdovi", "--analysis-quality", quality]).unwrap();
    let sampled = |downscale: u32, sample_rate: u32| {
        let mut json = sidecar_json(0, 24);
        json["analysis"]["downscale"] = json!(downscale);
        json["analysis"]["sample_rate"] = json!(sample_rate);
        serde_json::from_value::<L1Sidecar>(json).unwrap()
    };
    // accurate measures every frame at full size; fast at half size, every third frame
    // (docs/CLI_REFERENCE.md `--analysis-quality`).
    assert_eq!(check_sampling(&sampled(1, 1), &args("accurate")), Ok(()));
    assert_eq!(check_sampling(&sampled(2, 3), &args("fast")), Ok(()));

    let coarse = check_sampling(&sampled(2, 3), &args("accurate")).unwrap_err();
    assert!(coarse.contains("downscale 2 / sample-rate 3"), "{coarse}");
    let fine = check_sampling(&sampled(1, 1), &args("fast")).unwrap_err();
    assert!(fine.contains("downscale 1 / sample-rate 1"), "{fine}");

    let mut unrecorded = sidecar_json(0, 24);
    unrecorded.as_object_mut().unwrap().remove("analysis");
    let unrecorded: L1Sidecar = serde_json::from_value(unrecorded).unwrap();
    let missing = check_sampling(&unrecorded, &args("accurate")).unwrap_err();
    assert!(missing.contains("does not record"), "{missing}");
}

fn settled(profile: &'static str) -> Settled {
    Settled {
        dv_profile: profile,
        static_meta: HashMap::from([
            ("min_dml".to_owned(), 0.0001),
            ("max_dml".to_owned(), 1000.0),
            ("max_cll".to_owned(), 1000.0),
            ("max_fall".to_owned(), 400.0),
        ]),
        cm_v40_config: Some(CmV40Config {
            source_primary_index: 2,
            content_type: 1,
            reference_mode: false,
        }),
        level5_offsets: None,
        source_range: None,
    }
}

fn inputs<'a>(
    pair: &'a Pair,
    sidecar: &'a L1Sidecar,
    settled: &'a Settled,
    hlg_composer: Option<Composer>,
) -> Inputs<'a> {
    Inputs {
        file_name: "clip.mkv".into(),
        identity: FileIdentity {
            size_bytes: 1000,
            head: [0xaa; 32],
            tail: [0xaa; 32],
        },
        pair,
        sidecar,
        settled,
        trim_targets: &[100, 600, 1000],
        hlg_composer,
    }
}

#[test]
fn shots_move_back_by_the_leading_pictures_and_the_producer_is_named() {
    let pair = good_pair(3, 24);
    let sidecar = sidecar(3, 24);
    let settled = settled("8.1");
    let built = manifest(&inputs(&pair, &sidecar, &settled, None)).unwrap();
    // Scene 0 also covers the 3 leading pictures; scene 1 (measured frame 10) is picture 13.
    let starts: Vec<u64> = built.shots.iter().map(|shot| shot.start).collect();
    assert_eq!(starts, [0, 13]);
    assert_eq!(built.shots[1].l1.max_pq, 3100);
    assert_eq!(built.coverage.len(), 1);
    assert_eq!((built.coverage[0].start, built.coverage[0].end), (0, 27));
    assert_eq!(
        built.producer.sidecar_sha256.as_deref(),
        Some(pair.sidecar_sha256.as_str())
    );
    assert_eq!(built.producer.rpu_bin_sha256, None);
    assert_eq!(built.producer.au_table_version, 1);
    assert_eq!(built.producer.sidecar_version, Some(6));
    assert_eq!(
        built.producer.rpu_builder_version,
        dovi_rpu_builder::VERSION
    );
    assert_eq!(built.producer.mkvdovi_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(built.source.lookup_key, built_lookup_key());
    assert!(matches!(built.rpu.profile, Profile::P81 { .. }));
    // The written text parses back to the same manifest.
    let text = serde_json::to_string(&built.to_json().unwrap()).unwrap();
    assert_eq!(
        dovi_rpu_builder::manifest::PlaybackManifest::parse(&text).unwrap(),
        built
    );
}

#[test]
fn a_temp_file_another_writer_holds_is_left_alone() {
    // Two runs writing the same --out: if this run removed the other's `<out>.tmp`, the other
    // could rename this run's file, or its own empty one, into place and report success.
    let pair = good_pair(0, 24);
    let sidecar = sidecar(0, 24);
    let settled = settled("8.1");
    let built = manifest(&inputs(&pair, &sidecar, &settled, None)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(
        "clip.mkv",
        Some(dir.path()),
        Some(&dir.path().join("m.json")),
    );
    let mut other = std::fs::File::create(&paths.out_tmp).unwrap();

    let error = write_manifest(&built, &paths).unwrap_err();
    assert!(format!("{error:#}").contains("m.json.tmp"), "{error:#}");
    assert!(!paths.out.exists());
    // The other writer's file is still the one at that name.
    std::io::Write::write_all(&mut other, b"other").unwrap();
    assert_eq!(std::fs::read(&paths.out_tmp).unwrap(), b"other");

    // Once it is gone, the manifest is written and parses back.
    std::fs::remove_file(&paths.out_tmp).unwrap();
    write_manifest(&built, &paths).unwrap();
    let text = std::fs::read_to_string(&paths.out).unwrap();
    assert_eq!(
        dovi_rpu_builder::manifest::PlaybackManifest::parse(&text).unwrap(),
        built
    );
    assert!(!paths.out_tmp.exists());
}

fn built_lookup_key() -> String {
    FileIdentity {
        size_bytes: 1000,
        head: [0xaa; 32],
        tail: [0xaa; 32],
    }
    .lookup_key()
}

#[test]
fn a_profile_84_manifest_names_the_selected_composer() {
    let mut json = sidecar_json(0, 24);
    json["analysis"]["luminance_mapping"] = json!(Composer::Bt2100V1.luminance_mapping());
    let bytes = serde_json::to_vec(&json).unwrap();
    let pair = pair_from(&table_json(&bytes, 24, 0), &bytes).unwrap();
    let sidecar: L1Sidecar = serde_json::from_value(json).unwrap();
    let settled = settled("8.4");
    let built = manifest(&inputs(&pair, &sidecar, &settled, Some(Composer::Bt2100V1))).unwrap();
    assert_eq!(
        built.rpu.profile,
        Profile::P84 {
            composer: HlgComposer::Bt2100
        }
    );
    // The preset against a bt2100 measurement does not validate.
    let error = manifest(&inputs(&pair, &sidecar, &settled, Some(Composer::Preset))).unwrap_err();
    assert!(
        format!("{error:#}").contains("luminance_mapping"),
        "{error:#}"
    );
    let error = manifest(&inputs(&pair, &sidecar, &settled, None)).unwrap_err();
    assert!(format!("{error:#}").contains("HLG composer"), "{error:#}");
}

#[test]
fn shots_that_do_not_cover_the_table_are_refused() {
    // The table has one picture more than the sidecar's shots cover.
    let json = sidecar_json(0, 24);
    let bytes = serde_json::to_vec(&json).unwrap();
    let pair = pair_from(&table_json(&bytes, 25, 0), &bytes).unwrap();
    let sidecar: L1Sidecar = serde_json::from_value(json).unwrap();
    let settled = settled("8.1");
    let error = manifest(&inputs(&pair, &sidecar, &settled, None)).unwrap_err();
    assert!(
        format!("{error:#}").contains("cover 24 pictures"),
        "{error:#}"
    );
}
