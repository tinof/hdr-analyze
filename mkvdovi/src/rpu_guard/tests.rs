use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use dolby_vision::rpu::generate::{GenerateConfig, GenerateProfile, VideoShot};
use dolby_vision::rpu::vdr_dm_data::CmVersion;
use dovi_rpu_builder::{static_84_params, HlgComposer, RpuBuilder, L1};

use super::fixtures::{linear_interp_rpu_bin, rpu_bin, Piece, Rpu, LINEAR_INTERP, MMR, POLY};
use super::*;

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

/// Whether `parse_rpu_file` panics on the file: the independent oracle for "the parent aborts"
/// (tests unwind; release builds abort).
fn crate_panics(path: &Path) -> bool {
    catch_unwind(AssertUnwindSafe(|| parse_rpu_file(path))).is_err()
}

/// Valid RPUs as the crate writes them, as an `RPU.bin` (start code, then the NAL without its
/// two-byte header).
fn bin_of_nals(nals: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for nal in nals {
        bytes.extend_from_slice(&[0, 0, 0, 1]);
        bytes.extend_from_slice(&nal[2..]);
    }
    bytes
}

fn generated(profile: GenerateProfile, cm_version: CmVersion) -> Vec<Vec<u8>> {
    GenerateConfig {
        profile,
        cm_version,
        length: 3,
        shots: vec![VideoShot {
            duration: 3,
            ..Default::default()
        }],
        ..Default::default()
    }
    .generate_rpu_list()
    .unwrap()
    .iter()
    .map(|rpu| rpu.write_hevc_unspec62_nalu().unwrap())
    .collect()
}

#[test]
fn the_crate_still_panics_on_a_linear_interpolation_piece() {
    // When this fails, `dolby_vision` reads the piece: re-check the scan (module docs) and
    // whether E15.1's guard is still needed.
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "RPU.bin", &linear_interp_rpu_bin());
    assert!(crate_panics(&path));
}

#[test]
fn a_linear_interpolation_piece_is_an_error_where_the_crate_panics() {
    let good = Rpu::p81(POLY, MMR, MMR).payload();
    let mut multi_piece = Rpu::p81(POLY, MMR, MMR);
    multi_piece.num_pivots_minus2 = [1, 0, 0];
    multi_piece.pieces[0] = vec![
        Piece::Polynomial {
            order_minus1: 1,
            linear_interp: false,
        },
        LINEAR_INTERP,
    ];
    let mut profile7 = Rpu::p81(POLY, LINEAR_INTERP, MMR);
    profile7.profile7 = true;
    let mut float = Rpu::p81(POLY, MMR, LINEAR_INTERP);
    float.float_coefficients = true;

    let cases = [
        (
            "luma",
            Rpu::p81(LINEAR_INTERP, MMR, MMR),
            "piece 0 of the Y",
        ),
        (
            "cb",
            Rpu::p81(POLY, LINEAR_INTERP, MMR),
            "piece 0 of the Cb",
        ),
        (
            "cr after mmr",
            Rpu::p81(POLY, MMR, LINEAR_INTERP),
            "piece 0 of the Cr",
        ),
        (
            "cr after first-order mmr",
            Rpu::p81(POLY, Piece::Mmr { order_minus1: 0 }, LINEAR_INTERP),
            "piece 0 of the Cr",
        ),
        ("second piece", multi_piece, "piece 1 of the Y"),
        ("profile 7 nlq", profile7, "piece 0 of the Cb"),
        ("float coefficients", float, "piece 0 of the Cr"),
    ];
    let dir = tempfile::tempdir().unwrap();
    for (name, rpu, expected) in cases {
        // The bad RPU is the second of three: the error names it, by its index in the file.
        let bytes = rpu_bin(&[good.clone(), rpu.payload(), good.clone()]);
        let path = write(dir.path(), &format!("{name}.bin"), &bytes);
        assert!(
            crate_panics(&path),
            "{name}: the fixture must reach the panic"
        );

        let error = format!("{:#}", parse_source_rpu_file(&path).unwrap_err());
        assert!(error.contains("RPU 1:"), "{name}: {error}");
        assert!(error.contains(expected), "{name}: {error}");
        assert!(error.contains("linear interpolation"), "{name}: {error}");
        assert!(check_parseable(&bytes).is_err(), "{name}");
    }
}

#[test]
fn rpus_the_crate_reads_pass_the_guard_unchanged() {
    let p84 = |composer| {
        let mut shot = RpuBuilder::new(&static_84_params(composer))
            .unwrap()
            .shot(L1::new(0, 1500, 3000))
            .unwrap();
        vec![shot.write(true).unwrap(), shot.write(false).unwrap()]
    };
    let cases = [
        (
            "p81 v40",
            generated(GenerateProfile::Profile81, CmVersion::V40),
        ),
        (
            "p81 v29",
            generated(GenerateProfile::Profile81, CmVersion::V29),
        ),
        ("p5", generated(GenerateProfile::Profile5, CmVersion::V40)),
        ("p84 preset", p84(HlgComposer::Preset)),
        ("p84 bt2100", p84(HlgComposer::Bt2100)),
    ];
    let dir = tempfile::tempdir().unwrap();
    for (name, nals) in cases {
        let bytes = bin_of_nals(&nals);
        let path = write(dir.path(), &format!("{name}.bin"), &bytes);
        check_parseable(&bytes).unwrap_or_else(|error| panic!("{name}: {error:#}"));
        let guarded = parse_source_rpu_file(&path).unwrap();
        let direct = parse_rpu_file(&path).unwrap();
        assert_eq!(guarded.len(), nals.len(), "{name}");
        let written = |rpus: &[DoviRpu]| -> Vec<Vec<u8>> {
            rpus.iter().map(|rpu| rpu.write_rpu().unwrap()).collect()
        };
        assert_eq!(written(&guarded), written(&direct), "{name}");
    }
}

#[test]
fn rpus_without_the_piece_pass_the_guard_and_do_not_panic_the_crate() {
    let mut multi_piece = Rpu::p81(POLY, MMR, MMR);
    multi_piece.num_pivots_minus2 = [2, 1, 0];
    multi_piece.pieces = [
        vec![
            POLY,
            Piece::Polynomial {
                order_minus1: 1,
                linear_interp: false,
            },
            POLY,
        ],
        vec![MMR, Piece::Mmr { order_minus1: 0 }],
        vec![MMR],
    ];
    let mut profile7 = Rpu::p81(POLY, MMR, MMR);
    profile7.profile7 = true;
    let mut float = Rpu::p81(POLY, POLY, POLY);
    float.float_coefficients = true;
    let mut reuse = Rpu::p81(LINEAR_INTERP, MMR, MMR);
    // With `use_prev_vdr_rpu_flag` no mapping is read, so the piece below is never written.
    reuse.reuse_previous = true;

    let cases = [
        ("p81", Rpu::p81(POLY, MMR, MMR)),
        ("multi piece", multi_piece),
        ("profile 7 nlq", profile7),
        ("float coefficients", float),
        ("reuses the previous mapping", reuse),
    ];
    let dir = tempfile::tempdir().unwrap();
    for (name, rpu) in cases {
        let bytes = rpu_bin(&[rpu.payload()]);
        let path = write(dir.path(), &format!("{name}.bin"), &bytes);
        assert!(!crate_panics(&path), "{name}");
        check_parseable(&bytes).unwrap_or_else(|error| panic!("{name}: {error:#}"));
    }
}

#[test]
fn a_pivot_count_larger_than_the_rpu_is_an_error() {
    // The crate would allocate 2^40 pivots before reading the first: no crate oracle here, the
    // allocation failure aborts even a test build.
    let mut rpu = Rpu::p81(POLY, MMR, MMR);
    rpu.num_pivots_minus2 = [1 << 40, 0, 0];
    rpu.omit_pivots = true;
    let bytes = rpu_bin(&[rpu.payload()]);
    let error = format!("{:#}", check_parseable(&bytes).unwrap_err());
    assert!(error.contains("RPU 0:"), "{error}");
    assert!(
        error.contains(&format!("declares {} pivots", (1u64 << 40) + 2)),
        "{error}"
    );

    // Just above what the RPU holds (it is about 260 bits long).
    let mut near = Rpu::p81(POLY, MMR, MMR);
    near.num_pivots_minus2 = [298, 0, 0];
    near.omit_pivots = true;
    let error = format!(
        "{:#}",
        check_parseable(&rpu_bin(&[near.payload()])).unwrap_err()
    );
    assert!(error.contains("declares 300 pivots"), "{error}");

    // The crate's release build wraps 2^64 pivots to none, but then allocates 2^64 - 1 pieces.
    let mut wraps = Rpu::p81(POLY, MMR, MMR);
    wraps.num_pivots_minus2 = [u64::MAX - 1, 0, 0];
    wraps.omit_pivots = true;
    let error = format!(
        "{:#}",
        check_parseable(&rpu_bin(&[wraps.payload()])).unwrap_err()
    );
    assert!(
        error.contains(&format!("declares {} pivots", 1u128 << 64)),
        "{error}"
    );

    // The same count, when the pivots fit, is read and passes.
    let mut fits = Rpu::p81(POLY, MMR, MMR);
    fits.num_pivots_minus2 = [1, 0, 0];
    fits.omit_pivots = true;
    assert!(check_parseable(&rpu_bin(&[fits.payload()])).is_ok());
}

#[test]
fn a_value_the_crate_refuses_stops_the_walk_before_a_later_piece() {
    let mut wide_coefficients = Rpu::p81(POLY, POLY, LINEAR_INTERP);
    // More than 64 bits per coefficient: the crate refuses the header, and its read would fail.
    wide_coefficients.log2_denom = 65;
    let cases = [
        (
            "mmr order 4",
            Rpu::p81(POLY, Piece::Mmr { order_minus1: 3 }, LINEAR_INTERP),
        ),
        (
            "polynomial order 3",
            Rpu::p81(
                Piece::Polynomial {
                    order_minus1: 2,
                    linear_interp: false,
                },
                MMR,
                LINEAR_INTERP,
            ),
        ),
        ("coefficients over 64 bits", wide_coefficients),
    ];
    let dir = tempfile::tempdir().unwrap();
    for (name, rpu) in cases {
        let bytes = rpu_bin(&[rpu.payload()]);
        let path = write(dir.path(), &format!("{name}.bin"), &bytes);
        assert!(!crate_panics(&path), "{name}: the crate must stop first");
        check_parseable(&bytes).unwrap_or_else(|error| panic!("{name}: {error:#}"));
    }
}

#[test]
fn bytes_the_crate_refuses_pass_the_guard_to_its_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut truncated = linear_interp_rpu_bin();
    // Cut short: the crate refuses the length before it reads a field.
    truncated.truncate(12);
    for (name, bytes) in [
        ("empty", Vec::new()),
        ("no start code", vec![0x19; 64]),
        ("truncated", truncated),
    ] {
        assert!(check_parseable(&bytes).is_ok(), "{name}");
        let path = write(dir.path(), &format!("{name}.bin"), &bytes);
        assert!(!crate_panics(&path), "{name}");
        assert!(parse_source_rpu_file(&path).is_err(), "{name}");
    }
}

#[test]
fn the_walk_stops_where_the_crate_reads_past_the_end() {
    let payload = Rpu::p81(LINEAR_INTERP, MMR, MMR).payload();
    // Ten bytes end inside the mapping, before the luma piece: the crate's read fails there.
    assert_eq!(scan(&mut BitReader::new(&payload[..10])), None);
    assert_eq!(
        scan(&mut BitReader::new(&payload)),
        Some(Some(Unreadable::LinearInterpolation {
            component: 0,
            piece: 0
        }))
    );
}

#[test]
fn the_committed_fixture_is_the_linear_interpolation_rpu() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/linear_interp_RPU.bin");
    assert_eq!(fs::read(path).unwrap(), linear_interp_rpu_bin());
}
