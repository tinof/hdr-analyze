//! The Dolby Vision configuration phase of `convert_file`: static metadata, CM v4.0, the L1
//! sidecar, the profile, `extra.json`, and the withdrawal of an RPU built from an earlier one.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use anyhow::Result;
use dovi84_composer::Composer;

use super::analyzer::{print_sidecar_advisories, warn_if_gpu_analysis_missing};
use super::hdr10plus::inspect_hdr10plus_scene_peaks;
use super::hybrid;
use super::manifest::au_table_path;
use super::measure::Measured;
use super::source_checks::{self, Severity};
use crate::cli::{Args, CmVersion, HwAccel};
use crate::metadata::{self, HdrFormat};
use crate::progress;
use crate::resume;
use crate::rpu_check::{Level5Offsets, SourceL6};

/// What the configuration phase hands to the later steps.
pub(super) struct Configured {
    /// Source range written for Profile 8.1/8.4, checked again by `--verify`.
    pub(super) source_range: Option<(u16, u16)>,
}

/// The RPU inputs the configuration settles: what `extra.json` and the playback manifest's `rpu`
/// are built from.
pub(super) struct Settled {
    pub(super) dv_profile: &'static str,
    pub(super) static_meta: HashMap<String, f64>,
    pub(super) cm_v40_config: Option<metadata::CmV40Config>,
    pub(super) level5_offsets: Option<Level5Offsets>,
    pub(super) source_range: Option<(u16, u16)>,
}

/// Writes `extra.json` for `dovi_tool generate`. Settles which L1 sidecar the RPU carries
/// (`measured.l1_sidecar`). `None`: the phase printed why; `convert_file` returns `Ok(false)`.
/// `source_l6` is the sampled source RPU's L6 (see `settle_configuration`).
pub(super) fn prepare_configuration(
    input_file: &str,
    temp_dir: &Path,
    args: &Args,
    resume_enabled: bool,
    hlg_composer: Option<Composer>,
    measured: &mut Measured,
    level5_offsets: Option<Level5Offsets>,
    source_l6: Option<&SourceL6>,
) -> Result<Option<Configured>> {
    let Some(settled) = settle_configuration(
        input_file,
        args,
        hlg_composer,
        measured,
        level5_offsets,
        source_l6,
    ) else {
        return Ok(None);
    };

    // Generate extra.json
    let extra_json_path = temp_dir.join("extra.json");
    // Validated in main before any file is processed.
    let final_trims = args.trim_targets_nits().unwrap_or_default();
    // Withdrawn before `extra.json` changes: an interruption between the two must not leave the
    // new configuration beside an RPU built from the old one.
    let next_config = temp_dir.join("extra.json.next");
    metadata::generate_extra_json(
        &next_config,
        settled.dv_profile,
        &settled.static_meta,
        &final_trims,
        settled.cm_v40_config.as_ref(),
        settled.level5_offsets,
        measured.l1_sidecar.as_ref(),
    )?;
    commit_config(temp_dir, &next_config, &extra_json_path, resume_enabled)?;
    progress::print_info("Configuration written.");

    Ok(Some(Configured {
        source_range: settled.source_range,
    }))
}

/// Static metadata, CM v4.0, the L1 sidecar, L5, the profile and the source range: everything the
/// RPU parameters need. `None`: refused, the phase printed why. Under `--mdfix`, `source_l6` (the
/// sampled source RPU's L6) fills the mastering display the container does not state, before the
/// defaults (P12.1).
pub(super) fn settle_configuration(
    input_file: &str,
    args: &Args,
    hlg_composer: Option<Composer>,
    measured: &mut Measured,
    level5_offsets: Option<Level5Offsets>,
    source_l6: Option<&SourceL6>,
) -> Option<Settled> {
    // Static Metadata
    // MaxCLL / MaxFALL are resolved after the L1 sidecar is loaded: measured values fill what the
    // source does not state.
    let hlg_source = measured.hdr_type == HdrFormat::Hlg;
    let mut static_meta = metadata::read_static_metadata(input_file);
    match fill_mastering_from_source_l6(&mut static_meta, source_l6, args.mdfix) {
        Some((Severity::Info, message)) => progress::print_info(&message),
        Some((Severity::Warn, message)) => progress::print_warn(&message),
        None => {}
    }
    metadata::apply_static_defaults(&mut static_meta, &["max_dml", "min_dml"], hlg_source);

    let cm_v40_config = cm_v40_config(input_file, args);
    warn_on_hdr10plus_peaks(measured, &static_meta, args);

    // Each step below may take the HDR10+ metadata path instead (`fall_back_to_hdr10plus`), so
    // everything after them reads `measured` afresh.
    if !load_fresh_sidecar(args, hlg_composer, measured) || !combine_hybrid(args, measured) {
        return None;
    }
    // After the hybrid: whether dovi_tool reads the HDR10+ peaks depends on the path it settled.
    check_hdr10plus_source(measured, &static_meta, args);
    let level5_offsets = sidecar_level5(measured, level5_offsets);
    let dv_profile = choose_profile(measured, args)?;
    // `None` is not a refusal: dovi_tool then derives the source range from L6.
    let source_range = resolve_source_range(&mut static_meta, measured, dv_profile, hlg_source);
    Some(Settled {
        dv_profile,
        static_meta,
        cm_v40_config,
        level5_offsets,
        source_range,
    })
}

/// P12.1: fills `max_dml` / `min_dml` that the container does not state from the source RPU's L6
/// and returns the info line naming what it took. The container always wins. The block is used
/// only when it is a plausible mastering display, by the rule the 8.1 source range applies
/// (`MASTERING_PEAK_NITS`, `MASTERING_MIN_NITS`); otherwise the defaults apply, as before, and the
/// returned warning names the values that were not used. MaxCLL / MaxFALL are not taken from the
/// RPU; they stay with the container or the measurements. Only under `--mdfix` (`mdfix`): without
/// it no RPU is rebuilt from a Dolby Vision source.
fn fill_mastering_from_source_l6(
    meta: &mut HashMap<String, f64>,
    source_l6: Option<&SourceL6>,
    mdfix: bool,
) -> Option<(Severity, String)> {
    let l6 = source_l6.filter(|_| mdfix)?;
    if meta.contains_key("max_dml") && meta.contains_key("min_dml") {
        return None;
    }
    let (peak, min) = (f64::from(l6.max_dml_nits), min_dml_nits(l6.min_dml_raw));
    let min_label = format!("{:.4} nits", f64::from(l6.min_dml_raw) / 10_000.0);
    let plausible = dovi_rpu_builder::MASTERING_PEAK_NITS.contains(&peak)
        && dovi_rpu_builder::MASTERING_MIN_NITS.contains(&min);
    if !plausible {
        return Some((
            Severity::Warn,
            format!(
                "The source RPU's L6 states a mastering display of {} nits peak, {min_label} \
                 minimum, which is not plausible (peak 100 to 10000 nits, minimum 0 to 1 nit); \
                 the new L6 does not take it.",
                l6.max_dml_nits
            ),
        ));
    }
    let mut taken = Vec::new();
    if !meta.contains_key("max_dml") {
        meta.insert("max_dml".to_string(), peak);
        taken.push(format!("peak {} nits", l6.max_dml_nits));
    }
    if !meta.contains_key("min_dml") {
        meta.insert("min_dml".to_string(), min);
        taken.push(format!("minimum {min_label}"));
    }
    Some((
        Severity::Info,
        format!(
            "Mastering display {} from the source RPU's L6 (not stated by the container).",
            taken.join(", ")
        ),
    ))
}

/// The mastering minimum in nits for a raw L6 value in 0.0001 nits, chosen so that
/// `Level6::from_nits`, which truncates `nits * 10000`, gives `raw` back: `raw / 10000` alone
/// comes back one lower for some values (29 -> 28.999...). One step to the next `f64` up is
/// enough for every value up to `Level6::MAX_VALUE`; an exact quotient is kept as it is.
fn min_dml_nits(raw: u16) -> f64 {
    let nits = f64::from(raw) / 10_000.0;
    if ((nits * 10_000.0) as u32) < u32::from(raw) {
        nits.next_up()
    } else {
        nits
    }
}

/// The CM v4.0 configuration (L9 primaries, L11 content type), or `None` under `--cm-version v29`.
fn cm_v40_config(input_file: &str, args: &Args) -> Option<metadata::CmV40Config> {
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
    cm_v40_config
}

/// Warns when generated HDR10+ scene L1 peaks look suspicious. This is advisory: valid sources can
/// contain outliers, so never clamp them silently.
fn warn_on_hdr10plus_peaks(measured: &Measured, static_meta: &HashMap<String, f64>, args: &Args) {
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
}

/// P6.2 C4 (HDR10+ form) and C7: the HDR10+ peaks against the stated MaxCLL, and a MaxScl that is
/// 0 on every frame. `static_meta` holds only stated light levels here. Warn-only.
fn check_hdr10plus_source(measured: &Measured, static_meta: &HashMap<String, f64>, args: &Args) {
    let Some(path) = measured.hdr10plus_json.as_deref() else {
        return;
    };
    let metadata = match fs::File::open(path)
        .map_err(|e| e.to_string())
        .and_then(|file| {
            serde_json::from_reader(io::BufReader::new(file)).map_err(|e| e.to_string())
        }) {
        Ok(metadata) => metadata,
        Err(e) => {
            progress::print_warn(&format!(
                "Could not read the HDR10+ metadata for the source checks: {e}"
            ));
            return;
        }
    };
    if let Some(warning) =
        source_checks::hdr10plus_peak_against_max_cll(&metadata, static_meta, args.peak_source)
    {
        progress::print_warn(&warning);
    }
    let hybrid = measured.hdr10plus_mode.is_hybrid();
    match source_checks::hdr10plus_max_scl_all_zero(&metadata, args.peak_source, hybrid) {
        Some((Severity::Warn, message)) => progress::print_warn(&message),
        Some((Severity::Info, message)) => progress::print_info(&message),
        None => {}
    }
}

/// Loads and validates the sidecar the analyzer wrote in this run, or drops it under
/// `--legacy-madvr-l1`. False: refused, the phase printed why.
fn load_fresh_sidecar(
    args: &Args,
    hlg_composer: Option<Composer>,
    measured: &mut Measured,
) -> bool {
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
            // Freshly produced by the analyzer in this run: validate structure, for HLG the
            // luminance mapping (an older analyzer binary would measure without it), and under
            // --hwaccel cuda that CPU analysis read the NVDEC frames unconverted (sidecar v7;
            // older analyzers moved their codes). The frame count is the analyzer's own: a v5
            // sidecar accounts for every picture of the stream it demuxed, while MediaInfo can
            // report a count estimated from the duration or copied from stale statistics tags.
            // The inject step still refuses an RPU whose length differs from the video.
            let expect = metadata::SidecarExpectation::default()
                .with_hlg_composer(hlg_composer)
                .with_nvdec_route(args.hwaccel == HwAccel::Cuda);
            // Measurements refused for the decode route are removed whatever the run does next:
            // a later run with --hwaccel none would reuse them.
            match metadata::load_l1_sidecar(measurements, &expect) {
                Ok((sidecar, advisories)) => {
                    // An analyzer that ignored or lacks the requested estimator (ROADMAP WS1.7)
                    // measured something else: refuse, never deliver it as the requested one.
                    if let Err(reason) = super::estimator::check_fresh(&sidecar, args) {
                        progress::print_error(&format!(
                            "The analyzer's L1 sidecar cannot be used: {reason}."
                        ));
                        return false;
                    }
                    print_sidecar_advisories(&advisories);
                    warn_if_gpu_analysis_missing(&sidecar, args);
                    measured.l1_sidecar = Some(sidecar);
                }
                Err(error @ metadata::SidecarError::InexactNvdecRoute(_))
                    if !measured.hdr10plus_mode.is_hybrid() =>
                {
                    let removal = remove_inexact_measurements(measurements);
                    progress::print_error(&format!(
                        "The analyzer's L1 sidecar cannot be used ({error}). This hdr_analyzer_mvp analyzes NVDEC frames on the CPU through a conversion that moves their codes: update it to one that writes sidecar v{}, or run with --hwaccel none. Also, {removal}.",
                        metadata::L1_SIDECAR_EXACT_DECODE_VERSION
                    ));
                    return false;
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
                    return false;
                }
                Err(error) if measured.hdr10plus_mode.is_hybrid() => {
                    let mut reason = format!("the analyzer's L1 sidecar is unusable ({error})");
                    if matches!(error, metadata::SidecarError::InexactNvdecRoute(_)) {
                        let removal = remove_inexact_measurements(measurements);
                        reason.push_str(&format!("; {removal}"));
                    }
                    if !fall_back_to_hdr10plus(measured, &reason) {
                        return false;
                    }
                }
                Err(error) => {
                    progress::print_error(&format!(
                        "The analyzer's L1 sidecar is unusable ({error}). Refusing to fall back to optimizer-derived L1; pass --legacy-madvr-l1 to force it."
                    ));
                    return false;
                }
            }
        }
    }
    true
}

/// Removes the `.bin`, `.l1.json` and `.au.json` a fresh analysis just wrote after their sidecar
/// was refused as [`metadata::SidecarError::InexactNvdecRoute`], and returns the clause that
/// says so. A file that is already gone is not an error.
fn remove_inexact_measurements(measurements: &Path) -> String {
    let files = [
        measurements.to_path_buf(),
        metadata::l1_sidecar_path(measurements),
        au_table_path(measurements),
    ];
    let failed: Vec<String> = files
        .iter()
        .filter_map(|file| match fs::remove_file(file) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                Some(format!("{}: {error}", file.display()))
            }
            _ => None,
        })
        .collect();
    if failed.is_empty() {
        format!(
            "the measurements this run wrote ('{}' and its sidecars) were removed, so no later run reuses them",
            measurements.display()
        )
    } else {
        format!(
            "these measurements could not be removed; delete them before the next run: {}",
            failed.join("; ")
        )
    }
}

/// HDR10+ hybrid: the RPU's scenes become the union of the HDR10+ scenes and the analyzer's cuts,
/// with the HDR10+ peak and the measured average and minimum. False: refused, the phase printed
/// why.
fn combine_hybrid(args: &Args, measured: &mut Measured) -> bool {
    if measured.hdr10plus_mode.is_hybrid() {
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
                let reason =
                    format!("the HDR10+ metadata and the measurements do not combine ({error})");
                if !fall_back_to_hdr10plus(measured, &reason) {
                    return false;
                }
            }
        }
    }
    true
}

/// Announces the settled sidecar and returns the L5 offsets for the RPU: offsets sampled from a
/// source RPU, else the committed crop the sidecar describes.
fn sidecar_level5(
    measured: &Measured,
    mut level5_offsets: Option<Level5Offsets>,
) -> Option<Level5Offsets> {
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
    level5_offsets
}

/// The Dolby Vision profile for the settled measurements. `None`: refused, the phase printed why.
fn choose_profile(measured: &Measured, args: &Args) -> Option<&'static str> {
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
            return None;
        }
    };
    if dv_profile != "8.1" {
        progress::print_info(&format!(
            "Generating Dolby Vision Profile {dv_profile} (base layer kept bit-exact)."
        ));
    }
    Some(dv_profile)
}

/// Fills MaxCLL / MaxFALL the source does not state from the measurements, then returns the source
/// range the RPU states, or `None` (with a warning) when the mastering display is not plausible.
fn resolve_source_range(
    static_meta: &mut HashMap<String, f64>,
    measured: &Measured,
    dv_profile: &str,
    hlg_source: bool,
) -> Option<(u16, u16)> {
    // P6.2 C1 and C3, before measured values or defaults fill what the source does not state.
    let measured_max_cll = measured
        .l1_sidecar
        .as_ref()
        .and_then(|sidecar| sidecar.measured_light_levels().ok())
        .and_then(|levels| levels.max_cll);
    for warning in [
        source_checks::measured_max_cll_above_stated(static_meta, measured_max_cll),
        source_checks::stated_fall_above_cll(static_meta),
    ]
    .into_iter()
    .flatten()
    {
        progress::print_warn(&warning);
    }
    for message in
        metadata::resolve_light_levels(static_meta, measured.l1_sidecar.as_ref(), hlg_source)
    {
        progress::print_info(&message);
    }

    // Profile 8.1 states the mastering range explicitly; dovi_tool's own L6 lookup gets every
    // master other than 1000/2000/4000/10000 nits (and min 0.0001/0.005 nits) wrong.
    let source_range = metadata::expected_source_range(dv_profile, static_meta);
    if source_range.is_none() {
        progress::print_warn(&format!(
            "Mastering display luminance (min {} nits, max {} nits) is not plausible; dovi_tool derives the source range from L6.",
            static_meta.get("min_dml").copied().unwrap_or(f64::NAN),
            static_meta.get("max_dml").copied().unwrap_or(f64::NAN)
        ));
    }
    source_range
}

/// Takes the HDR10+ metadata path after the hybrid failed for `reason`, as if the input had never
/// been measured: everything that depends on the path is computed after this. False after the
/// refusal of an explicit hybrid.
pub(super) fn fall_back_to_hdr10plus(measured: &mut Measured, reason: &str) -> bool {
    let Some(mode) = measured.hdr10plus_mode.after_failure(reason) else {
        return false;
    };
    measured.hdr10plus_mode = mode;
    measured.hdr_type = HdrFormat::Hdr10Plus;
    measured.measurements_file = None;
    measured.l1_sidecar = None;
    true
}

/// Replaces `extra_json` with `next`. The resume fingerprint does not cover the measurements, so
/// an RPU built from an earlier configuration (for example before a re-analysis changed L1, or
/// on the other HDR10+ path) must not be reused.
pub(super) fn commit_config(
    temp_dir: &Path,
    next: &Path,
    extra_json: &Path,
    resume_enabled: bool,
) -> io::Result<()> {
    if resume_enabled {
        let previous = fs::read(extra_json).ok();
        withdraw_stale_rpu(temp_dir, next, previous.as_deref())?;
    }
    fs::rename(next, extra_json)
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;
    use serde_json::{json, Value};

    use super::super::measure::{find_reusable_measurements, Hdr10PlusMode};
    use super::*;

    /// The r5 source's L6: 1000 nits, 0.0001 nits, no light levels.
    const R5_L6: SourceL6 = SourceL6 {
        max_dml_nits: 1000,
        min_dml_raw: 1,
        max_cll: 0,
        max_fall: 0,
    };

    fn stated(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
        pairs.iter().map(|&(k, v)| (k.to_string(), v)).collect()
    }

    fn info(meta: &mut HashMap<String, f64>, l6: &SourceL6) -> String {
        match fill_mastering_from_source_l6(meta, Some(l6), true) {
            Some((Severity::Info, message)) => message,
            other => panic!("expected an info line for {l6:?}, got {other:?}"),
        }
    }

    #[test]
    fn under_mdfix_the_source_l6_fills_a_container_without_mastering_display() {
        let mut meta = stated(&[("max_cll", 794.0)]);
        let message = info(&mut meta, &R5_L6);
        assert_eq!(
            meta,
            stated(&[("max_cll", 794.0), ("max_dml", 1000.0), ("min_dml", 0.0001)])
        );
        assert_eq!(
            message,
            "Mastering display peak 1000 nits, minimum 0.0001 nits from the source RPU's L6 \
             (not stated by the container)."
        );
    }

    #[test]
    fn without_mdfix_or_without_a_sample_nothing_is_filled() {
        for (l6, mdfix) in [(Some(&R5_L6), false), (None, true)] {
            let mut meta = HashMap::new();
            assert_eq!(fill_mastering_from_source_l6(&mut meta, l6, mdfix), None);
            assert!(meta.is_empty(), "mdfix={mdfix}");
        }
    }

    #[test]
    fn the_container_wins_for_each_value_it_states() {
        let mut both = stated(&[("max_dml", 4000.0), ("min_dml", 0.005)]);
        assert_eq!(
            fill_mastering_from_source_l6(&mut both, Some(&R5_L6), true),
            None
        );
        assert_eq!(both, stated(&[("max_dml", 4000.0), ("min_dml", 0.005)]));
        // A container that states both is not compared here (C5 does that): even an
        // implausible RPU L6 then prints nothing.
        let implausible = SourceL6 {
            max_dml_nits: 0,
            ..R5_L6
        };
        assert_eq!(
            fill_mastering_from_source_l6(&mut both, Some(&implausible), true),
            None
        );

        let mut only_max = stated(&[("max_dml", 4000.0)]);
        let message = info(&mut only_max, &R5_L6);
        assert_eq!(
            only_max,
            stated(&[("max_dml", 4000.0), ("min_dml", 0.0001)])
        );
        assert!(message.contains("minimum 0.0001 nits") && !message.contains("peak"));

        let mut only_min = stated(&[("min_dml", 0.005)]);
        let message = info(&mut only_min, &R5_L6);
        assert_eq!(only_min, stated(&[("max_dml", 1000.0), ("min_dml", 0.005)]));
        assert!(message.contains("peak 1000 nits") && !message.contains("minimum"));
    }

    #[test]
    fn light_levels_are_never_taken_from_the_rpu() {
        let l6 = SourceL6 {
            max_cll: 992,
            max_fall: 285,
            ..R5_L6
        };
        let mut meta = HashMap::new();
        info(&mut meta, &l6);
        assert!(!meta.contains_key("max_cll") && !meta.contains_key("max_fall"));
    }

    #[test]
    fn an_implausible_l6_fills_nothing_and_says_so() {
        // The 8.1 source-range rule: peak 100 to 10000 nits, minimum 0 to 1 nit (raw 10000).
        for (peak, min_raw) in [(0, 1), (99, 1), (10_001, 1), (1000, 10_001), (50, 0)] {
            let l6 = SourceL6 {
                max_dml_nits: peak,
                min_dml_raw: min_raw,
                ..R5_L6
            };
            let mut meta = stated(&[("max_cll", 794.0)]);
            match fill_mastering_from_source_l6(&mut meta, Some(&l6), true) {
                Some((Severity::Warn, message)) => {
                    assert!(
                        message.contains(&format!("{peak} nits peak"))
                            && message.contains("not plausible"),
                        "{message}"
                    );
                }
                other => panic!("{l6:?}: {other:?}"),
            }
            assert_eq!(meta, stated(&[("max_cll", 794.0)]), "{l6:?}");
        }
        // The edges of the range are used, and a minimum of 0 is a stated minimum.
        for (peak, min_raw, min) in [(10_000, 10_000, 1.0), (100, 0, 0.0)] {
            let l6 = SourceL6 {
                max_dml_nits: peak,
                min_dml_raw: min_raw,
                ..R5_L6
            };
            let mut meta = HashMap::new();
            info(&mut meta, &l6);
            assert_eq!(
                meta,
                stated(&[("max_dml", f64::from(peak)), ("min_dml", min)])
            );
        }
    }

    #[test]
    fn every_raw_minimum_survives_the_generator_truncation() {
        for raw in 0..=10_000u16 {
            let level6 = dovi_rpu_builder::Level6::from_nits(min_dml_nits(raw), 1000.0, 0.0, 0.0);
            assert_eq!(
                level6.min_display_mastering_luminance,
                u32::from(raw),
                "raw {raw}"
            );
        }
        // An exact quotient is kept; only a short one moves up by one step.
        assert_eq!(min_dml_nits(1), 0.0001);
        assert_eq!(min_dml_nits(29), (29.0_f64 / 10_000.0).next_up());
        // The plain quotient does not: the reason for `min_dml_nits`.
        let plain = dovi_rpu_builder::Level6::from_nits(29.0 / 10_000.0, 1000.0, 0.0, 0.0);
        assert_eq!(plain.min_display_mastering_luminance, 28);
    }

    /// What a fresh analysis of `<dir>/clip.mkv` leaves beside it: the measurements, their
    /// sidecar `sidecar` (none for `None`) and an AU table.
    struct FreshAnalysis {
        dir: tempfile::TempDir,
    }

    impl FreshAnalysis {
        fn new(sidecar: Option<&Value>) -> Self {
            let fresh = Self {
                dir: tempfile::tempdir().unwrap(),
            };
            fs::write(fresh.input(), b"src").unwrap();
            fs::write(fresh.measurements(), b"bin").unwrap();
            fs::write(au_table_path(&fresh.measurements()), b"{}").unwrap();
            if let Some(sidecar) = sidecar {
                let path = metadata::l1_sidecar_path(&fresh.measurements());
                fs::write(path, sidecar.to_string()).unwrap();
            }
            fresh
        }

        fn input(&self) -> PathBuf {
            self.dir.path().join("clip.mkv")
        }

        fn measurements(&self) -> PathBuf {
            self.dir.path().join("clip_measurements.bin")
        }

        /// Whether the measurements, the sidecar and the AU table are still there.
        fn files(&self) -> [bool; 3] {
            let measurements = self.measurements();
            [
                measurements.exists(),
                metadata::l1_sidecar_path(&measurements).exists(),
                au_table_path(&measurements).exists(),
            ]
        }

        /// `load_fresh_sidecar` under `--hwaccel <hwaccel>` for an input of `hdr_type`.
        fn load(
            &self,
            hwaccel: &str,
            hdr_type: HdrFormat,
            mode: Hdr10PlusMode,
            hlg_composer: Option<Composer>,
        ) -> (bool, Measured) {
            let args = Args::try_parse_from(["mkvdovi", "--hwaccel", hwaccel]).unwrap();
            let mut measured = Measured {
                hdr_type,
                measurements_file: Some(self.measurements()),
                hdr10plus_json: None,
                bl_source_file: self.input(),
                l1_sidecar: None,
                hdr10plus_mode: mode,
            };
            let loaded = load_fresh_sidecar(&args, hlg_composer, &mut measured);
            (loaded, measured)
        }

        fn load_pq(&self, hwaccel: &str, mode: Hdr10PlusMode) -> (bool, Measured) {
            self.load(hwaccel, HdrFormat::Hdr10WithMeasurements, mode, None)
        }
    }

    /// A sidecar of `version` for the 4 frames of `clip.mkv`: CPU analysis at downscale 1, PQ or
    /// HLG through `mapping`, as the analyzer writes it.
    fn fresh_sidecar(version: u32, mapping: &str) -> Value {
        let transfer = if mapping == "pq" {
            "PQ (SMPTE 2084)"
        } else {
            "HLG (ARIB STD-B67)"
        };
        json!({
            "version": version,
            "source": {"file_name": "clip.mkv", "size_bytes": 3, "width": 1920, "height": 1080,
                       "transfer_function": transfer, "stream_frames": 4,
                       "leading_skipped_frames": 0},
            "analysis": {"downscale": 1, "sample_rate": 1, "gpu": false, "no_crop": false,
                         "luminance_mapping": mapping},
            "peak_domain": "max-rgb", "peak_estimator": "max", "peak_source": "max",
            "scenes": [{"start": 0, "end": 3, "min_pq_12bit": 1, "avg_luma_pq_12bit": 500,
                        "avg_max_rgb_pq_12bit": 520, "max_pq_12bit": 2400}],
            "frames": {"min_pq_12bit": [1, 1, 1, 1], "avg_max_rgb_pq_12bit": [520, 520, 520, 520],
                       "max_pq_12bit": [2400, 2300, 2200, 2100]}
        })
    }

    const GONE: [bool; 3] = [false, false, false];
    const KEPT: [bool; 3] = [true, true, true];

    #[test]
    fn a_fresh_cpu_sidecar_before_v7_under_hwaccel_cuda_is_refused_and_removed() {
        let pq = fresh_sidecar(6, "pq");
        for mode in [Hdr10PlusMode::Metadata, Hdr10PlusMode::ExplicitHybrid] {
            let fresh = FreshAnalysis::new(Some(&pq));
            let (loaded, measured) = fresh.load_pq("cuda", mode);
            assert!(!loaded, "{mode:?}");
            assert!(measured.l1_sidecar.is_none(), "{mode:?}");
            assert_eq!(fresh.files(), GONE, "{mode:?}");
        }

        let hlg = fresh_sidecar(6, Composer::Preset.luminance_mapping());
        let fresh = FreshAnalysis::new(Some(&hlg));
        let (loaded, _) = fresh.load(
            "cuda",
            HdrFormat::Hlg,
            Hdr10PlusMode::Metadata,
            Some(Composer::Preset),
        );
        assert!(!loaded);
        assert_eq!(fresh.files(), GONE);

        // The default hybrid goes on with the HDR10+ metadata alone.
        let fresh = FreshAnalysis::new(Some(&pq));
        let (loaded, measured) = fresh.load_pq("cuda", Hdr10PlusMode::ImplicitHybrid);
        assert!(loaded);
        assert_eq!(measured.hdr10plus_mode, Hdr10PlusMode::Metadata);
        assert_eq!(measured.hdr_type, HdrFormat::Hdr10Plus);
        assert!(measured.measurements_file.is_none() && measured.l1_sidecar.is_none());
        assert_eq!(fresh.files(), GONE);
    }

    #[test]
    fn a_fresh_sidecar_the_decode_route_allows_is_loaded_and_kept() {
        for (version, hwaccel) in [(6, "none"), (7, "cuda"), (7, "none")] {
            let fresh = FreshAnalysis::new(Some(&fresh_sidecar(version, "pq")));
            let (loaded, measured) = fresh.load_pq(hwaccel, Hdr10PlusMode::Metadata);
            assert!(loaded, "v{version} {hwaccel}");
            assert!(measured.l1_sidecar.is_some(), "v{version} {hwaccel}");
            assert_eq!(fresh.files(), KEPT, "v{version} {hwaccel}");
        }
        let mut gpu = fresh_sidecar(6, "pq");
        gpu["analysis"]["gpu"] = json!(true);
        let fresh = FreshAnalysis::new(Some(&gpu));
        assert!(fresh.load_pq("cuda", Hdr10PlusMode::Metadata).0);
        assert_eq!(fresh.files(), KEPT);
    }

    #[test]
    fn other_fresh_sidecar_failures_under_hwaccel_cuda_keep_their_own_refusal() {
        // An HLG sidecar measured through the other composer: the mapping refusal, which an
        // update of the analyzer does not fix, and nothing is removed.
        let hlg = fresh_sidecar(6, Composer::Preset.luminance_mapping());
        let fresh = FreshAnalysis::new(Some(&hlg));
        let (loaded, _) = fresh.load(
            "cuda",
            HdrFormat::Hlg,
            Hdr10PlusMode::Metadata,
            Some(Composer::Bt2100V1),
        );
        assert!(!loaded);
        assert_eq!(fresh.files(), KEPT);

        let missing = FreshAnalysis::new(None);
        assert!(!missing.load_pq("cuda", Hdr10PlusMode::Metadata).0);
        assert_eq!(missing.files(), [true, false, true]);

        let mut invalid = fresh_sidecar(7, "pq");
        invalid["scenes"] = json!([]);
        let fresh = FreshAnalysis::new(Some(&invalid));
        assert!(!fresh.load_pq("cuda", Hdr10PlusMode::Metadata).0);
        assert_eq!(fresh.files(), KEPT);
    }

    #[test]
    fn a_refused_nvdec_analysis_is_not_reused_by_a_retry_with_hwaccel_none() {
        let fresh = FreshAnalysis::new(Some(&fresh_sidecar(6, "pq")));
        let input = fresh.input();
        let none = Args::try_parse_from(["mkvdovi", "--hwaccel", "none"]).unwrap();
        let reused = || {
            let (mut measurements_file, mut l1_sidecar) = (None, None);
            find_reusable_measurements(
                &input,
                input.to_str().unwrap(),
                &none,
                None,
                false,
                &mut measurements_file,
                &mut l1_sidecar,
            )
        };
        // Left in place, the retry would take these measurements as software decode's.
        assert!(reused());

        assert!(!fresh.load_pq("cuda", Hdr10PlusMode::Metadata).0);
        assert_eq!(fresh.files(), GONE);
        assert!(!reused(), "the retry must analyze afresh");
    }
}
