//! `mkvdovi manifest --rpu`: the playback manifest of an imported `RPU.bin` (ROADMAP P15.4,
//! `docs/PLAYBACK_MANIFEST.md`). The shots and the RPU parameters are read from the file itself:
//! the parameters from its first picture's RPU, a shot at every picture whose RPU sets
//! `scene_refresh_flag`, with that RPU's L1. The analyzer writes only the AU table. The file is
//! accepted only when the builder, from these parameters and shots, rebuilds every picture's RPU
//! byte for byte; otherwise the first picture that differs, and its AU, are named.
//!
//! The file is read once: the guarded parse (`rpu_guard`, E15.1/E15.2), the bytes compared and
//! the digest in the manifest all come from the same buffer.

#[cfg(test)]
mod tests;

use std::path::Path;
use std::process::Command;

use anyhow::Result;
use dolby_vision::rpu::dovi_rpu::DoviRpu;
use dolby_vision::rpu::extension_metadata::blocks::ExtMetadataBlock;
use dolby_vision::rpu::vdr_dm_data::VdrDmData;
use dovi84_composer::Composer;
use dovi_rpu_builder::manifest::ManifestShot;
use dovi_rpu_builder::{
    nits_to_pq_code, CmMetadata, CmV40Config, CmVersion, Level5, Level6, Profile, RpuBuilder,
    RpuParams, L1, RPU_NAL_HEADER,
};

use super::au_table::AuTable;
use super::build;
use super::identity::{sha256_hex, FileIdentity};
use super::{write_manifest, Paths};
use crate::cli::{Args, HwAccel};
use crate::external;
use crate::pipeline::analyzer::analyzer_executable;
use crate::progress;
use crate::rpu_guard::{parse_source_rpu_bytes, SourceRpu};

/// The CM version whose L1 average floor the rebuild clamps with: V29's 819 is the lower one,
/// and clamping is the identity on what the generator already clamped, so it reproduces both
/// generator settings.
const IMPORT_L1_AVG_CM_VERSION: CmVersion = CmVersion::V29;

/// What one import needs from `manifest_file`.
pub(super) struct Request<'a> {
    pub(super) input_file: &'a str,
    pub(super) rpu_file: &'a Path,
    pub(super) args: &'a Args,
    /// `Some` for HLG: the composer the file must carry.
    pub(super) hlg_composer: Option<Composer>,
    pub(super) paths: &'a Paths,
    pub(super) identity: FileIdentity,
}

/// Runs the import. `Ok(false)`: refused, with the reason printed.
pub(super) fn manifest_from_rpu(request: Request<'_>) -> Result<bool> {
    let rpu_file = request.rpu_file;
    let bytes = match std::fs::read(rpu_file) {
        Ok(bytes) => bytes,
        Err(error) => {
            progress::print_error(&format!(
                "Cannot read the RPU file {}: {error}",
                rpu_file.display()
            ));
            return Ok(false);
        }
    };
    let rpus = match parse_source_rpu_bytes(&bytes) {
        Ok(rpus) => rpus,
        Err(error) => {
            progress::print_error(&format!(
                "The RPU file {} cannot be imported: {error:#}",
                rpu_file.display()
            ));
            return Ok(false);
        }
    };
    let derived = params_from_rpu(&rpus[0].rpu, request.hlg_composer)
        .and_then(|params| Ok((params, shots_from_rpus(&rpus)?)));
    let (params, shots) = match derived {
        Ok(derived) => derived,
        Err(reason) => {
            progress::print_error(&format!(
                "The RPU file {} cannot be imported: {reason}.",
                rpu_file.display()
            ));
            return Ok(false);
        }
    };

    let Some(table) = au_table(&request)? else {
        return Ok(false);
    };
    let input = Path::new(request.input_file);
    if let Err(error) = check_reproduced(&params, &shots, &rpus, &table, request.hlg_composer) {
        progress::print_error(&format!(
            "The RPU file {} is not reproducible from a manifest, so it is not imported: {error}.",
            rpu_file.display()
        ));
        return Ok(false);
    }
    let manifest = build::imported(build::Imported {
        file_name: input
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        identity: request.identity,
        table: &table,
        rpu_bin_sha256: sha256_hex(&bytes),
        rpu: params,
        shots,
        hlg_composer: request.hlg_composer,
    })?;
    write_manifest(&manifest, request.paths)?;
    progress::print_info(&format!(
        "Manifest written from {}: {} ({} shots over {} pictures, every RPU reproduced; coverage approximate, lookup key {}).",
        rpu_file.display(),
        request.paths.out.display(),
        manifest.shots.len(),
        manifest.aus.picture_count(),
        manifest.source.lookup_key
    ));
    Ok(true)
}

/// The RPU parameters the file's first RPU states. The profile comes from the input (HLG: 8.4
/// with the composer the file must carry); everything else from the RPU. Only the parameters
/// are read here: [`check_reproduced`] proves them on every picture.
pub(super) fn params_from_rpu(
    rpu: &DoviRpu,
    hlg_composer: Option<Composer>,
) -> Result<RpuParams, String> {
    let dm = dm_data(rpu, 0)?;
    let profile = match hlg_composer {
        Some(composer) => Profile::P84 {
            composer: composer.into(),
        },
        None => Profile::P81 {
            source_range: Some((dm.source_min_pq, dm.source_max_pq)),
        },
    };
    let cm = if dm.cmv40_metadata.is_some() {
        let Some(ExtMetadataBlock::Level9(level9)) = dm.get_block(9) else {
            return Err("picture 0's CM v4.0 RPU carries no L9".into());
        };
        let Some(ExtMetadataBlock::Level11(level11)) = dm.get_block(11) else {
            return Err("picture 0's CM v4.0 RPU carries no L11".into());
        };
        let trim_targets_nits = dm
            .level_blocks_iter(2)
            .map(|block| match block {
                ExtMetadataBlock::Level2(level2) => trim_target_nits(level2.target_max_pq)
                    .ok_or_else(|| {
                        format!(
                            "picture 0's L2 target_max_pq {} is no whole-nit target",
                            level2.target_max_pq
                        )
                    }),
                _ => Err("picture 0 has an L2 entry that is not an L2 block".into()),
            })
            .collect::<Result<Vec<u32>, String>>()?;
        CmMetadata::V40 {
            config: CmV40Config {
                source_primary_index: level9.source_primary_index,
                content_type: level11.content_type,
                reference_mode: level11.reference_mode_flag,
            },
            trim_targets_nits,
        }
    } else {
        CmMetadata::V29
    };
    let Some(ExtMetadataBlock::Level6(level6)) = dm.get_block(6) else {
        return Err("picture 0's RPU carries no L6".into());
    };
    let level5 = match dm.get_block(5) {
        Some(ExtMetadataBlock::Level5(level5)) => {
            let offsets = Level5 {
                left: level5.active_area_left_offset,
                right: level5.active_area_right_offset,
                top: level5.active_area_top_offset,
                bottom: level5.active_area_bottom_offset,
            };
            // The builder writes a zero L5 for `None`, as `dovi_tool generate` does.
            (offsets != Level5::default()).then_some(offsets)
        }
        _ => None,
    };
    Ok(RpuParams {
        profile,
        cm,
        level6: Level6 {
            max_display_mastering_luminance: u32::from(level6.max_display_mastering_luminance),
            min_display_mastering_luminance: u32::from(level6.min_display_mastering_luminance),
            max_content_light_level: u32::from(level6.max_content_light_level),
            max_frame_average_light_level: u32::from(level6.max_frame_average_light_level),
        },
        level5,
        l1_avg_pq_cm_version: IMPORT_L1_AVG_CM_VERSION,
    })
}

/// The whole-nit trim target whose PQ code is `target_max_pq` (`nits_to_pq_code`, as mkvdovi
/// writes the L2 targets). Several targets can share one code; the roundest is taken (most
/// trailing decimal zeros, then the smallest), so a 400-nit target reads back as 400.
pub(super) fn trim_target_nits(target_max_pq: u16) -> Option<u32> {
    let roundness = |mut nits: u32| {
        let mut zeros = 0;
        while nits % 10 == 0 {
            nits /= 10;
            zeros += 1;
        }
        zeros
    };
    (1..=10_000)
        .filter(|&nits| nits_to_pq_code(nits) == u32::from(target_max_pq))
        .max_by_key(|&nits| (roundness(nits), std::cmp::Reverse(nits)))
}

/// A shot at picture 0 and at every later picture whose RPU sets `scene_refresh_flag`, with that
/// RPU's L1, as `dovi_tool generate` lays out a shot list.
pub(super) fn shots_from_rpus(rpus: &[SourceRpu<'_>]) -> Result<Vec<ManifestShot>, String> {
    let mut shots = Vec::new();
    for (picture, source) in rpus.iter().enumerate() {
        let dm = dm_data(&source.rpu, picture)?;
        if picture == 0 || dm.scene_refresh_flag != 0 {
            let Some(ExtMetadataBlock::Level1(level1)) = dm.get_block(1) else {
                return Err(format!(
                    "picture {picture} starts a shot but its RPU carries no L1"
                ));
            };
            shots.push(ManifestShot {
                start: picture as u64,
                l1: L1::new(level1.min_pq, level1.avg_pq, level1.max_pq),
            });
        }
    }
    Ok(shots)
}

fn dm_data(rpu: &DoviRpu, picture: usize) -> Result<&VdrDmData, String> {
    rpu.vdr_dm_data
        .as_ref()
        .ok_or_else(|| format!("picture {picture}'s RPU carries no DM data"))
}

/// The AU table of the input, written afresh by `--au-table-only` (no measurement). `None`:
/// refused, printed.
fn au_table(request: &Request<'_>) -> Result<Option<AuTable>> {
    let paths = request.paths;
    let mut command = Command::new(analyzer_executable());
    command
        .arg(request.input_file)
        .arg("-o")
        .arg(&paths.measurements)
        .arg("--au-table-only");
    if request.hlg_composer.is_some() {
        // As for a measurement: the analyzer's FFmpeg may not see an HLG tag MediaInfo saw.
        command.args(["--transfer", "hlg"]);
    }
    if request.args.hwaccel != HwAccel::None {
        command
            .arg("--hwaccel")
            .arg(request.args.hwaccel.to_string());
    }
    progress::print_info(&format!(
        "Writing the AU table of {} into {} (no measurement)...",
        request.input_file,
        paths.work_dir.display()
    ));
    let ran = external::run_command_inherit_stderr(&mut command, &paths.log)?;
    if !ran || !paths.au_table.exists() {
        progress::print_error(&format!(
            "The analyzer wrote no AU table (log: {}). Its messages above say why.",
            paths.log.display()
        ));
        return Ok(None);
    }
    let table = AuTable::read_unbound(&paths.au_table).and_then(|table| {
        table
            .check_source(Path::new(request.input_file), &request.identity)
            .map(|()| table)
    });
    match table {
        Ok(table) => Ok(Some(table)),
        Err(reason) => {
            progress::print_error(&format!(
                "The analyzer's AU table cannot be used: {reason}."
            ));
            Ok(None)
        }
    }
}

/// `Ok` when the file has one RPU per picture and the builder rebuilds each one byte for byte
/// from `params` and `shots`; otherwise the first picture that differs, its AU in decode order,
/// and what differs.
pub(super) fn check_reproduced(
    params: &RpuParams,
    shots: &[ManifestShot],
    rpus: &[SourceRpu<'_>],
    table: &AuTable,
    hlg_composer: Option<Composer>,
) -> Result<(), String> {
    let decode_index = table.aus.decode_index_by_presentation();
    let pictures = decode_index.len();
    let au = |picture: usize| {
        decode_index.get(picture).map_or_else(String::new, |index| {
            format!(" (AU {index} in decode order)")
        })
    };
    if rpus.len() != pictures {
        let first = rpus.len().min(pictures);
        return Err(if rpus.len() < pictures {
            format!(
                "it has {} RPUs for {pictures} pictures: picture {first}{} has no RPU",
                rpus.len(),
                au(first)
            )
        } else {
            format!(
                "it has {} RPUs for {pictures} pictures: RPU {first} has no picture",
                rpus.len()
            )
        });
    }
    let builder = RpuBuilder::new(params)
        .map_err(|error| format!("the RPU parameters of picture 0 cannot be built ({error})"))?;
    for (shot_index, shot) in shots.iter().enumerate() {
        let start = shot.start as usize;
        let end = shots
            .get(shot_index + 1)
            .map_or(pictures, |next| next.start as usize);
        let mut built = builder
            .shot(shot.l1)
            .map_err(|error| format!("the L1 of picture {start} cannot be built ({error})"))?;
        for (picture, source) in rpus.iter().enumerate().take(end).skip(start) {
            let rebuilt = built
                .write(picture == start)
                .map_err(|error| format!("picture {picture} cannot be built ({error})"))?;
            if rebuilt.strip_prefix(&RPU_NAL_HEADER) != Some(rpu_payload(source.data)) {
                return Err(format!(
                    "picture {picture}{} differs from its rebuilt RPU: {}",
                    au(picture),
                    difference(&source.rpu, &rebuilt, hlg_composer)
                ));
            }
        }
    }
    Ok(())
}

/// An RPU of the file without the HEVC NAL header (`7C 01`): `dovi_tool` writes an `RPU.bin`
/// without it, and the `dolby_vision` parser reads either form.
fn rpu_payload(data: &[u8]) -> &[u8] {
    data.strip_prefix(&RPU_NAL_HEADER).unwrap_or(data)
}

/// What a picture's RPU and its rebuild differ in, for the refusal: the HLG composer, the
/// scene-refresh flag, the levels that differ (named by what usually causes it), or the rest.
pub(super) fn difference(
    source: &DoviRpu,
    rebuilt: &[u8],
    hlg_composer: Option<Composer>,
) -> String {
    if let Some(expected) = hlg_composer {
        let source = std::slice::from_ref(source);
        if dovi84_composer::check_rpus(source, expected).is_err() {
            let carried = Composer::ALL
                .into_iter()
                .find(|composer| dovi84_composer::check_rpus(source, *composer).is_ok());
            return match carried {
                Some(carried) => format!(
                    "it carries the {} HLG composer, and --hlg-composer is {}",
                    carried.cli_name(),
                    expected.cli_name()
                ),
                None => format!(
                    "its data mapping is not the {} HLG composer (nor any other this build knows)",
                    expected.cli_name()
                ),
            };
        }
    }
    let Ok(rebuilt) = DoviRpu::parse_unspec62_nalu(rebuilt) else {
        return "the rebuilt RPU does not parse back".into();
    };
    let (Some(source_dm), Some(rebuilt_dm)) = (&source.vdr_dm_data, &rebuilt.vdr_dm_data) else {
        return "one of the two RPUs carries no DM data".into();
    };
    if source_dm.scene_refresh_flag != rebuilt_dm.scene_refresh_flag {
        return format!(
            "scene_refresh_flag is {}, the shot list needs {} (picture 0 must start a scene)",
            source_dm.scene_refresh_flag, rebuilt_dm.scene_refresh_flag
        );
    }
    let levels: Vec<String> = (1..=255u8)
        .filter(|&level| {
            let blocks =
                |dm: &VdrDmData| format!("{:?}", dm.level_blocks_iter(level).collect::<Vec<_>>());
            blocks(source_dm) != blocks(rebuilt_dm)
        })
        .map(|level| match level {
            1 => "L1 (per-frame L1 inside a shot)".to_owned(),
            2 => "L2 (per-frame or authored trims)".to_owned(),
            other => format!("L{other}"),
        })
        .collect();
    if levels.is_empty() {
        "the header, the data mapping or the DM fields outside the extension blocks".into()
    } else {
        format!("it differs in {}", levels.join(", "))
    }
}
