//! Assembling the [`PlaybackManifest`] from a checked pair, the settled RPU inputs and the source
//! identity, and the coverage class.

use anyhow::{bail, Context, Result};
use dovi84_composer::Composer;
use dovi_rpu_builder::manifest::{
    Aus, Coverage, L1Policy, ManifestShot, PlaybackManifest, Producer, QualityClass, Source, Track,
};
use dovi_rpu_builder::{Profile, RpuParams};
use serde_json::{Map, Value};

use super::au_table::{Pair, AU_TABLE_VERSION};
use super::identity::FileIdentity;
use crate::metadata::{self, L1Sidecar};
use crate::pipeline::config::Settled;

/// Sidecar fields besides `analysis` that the manifest's `analysis` object copies: the crop and
/// the peak and denoise settings.
const PROVENANCE_KEYS: [&str; 7] = [
    "crop",
    "peak_domain",
    "peak_estimator",
    "peak_source",
    "peak_percentile",
    "min_percentile",
    "denoise_mode",
];

/// The analyzer settings of a `full` coverage range: the defaults mkvdovi analyzes with (it passes
/// no scene, peak or denoise flag; `hdr_analyzer_mvp/src/cli.rs`), at full resolution on every
/// frame. Text values, then numeric ones.
const FULL_TEXT: [(&str, &str); 4] = [
    ("peak_domain", "max-rgb"),
    ("peak_estimator", "max"),
    ("peak_source", "max"),
    ("denoise_mode", "off"),
];
const FULL_NUMBERS: [(&str, f64); 4] = [
    ("downscale", 1.0),
    ("sample_rate", 1.0),
    ("peak_percentile", 99.99),
    ("min_percentile", 0.1),
];
const FULL_SCENE_TEXT: [(&str, &str); 2] = [("detector", "global"), ("metric", "hist")];
const FULL_SCENE_NUMBERS: [(&str, f64); 2] = [("threshold", 3.0), ("min_length", 12.0)];

/// Everything the manifest is built from.
pub(super) struct Inputs<'a> {
    pub(super) file_name: String,
    pub(super) identity: FileIdentity,
    pub(super) pair: &'a Pair,
    pub(super) sidecar: &'a L1Sidecar,
    pub(super) settled: &'a Settled,
    pub(super) trim_targets: &'a [u32],
    pub(super) hlg_composer: Option<Composer>,
}

pub(super) fn manifest(inputs: &Inputs<'_>) -> Result<PlaybackManifest> {
    let table = &inputs.pair.table;
    let rpu = rpu(inputs)?;
    let pictures = table.aus.picture_count();
    let extra = metadata::sidecar_shots(inputs.sidecar);
    if extra.length != pictures {
        bail!(
            "the sidecar's shots cover {} pictures, the AU table has {pictures}",
            extra.length
        );
    }
    let shots = extra
        .shots
        .iter()
        .map(|shot| ManifestShot {
            start: shot.start,
            l1: shot.l1,
        })
        .collect();
    let analysis = analysis(&inputs.pair.sidecar, &table.scene)?;
    let class = quality_class(&analysis);
    let manifest = PlaybackManifest {
        producer: Producer {
            mkvdovi_version: env!("CARGO_PKG_VERSION").to_owned(),
            analyzer_version: table.analyzer_version.clone(),
            sidecar_version: u64::from(inputs.sidecar.version),
            au_table_version: AU_TABLE_VERSION,
            rpu_builder_version: dovi_rpu_builder::VERSION.to_owned(),
            sidecar_sha256: inputs.pair.sidecar_sha256.clone(),
        },
        source: Source {
            file_name: inputs.file_name.clone(),
            size_bytes: inputs.identity.size_bytes,
            head_sha256: table.source.head_sha256.clone(),
            tail_sha256: table.source.tail_sha256.clone(),
            lookup_key: inputs.identity.lookup_key(),
        },
        track: Track {
            container: table.track.container.clone(),
            codec: table.track.codec.clone(),
            width: table.track.width,
            height: table.track.height,
            codec_private_sha256: table.track.codec_private_sha256.clone(),
            time_base: (table.track.time_base[0], table.track.time_base[1]),
        },
        aus: Aus {
            pts: table.aus.pts(),
            size: table.aus.size.clone(),
            non_picture: table.aus.non_picture.clone(),
            irap_crc32: table.aus.irap_crc32.clone(),
        },
        leading_skipped: table.leading_skipped,
        analysis,
        rpu,
        l1_policy: L1Policy::Measured,
        shots,
        coverage: vec![Coverage {
            start: 0,
            end: pictures,
            class,
        }],
    };
    manifest
        .validate()
        .map_err(|error| anyhow::anyhow!("the manifest does not validate: {error}"))?;
    Ok(manifest)
}

/// The RPU parameters `extra.json` gets ([`metadata::rpu_params`]), with the 8.4 composer named:
/// `extra.json` carries the preset because `generate_rpu` installs the selected composer
/// afterwards, and a player has no such step.
fn rpu(inputs: &Inputs<'_>) -> Result<RpuParams> {
    let settled = inputs.settled;
    let mut params = metadata::rpu_params(
        settled.dv_profile,
        &settled.static_meta,
        inputs.trim_targets,
        settled.cm_v40_config.as_ref(),
        settled.level5_offsets,
        true,
    )?;
    if let Profile::P84 { composer } = &mut params.profile {
        let selected = inputs
            .hlg_composer
            .context("a Profile 8.4 manifest needs the HLG composer")?;
        *composer = selected.into();
    }
    Ok(params)
}

/// The manifest's `analysis` object: the sidecar's `analysis` fields (sampling, `gpu`, `no_crop`,
/// `luminance_mapping`), its crop and peak and denoise settings, and the table's scene settings.
/// Only what the files state; nothing is filled in.
pub(super) fn analysis(sidecar: &Value, scene: &Value) -> Result<Map<String, Value>> {
    let mut analysis = sidecar
        .get("analysis")
        .and_then(Value::as_object)
        .cloned()
        .context("the sidecar has no analysis provenance")?;
    for key in PROVENANCE_KEYS {
        if let Some(value) = sidecar.get(key) {
            analysis.insert(key.to_owned(), value.clone());
        }
    }
    analysis.insert("scene".to_owned(), scene.clone());
    Ok(analysis)
}

/// `full` when the analysis ran with every setting of a default mkvdovi analysis (full
/// resolution, every frame, crop detection on, the default scene, peak and denoise settings);
/// `approximate` otherwise, also when a setting is missing.
pub(super) fn quality_class(analysis: &Map<String, Value>) -> QualityClass {
    let text_is = |map: &Map<String, Value>, key: &str, value: &str| {
        map.get(key).and_then(Value::as_str) == Some(value)
    };
    let number_is = |map: &Map<String, Value>, key: &str, value: f64| {
        map.get(key).and_then(Value::as_f64) == Some(value)
    };
    let scene = analysis.get("scene").and_then(Value::as_object);
    let full = FULL_TEXT
        .iter()
        .all(|&(key, value)| text_is(analysis, key, value))
        && FULL_NUMBERS
            .iter()
            .all(|&(key, value)| number_is(analysis, key, value))
        && analysis.get("no_crop").and_then(Value::as_bool) == Some(false)
        && scene.is_some_and(|scene| {
            FULL_SCENE_TEXT
                .iter()
                .all(|&(key, value)| text_is(scene, key, value))
                && FULL_SCENE_NUMBERS
                    .iter()
                    .all(|&(key, value)| number_is(scene, key, value))
        });
    if full {
        QualityClass::Full
    } else {
        QualityClass::Approximate
    }
}
