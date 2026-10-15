//! The Dolby Vision configuration phase of `convert_file`: static metadata, CM v4.0, the L1
//! sidecar, the profile, `extra.json`, and the withdrawal of an RPU built from an earlier one.

use std::fs;
use std::io;
use std::path::Path;

use anyhow::Result;
use dovi84_composer::Composer;

use super::analyzer::{print_sidecar_advisories, warn_if_gpu_analysis_missing};
use super::hdr10plus::inspect_hdr10plus_scene_peaks;
use super::hybrid;
use super::measure::Measured;
use crate::cli::{Args, CmVersion};
use crate::metadata::{self, HdrFormat};
use crate::progress;
use crate::resume;
use crate::rpu_check::Level5Offsets;

/// What the configuration phase hands to the later steps.
pub(super) struct Configured {
    /// Source range written for Profile 8.1/8.4, checked again by `--verify`.
    pub(super) source_range: Option<(u16, u16)>,
}

/// Writes `extra.json` for `dovi_tool generate`. Settles which L1 sidecar the RPU carries
/// (`measured.l1_sidecar`). `None`: the phase printed why; `convert_file` returns `Ok(false)`.
pub(super) fn prepare_configuration(
    input_file: &str,
    temp_dir: &Path,
    args: &Args,
    resume_enabled: bool,
    hlg_composer: Option<Composer>,
    measured: &mut Measured,
    mut level5_offsets: Option<Level5Offsets>,
) -> Result<Option<Configured>> {
    // Static Metadata
    // MaxCLL / MaxFALL are resolved after the L1 sidecar is loaded: measured values fill what the
    // source does not state.
    let hlg_source = measured.hdr_type == HdrFormat::Hlg;
    let mut static_meta = metadata::read_static_metadata(input_file);
    metadata::apply_static_defaults(&mut static_meta, &["max_dml", "min_dml"], hlg_source);

    // Build CM v4.0 config if enabled
    let cm_v40_config = if args.cm_version == CmVersion::V40 {
        // Detect or use provided source primaries
        let source_primaries = args
            .source_primaries
            .unwrap_or_else(|| metadata::detect_source_primaries(input_file));

        Some(metadata::CmV40Config {
            source_primary_index: source_primaries,
            content_type: args.content_type.as_u8(),
            reference_mode: args.reference_mode,
        })
    } else {
        None
    };

    if args.cm_version == CmVersion::V40 {
        if let Some(ref cfg) = cm_v40_config {
            progress::print_info(&format!(
                "CM v4.0 — L9: primaries={}, L11: content_type={}, reference_mode={}",
                cfg.source_primary_index, cfg.content_type, cfg.reference_mode
            ));
        }
    }

    // Warn when generated HDR10+ scene L1 peaks look suspicious. This is advisory:
    // valid sources can contain outliers, so never clamp them silently.
    if let (Some(metadata_path), Some(&max_dml)) = (
        measured.hdr10plus_json.as_deref(),
        static_meta.get("max_dml"),
    ) {
        match inspect_hdr10plus_scene_peaks(metadata_path, args.peak_source, max_dml * 3.0) {
            Ok(Some(stats)) => progress::print_warn(&format!(
                "{} HDR10+ scene(s) produce L1 peaks above 3× the mastering display peak \
                 ({:.0} nits); highest selected peak is {:.0} nits. Review the source metadata \
                 and compare --peak-source modes before deciding whether to use an opt-in clamp.",
                stats.outlier_scene_count, max_dml, stats.max_peak_nits
            )),
            Ok(None) => {}
            Err(e) => progress::print_warn(&format!(
                "Could not inspect extracted HDR10+ scene peaks: {e}"
            )),
        }
    }

    // Generate extra.json
    let extra_json_path = temp_dir.join("extra.json");
    // Validated in main before any file is processed.
    let final_trims = args.trim_targets_nits().unwrap_or_default();

    // Source-honest per-scene L1 from the analyzer sidecar is mandatory for measurement-based
    // generation. dovi_tool's madVR path fills L1 avg with a fixed placeholder and (with custom
    // targets) replaces L1 max with optimizer targets, so it is reachable only by explicit opt-in.
    if let Some(measurements) = measured.measurements_file.as_deref() {
        if args.legacy_madvr_l1 {
            measured.l1_sidecar = None;
            progress::print_warn(
                "--legacy-madvr-l1: L1 max comes from optimizer targets and L1 avg is a placeholder, not measured values.",
            );
        } else if measured.l1_sidecar.is_none() {
            // Freshly produced by the analyzer in this run: validate structure and, for HLG, the
            // luminance mapping (an older analyzer binary would measure without it). The frame
            // count is the analyzer's own: a v5 sidecar accounts for every picture of the stream
            // it demuxed, while MediaInfo can report a count estimated from the duration or
            // copied from stale statistics tags. The inject step still refuses an RPU whose
            // length differs from the video.
            let expect = metadata::SidecarExpectation::default().with_hlg_composer(hlg_composer);
            match metadata::load_l1_sidecar(measurements, &expect) {
                Ok((sidecar, advisories)) => {
                    print_sidecar_advisories(&advisories);
                    warn_if_gpu_analysis_missing(&sidecar, args);
                    measured.l1_sidecar = Some(sidecar);
                }
                Err(error) if measured.hdr_type == HdrFormat::Hlg => {
                    let required = hlg_composer.map_or(
                        "`--transfer` and `--hlg-composer`".to_string(),
                        |composer| {
                            format!(
                                "`--transfer`, `--hlg-composer` and the mapping {}",
                                composer.luminance_mapping()
                            )
                        },
                    );
                    progress::print_error(&format!(
                        "The analyzer's L1 sidecar cannot be used for Profile 8.4 ({error}). The analyzer did not measure through the selected Dolby Vision 8.4 HLG composer and decode, so it is probably an older build: `hdr_analyzer_mvp --help` must list {required}."
                    ));
                    return Ok(None);
                }
                Err(error) => {
                    progress::print_error(&format!(
                        "The analyzer's L1 sidecar is unusable ({error}). Refusing to fall back to optimizer-derived L1; pass --legacy-madvr-l1 to force it."
                    ));
                    return Ok(None);
                }
            }
        }
    }
    // --hdr10plus-l1 hybrid: the RPU's scenes become the union of the HDR10+ scenes and the
    // analyzer's cuts, with the HDR10+ peak and the measured average and minimum.
    if measured.hdr10plus_hybrid {
        let combined = match (
            measured.l1_sidecar.take(),
            measured.hdr10plus_json.as_deref(),
        ) {
            (Some(sidecar), Some(hdr10plus_json)) => {
                hybrid::apply_file(sidecar, hdr10plus_json, args.peak_source)
            }
            _ => Err("the measurements or the HDR10+ metadata are missing".to_string()),
        };
        match combined {
            Ok((sidecar, report)) => {
                report.print(args.peak_source);
                measured.l1_sidecar = Some(sidecar);
            }
            Err(error) => {
                progress::print_error(&format!(
                    "--hdr10plus-l1 hybrid cannot combine the HDR10+ metadata with the measurements: {error}. Convert without --hdr10plus-l1 hybrid to use the HDR10+ metadata alone."
                ));
                return Ok(None);
            }
        }
    }
    if let Some(sidecar) = &measured.l1_sidecar {
        progress::print_info(&format!(
            "Using measured L1 sidecar ({}): source-honest per-scene min/avg/max in the RPU.",
            sidecar.provenance_summary()
        ));
        // Offsets sampled from a source RPU keep precedence; otherwise describe the same active
        // area the measurements were taken over.
        if level5_offsets.is_none() {
            if let Some(offsets) = metadata::level5_from_sidecar(sidecar) {
                progress::print_info(&format!(
                    "L5 active area from the committed crop: left {}, right {}, top {}, bottom {}.",
                    offsets.left, offsets.right, offsets.top, offsets.bottom
                ));
                level5_offsets = Some(offsets);
            }
        }
    }

    let dv_profile = match metadata::dv_profile_for(
        measured.hdr_type,
        measured.l1_sidecar.as_ref(),
        args.hlg_composer,
    ) {
        Ok(profile) => profile,
        Err(error) => {
            progress::print_error(&format!(
                "Cannot choose the Dolby Vision profile: {error:#}"
            ));
            return Ok(None);
        }
    };
    if dv_profile != "8.1" {
        progress::print_info(&format!(
            "Generating Dolby Vision Profile {dv_profile} (base layer kept bit-exact)."
        ));
    }

    for message in
        metadata::resolve_light_levels(&mut static_meta, measured.l1_sidecar.as_ref(), hlg_source)
    {
        progress::print_info(&message);
    }

    // Profile 8.1 states the mastering range explicitly; dovi_tool's own L6 lookup gets every
    // master other than 1000/2000/4000/10000 nits (and min 0.0001/0.005 nits) wrong.
    let source_range = metadata::expected_source_range(dv_profile, &static_meta);
    if source_range.is_none() {
        progress::print_warn(&format!(
            "Mastering display luminance (min {} nits, max {} nits) is not plausible; dovi_tool derives the source range from L6.",
            static_meta.get("min_dml").copied().unwrap_or(f64::NAN),
            static_meta.get("max_dml").copied().unwrap_or(f64::NAN)
        ));
    }

    let previous_config = fs::read(&extra_json_path).ok();
    metadata::generate_extra_json(
        &extra_json_path,
        dv_profile,
        &static_meta,
        &final_trims,
        cm_v40_config.as_ref(),
        level5_offsets,
        measured.l1_sidecar.as_ref(),
    )?;
    progress::print_info("Configuration written.");
    // The resume fingerprint does not cover the measurements. An RPU built from an earlier
    // configuration (for example before a re-analysis changed L1) must not be reused.
    if resume_enabled {
        withdraw_stale_rpu(temp_dir, &extra_json_path, previous_config.as_deref())?;
    }

    Ok(Some(Configured { source_range }))
}

/// Withdraws the RPU sentinels and the mux marker when `extra_json` differs from `previous`, the
/// configuration a resumed temp directory held. Returns true when they were withdrawn.
pub(super) fn withdraw_stale_rpu(
    temp_dir: &Path,
    extra_json: &Path,
    previous: Option<&[u8]>,
) -> io::Result<bool> {
    if previous == fs::read(extra_json).ok().as_deref() {
        return Ok(false);
    }
    let stale = [temp_dir.join("RPU.bin"), temp_dir.join("BL_RPU.hevc")];
    if stale.iter().any(|artifact| resume::is_complete(artifact)) {
        progress::print_warn(
            "The generation settings differ from the interrupted run; regenerating the RPU instead of reusing it.",
        );
    }
    for artifact in &stale {
        resume::invalidate(artifact)?;
    }
    // A muxed output from that run carries the old RPU too.
    let _ = fs::remove_file(temp_dir.join("mux.done"));
    Ok(true)
}
