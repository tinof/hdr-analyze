//! `mkvdovi manifest --rpu` (ROADMAP P15.4, docs/PLAYBACK_MANIFEST.md) end to end. Each source
//! is converted with `--keep-source`, the conversion's `RPU.bin` is extracted with `dovi_tool
//! extract-rpu`, and the manifest is made from that file without a measurement. A good file is
//! accepted, and the player's path over the manifest rebuilds every RPU of the conversion byte
//! for byte; its parameters equal those of a measured manifest of the same source, so they come
//! from the file and not from defaults. A wrong composer, a wrong count, per-frame trims or L1,
//! and a change in another level are refused, naming the picture and the AU (decode index, taken
//! here from ffprobe's packet order) that differ.
//!
//! Skipped (with a message) like `playback_manifest.rs`: ffmpeg with libx265, ffprobe, mkvmerge,
//! mediainfo, dovi_tool and an `hdr_analyzer_mvp` that writes the AU table, built next to the
//! test's mkvdovi binary.

#[macro_use]
#[path = "support/playback.rs"]
mod playback;

use std::path::{Path, PathBuf};
use std::process::Command;

use dolby_vision::rpu::dovi_rpu::DoviRpu;
use dolby_vision::rpu::extension_metadata::blocks::{
    ExtMetadataBlock, ExtMetadataBlockLevel1, ExtMetadataBlockLevel6,
};
use dolby_vision::rpu::vdr_dm_data::VdrDmData;
use dovi_rpu_builder::manifest::{L1Policy, PlaybackManifest, QualityClass};
use dovi_rpu_builder::{HlgComposer, Profile, RpuParams};
use serde_json::Value;

use self::playback::*;

const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// `dovi_tool extract-rpu` of the converted file's video: the RPU.bin a user would import.
fn extract_rpu(converted: &Path, rpu: &Path) {
    let annexb = converted.with_extension("rpu-extract.hevc");
    run(Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(converted)
        .args([
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-bsf:v",
            "hevc_mp4toannexb",
        ])
        .args(["-f", "hevc", "-y"])
        .arg(&annexb));
    run(Command::new("dovi_tool")
        .args(["extract-rpu", "-i"])
        .arg(&annexb)
        .arg("-o")
        .arg(rpu));
    std::fs::remove_file(&annexb).unwrap();
}

/// The RPUs of an `RPU.bin` (start codes removed), and a file made of such RPUs.
fn rpu_nals(rpu: &Path) -> Vec<Vec<u8>> {
    nal_units(&std::fs::read(rpu).unwrap())
        .into_iter()
        .map(<[u8]>::to_vec)
        .collect()
}

fn write_rpu_file(path: &Path, nals: &[Vec<u8>]) {
    let bytes: Vec<u8> = nals
        .iter()
        .flat_map(|nal| START_CODE.iter().chain(nal).copied())
        .collect();
    std::fs::write(path, bytes).unwrap();
}

/// An RPU of the file with its DM data edited, written back with a fresh CRC and, as
/// `dovi_tool` writes an `RPU.bin`, without the HEVC NAL header.
fn edited(data: &[u8], edit: impl FnOnce(&mut VdrDmData)) -> Vec<u8> {
    assert_eq!(data[0], 0x19, "dovi_tool writes no NAL header");
    let mut rpu = DoviRpu::parse_unspec62_nalu(data).unwrap();
    edit(rpu.vdr_dm_data.as_mut().unwrap());
    rpu.modified = true;
    rpu.write_hevc_unspec62_nalu().unwrap()[2..].to_vec()
}

/// The decode index of each picture of `source` by presentation index (pts rank), from
/// ffprobe's packets. Every packet of these x265 clips is a picture.
fn decode_index_by_presentation(source: &Path) -> Vec<usize> {
    let packets = packets(source);
    let mut indices: Vec<usize> = (0..packets.len()).collect();
    indices.sort_by_key(|&index| packets[index].0);
    indices
}

/// `mkvdovi manifest clip.mkv --rpu <rpu>` into its own work directory `<dir>/<work>`.
fn import(dir: &Path, work: &str, rpu: &Path, flags: &[&str]) -> (bool, String, PathBuf) {
    let work = dir.join(work);
    let output = mkvdovi_cmd()
        .current_dir(dir)
        .args(["manifest", SOURCE, "--work-dir"])
        .arg(&work)
        .arg("--rpu")
        .arg(rpu)
        .args(["--hwaccel", "none"])
        .args(flags)
        .output()
        .unwrap();
    (output.status.success(), log_of(&output), work)
}

/// An accepted import: the manifest, checked for the `rpu-bin` provenance and for writing no
/// measurement, and the work directory's files.
fn accepted(dir: &Path, work: &str, rpu: &Path, flags: &[&str]) -> (PlaybackManifest, Value) {
    let before = std::fs::read(rpu).unwrap();
    let (ok, log, work) = import(dir, work, rpu, flags);
    assert!(ok, "the import was refused:\n{log}");
    assert_eq!(std::fs::read(rpu).unwrap(), before, "the RPU file changed");
    let mut files: Vec<String> = std::fs::read_dir(&work)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        [
            "clip.dvmanifest.json",
            "clip.manifest-analyzer.log",
            "clip.rpu-import.au.json"
        ],
        "an import measures nothing"
    );
    let text = std::fs::read_to_string(work.join(MANIFEST)).unwrap();
    let manifest = PlaybackManifest::parse(&text).unwrap();
    let json: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(manifest.l1_policy, L1Policy::RpuBin);
    assert_eq!(json["l1_policy"], "rpu-bin");
    assert_eq!(manifest.coverage.len(), 1);
    assert_eq!(manifest.coverage[0].class, QualityClass::Approximate);
    assert_eq!(
        manifest.producer.rpu_bin_sha256.as_deref(),
        Some(sha256(&before).as_str())
    );
    assert_eq!(json["producer"]["sidecar_sha256"], Value::Null);
    assert_eq!(json["producer"]["sidecar_version"], Value::Null);
    (manifest, json)
}

fn refused(dir: &Path, rpu: &Path, flags: &[&str], expected: &[&str]) {
    let (ok, log, work) = import(dir, "refused", rpu, flags);
    assert!(!ok, "the import must be refused:\n{log}");
    for needle in expected {
        assert!(log.contains(needle), "'{needle}' not in:\n{log}");
    }
    assert!(
        !work.join(MANIFEST).exists(),
        "a refused import wrote a manifest"
    );
    std::fs::remove_dir_all(work).ok();
}

/// A measured manifest of the same source in `<dir>/measured`: the independent source of the
/// parameters an import must read.
fn measured_params(dir: &Path, flags: &[&str]) -> RpuParams {
    let work = dir.join("measured");
    let output = mkvdovi_cmd()
        .current_dir(dir)
        .args(["manifest", SOURCE, "--work-dir"])
        .arg(&work)
        .args(["--hwaccel", "none"])
        .args(flags)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", log_of(&output));
    let text = std::fs::read_to_string(work.join(MANIFEST)).unwrap();
    std::fs::remove_dir_all(&work).unwrap();
    PlaybackManifest::parse(&text).unwrap().rpu
}

/// The parameters an import reads equal a measured manifest's. An 8.1 import always states the
/// source range the RPU carries; the measured manifest may leave it to the L6 lookup, which
/// writes the same codes.
fn assert_same_params(imported: &RpuParams, measured: &RpuParams) {
    let without_range = |params: &RpuParams| RpuParams {
        profile: match params.profile {
            Profile::P81 { .. } => Profile::P81 { source_range: None },
            other => other,
        },
        ..params.clone()
    };
    assert_eq!(without_range(imported), without_range(measured));
}

/// Converts `<dir>/clip.mkv` with `flags`, extracts its RPU.bin, imports it with `import_flags`
/// and checks the import against the conversion and against a measured manifest.
fn check_import(dir: &Path, source: &Path, flags: &[&str], import_flags: &[&str]) -> PathBuf {
    let converted = convert(dir, flags);
    let rpu = dir.join("RPU.bin");
    extract_rpu(&converted, &rpu);
    let (manifest, _) = accepted(dir, "work", &rpu, import_flags);
    assert_rpus_equal_the_conversion(&manifest, source, &converted);
    assert_same_params(&manifest.rpu, &measured_params(dir, flags));
    rpu
}

/// ROADMAP P15.5 on the `--au-table-only` path: the FFmpeg-cut open-GOP file is refused with the
/// remedy and writes no AU table, and the conversion's `RPU.bin` imports on its mkvmerge remux.
#[test]
fn an_ffmpeg_open_gop_cut_is_refused_on_import_and_its_remux_imports() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let cut = ffmpeg_open_gop_cut(dir.path());
    let converted = convert(dir.path(), &[]);
    let rpu = dir.path().join("RPU.bin");
    extract_rpu(&converted, &rpu);

    let (ok, log, work) = import(dir.path(), "work", &rpu, &[]);
    assert!(!ok, "the cut must be refused:\n{log}");
    assert!(log.contains("video packet 1 has no pts"), "{log}");
    assert!(
        log.contains("Remux the file with `mkvmerge -o <new>.mkv <input>`"),
        "{log}"
    );
    assert!(!work.join(MANIFEST).exists(), "a refusal wrote a manifest");
    assert!(!work.join("clip.rpu-import.au.json").exists());

    let remux_dir = dir.path().join("remux");
    let remux = mkvmerge_remux(&cut, &remux_dir);
    let (manifest, _) = accepted(&remux_dir, "work", &rpu, &[]);
    assert_eq!(manifest.leading_skipped, 1);
    assert_rpus_equal_the_conversion(&manifest, &remux, &converted);
}

#[test]
fn a_conversion_rpu_with_b_frames_is_imported_and_its_edits_are_refused() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "pq",
        STEP,
        48,
        &format!("keyint=24:min-keyint=24:bframes=3:b-adapt=0:{PQ_TAGS}:{MASTERING}"),
    );
    let source = mux(dir.path(), SOURCE, &raw, &[]);
    let rpu = check_import(dir.path(), &source, &[], &[]);
    let (manifest, _) = accepted(dir.path(), "work", &rpu, &[]);
    assert!(matches!(manifest.rpu.profile, Profile::P81 { .. }));
    assert_eq!(manifest.luminance_mapping().unwrap(), "pq");
    assert!(manifest.shots.len() >= 2, "{:?}", manifest.shots);

    let decode = decode_index_by_presentation(&source);
    assert!(
        decode.iter().enumerate().any(|(p, &k)| p != k),
        "decode order differs from presentation order"
    );
    let picture_au =
        |picture: usize| format!("picture {picture} (AU {} in decode order)", decode[picture]);
    let good = rpu_nals(&rpu);
    assert_eq!(good.len(), 48);
    let second = manifest.shots[1].start as usize;
    let inside = second + 3;
    assert!(manifest
        .shots
        .iter()
        .all(|shot| shot.start as usize != inside));
    let edited_file = dir.path().join("edited.bin");

    // A wrong count: one RPU missing at the end, or one too many.
    write_rpu_file(&edited_file, &good[..47]);
    refused(
        dir.path(),
        &edited_file,
        &[],
        &[&format!(
            "it has 47 RPUs for 48 pictures: {} has no RPU",
            picture_au(47)
        )],
    );
    let mut long = good.clone();
    long.push(good[47].clone());
    write_rpu_file(&edited_file, &long);
    refused(
        dir.path(),
        &edited_file,
        &[],
        &["it has 49 RPUs for 48 pictures: RPU 48 has no picture"],
    );

    // Per-frame trims and per-frame L1 inside a shot.
    let mut trims = good.clone();
    trims[inside] = edited(&trims[inside], |dm| {
        if let Some(ExtMetadataBlock::Level2(level2)) = dm.get_block_mut(2) {
            level2.trim_slope = 2200;
        }
    });
    write_rpu_file(&edited_file, &trims);
    refused(
        dir.path(),
        &edited_file,
        &[],
        &[&picture_au(inside), "L2 (per-frame or authored trims)"],
    );
    let mut l1 = good.clone();
    l1[inside] = edited(&l1[inside], |dm| {
        dm.replace_metadata_block(ExtMetadataBlock::Level1(ExtMetadataBlockLevel1::new(
            0, 2900, 1500,
        )))
        .unwrap();
    });
    write_rpu_file(&edited_file, &l1);
    refused(
        dir.path(),
        &edited_file,
        &[],
        &[&picture_au(inside), "L1 (per-frame L1 inside a shot)"],
    );

    // Any other byte: another MaxCLL on a late picture.
    let mut l6 = good.clone();
    l6[40] = edited(&l6[40], |dm| {
        let Some(ExtMetadataBlock::Level6(level6)) = dm.get_block(6).cloned() else {
            panic!("a conversion RPU carries L6");
        };
        dm.replace_metadata_block(ExtMetadataBlock::Level6(ExtMetadataBlockLevel6 {
            max_content_light_level: level6.max_content_light_level - 1,
            ..level6
        }))
        .unwrap();
    });
    write_rpu_file(&edited_file, &l6);
    refused(
        dir.path(),
        &edited_file,
        &[],
        &[&picture_au(40), "it differs in L6"],
    );

    // The flags the file states cannot be given beside it.
    refused(
        dir.path(),
        &rpu,
        &["--cm-version", "v29"],
        &["cannot be used with"],
    );
    // The E15.1 abort fixture is refused by the guard; the process does not abort.
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/linear_interp_RPU.bin");
    refused(
        dir.path(),
        &fixture,
        &[],
        &["cannot be imported", "linear interpolation"],
    );
    // The file is never one this run writes.
    let (ok, log, _) = import(dir.path(), "alias", &rpu, &["--out", rpu.to_str().unwrap()]);
    assert!(!ok && log.contains("is the --rpu file"), "{log}");
}

#[test]
fn every_parameter_comes_from_the_rpu_file() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "box",
        LETTERBOX,
        48,
        &format!(
            "keyint=24:min-keyint=24:bframes=2:b-adapt=0:{PQ_TAGS}:\
             master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)"
        ),
    );
    let source = mux(dir.path(), SOURCE, &raw, &[]);
    for flags in [
        &["--cm-version", "v29"][..],
        &[
            "--trim-targets",
            "100,400",
            "--content-type",
            "game",
            "--source-primaries",
            "0",
            "--reference-mode",
            "true",
        ],
    ] {
        // Imported without the flags: the file states them.
        let rpu = check_import(dir.path(), &source, flags, &[]);
        let (manifest, json) = accepted(dir.path(), "work", &rpu, &[]);
        assert!(manifest.rpu.level5.is_some(), "the letterbox gives L5");
        let cm = &json["rpu"]["cm"];
        if flags[0] == "--cm-version" {
            assert_eq!(cm["version"], "V29");
        } else {
            assert_eq!(cm["trim_targets_nits"], serde_json::json!([100, 400]));
            assert_eq!(cm["content_type"], 2);
            assert_eq!(cm["source_primary_index"], 0);
            assert_eq!(cm["reference_mode"], true);
        }
        std::fs::remove_dir_all(dir.path().join("conversion")).unwrap();
        std::fs::remove_dir_all(dir.path().join("work")).unwrap();
    }
}

#[test]
fn an_hlg_rpu_is_imported_with_its_composer_and_refused_under_another() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let raw = encode(
        dir.path(),
        "hlg",
        HLG,
        48,
        &format!("keyint=24:min-keyint=24:bframes=2:b-adapt=0:{HLG_TAGS}"),
    );
    let source = mux(dir.path(), SOURCE, &raw, &[]);

    // The default composer, bt2100.
    let rpu = check_import(dir.path(), &source, &[], &[]);
    let (manifest, _) = accepted(dir.path(), "work", &rpu, &[]);
    assert_eq!(
        manifest.rpu.profile,
        Profile::P84 {
            composer: HlgComposer::Bt2100
        }
    );
    refused(
        dir.path(),
        &rpu,
        &["--hlg-composer", "preset"],
        &[
            "picture 0 (AU 0 in decode order)",
            "carries the bt2100 HLG composer, and --hlg-composer is preset",
        ],
    );
    std::fs::remove_dir_all(dir.path().join("conversion")).unwrap();
    std::fs::remove_dir_all(dir.path().join("work")).unwrap();

    // A preset conversion: accepted when named, refused under the default.
    let rpu = check_import(
        dir.path(),
        &source,
        &["--hlg-composer", "preset"],
        &["--hlg-composer", "preset"],
    );
    refused(
        dir.path(),
        &rpu,
        &[],
        &[
            "picture 0 (AU 0 in decode order)",
            "carries the preset HLG composer, and --hlg-composer is bt2100",
        ],
    );
}
