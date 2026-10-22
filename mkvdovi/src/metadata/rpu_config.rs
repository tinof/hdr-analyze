use anyhow::{bail, Context, Result};
use dovi84_composer::Composer;
use dovi_rpu_builder::{
    CmMetadata, CmVersion, ExtraShot, ExtraShots, HlgComposer, Level5, Level6, Profile, RpuParams,
    L1,
};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

use super::sidecar::is_dovi84_family;
use super::{HdrFormat, L1Sidecar};
use crate::rpu_check::Level5Offsets;

#[cfg(test)]
pub(super) use dovi_rpu_builder::nits_to_pq_code;
/// Source range of every Profile 8.4 RPU, whatever the composer (the preset's 62..3079).
pub use dovi_rpu_builder::source_range_84 as source_range_pq_84;
/// Configuration for CM v4.0 metadata generation (one definition, in `dovi_rpu_builder`).
pub use dovi_rpu_builder::CmV40Config;

/// Dolby Vision profile for the generated RPU. HLG input becomes Profile 8.4 (the HLG base layer
/// is kept bit-exact) and needs L1 measured through the 8.4 reconstruction of `hlg_composer`, the
/// composer the RPU will carry; everything else is 8.1.
pub fn dv_profile_for(
    hdr_type: HdrFormat,
    sidecar: Option<&L1Sidecar>,
    hlg_composer: Composer,
) -> Result<&'static str> {
    let mapping = sidecar.and_then(L1Sidecar::luminance_mapping);
    let expected = hlg_composer.luminance_mapping();
    match (hdr_type, sidecar) {
        (HdrFormat::Hlg, None) => {
            anyhow::bail!("HLG input needs measured L1 (an analyzer sidecar) for Profile 8.4")
        }
        (HdrFormat::Hlg, Some(_)) if mapping != Some(expected) => anyhow::bail!(
            "HLG input with --hlg-composer {} needs L1 measured through the Dolby Vision 8.4 mapping {expected}, but the sidecar mapping is {}",
            hlg_composer.cli_name(),
            mapping.unwrap_or("none")
        ),
        (HdrFormat::Hlg, Some(_)) => Ok("8.4"),
        (_, Some(_)) if is_dovi84_family(mapping) => anyhow::bail!(
            "L1 measured through the Dolby Vision 8.4 HLG mapping cannot describe a non-HLG input"
        ),
        _ => Ok("8.1"),
    }
}

/// Profile 8.1 `source_min_pq` / `source_max_pq` (12-bit PQ codes) from the mastering display
/// luminance in nits (`min_dml` / `max_dml`): [`dovi_rpu_builder::source_range_81`], `None` when
/// either value is missing or not a plausible mastering display, so `dovi_tool`'s L6 lookup stays
/// in charge.
pub fn source_range_pq_81(metadata: &HashMap<String, f64>) -> Option<(u16, u16)> {
    dovi_rpu_builder::source_range_81(*metadata.get("min_dml")?, *metadata.get("max_dml")?)
}

/// Source range the generated RPU of `profile` must carry, `None` when mkvdovi leaves it to
/// `dovi_tool` (a Profile 8.1 mastering range [`source_range_pq_81`] cannot use).
pub fn expected_source_range(profile: &str, metadata: &HashMap<String, f64>) -> Option<(u16, u16)> {
    match profile {
        "8.4" => Some(source_range_pq_84()),
        "8.1" => source_range_pq_81(metadata),
        _ => None,
    }
}

/// Write the `extra.json` of `dovi_tool generate`: the RPU parameters ([`RpuParams`], shared
/// with the in-process builders) and, with a sidecar, its per-scene L1 as shots.
pub fn generate_extra_json(
    output_path: &Path,
    profile: &str,
    metadata: &HashMap<String, f64>,
    trim_targets: &[u32],
    cm_v40_config: Option<&CmV40Config>,
    level5_offsets: Option<Level5Offsets>,
    l1_sidecar: Option<&L1Sidecar>,
) -> Result<()> {
    let required = |key| {
        metadata
            .get(key)
            .copied()
            .with_context(|| format!("Missing required static metadata value: {key}"))
    };
    let min_dml = required("min_dml")?;
    let max_dml = required("max_dml")?;
    let max_cll = required("max_cll")?;
    let max_fall = required("max_fall")?;

    let profile = match profile {
        "8.1" => Profile::P81 {
            source_range: source_range_pq_81(metadata),
        },
        // `extra.json` does not name the composer: `dovi_tool generate` writes the preset, and
        // `generate_rpu` installs the selected one afterwards.
        "8.4" => Profile::P84 {
            composer: HlgComposer::Preset,
        },
        other => bail!("unsupported Dolby Vision profile {other} for RPU generation"),
    };
    let cm = match cm_v40_config {
        Some(config) => CmMetadata::V40 {
            config: config.clone(),
            trim_targets_nits: trim_targets.to_vec(),
        },
        None => CmMetadata::V29,
    };
    // Source-honest per-scene L1 from the analyzer sidecar. Explicit shots carry the measured
    // min/avg/max; `l1_avg_pq_cm_version: V29` selects the lower spec floor for `avg_pq` (819 vs
    // the CM v4.0 anchor 1229) so CM v2.9-only displays receive the true scene average instead of
    // a placeholder. dovi_tool still clamps to spec limits. Without a sidecar (HDR10+) the floor
    // is the CM version's, `dovi_tool`'s default.
    let l1_avg_pq_cm_version = if l1_sidecar.is_some() {
        CmVersion::V29
    } else {
        cm.cm_version()
    };
    let params = RpuParams {
        profile,
        cm,
        level6: Level6::from_nits(min_dml, max_dml, max_cll, max_fall),
        level5: level5_offsets.map(|offsets| Level5 {
            left: offsets.left,
            right: offsets.right,
            top: offsets.top,
            bottom: offsets.bottom,
        }),
        l1_avg_pq_cm_version,
    };
    let shots = l1_sidecar.map(sidecar_shots);

    let file = File::create(output_path)?;
    serde_json::to_writer_pretty(file, &params.extra_json(shots.as_ref()))?;
    Ok(())
}

/// The sidecar's scenes as `extra.json` shots in stream frames. Undecodable leading pictures come
/// first in presentation order, so every scene moves back by their count and the first scene also
/// covers them (they are never displayed).
fn sidecar_shots(sidecar: &L1Sidecar) -> ExtraShots {
    let leading = sidecar.leading_frames();
    let shots = sidecar
        .scenes
        .iter()
        .enumerate()
        .map(|(index, scene)| {
            let start = if index == 0 { 0 } else { scene.start + leading };
            ExtraShot {
                start,
                duration: scene.end + leading - start + 1,
                l1: L1 {
                    min_pq: scene.min_pq_12bit,
                    avg_pq: scene.avg_max_rgb_pq_12bit,
                    max_pq: scene.max_pq_12bit,
                },
            }
        })
        .collect();
    ExtraShots {
        length: sidecar.stream_frame_count(),
        shots,
    }
}
