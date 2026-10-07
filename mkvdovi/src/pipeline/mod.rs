use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use colored::Colorize;
use dovi84_composer::Composer;

use crate::cli::{Args, CmVersion};
use crate::external::{self, run_command_with_progress};
use crate::metadata::{self, HdrFormat};
use crate::progress;
use crate::resume;
use crate::rpu_check::{self, Level5Offsets};

mod analyzer;
mod dovi_steps;
mod hdr10plus;

pub use self::analyzer::{analyzer_executable, resolve_auto_settings};
use self::analyzer::{
    print_sidecar_advisories, rejects_coarser_sidecar, report_reused_sidecar, run_hdr_analyzer,
    warn_if_gpu_analysis_missing,
};
pub use self::dovi_steps::DOVI_TOOL_MKV_INPUT_MIN;
use self::dovi_steps::{convert_mel_to_profile81, extract_clean_base_layer, generate_rpu};
use self::hdr10plus::{extract_hdr10plus_metadata, inspect_hdr10plus_scene_peaks};

/// Error for Profile 7 FEL inputs. The BL+EL compositor and its re-encode were removed; FEL is
/// refused until a design that keeps the base layer bit-exact exists (docs/FEL_PLAN.md).
pub(crate) const FEL_UNSUPPORTED_MESSAGE: &str = "Profile 7 FEL input is not supported: the BL+EL compositor was removed because it did not match the Dolby Vision reconstruction specification. A no-re-encode FEL design is planned (docs/FEL_PLAN.md).";

/// Refuse formats mkvdovi detects but cannot convert. Called before any temp-directory work,
/// so a refused input leaves no artifacts behind and an old temp directory stays untouched.
fn reject_unsupported_input(hdr_type: HdrFormat) -> Result<()> {
    if hdr_type == HdrFormat::DolbyVisionFel {
        bail!(FEL_UNSUPPORTED_MESSAGE);
    }
    Ok(())
}

pub fn convert_file(input_file: &str, args: &Args) -> Result<bool> {
    let input_path = Path::new(input_file);
    if !input_path.exists() {
        progress::print_warn(&format!("Input file not found: {}", input_file));
        return Ok(false);
    }

    // Output filename: name.DV.mkv. A repair of an existing `name.DV.mkv` gets a distinct,
    // deterministic name so the source and rebuilt candidate can coexist for A/B testing.
    let stem = input_path.file_stem().unwrap().to_string_lossy();
    let dir = input_path.parent().unwrap_or(Path::new("."));
    let output_file = output_path_for(input_path, args.mdfix);

    let resume_enabled = !args.no_resume;
    let temp_dir_name = format!("mkvdovi_temp_{}", stem);
    let mut temp_dir = dir.join(&temp_dir_name);
    // Pre-rename compat (mkvdolby -> mkvdovi in v0.3.0): resume from a leftover
    // `mkvdolby_temp_*` directory when no new-style one exists. Such directories predate resume
    // fingerprints, so they resume through the missing-fingerprint path below.
    if resume_enabled && !temp_dir.exists() {
        let legacy_temp_dir = dir.join(format!("mkvdolby_temp_{}", stem));
        if legacy_temp_dir.exists() {
            temp_dir = legacy_temp_dir;
        }
    }

    // With no leftover temp dir, an existing output means the file is already converted. This is
    // checked before format detection so that a finished batch is skipped without probing.
    let warn_output_exists = || {
        progress::print_warn(&format!(
            "Output file '{}' already exists. Skipping.",
            output_file.display()
        ));
    };
    if output_file.exists() && !(resume_enabled && temp_dir.exists()) {
        warn_output_exists();
        return Ok(true);
    }

    // Detect the format before the temp directory is created, resumed or discarded: an input
    // that is refused (Profile 7 FEL, also under --mdfix) must not cause any temp work.
    let detected_hdr_type = metadata::check_hdr_format(input_file);
    reject_unsupported_input(detected_hdr_type)?;

    // HLG becomes Profile 8.4 with the base layer copied unchanged, so a stream tagged with
    // colorimetry the RPU cannot describe is refused here, before any temp work or extraction
    // (the hevc_metadata filter in the base-layer step writes a BT.2020 NCL matrix tag).
    let hlg_composer = (detected_hdr_type == HdrFormat::Hlg).then_some(args.hlg_composer);
    let colour_warnings = if hlg_composer.is_some() {
        match metadata::check_hlg_colour_contract(input_file) {
            Ok(warnings) => warnings,
            Err(refusal) => {
                progress::print_error(&refusal);
                return Ok(false);
            }
        }
    } else {
        Vec::new()
    };
    let custom_composer = hlg_composer.filter(|&composer| composer != Composer::Preset);

    // A leftover temp dir means a previous run for this file was interrupted. With resume
    // enabled we reuse its completed steps when it was created for this exact input and these
    // settings. A directory with no fingerprint was left by an older mkvdovi, so it resumes with
    // a warning, unless a non-preset HLG composer is selected (its RPU.bin would carry the
    // preset). A directory with a different fingerprint is discarded.
    let fingerprint =
        resume::Fingerprint::for_input(input_path, resume_settings(args, hlg_composer)).ok();
    let mut resuming = resume_enabled && temp_dir.exists();
    if resuming && resume::is_legacy_hlg_dir(&temp_dir) {
        // Only the removed HLG-to-PQ (Profile 8.1) path wrote HLG_to_PQ.mkv. Its PQ base layer and
        // 8.1 RPU do not fit the Profile 8.4 conversion that replaced it, whatever the fingerprint
        // says (v0.4.0 already wrote fingerprints for those directories).
        progress::print_info(&format!(
            "Leftover temp dir '{}' comes from the removed HLG-to-PQ (Profile 8.1) conversion; starting clean.",
            temp_dir.display()
        ));
        let _ = fs::remove_dir_all(&temp_dir);
        temp_dir = dir.join(&temp_dir_name);
        resuming = false;
    }
    if resuming {
        let status = fingerprint
            .as_ref()
            .map_or(resume::FingerprintStatus::Differs, |current| {
                current.check(&temp_dir)
            });
        match status {
            resume::FingerprintStatus::Matches => {}
            resume::FingerprintStatus::Missing if custom_composer.is_some() => {
                progress::print_warn(&format!(
                    "Leftover temp dir '{}' has no resume fingerprint (created by an older mkvdovi), so its RPU may carry another HLG composer than --hlg-composer {}; starting clean.",
                    temp_dir.display(),
                    args.hlg_composer.cli_name()
                ));
                let _ = fs::remove_dir_all(&temp_dir);
                temp_dir = dir.join(&temp_dir_name);
                resuming = false;
            }
            resume::FingerprintStatus::Missing => progress::print_warn(&format!(
                "Leftover temp dir '{}' has no resume fingerprint (created by an older mkvdovi); resuming its completed steps, which may carry metadata from that version. Pass --no-resume to start clean.",
                temp_dir.display()
            )),
            resume::FingerprintStatus::Differs => {
                progress::print_warn(&format!(
                    "Leftover temp dir '{}' was created for a different input, settings, or mkvdovi version; starting clean.",
                    temp_dir.display()
                ));
                let _ = fs::remove_dir_all(&temp_dir);
                temp_dir = dir.join(&temp_dir_name);
                resuming = false;
            }
        }
    }

    if output_file.exists() && !resuming {
        warn_output_exists();
        return Ok(true);
    }

    // --- File header ---
    let display_name = input_path.file_name().unwrap_or_default().to_string_lossy();
    if !progress::is_quiet() {
        eprintln!(
            "\n{}",
            format!("━━━ Processing: {} ━━━", display_name)
                .cyan()
                .bold()
        );
    }

    let file_start = Instant::now();

    // Create (or reset) the temp directory.
    if temp_dir.exists() && !resuming {
        let _ = fs::remove_dir_all(&temp_dir);
    }
    fs::create_dir_all(&temp_dir).context("Failed to create temp directory")?;
    if let Some(fingerprint) = &fingerprint {
        fingerprint
            .write(&temp_dir)
            .context("Failed to write resume fingerprint")?;
    }
    if resuming {
        progress::print_info("Resuming from a previous run — completed steps will be reused.");
    }

    // --- Step 1: Detect HDR format ---
    progress::print_step(1, 0, "Detecting HDR format...");
    let mut hdr_type = detected_hdr_type;
    let original_hdr_type = hdr_type;
    let mut measurements_file: Option<PathBuf> = None;
    let mut hdr10plus_json: Option<PathBuf> = None;
    let mut bl_source_file = PathBuf::from(input_file);
    let mut level5_offsets: Option<Level5Offsets> = None;

    if is_dolby_vision(hdr_type) {
        match rpu_check::sample_rpu_windows(input_file, &temp_dir) {
            Ok(sample) => {
                level5_offsets = sample.level5;
                if let Ok(report) =
                    rpu_check::analyze_rpus(sample.l1_frames, sample.mastering_peak_pq)
                {
                    if let Some(detail) = rpu_check::warning_summary(&report) {
                        progress::print_warn(&format!(
                            "DV metadata looks unreliable ({}); consider --mdfix. \
                             (sampled {} windows; run `inspect` for a full check)",
                            detail, sample.windows_sampled
                        ));
                    }
                }
            }
            Err(error) => progress::print_warn(&format!(
                "Could not inspect source Dolby Vision RPU: {error}"
            )),
        }
    }

    let format_label = match hdr_type {
        HdrFormat::Hdr10Plus => "HDR10+",
        HdrFormat::Hlg => "HLG",
        HdrFormat::Hdr10WithMeasurements => "HDR10 (measurements found)",
        HdrFormat::Hdr10Unsupported => "HDR10",
        HdrFormat::DolbyVisionMel => "Dolby Vision Profile 7 MEL",
        HdrFormat::DolbyVisionFel => "Dolby Vision Profile 7 FEL",
        HdrFormat::DolbyVisionP8 => "Dolby Vision Profile 8",
        HdrFormat::Unsupported => "Unsupported",
    };
    progress::print_info(&format!("Detected: {}", format_label));
    for warning in &colour_warnings {
        progress::print_warn(warning);
    }
    if let Some(composer) = custom_composer {
        progress::print_info(&format!(
            "HLG composer: {} ({}); not yet confirmed on playback devices. If a display renders \
             the output wrongly, convert with --hlg-composer preset.",
            composer.cli_name(),
            composer.luminance_mapping()
        ));
    }

    if hdr_type == HdrFormat::Unsupported {
        progress::print_error("Unsupported HDR format. Cannot process this file.");
        let _ = fs::remove_dir_all(&temp_dir);
        return Ok(false);
    }

    // Compute total steps based on the detected format
    let mut total_steps: u8 = match hdr_type {
        HdrFormat::Hdr10Plus => 7, // detect, extract HEVC, extract meta, config, RPU, inject, mux
        HdrFormat::Hlg => 7,       // detect, analyze, config, RPU, extract BL, inject, mux
        HdrFormat::Hdr10WithMeasurements => 6, // detect, config, RPU, extract BL, inject, mux
        HdrFormat::Hdr10Unsupported => 7, // detect, analyze, config, RPU, extract BL, inject, mux
        HdrFormat::DolbyVisionMel => 7,
        HdrFormat::DolbyVisionFel => 0, // unreachable — refused before the temp dir
        HdrFormat::DolbyVisionP8 => 7,
        HdrFormat::Unsupported => 0, // unreachable — handled above
    };

    let mut current_step: u8 = 1; // Step 1 (detect) already done

    // Measured L1 sidecar, loaded when existing measurements are reused (validated against the
    // input) or right after a fresh analyzer run.
    let mut l1_sidecar: Option<metadata::L1Sidecar> = None;

    // --- Format-specific metadata extraction ---
    // Pre-handle HDR10+ to allow fallback to HDR10Unsupported if metadata is missing
    if hdr_type == HdrFormat::Hdr10Plus {
        current_step += 1;
        progress::print_step(
            current_step,
            total_steps,
            "Extracting HEVC stream for HDR10+ analysis...",
        );
        match extract_hdr10plus_metadata(input_file, &temp_dir, resume_enabled, args.stall_timeout)
        {
            Ok(Some(json_path)) => hdr10plus_json = Some(json_path),
            Ok(None) => {
                progress::print_warn(
                    "HDR10+ tagged but no dynamic metadata found. Falling back to HDR10 analysis.",
                );
                hdr_type = HdrFormat::Hdr10Unsupported;
            }
            Err(_) => return Ok(false),
        }
    }

    // Logic branching
    match hdr_type {
        HdrFormat::Hdr10Plus => {
            // Metadata already extracted above
        }
        HdrFormat::DolbyVisionMel => {
            if !args.mdfix {
                progress::print_info(
                    "Converting Profile 7 MEL to Profile 8.1 without rebuilding metadata.",
                );
                let success = convert_mel_to_profile81(
                    input_file,
                    &temp_dir,
                    &output_file,
                    args,
                    resume_enabled,
                )?;
                if success && args.verify {
                    progress::print_info("Running post-mux verification (--verify)...");
                    if !crate::verify::verify_post_mux(input_file, &output_file, None, &temp_dir) {
                        progress::print_error("Inconsistencies detected during verification.");
                        return Ok(false);
                    }
                }
                if success {
                    finish_success(
                        input_file,
                        &output_file,
                        &temp_dir,
                        args,
                        original_hdr_type,
                        file_start,
                    );
                }
                return Ok(success);
            }

            progress::print_info(
                "Rebuilding Profile 7 MEL metadata from fresh base-layer measurements (--mdfix).",
            );
            let clean_bl = extract_clean_base_layer(
                input_file,
                &temp_dir,
                "Extracting Dolby Vision HEVC stream",
                args,
                resume_enabled,
            )?;
            bl_source_file = clean_bl.clone();

            let mut extra_args = Vec::new();
            add_optimizer_args(&mut extra_args, args);
            // Analyze the original container, not the raw Annex B stream: without a container
            // duration the seek-based crop probes are unavailable and the in-stream fallback
            // can commit a degenerate active area on dark openings. MEL base-layer pixels are
            // identical in the source.
            measurements_file = run_hdr_analyzer(input_file, &temp_dir, &extra_args, args)?;
            if measurements_file.is_none() {
                return Ok(false);
            }
            hdr_type = HdrFormat::Hdr10WithMeasurements;
        }
        // Refused by reject_unsupported_input before the temp directory was touched.
        HdrFormat::DolbyVisionFel => bail!(FEL_UNSUPPORTED_MESSAGE),
        HdrFormat::DolbyVisionP8 => {
            if !args.mdfix {
                progress::print_warn(
                    "Profile 8 input is already converted; use `inspect` or --mdfix to rebuild metadata.",
                );
                return Ok(false);
            }

            // A Profile 8.4 base layer is HLG; rebuilding it would put a PQ (8.1) RPU on it.
            if metadata::has_hlg_transfer(input_file) {
                progress::print_error("Profile 8.4 (HLG base layer) --mdfix is not supported yet.");
                return Ok(false);
            }

            progress::print_info(
                "Rebuilding Profile 8 metadata from fresh base-layer measurements (--mdfix).",
            );
            let clean_bl = extract_clean_base_layer(
                input_file,
                &temp_dir,
                "Extracting Profile 8 HEVC stream",
                args,
                resume_enabled,
            )?;
            bl_source_file = clean_bl.clone();

            let mut extra_args = Vec::new();
            add_optimizer_args(&mut extra_args, args);
            // Same rationale as the MEL path: analyze the container so crop probing can seek.
            measurements_file = run_hdr_analyzer(input_file, &temp_dir, &extra_args, args)?;
            if measurements_file.is_none() {
                return Ok(false);
            }
            hdr_type = HdrFormat::Hdr10WithMeasurements;
        }
        HdrFormat::Hdr10WithMeasurements | HdrFormat::Hdr10Unsupported | HdrFormat::Hlg => {
            // HLG goes to Profile 8.4: the base layer is the untouched source, and L1 must be
            // measured through the Dolby Vision 8.4 reconstruction of the selected composer
            // (sidecar v3+, `Composer::luminance_mapping`).
            let hlg = hdr_type == HdrFormat::Hlg;
            if hlg && args.legacy_madvr_l1 {
                progress::print_error(
                    "--legacy-madvr-l1 is not supported for HLG input: Profile 8.4 needs L1 measured through the Dolby Vision 8.4 mapping.",
                );
                return Ok(false);
            }
            // Reuse existing measurements only when their L1 sidecar is valid for this input
            // (or the user explicitly asked for the legacy optimizer-target path). Every
            // candidate is tried in turn, so a stale shared file cannot shadow a valid one.
            let candidates = metadata::measurements_candidates(input_path);
            let reuse = if args.legacy_madvr_l1 {
                measurements_file = candidates.into_iter().next();
                measurements_file.is_some()
            } else if candidates.is_empty() {
                false
            } else {
                let expect = metadata::SidecarExpectation::for_input(
                    input_path,
                    metadata::get_frame_count(input_file),
                )
                .with_hlg_composer(hlg_composer);
                let mut reused = false;
                for existing in candidates {
                    match metadata::load_l1_sidecar(&existing, &expect) {
                        Ok((sidecar, _)) if rejects_coarser_sidecar(&sidecar, &existing, args) => {}
                        Ok((sidecar, advisories)) => {
                            report_reused_sidecar(&sidecar, &existing, args);
                            print_sidecar_advisories(&advisories);
                            l1_sidecar = Some(sidecar);
                            measurements_file = Some(existing);
                            reused = true;
                            break;
                        }
                        Err(error) => progress::print_warn(&format!(
                            "Existing measurements '{}' cannot be reused ({error}).",
                            existing.display()
                        )),
                    }
                }
                if !reused {
                    progress::print_info("No reusable measurements found; re-running analysis.");
                }
                reused
            };

            if reuse {
                if hlg {
                    // HLG's step count assumes a fresh analysis.
                    total_steps -= 1;
                }
                progress::print_info("Using existing measurements file.");
                if args.boost_experimental {
                    progress::print_warn(
                        "Experimental boost requested, but using existing measurements.",
                    );
                }
            } else {
                // Generate them
                if hdr_type == HdrFormat::Hdr10WithMeasurements {
                    total_steps += 1;
                }
                current_step += 1;
                progress::print_step(
                    current_step,
                    total_steps,
                    if hlg {
                        "Generating measurements (HLG via the Dolby Vision 8.4 mapping)..."
                    } else {
                        "Generating measurements..."
                    },
                );

                let mut extra_args = Vec::new();
                if args.boost_experimental {
                    progress::print_info("Experimental boost: using 'aggressive' optimizer.");
                    extra_args
                        .extend(["--optimizer-profile".to_string(), "aggressive".to_string()]);
                } else {
                    add_optimizer_args(&mut extra_args, args);
                }
                if hlg {
                    // MediaInfo classified the input as HLG; the analyzer's linked FFmpeg may not
                    // see the tag (e.g. HLG only in the MKV colour element), so state it.
                    extra_args.extend(["--transfer".to_string(), "hlg".to_string()]);
                }
                if let Some(composer) = hlg_composer {
                    // Always name the composer: the analyzer's own default may differ from the
                    // one this run writes. The analyzer's --help names the luminance mappings
                    // it writes (a cross-binary contract, like the +cuda version probe); one
                    // that does not list this composer's predates the spec 4:2:0 HLG decode,
                    // and its measurement would be refused after a full analysis.
                    if !external::analyzer_lists_option(
                        &analyzer_executable(),
                        composer.luminance_mapping(),
                    ) {
                        progress::print_error(&format!(
                            "--hlg-composer {} needs an hdr_analyzer_mvp that measures HLG \
                             through the spec 4:2:0 decode (sidecar mapping {}), and this \
                             analyzer's --help does not name it; rebuild or update \
                             hdr_analyzer_mvp.",
                            composer.cli_name(),
                            composer.luminance_mapping()
                        ));
                        return Ok(false);
                    }
                    extra_args.extend(["--hlg-composer".to_string(), composer.cli_name().into()]);
                }
                measurements_file = run_hdr_analyzer(input_file, &temp_dir, &extra_args, args)?;
                if measurements_file.is_none() {
                    return Ok(false);
                }
            }
        }
        HdrFormat::Unsupported => {
            // Already handled above
            unreachable!();
        }
    }

    // --- Configuration step ---
    current_step += 1;
    progress::print_step(
        current_step,
        total_steps,
        "Preparing Dolby Vision configuration...",
    );

    // Static Metadata
    // MaxCLL / MaxFALL are resolved after the L1 sidecar is loaded: measured values fill what the
    // source does not state.
    let hlg_source = hdr_type == HdrFormat::Hlg;
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
    if let (Some(metadata_path), Some(&max_dml)) =
        (hdr10plus_json.as_deref(), static_meta.get("max_dml"))
    {
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
    let final_trims: Vec<u32> = args
        .trim_targets
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    // Source-honest per-scene L1 from the analyzer sidecar is mandatory for measurement-based
    // generation. dovi_tool's madVR path fills L1 avg with a fixed placeholder and (with custom
    // targets) replaces L1 max with optimizer targets, so it is reachable only by explicit opt-in.
    if let Some(measurements) = measurements_file.as_deref() {
        if args.legacy_madvr_l1 {
            l1_sidecar = None;
            progress::print_warn(
                "--legacy-madvr-l1: L1 max comes from optimizer targets and L1 avg is a placeholder, not measured values.",
            );
        } else if l1_sidecar.is_none() {
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
                    l1_sidecar = Some(sidecar);
                }
                Err(error) if hdr_type == HdrFormat::Hlg => {
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
                    return Ok(false);
                }
                Err(error) => {
                    progress::print_error(&format!(
                        "The analyzer's L1 sidecar is unusable ({error}). Refusing to fall back to optimizer-derived L1; pass --legacy-madvr-l1 to force it."
                    ));
                    return Ok(false);
                }
            }
        }
    }
    if let Some(sidecar) = &l1_sidecar {
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

    let dv_profile =
        match metadata::dv_profile_for(hdr_type, l1_sidecar.as_ref(), args.hlg_composer) {
            Ok(profile) => profile,
            Err(error) => {
                progress::print_error(&format!(
                    "Cannot choose the Dolby Vision profile: {error:#}"
                ));
                return Ok(false);
            }
        };
    if dv_profile != "8.1" {
        progress::print_info(&format!(
            "Generating Dolby Vision Profile {dv_profile} (base layer kept bit-exact)."
        ));
    }

    for message in metadata::resolve_light_levels(&mut static_meta, l1_sidecar.as_ref(), hlg_source)
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
        l1_sidecar.as_ref(),
    )?;
    progress::print_info("Configuration written.");
    // The resume fingerprint does not cover the measurements. An RPU built from an earlier
    // configuration (for example before a re-analysis changed L1) must not be reused.
    if resume_enabled && previous_config != fs::read(&extra_json_path).ok() {
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
    }

    // --- Generate RPU ---
    current_step += 1;
    progress::print_step(current_step, total_steps, "Generating Dolby Vision RPU...");
    let rpu_path = generate_rpu(
        hdr_type,
        &temp_dir,
        args.peak_source,
        hdr10plus_json.as_deref(),
        measurements_file.as_deref(),
        l1_sidecar.is_some(),
        custom_composer,
        resume_enabled,
    )?;

    if rpu_path.is_none() {
        return Ok(false);
    }
    let rpu_path = rpu_path.unwrap();

    // --- Extract base layer ---
    current_step += 1;
    progress::print_step(current_step, total_steps, "Extracting base layer...");

    // When the BL source is already a raw Annex B HEVC stream in the temp dir (mdfix paths),
    // re-extracting it with ffmpeg would just duplicate ~the full video size on disk.
    let bl_hevc = if bl_source_file.extension().is_some_and(|ext| ext == "hevc") {
        progress::print_info("Base layer is already a raw HEVC stream; skipping re-extraction.");
        bl_source_file.clone()
    } else {
        let bl_hevc = temp_dir.join("BL.hevc");

        if resume_enabled && resume::is_complete(&bl_hevc) {
            progress::print_info("Reusing extracted base layer from a previous run.");
        } else {
            let mut ffmpeg_cmd = Command::new("ffmpeg");
            ffmpeg_cmd.args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-stats",
                "-i",
                bl_source_file.to_str().unwrap(),
                "-map",
                "0:v:0",
                "-c:v",
                "copy",
            ]);
            if hdr_type == HdrFormat::Hlg && metadata::hlg_bitstream_lacks_transfer(input_file) {
                // Lossless SPS/VUI edit (slices untouched): without an HLG tag in the bitstream
                // mkvmerge would give the Profile 8.4 track compatibility ID 2 (SDR) instead of 4.
                progress::print_info(
                    "HLG is tagged only in the container; writing the HLG transfer into the HEVC VUI.",
                );
                ffmpeg_cmd.args([
                    "-bsf:v",
                    "hevc_metadata=colour_primaries=9:transfer_characteristics=18:matrix_coefficients=9",
                ]);
            }
            ffmpeg_cmd.args(["-f", "hevc", "-y", bl_hevc.to_str().unwrap()]);

            let bl_total = fs::metadata(&bl_source_file).ok().map(|m| m.len());
            if !run_command_with_progress(
                &mut ffmpeg_cmd,
                &temp_dir.join("ffmpeg_extract_bl.log"),
                "Extracting base layer HEVC stream",
                &bl_hevc,
                bl_total,
                args.stall_timeout,
            )? {
                return Ok(false);
            }
            resume::mark_done(&bl_hevc)?;
        }

        bl_hevc
    };

    // --- Inject RPU ---
    current_step += 1;
    progress::print_step(
        current_step,
        total_steps,
        "Injecting RPU into base layer...",
    );
    let bl_rpu_hevc = temp_dir.join("BL_RPU.hevc");

    if resume_enabled && resume::is_complete(&bl_rpu_hevc) {
        progress::print_info("Reusing RPU-injected base layer from a previous run.");
    } else {
        let mut dovi_cmd = external::dovi_tool_command();

        dovi_cmd.args([
            "inject-rpu",
            "-i",
            bl_hevc.to_str().unwrap(),
            "--rpu-in",
            rpu_path.to_str().unwrap(),
            "-o",
            bl_rpu_hevc.to_str().unwrap(),
        ]);

        let inject_total = fs::metadata(&bl_hevc).ok().map(|m| m.len());
        if !run_command_with_progress(
            &mut dovi_cmd,
            &temp_dir.join("dovi_inject.log"),
            "Injecting RPU into base layer",
            &bl_rpu_hevc,
            inject_total,
            args.stall_timeout,
        )? {
            return Ok(false);
        }
        // dovi_tool repeats or drops RPUs at the end when the RPU and the stream differ in
        // length, so measurements no longer line up with the pictures they were taken from.
        let inject_log = fs::read_to_string(temp_dir.join("dovi_inject.log")).unwrap_or_default();
        if let Some(line) = inject_log
            .lines()
            .find(|line| line.contains("mismatched lengths"))
        {
            if measurements_file.is_some() {
                progress::print_error(&format!(
                    "dovi_tool inject-rpu: {}. The measured RPU does not match the video picture for picture; refusing to mux it.",
                    line.trim().trim_start_matches("Warning: ")
                ));
                let _ = fs::remove_file(&bl_rpu_hevc);
                return Ok(false);
            }
            progress::print_warn(&format!("dovi_tool inject-rpu: {}", line.trim()));
        }
        resume::mark_done(&bl_rpu_hevc)?;

        // The injected stream supersedes its temp-dir input; drop it early so the mux step
        // does not need both full-size streams on disk. Resume never re-reads it once
        // BL_RPU.hevc is sealed. Inputs outside the temp dir (e.g. the source MKV) are kept.
        if bl_hevc.starts_with(&temp_dir) {
            let _ = fs::remove_file(&bl_hevc);
        }
    }

    // --- Mux ---
    current_step += 1;
    progress::print_step(current_step, total_steps, "Muxing final MKV...");
    // The mux sentinel lives inside the temp dir (the output file is outside it), so cleanup
    // removes it and no stray marker is left beside the final `.DV.mkv`.
    let mux_marker = temp_dir.join("mux.done");

    if resume_enabled && output_file.exists() && mux_marker.exists() {
        progress::print_info("Reusing muxed output from a previous run.");
    } else {
        let mut mkvmerge_cmd = Command::new("mkvmerge");
        mkvmerge_cmd.arg("-q").arg("-o").arg(&output_file);
        if args.drop_tags {
            mkvmerge_cmd.arg("--no-global-tags");
        }
        if args.drop_chapters {
            mkvmerge_cmd.arg("--no-chapters");
        }

        mkvmerge_cmd.arg(&bl_rpu_hevc);
        mkvmerge_cmd.arg("--no-video").arg(input_file);

        let mux_total = fs::metadata(input_file).ok().map(|m| m.len());
        if !run_command_with_progress(
            &mut mkvmerge_cmd,
            &temp_dir.join("mkvmerge.log"),
            "Muxing final MKV",
            &output_file,
            mux_total,
            args.stall_timeout,
        )? {
            return Ok(false);
        }
        let _ = fs::write(&mux_marker, b"");
    }

    // --- Optional verification ---
    if args.verify {
        progress::print_info("Running post-mux verification (--verify)...");
        let measurements_file_path = measurements_file.clone();
        let expected_cm = match args.cm_version {
            CmVersion::V40 => Some("V40"),
            CmVersion::V29 => None,
        };
        let ok = crate::verify::verify_post_mux_with_options(
            input_file,
            &output_file,
            measurements_file_path.as_deref(),
            &temp_dir,
            expected_cm,
            hlg_composer,
            Some(crate::verify::DeliveryExpectation {
                // dovi_tool takes L1 from the HDR10+ metadata when it is given, never the shots.
                l1_sidecar: l1_sidecar
                    .as_ref()
                    .filter(|_| hdr_type != HdrFormat::Hdr10Plus),
                source_range,
            }),
        );
        if !ok {
            progress::print_error("Inconsistencies detected during verification.");
            return Ok(false);
        }
        progress::print_info("Verification passed.");
    }

    // --- Cleanup ---
    let _ = fs::remove_dir_all(&temp_dir);

    // Dolby Vision and metadata-repair inputs are preservation-first: keep the source unless the
    // user is converting a non-DV input without --keep-source.
    if should_keep_source(args, original_hdr_type) {
        if !args.keep_source {
            progress::print_info("Keeping source file (Dolby Vision/--mdfix safety default).");
        }
    } else {
        progress::print_info(&format!("Deleting source file: {}", display_name));
        if let Err(e) = fs::remove_file(input_file) {
            progress::print_warn(&format!("Failed to delete source file: {}", e));
        }
    }

    // --- Success ---
    let elapsed = file_start.elapsed();
    let elapsed_str = progress::format_duration_pub(elapsed);
    if !progress::is_quiet() {
        eprintln!(
            "\n{}",
            format!(
                "✓ Done: {} ({})",
                output_file.file_name().unwrap().to_string_lossy(),
                elapsed_str
            )
            .green()
            .bold()
        );
    }
    Ok(true)
}

/// Settings that change the artifacts a temp directory holds. Part of the resume fingerprint.
///
/// The removed `--hlg-peak-nits` flag left no token: the fingerprint also records the mkvdovi
/// version, so v0.4.0 temp dirs are discarded anyway. Fingerprint-less legacy HLG temp dirs are
/// discarded separately by `resume::is_legacy_hlg_dir`.
///
/// `hlg_composer` is `Some` for HLG inputs. The composer changes `RPU.bin` but not `extra.json`, so
/// the fingerprint must carry it. It is appended only when it is not the preset: a fingerprint is
/// compared as a whole, so an unconditional token would discard every temp dir left by an earlier
/// build of this version, and the composer is irrelevant for other inputs. The token is the
/// sidecar mapping name, so a decode change (dovi84-bt2100-v1 to -spec420) discards bt2100 temp
/// dirs too; preset temp dirs rely on the `extra.json` comparison, which withdraws the RPU when
/// the re-analyzed L1 differs (the RPU's composer is unchanged).
fn resume_settings(args: &Args, hlg_composer: Option<Composer>) -> String {
    let mut settings = format!(
        "hwaccel={:?} analysis_quality={:?} optimizer={:?} boost={} boost_experimental={} cm={:?} content_type={:?} reference_mode={} source_primaries={:?} trim_targets={} peak_source={:?} mdfix={} legacy_madvr_l1={}",
        args.hwaccel,
        args.analysis_quality,
        args.optimizer_profile,
        args.boost,
        args.boost_experimental,
        args.cm_version,
        args.content_type,
        args.reference_mode,
        args.source_primaries,
        args.trim_targets,
        args.peak_source,
        args.mdfix,
        args.legacy_madvr_l1,
    );
    if let Some(composer) = hlg_composer.filter(|&composer| composer != Composer::Preset) {
        settings.push_str(&format!(" hlg_composer={}", composer.luminance_mapping()));
    }
    settings
}

fn output_path_for(input_path: &Path, mdfix: bool) -> PathBuf {
    let stem = input_path.file_stem().unwrap().to_string_lossy();
    let dir = input_path.parent().unwrap_or(Path::new("."));
    if mdfix {
        let base = stem.strip_suffix(".DV").unwrap_or(&stem);
        dir.join(format!("{}.mdfix.DV.mkv", base))
    } else {
        dir.join(format!("{}.DV.mkv", stem))
    }
}

fn add_optimizer_args(args_vec: &mut Vec<String>, args: &Args) {
    args_vec.push("--optimizer-profile".to_string());
    args_vec.push(args.optimizer_profile.to_string());
}

fn finish_success(
    input_file: &str,
    output_file: &Path,
    temp_dir: &Path,
    args: &Args,
    original_hdr_type: HdrFormat,
    started_at: Instant,
) {
    let _ = fs::remove_dir_all(temp_dir);
    if should_keep_source(args, original_hdr_type) {
        if !args.keep_source {
            progress::print_info("Keeping source file (Dolby Vision/--mdfix safety default).");
        }
    } else if let Err(error) = fs::remove_file(input_file) {
        progress::print_warn(&format!("Failed to delete source file: {error}"));
    }

    if !progress::is_quiet() {
        eprintln!(
            "\n{}",
            format!(
                "✓ Done: {} ({})",
                output_file.file_name().unwrap().to_string_lossy(),
                progress::format_duration_pub(started_at.elapsed())
            )
            .green()
            .bold()
        );
    }
}

fn should_keep_source(args: &Args, original_hdr_type: HdrFormat) -> bool {
    args.keep_source || args.mdfix || is_dolby_vision(original_hdr_type)
}

fn is_dolby_vision(hdr_type: HdrFormat) -> bool {
    matches!(
        hdr_type,
        HdrFormat::DolbyVisionMel | HdrFormat::DolbyVisionFel | HdrFormat::DolbyVisionP8
    )
}

#[cfg(test)]
mod tests;
