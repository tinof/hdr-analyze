use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use dolby_vision::rpu::extension_metadata::blocks::{
    ExtMetadataBlock, ExtMetadataBlockLevel10, ExtMetadataBlockLevel8, ExtMetadataBlockLevel9,
};
use dolby_vision::rpu::extension_metadata::{DmData, WithExtMetadataBlocks};
use dolby_vision::rpu::generate::{GenerateConfig, GenerateProfile, VideoShot};
use dolby_vision::rpu::rpu_data_mapping::DoviNlqMethod;
use dolby_vision::rpu::rpu_data_nlq::RpuDataNlq;
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

#[test]
fn is_unparseable_tells_the_guards_refusal_from_other_parse_errors() {
    let dir = tempfile::tempdir().unwrap();
    let refused = write(dir.path(), "refused.bin", &linear_interp_rpu_bin());
    let error = parse_source_rpu_file(&refused)
        .context("classify")
        .unwrap_err();
    assert!(is_unparseable(&error), "{error:#}");

    let empty = write(dir.path(), "empty.bin", &[]);
    let error = parse_source_rpu_file(&empty).unwrap_err();
    assert!(!is_unparseable(&error), "{error:#}");
}

/// A count the crate cannot allocate: `Vec::with_capacity` panics with a capacity overflow
/// (unwinding in tests, an abort in release builds) instead of failing the allocation.
const HUGE_COUNT: u64 = 1 << 62;

/// One generated Profile 8.1 RPU, changed by `edit` and written back by the crate's own writer,
/// so the CRC is real and a file of such RPUs parses when the edit is valid.
fn crate_written(cm_version: CmVersion, edit: impl FnOnce(&mut DoviRpu)) -> Vec<u8> {
    let nal = &generated(GenerateProfile::Profile81, cm_version)[0];
    let mut rpu = DoviRpu::parse_unspec62_nalu(nal).unwrap();
    edit(&mut rpu);
    rpu.modified = true;
    rpu.write_hevc_unspec62_nalu().unwrap()
}

/// A Profile 7 MEL/FEL layout: residual enabled, so the mapping carries NLQ pivots and the NLQ
/// data follows the curve pieces. Non-zero NLQ fields, so a walk that skips them misreads the
/// DM payload.
fn to_profile7(rpu: &mut DoviRpu) {
    rpu.dovi_profile = 7;
    rpu.header.el_spatial_resampling_filter_flag = true;
    rpu.header.disable_residual_flag = false;
    rpu.header.vdr_bit_depth_minus8 = 4;
    let mapping = rpu.rpu_data_mapping.as_mut().unwrap();
    mapping.nlq_method_idc = Some(DoviNlqMethod::LinearDeadzone);
    mapping.nlq_num_pivots_minus2 = Some(0);
    mapping.nlq_pred_pivot_value = Some([0, 1023]);
    mapping.nlq = Some(RpuDataNlq {
        nlq_offset: [512, 300, 7],
        vdr_in_max_int: [1, 2, 3],
        vdr_in_max: [0x7F_FFFF, 0x40_0000, 1],
        linear_deadzone_slope_int: [0, 5, 0],
        linear_deadzone_slope: [0x12_3456, 0, 0x7F_0000],
        linear_deadzone_threshold_int: [9, 0, 1],
        linear_deadzone_threshold: [3, 0x55_5555, 0],
    });
}

/// The compressed DM form: only the three ids before the extension blocks.
fn compress_dm(rpu: &mut DoviRpu) {
    rpu.header.reserved_zero_3bits = 1;
    rpu.vdr_dm_data.as_mut().unwrap().compressed = true;
}

/// `use_prev_vdr_rpu_flag`: no mapping is written, the DM payload still is.
fn reuse_mapping(rpu: &mut DoviRpu) {
    rpu.header.use_prev_vdr_rpu_flag = true;
    rpu.header.prev_vdr_rpu_id = 0;
}

/// Declare `count` extension blocks for the CM v2.9 (`v40 == false`) or v4.0 metadata, without
/// changing the blocks the writer writes after the count.
fn declare_blocks(rpu: &mut DoviRpu, v40: bool, count: u64) {
    let dm = rpu.vdr_dm_data.as_mut().unwrap();
    let metadata = if v40 {
        &mut dm.cmv40_metadata
    } else {
        &mut dm.cmv29_metadata
    };
    match metadata.as_mut().unwrap() {
        DmData::V29(blocks) => blocks.set_num_ext_blocks(count),
        DmData::V40(blocks) => blocks.set_num_ext_blocks(count),
    }
}

/// The edit and the CM version of each RPU layout the DM walk must follow.
fn layouts() -> Vec<(&'static str, CmVersion, fn(&mut DoviRpu))> {
    vec![
        ("p81 v29", CmVersion::V29, |_| {}),
        ("p81 v40", CmVersion::V40, |_| {}),
        ("profile 7 nlq", CmVersion::V40, to_profile7),
        ("compressed dm", CmVersion::V40, compress_dm),
        ("reuses the previous mapping", CmVersion::V40, reuse_mapping),
    ]
}

#[test]
fn the_crate_still_panics_on_an_extension_block_count() {
    // When this fails, `dolby_vision` no longer allocates the blocks before it reads them:
    // re-check the DM walk (module docs) and whether E15.2's guard is still needed.
    let nal = crate_written(CmVersion::V29, |rpu| declare_blocks(rpu, false, HUGE_COUNT));
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "RPU.bin", &bin_of_nals(&[nal]));
    assert!(crate_panics(&path));
}

#[test]
fn an_extension_block_count_is_an_error_where_the_crate_panics() {
    let good = crate_written(CmVersion::V40, |_| {});
    let dir = tempfile::tempdir().unwrap();
    for (name, cm_version, edit) in layouts() {
        let mut targets = vec![(false, "CM v2.9")];
        if cm_version == CmVersion::V40 {
            targets.push((true, "CM v4.0"));
        }
        for (v40, expected) in targets {
            let bad = crate_written(cm_version, |rpu| {
                edit(rpu);
                declare_blocks(rpu, v40, HUGE_COUNT);
            });
            // The bad RPU is the second of three: the error names it, by its index in the file.
            let bytes = bin_of_nals(&[good.clone(), bad, good.clone()]);
            let case = format!("{name}, {expected}");
            let path = write(dir.path(), "RPU.bin", &bytes);
            assert!(
                crate_panics(&path),
                "{case}: the fixture must reach the panic"
            );

            let error = parse_source_rpu_file(&path).unwrap_err();
            assert!(is_unparseable(&error), "{case}: {error:#}");
            let error = format!("{error:#}");
            assert!(error.contains("RPU 1:"), "{case}: {error}");
            assert!(
                error.contains(&format!(
                    "the {expected} DM metadata declares {HUGE_COUNT} extension blocks"
                )),
                "{case}: {error}"
            );
        }
    }
}

#[test]
fn valid_dm_payloads_pass_and_the_walk_ends_at_the_crc() {
    let dir = tempfile::tempdir().unwrap();
    for (name, cm_version, edit) in layouts() {
        let nal = crate_written(cm_version, edit);
        let bytes = bin_of_nals(std::slice::from_ref(&nal));
        let path = write(dir.path(), "RPU.bin", &bytes);
        let parsed = parse_rpu_file(&path).unwrap_or_else(|error| panic!("{name}: {error:#}"));
        assert_eq!(parsed.len(), 1, "{name}");
        check_parseable(&bytes).unwrap_or_else(|error| panic!("{name}: {error:#}"));

        // The walk reads every field the crate reads: after it only the rpu_alignment_zero_bits,
        // the CRC32 and the final byte are left (32 + 8 bits, from the RPU syntax).
        let payload = clear_start_code_emulation_prevention_3_byte(&nal[2..]);
        let mut reader = BitReader::new(&payload);
        assert_eq!(scan(&mut reader), Some(None), "{name}");
        let alignment = (8 - reader.pos % 8) % 8;
        assert_eq!(reader.remaining(), alignment + 40, "{name}");
    }
}

#[test]
fn a_count_the_rpu_could_hold_is_left_to_the_crates_error() {
    let dir = tempfile::tempdir().unwrap();
    // The generated CM v2.9 metadata holds a few blocks: one more is still fewer than the
    // RPU's bits, so the crate allocates it and fails reading the missing block.
    let one_more = crate_written(CmVersion::V29, |rpu| {
        let dm = rpu.vdr_dm_data.as_mut().unwrap();
        let Some(DmData::V29(blocks)) = dm.cmv29_metadata.as_mut() else {
            unreachable!()
        };
        let count = blocks.num_ext_blocks();
        blocks.set_num_ext_blocks(count + 1);
    });
    let bytes = bin_of_nals(&[one_more]);
    let path = write(dir.path(), "RPU.bin", &bytes);
    assert!(!crate_panics(&path));
    check_parseable(&bytes).unwrap();
    let error = parse_source_rpu_file(&path).unwrap_err();
    assert!(!is_unparseable(&error), "{error:#}");
}

#[test]
fn the_dm_walk_stops_where_the_crate_refuses_a_block() {
    // A CM v4.0 level in the CM v2.9 metadata: the crate refuses the block ("Disallowed block
    // level") before it reaches the CM v4.0 count, so the huge count behind it is not refused.
    let nal = crate_written(CmVersion::V40, |rpu| declare_blocks(rpu, true, HUGE_COUNT));
    let mut payload = clear_start_code_emulation_prevention_3_byte(&nal[2..]);
    let mut reader = BitReader::new(&payload);
    assert!(matches!(
        scan(&mut reader),
        Some(Some(Unreadable::ExtBlockCount {
            dm_version: DmVersion::V40,
            ..
        }))
    ));

    // Find the first CM v2.9 level byte and make it level 254 (allowed only in CM v4.0).
    let mut probe = BitReader::new(&payload);
    probe.bits(8).unwrap();
    let header = read_header(&mut probe).unwrap();
    scan_mapping(&mut probe, &header).unwrap();
    probe.ue().unwrap();
    probe.ue().unwrap();
    probe.ue().unwrap();
    probe.skip(DM_FIXED_BITS).unwrap();
    assert!(probe.ue().unwrap() > 0, "the fixture needs a CM v2.9 block");
    while probe.pos % 8 != 0 {
        probe.bit().unwrap();
    }
    probe.ue().unwrap(); // ext_block_length
    let level_at = probe.pos;
    for bit in 0..8 {
        let at = level_at + bit;
        let mask = 0x80 >> (at % 8);
        if (254u8 >> (7 - bit)) & 1 == 1 {
            payload[at / 8] |= mask;
        } else {
            payload[at / 8] &= !mask;
        }
    }
    assert_eq!(scan(&mut BitReader::new(&payload)), None);

    // The crate stops at the same block, with an error and without the allocation.
    dolby_vision::utils::add_start_code_emulation_prevention_3_byte(&mut payload);
    let mut patched = nal[..2].to_vec(); // the NAL header `bin_of_nals` drops
    patched.extend(payload);
    let bytes = bin_of_nals(&[patched]);
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "RPU.bin", &bytes);
    assert!(!crate_panics(&path));
    check_parseable(&bytes).unwrap();
    assert!(parse_rpu_file(&path).is_err());
}

#[test]
fn trailing_zero_bytes_do_not_open_the_cm_v40_metadata() {
    // The crate reads up to the last non-zero byte. Behind CM v2.9 metadata only the alignment,
    // the CRC32 and the final byte are left (under 56 bits), so it reads no CM v4.0 count; with
    // the zeros counted, the walk would read one from the CRC.
    // A CRC that starts with eight zero bits reads as a count above the bits left: about one
    // CRC in 256 does, so 1024 variants reach it several times.
    let dir = tempfile::tempdir().unwrap();
    for crc_lead in 0..1024u16 {
        let nal = crate_written(CmVersion::V29, |rpu| {
            rpu.vdr_dm_data.as_mut().unwrap().source_diagonal = crc_lead;
        });
        let mut bytes = bin_of_nals(&[nal]);
        bytes.extend_from_slice(&[0; 16]);
        let path = write(dir.path(), "RPU.bin", &bytes);
        assert_eq!(parse_rpu_file(&path).unwrap().len(), 1, "{crc_lead}");
        check_parseable(&bytes).unwrap_or_else(|error| panic!("{crc_lead}: {error:#}"));
    }
}

/// A CM v4.0 RPU written by the crate with an L8, L9 and L10 block of their longest lengths.
fn with_variable_blocks() -> Vec<u8> {
    crate_written(CmVersion::V40, |rpu| {
        let dm = rpu.vdr_dm_data.as_mut().unwrap();
        let Some(DmData::V40(blocks)) = dm.cmv40_metadata.as_mut() else {
            unreachable!()
        };
        for level in [8, 9, 10] {
            blocks.remove_level(level);
        }
        let added = [
            ExtMetadataBlock::Level8(ExtMetadataBlockLevel8 {
                length: 25,
                ..Default::default()
            }),
            // Custom primaries (index 255, every coordinate set), as the crate requires.
            ExtMetadataBlock::Level9(ExtMetadataBlockLevel9 {
                length: 17,
                source_primary_index: 255,
                source_primary_red_x: 1,
                source_primary_red_y: 2,
                source_primary_green_x: 3,
                source_primary_green_y: 4,
                source_primary_blue_x: 5,
                source_primary_blue_y: 6,
                source_primary_white_x: 7,
                source_primary_white_y: 8,
            }),
            ExtMetadataBlock::Level10(ExtMetadataBlockLevel10 {
                length: 21,
                target_display_index: 2, // not a preset display
                target_max_pq: 3079,
                target_min_pq: 62,
                target_primary_index: 255,
                target_primary_red_x: 1,
                target_primary_red_y: 2,
                target_primary_green_x: 3,
                target_primary_green_y: 4,
                target_primary_blue_x: 5,
                target_primary_blue_y: 6,
                target_primary_white_x: 7,
                target_primary_white_y: 8,
            }),
        ];
        for block in added {
            blocks.add_block(block).unwrap();
        }
    })
}

/// The bit position, in `payload`, of the `ext_block_length` of the first DM block of `level`.
fn block_length_at(payload: &[u8], level: u64) -> usize {
    let mut r = BitReader::new(payload);
    r.bits(8).unwrap();
    let header = read_header(&mut r).unwrap();
    scan_mapping(&mut r, &header).unwrap();
    for _ in 0..3 {
        r.ue().unwrap(); // the DM ids
    }
    r.skip(DM_FIXED_BITS).unwrap();
    loop {
        let count = r.ue().unwrap();
        while r.pos % 8 != 0 {
            r.bit().unwrap();
        }
        for _ in 0..count {
            let at = r.pos;
            let length = r.ue().unwrap();
            if r.bits(8).unwrap() == level {
                return at;
            }
            r.skip(length * 8).unwrap();
        }
    }
}

/// `nal` with the `ue(v)` length of its first `level` block one larger. The lengths used here
/// (25, 17, 21) have an even code number, so `ue(length + 1)` differs only in its last bit.
fn length_plus_one(nal: &[u8], level: u64) -> Vec<u8> {
    let mut payload = clear_start_code_emulation_prevention_3_byte(&nal[2..]);
    let at = block_length_at(&payload, level);
    let mut r = BitReader::new(&payload);
    r.skip(at as u64).unwrap();
    let length = r.ue().unwrap();
    assert_eq!((length + 1) % 2, 0, "level {level}: length {length}");
    let last = r.pos - 1;
    payload[last / 8] |= 0x80 >> (last % 8);
    dolby_vision::utils::add_start_code_emulation_prevention_3_byte(&mut payload);
    let mut patched = nal[..2].to_vec();
    patched.extend(payload);
    patched
}

#[test]
fn the_crate_still_panics_on_a_block_length() {
    // When this fails, `dolby_vision` reads an L9 block of 18 bytes: re-check
    // `unknown_length_reads` (module docs) and whether that part of E15.2's guard is needed.
    let nal = length_plus_one(&with_variable_blocks(), 9);
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "RPU.bin", &bin_of_nals(&[nal]));
    assert!(crate_panics(&path));
}

#[test]
fn a_block_length_the_crate_cannot_read_is_an_error_where_it_panics() {
    let good = with_variable_blocks();
    let dir = tempfile::tempdir().unwrap();
    // The written blocks are read: the crate parses the file and the guard lets it through.
    let path = write(
        dir.path(),
        "good.bin",
        &bin_of_nals(std::slice::from_ref(&good)),
    );
    assert!(parse_rpu_file(&path).is_ok());
    assert!(parse_source_rpu_file(&path).is_ok());

    for (level, length) in [(8, 26), (9, 18), (10, 22)] {
        let bad = length_plus_one(&good, level);
        let bytes = bin_of_nals(&[good.clone(), bad, good.clone()]);
        let path = write(dir.path(), "RPU.bin", &bytes);
        assert!(
            crate_panics(&path),
            "level {level}: the fixture must reach the panic"
        );

        let error = parse_source_rpu_file(&path).unwrap_err();
        assert!(is_unparseable(&error), "level {level}: {error:#}");
        let error = format!("{error:#}");
        assert!(error.contains("RPU 1:"), "level {level}: {error}");
        assert!(
            error.contains(&format!(
                "a level {level} DM metadata block declares {length} bytes"
            )),
            "level {level}: {error}"
        );
    }
}

#[test]
fn a_block_cut_short_is_left_to_the_crates_error() {
    // The L9 block of 18 bytes, but the RPU ends inside its primaries: the crate's read fails
    // before `required_bits`, so the walk stops with it.
    let nal = length_plus_one(&with_variable_blocks(), 9);
    let payload = clear_start_code_emulation_prevention_3_byte(&nal[2..]);
    let fields = block_length_at(&payload, 9) + 9 + 8; // ue(18) and the level
    assert!(scan(&mut BitReader::new(&payload)).flatten().is_some());
    assert_eq!(
        scan(&mut BitReader::new(&payload[..(fields + 64) / 8])),
        None
    );
}

#[test]
fn the_crate_reads_these_bits_for_a_length_it_does_not_know() {
    // The field reads of `ExtMetadataBlockLevel{8,9,10}::parse`, by length threshold.
    let cases = [
        (8, 9, Some(80)),
        (8, 11, Some(92)),
        (8, 14, Some(152)),
        (8, 26, Some(200)),
        (8, 25, None),
        (9, 0, Some(8)),
        (9, 2, Some(136)),
        (9, 17, None),
        (10, 4, Some(40)),
        (10, 6, Some(168)),
        (10, 5, None),
        (11, 5, None),
        (254, 3, None),
    ];
    for (level, length, bits) in cases {
        assert_eq!(
            unknown_length_reads(level, length),
            bits,
            "L{level} {length}"
        );
    }
}

#[test]
fn a_count_above_one_block_per_nine_bits_is_refused() {
    // The whole payload has fewer bits than the count times nine (a block's length and level),
    // so the crate cannot read that many blocks: refused, though the allocation would succeed.
    let probe = crate_written(CmVersion::V29, |_| {});
    let total_bits = clear_start_code_emulation_prevention_3_byte(&probe[2..]).len() as u64 * 8;
    let count = total_bits / 9 + 1;
    let nal = crate_written(CmVersion::V29, |rpu| declare_blocks(rpu, false, count));
    let error = format!("{:#}", check_parseable(&bin_of_nals(&[nal])).unwrap_err());
    assert!(
        error.contains(&format!("declares {count} extension blocks")),
        "{error}"
    );
}
