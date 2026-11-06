//! Unit tests of the `RPU.bin` import (ROADMAP P15.4). The files are built with
//! `dovi_rpu_builder` from known parameters and shots, the independent source of the expected
//! values, and then edited with the `dolby_vision` crate where a test needs a picture that the
//! builder would not write.

use dolby_vision::rpu::extension_metadata::blocks::{
    ExtMetadataBlockLevel1, ExtMetadataBlockLevel6,
};
use serde_json::json;

use super::*;
use crate::rpu_guard::is_unparseable;

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// Ten pictures in 11 AUs. Decode order: IDR(pts 0) P(4) B(1) B(2) B(3) EOS(100, no picture)
/// CRA(5) P(9) B(6) B(7) B(8). Pictures by pts: decode indices 0 2 3 4 1 6 8 9 10 7.
const PTS: [i64; 11] = [0, 4, 1, 2, 3, 100, 5, 9, 6, 7, 8];
const DECODE_BY_PRESENTATION: [u64; 10] = [0, 2, 3, 4, 1, 6, 8, 9, 10, 7];

fn table() -> AuTable {
    let deltas: Vec<i64> = PTS.windows(2).map(|pair| pair[1] - pair[0]).collect();
    serde_json::from_value(json!({
        "analyzer_version": "0.8.0",
        "sidecar_sha256": null,
        "source": {"file_name": "clip.mkv", "size_bytes": 1000,
                   "head_sha256": DIGEST, "tail_sha256": DIGEST},
        "track": {"container": "matroska", "codec": "hevc", "width": 256, "height": 144,
                  "codec_private_sha256": DIGEST, "time_base": [1, 1000]},
        "aus": {"count": 11, "pts_first": 0, "pts_delta": deltas, "size": vec![100; 11],
                "non_picture": [5], "irap_crc32": [[0, 7], [6, 7]]},
        "leading_skipped": 0,
        "scene": {},
    }))
    .unwrap()
}

/// Shots at pictures 0 and 5 of ten.
fn shots() -> Vec<ManifestShot> {
    vec![
        ManifestShot {
            start: 0,
            l1: L1::new(0, 1300, 2600),
        },
        ManifestShot {
            start: 5,
            l1: L1::new(10, 1800, 3200),
        },
    ]
}

/// CM v4.0 with every field off its default: trims, L9, L11, L5 and L6.
fn custom_81() -> RpuParams {
    RpuParams {
        profile: Profile::P81 {
            source_range: Some((7, 3696)),
        },
        cm: CmMetadata::V40 {
            config: CmV40Config {
                source_primary_index: 0,
                content_type: 3,
                reference_mode: true,
            },
            trim_targets_nits: vec![100, 400],
        },
        level6: Level6 {
            max_display_mastering_luminance: 4000,
            min_display_mastering_luminance: 50,
            max_content_light_level: 1500,
            max_frame_average_light_level: 300,
        },
        level5: Some(Level5 {
            left: 0,
            right: 0,
            top: 16,
            bottom: 24,
        }),
        l1_avg_pq_cm_version: IMPORT_L1_AVG_CM_VERSION,
    }
}

fn v29_81() -> RpuParams {
    RpuParams {
        cm: CmMetadata::V29,
        level5: None,
        ..custom_81()
    }
}

fn hlg(composer: Composer) -> RpuParams {
    RpuParams {
        profile: Profile::P84 {
            composer: composer.into(),
        },
        cm: CmMetadata::mkvdovi_default(),
        level6: Level6::mkvdovi_default(),
        level5: None,
        l1_avg_pq_cm_version: IMPORT_L1_AVG_CM_VERSION,
    }
}

/// One RPU NAL per picture, as a conversion with these parameters and shots writes them.
fn nals(params: &RpuParams, shots: &[ManifestShot], pictures: u64) -> Vec<Vec<u8>> {
    let builder = RpuBuilder::new(params).unwrap();
    let mut nals = Vec::new();
    for (index, shot) in shots.iter().enumerate() {
        let end = shots.get(index + 1).map_or(pictures, |next| next.start);
        let mut built = builder.shot(shot.l1).unwrap();
        for picture in shot.start..end {
            nals.push(built.write(picture == shot.start).unwrap());
        }
    }
    nals
}

/// An `RPU.bin` as `dovi_tool` writes it: each RPU behind a four-byte start code, without the
/// HEVC NAL header.
fn file(nals: &[Vec<u8>]) -> Vec<u8> {
    nals.iter()
        .flat_map(|nal| {
            START_CODE
                .iter()
                .chain(&nal[RPU_NAL_HEADER.len()..])
                .copied()
        })
        .collect()
}

/// The same RPUs with their NAL headers, which the parser also reads.
fn file_with_nal_headers(nals: &[Vec<u8>]) -> Vec<u8> {
    nals.iter()
        .flat_map(|nal| START_CODE.iter().chain(nal).copied())
        .collect()
}

/// `nal` with its DM data edited, written back with a fresh CRC.
fn edited(nal: &[u8], edit: impl FnOnce(&mut VdrDmData)) -> Vec<u8> {
    let mut rpu = DoviRpu::parse_unspec62_nalu(nal).unwrap();
    edit(rpu.vdr_dm_data.as_mut().unwrap());
    rpu.modified = true;
    rpu.write_hevc_unspec62_nalu().unwrap()
}

/// The import's verdict on `bytes`: parameters and shots from the file, then the rebuild check.
fn import(bytes: &[u8], hlg_composer: Option<Composer>) -> Result<RpuParams, String> {
    let rpus = parse_source_rpu_bytes(bytes).map_err(|error| format!("{error:#}"))?;
    let params = params_from_rpu(&rpus[0].rpu, hlg_composer)?;
    let shots = shots_from_rpus(&rpus)?;
    check_reproduced(&params, &shots, &rpus, &table(), hlg_composer)?;
    Ok(params)
}

fn refusal(bytes: &[u8], hlg_composer: Option<Composer>) -> String {
    import(bytes, hlg_composer).unwrap_err()
}

#[test]
fn the_presentation_order_maps_to_decode_indices_past_a_non_picture() {
    let table = table();
    assert_eq!(table.aus.picture_count(), 10);
    assert_eq!(
        table.aus.decode_index_by_presentation(),
        DECODE_BY_PRESENTATION
    );
}

#[test]
fn every_parameter_is_read_from_the_rpu() {
    for (params, composer) in [
        (custom_81(), None),
        (v29_81(), None),
        (hlg(Composer::Bt2100V1), Some(Composer::Bt2100V1)),
        (hlg(Composer::Preset), Some(Composer::Preset)),
    ] {
        let bytes = file(&nals(&params, &shots(), 10));
        assert_eq!(import(&bytes, composer), Ok(params));
    }
}

#[test]
fn the_shots_are_the_scene_refresh_pictures_with_their_l1() {
    let bytes = file(&nals(&custom_81(), &shots(), 10));
    let rpus = parse_source_rpu_bytes(&bytes).unwrap();
    assert_eq!(shots_from_rpus(&rpus).unwrap(), shots());
    // The bytes are the builder's, without the start code and the NAL header.
    assert_eq!(rpus[3].data, &nals(&custom_81(), &shots(), 10)[3][2..]);
}

#[test]
fn a_file_with_nal_headers_is_read_too() {
    let nals = nals(&custom_81(), &shots(), 10);
    assert_eq!(import(&file_with_nal_headers(&nals), None), Ok(custom_81()));
    let mut edited_nals = nals.clone();
    edited_nals[7] = edited(&nals[7], |dm| dm.source_max_pq = 3500);
    let error = refusal(&file_with_nal_headers(&edited_nals), None);
    assert!(
        error.contains("picture 7 (AU 9 in decode order)"),
        "{error}"
    );
}

#[test]
fn a_zero_l5_reads_as_none_and_rebuilds_the_same_bytes() {
    let zero = RpuParams {
        level5: Some(Level5::default()),
        ..custom_81()
    };
    let bytes = file(&nals(&zero, &shots(), 10));
    assert_eq!(
        import(&bytes, None).unwrap().level5,
        None,
        "a zero L5 is the builder's None"
    );
}

#[test]
fn an_rpu_clamped_under_the_v40_average_floor_is_reproduced() {
    let v40_floor = RpuParams {
        l1_avg_pq_cm_version: CmVersion::V40,
        ..custom_81()
    };
    // An average below the V40 floor (1229): the generator writes the floor.
    let shots = vec![ManifestShot {
        start: 0,
        l1: L1::new(0, 900, 2600),
    }];
    let bytes = file(&nals(&v40_floor, &shots, 10));
    assert!(import(&bytes, None).is_ok());
}

#[test]
fn trim_targets_read_back_as_the_roundest_whole_nits() {
    for nits in [100, 400, 600, 1000, 4000] {
        let code = u16::try_from(nits_to_pq_code(nits)).unwrap();
        assert_eq!(trim_target_nits(code), Some(nits));
    }
    let code = u16::try_from(nits_to_pq_code(123)).unwrap();
    let read = trim_target_nits(code).unwrap();
    assert_eq!(nits_to_pq_code(read), u32::from(code));
    // Codes between 1 and 2 nits that no whole number reaches.
    let gap = nits_to_pq_code(1) + 1;
    assert!(nits_to_pq_code(2) > gap);
    assert_eq!(trim_target_nits(u16::try_from(gap).unwrap()), None);
}

#[test]
fn a_wrong_composer_is_refused_at_picture_0_naming_both() {
    let bytes = file(&nals(&hlg(Composer::Bt2100V1), &shots(), 10));
    let error = refusal(&bytes, Some(Composer::Preset));
    assert!(
        error.contains("picture 0 (AU 0 in decode order)")
            && error.contains("carries the bt2100 HLG composer, and --hlg-composer is preset"),
        "{error}"
    );
    // A Profile 8.4 file for a PQ input differs too.
    let error = refusal(&bytes, None);
    assert!(
        error.contains("picture 0 (AU 0 in decode order)"),
        "{error}"
    );
}

#[test]
fn a_wrong_count_is_refused_naming_the_first_unmatched_picture() {
    let mut short = nals(&custom_81(), &shots(), 10);
    short.pop();
    let error = refusal(&file(&short), None);
    assert!(
        error
            .contains("it has 9 RPUs for 10 pictures: picture 9 (AU 7 in decode order) has no RPU"),
        "{error}"
    );
    let mut long = nals(&custom_81(), &shots(), 10);
    long.push(long[9].clone());
    let error = refusal(&file(&long), None);
    assert!(
        error.contains("it has 11 RPUs for 10 pictures: RPU 10 has no picture"),
        "{error}"
    );
}

#[test]
fn per_frame_trims_and_l1_are_refused_at_their_picture() {
    let good = nals(&custom_81(), &shots(), 10);
    // Picture 7 is inside the second shot (5..10); its AU is 9 in decode order.
    let mut trims = good.clone();
    trims[7] = edited(&trims[7], |dm| {
        if let Some(ExtMetadataBlock::Level2(level2)) = dm.get_block_mut(2) {
            level2.trim_slope = 2100;
        }
    });
    let error = refusal(&file(&trims), None);
    assert!(
        error.contains("picture 7 (AU 9 in decode order)") && error.contains("L2 (per-frame"),
        "{error}"
    );
    // Picture 3 is inside the first shot; its AU is 4.
    let mut l1 = good.clone();
    l1[3] = edited(&l1[3], |dm| {
        dm.replace_metadata_block(ExtMetadataBlock::Level1(ExtMetadataBlockLevel1::new(
            0, 2700, 1300,
        )))
        .unwrap();
    });
    let error = refusal(&file(&l1), None);
    assert!(
        error.contains("picture 3 (AU 4 in decode order)") && error.contains("L1 (per-frame"),
        "{error}"
    );
    assert!(import(&file(&good), None).is_ok());
}

#[test]
fn a_change_outside_l1_and_l2_is_refused_at_its_picture() {
    let mut nals = nals(&custom_81(), &shots(), 10);
    // Picture 8 (AU 10): another MaxCLL.
    nals[8] = edited(&nals[8], |dm| {
        dm.replace_metadata_block(ExtMetadataBlock::Level6(ExtMetadataBlockLevel6 {
            max_content_light_level: 1400,
            ..Default::default()
        }))
        .unwrap();
    });
    let error = refusal(&file(&nals), None);
    assert!(
        error.contains("picture 8 (AU 10 in decode order)") && error.contains("it differs in L6"),
        "{error}"
    );
    // The source range is a DM field outside the blocks.
    let mut range = self::nals(&custom_81(), &shots(), 10);
    range[6] = edited(&range[6], |dm| dm.source_max_pq = 3500);
    let error = refusal(&file(&range), None);
    assert!(
        error.contains("picture 6 (AU 8 in decode order)")
            && error.contains("DM fields outside the extension blocks"),
        "{error}"
    );
}

#[test]
fn picture_0_must_start_a_scene() {
    let mut nals = nals(&custom_81(), &shots(), 10);
    nals[0] = edited(&nals[0], |dm| dm.set_scene_cut(false));
    let error = refusal(&file(&nals), None);
    assert!(
        error.contains("picture 0 (AU 0 in decode order)")
            && error.contains("scene_refresh_flag is 0"),
        "{error}"
    );
}

#[test]
fn missing_levels_are_refused_by_name() {
    let mut nals = nals(&custom_81(), &shots(), 10);
    let without = |level: u8| {
        move |dm: &mut VdrDmData| {
            dm.remove_metadata_level(level);
        }
    };
    nals[0] = edited(&nals[0], without(6));
    assert_eq!(refusal(&file(&nals), None), "picture 0's RPU carries no L6");
    let mut nals = self::nals(&custom_81(), &shots(), 10);
    nals[0] = edited(&nals[0], without(11));
    assert_eq!(
        refusal(&file(&nals), None),
        "picture 0's CM v4.0 RPU carries no L11"
    );
}

#[test]
fn the_file_is_parsed_strictly_and_behind_the_guard() {
    let good = file(&nals(&custom_81(), &shots(), 10));
    let mut leading = vec![7];
    leading.extend(&good);
    assert!(refusal(&leading, None).contains("does not start with an Annex B start code"));
    assert!(refusal(&[], None).contains("does not start with an Annex B start code"));
    // A broken RPU in the middle is named, not skipped as `parse_rpu_file` would.
    let mut broken = nals(&custom_81(), &shots(), 10);
    let last = broken[4].len() - 2;
    broken[4][last] ^= 0xff;
    assert!(
        refusal(&file(&broken), None).contains("RPU 4 does not parse"),
        "{}",
        refusal(&file(&broken), None)
    );
    // The abort fixture of E15.1 is refused by the guard, before the crate's parser sees it.
    let fixture = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/linear_interp_RPU.bin"),
    )
    .unwrap();
    let error = parse_source_rpu_bytes(&fixture).unwrap_err();
    assert!(is_unparseable(&error), "{error:#}");
}
