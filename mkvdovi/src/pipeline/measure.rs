//! Detection report and measurements: the phases of `convert_file` between the temp directory
//! and the Dolby Vision configuration.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use dovi84_composer::Composer;

use super::analyzer::{
    print_sidecar_advisories, rejects_coarser_sidecar, report_reused_sidecar, run_hdr_analyzer,
};
use super::dovi_steps::extract_clean_base_layer;
use super::hdr10plus::extract_hdr10plus_metadata;
use super::hybrid;
use super::mdfix_notice::print_mdfix_notice;
use super::{add_optimizer_args, analyzer_executable, is_dolby_vision, FEL_UNSUPPORTED_MESSAGE};
use crate::cli::{Args, Hdr10PlusL1};
use crate::external;
use crate::metadata::{self, HdrFormat};
use crate::progress;
use crate::rpu_check::{self, AuthoredLevels, Level5Offsets};

/// What the measurement phase hands to the configuration: the format after any fallback or
/// `--mdfix` rebuild, and where the L1 comes from.
pub(super) struct Measured {
    pub(super) hdr_type: HdrFormat,
    pub(super) measurements_file: Option<PathBuf>,
    pub(super) hdr10plus_json: Option<PathBuf>,
    pub(super) bl_source_file: PathBuf,
    pub(super) l1_sidecar: Option<metadata::L1Sidecar>,
    /// An HDR10+ input under `--hdr10plus-l1 hybrid`, measured as HDR10 (`hdr_type` is
    /// `Hdr10WithMeasurements`); the configuration replaces the sidecar's scenes with the
    /// hybrid pieces.
    pub(super) hdr10plus_hybrid: bool,
}

/// What the source RPU sample of a Dolby Vision input gives the later phases.
#[derive(Default)]
pub(super) struct Detection {
    /// L5 offsets sampled from the source RPU.
    pub(super) level5: Option<Level5Offsets>,
    /// Authored levels seen in the sample, for the `--mdfix` notice; `None` when the input is not
    /// Dolby Vision or the sample failed.
    pub(super) authored: Option<AuthoredLevels>,
}

/// Step 1: samples a Dolby Vision source RPU and reports the detected format.
pub(super) fn report_detection(
    input_file: &str,
    temp_dir: &Path,
    hdr_type: HdrFormat,
    colour_warnings: &[String],
    custom_composer: Option<Composer>,
) -> Detection {
    progress::print_step(1, 0, "Detecting HDR format...");
    let mut detection = Detection::default();

    if is_dolby_vision(hdr_type) {
        match rpu_check::sample_rpu_windows(input_file, temp_dir) {
            Ok(sample) => {
                detection.level5 = sample.level5;
                detection.authored = Some(sample.authored);
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
    for warning in colour_warnings {
        progress::print_warn(warning);
    }
    if let Some(composer) = custom_composer {
        progress::print_info(&format!(
            "HLG composer: {} ({}); playback tested on an LG OLED C9 only. If a display renders \
             the output wrongly, convert with --hlg-composer preset.",
            composer.cli_name(),
            composer.luminance_mapping()
        ));
    }
    detection
}

/// Refuses an unsupported format after the detection report. The only early return that removes
/// the temp directory. Returns true when `convert_file` must return `Ok(false)`.
pub(super) fn abort_unsupported(hdr_type: HdrFormat, temp_dir: &Path) -> bool {
    if hdr_type == HdrFormat::Unsupported {
        progress::print_error("Unsupported HDR format. Cannot process this file.");
        let _ = fs::remove_dir_all(temp_dir);
        return true;
    }
    false
}

/// Number of steps announced for the detected format.
pub(super) fn total_steps_for(hdr_type: HdrFormat) -> u8 {
    match hdr_type {
        HdrFormat::Hdr10Plus => 7, // detect, extract HEVC, extract meta, config, RPU, inject, mux
        HdrFormat::Hlg => 7,       // detect, analyze, config, RPU, extract BL, inject, mux
        HdrFormat::Hdr10WithMeasurements => 6, // detect, config, RPU, extract BL, inject, mux
        HdrFormat::Hdr10Unsupported => 7, // detect, analyze, config, RPU, extract BL, inject, mux
        HdrFormat::DolbyVisionMel => 7,
        HdrFormat::DolbyVisionFel => 0, // unreachable — refused before the temp dir
        HdrFormat::DolbyVisionP8 => 7,
        HdrFormat::Unsupported => 0, // unreachable — handled above
    }
}

/// Extracts HDR10+ metadata, rebuilds a Dolby Vision base layer under `--mdfix`, and reuses or
/// generates measurements. `None`: the phase printed why; `convert_file` returns `Ok(false)`.
#[expect(clippy::too_many_lines, reason = "E13.14: split by phase")]
pub(super) fn acquire_measurements(
    input_file: &str,
    temp_dir: &Path,
    args: &Args,
    resume_enabled: bool,
    hlg_composer: Option<Composer>,
    hdr_type: HdrFormat,
    authored: Option<&AuthoredLevels>,
    total_steps: &mut u8,
    current_step: &mut u8,
) -> Result<Option<Measured>> {
    let input_path = Path::new(input_file);
    let mut hdr_type = hdr_type;
    let mut measurements_file: Option<PathBuf> = None;
    let mut hdr10plus_json: Option<PathBuf> = None;
    let mut bl_source_file = PathBuf::from(input_file);
    let mut hdr10plus_hybrid = false;

    // Measured L1 sidecar, loaded when existing measurements are reused (validated against the
    // input) or right after a fresh analyzer run.
    let mut l1_sidecar: Option<metadata::L1Sidecar> = None;

    // --- Format-specific metadata extraction ---
    // Pre-handle HDR10+ to allow fallback to HDR10Unsupported if metadata is missing
    if hdr_type == HdrFormat::Hdr10Plus {
        *current_step += 1;
        progress::print_step(
            *current_step,
            *total_steps,
            "Extracting HEVC stream for HDR10+ analysis...",
        );
        match extract_hdr10plus_metadata(input_file, temp_dir, resume_enabled, args.stall_timeout) {
            Ok(Some(json_path)) => hdr10plus_json = Some(json_path),
            Ok(None) => {
                progress::print_warn(
                    "HDR10+ tagged but no dynamic metadata found. Falling back to HDR10 analysis.",
                );
                if args.hdr10plus_l1 == Hdr10PlusL1::Hybrid {
                    progress::print_info(
                        "--hdr10plus-l1 hybrid: without HDR10+ metadata the measured L1 alone is used.",
                    );
                }
                hdr_type = HdrFormat::Hdr10Unsupported;
            }
            Err(_) => return Ok(None),
        }
    }

    // Logic branching
    match hdr_type {
        HdrFormat::Hdr10Plus if args.hdr10plus_l1 == Hdr10PlusL1::Hybrid => {
            // The HDR10+ metadata stays for the peaks; average, minimum and crop are measured.
            // `convert_file` refused a sampling that skips frames before any temp work.
            progress::print_info(
                "--hdr10plus-l1 hybrid: measuring the HDR10+ input for its average, minimum and active area.",
            );
            let reuse = find_reusable_measurements(
                input_path,
                input_file,
                args,
                None,
                true,
                &mut measurements_file,
                &mut l1_sidecar,
            );
            if reuse {
                progress::print_info("Using existing measurements file.");
            } else {
                // Counted as an HDR10 analysis: `total_steps_for(Hdr10Plus)` has no analysis step.
                match run_fresh_analysis(
                    input_file,
                    temp_dir,
                    args,
                    HdrFormat::Hdr10WithMeasurements,
                    false,
                    None,
                    total_steps,
                    current_step,
                )? {
                    Some(measurements) => measurements_file = Some(measurements),
                    None => return Ok(None),
                }
            }
            hdr_type = HdrFormat::Hdr10WithMeasurements;
            hdr10plus_hybrid = true;
        }
        HdrFormat::Hdr10Plus => {
            // Metadata already extracted above
        }
        HdrFormat::DolbyVisionMel => {
            // Without --mdfix, convert_file took the fast path (convert_mel_fast_path) before
            // this phase.
            progress::print_info(
                "Rebuilding Profile 7 MEL metadata from fresh base-layer measurements (--mdfix).",
            );
            print_mdfix_notice(authored, args, input_file);
            let clean_bl = extract_clean_base_layer(
                input_file,
                temp_dir,
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
            measurements_file = run_hdr_analyzer(input_file, temp_dir, &extra_args, args)?;
            if measurements_file.is_none() {
                return Ok(None);
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
                return Ok(None);
            }

            // A Profile 8.4 base layer is HLG; rebuilding it would put a PQ (8.1) RPU on it.
            if metadata::has_hlg_transfer(input_file) {
                progress::print_error("Profile 8.4 (HLG base layer) --mdfix is not supported yet.");
                return Ok(None);
            }

            progress::print_info(
                "Rebuilding Profile 8 metadata from fresh base-layer measurements (--mdfix).",
            );
            print_mdfix_notice(authored, args, input_file);
            let clean_bl = extract_clean_base_layer(
                input_file,
                temp_dir,
                "Extracting Profile 8 HEVC stream",
                args,
                resume_enabled,
            )?;
            bl_source_file = clean_bl.clone();

            let mut extra_args = Vec::new();
            add_optimizer_args(&mut extra_args, args);
            // Same rationale as the MEL path: analyze the container so crop probing can seek.
            measurements_file = run_hdr_analyzer(input_file, temp_dir, &extra_args, args)?;
            if measurements_file.is_none() {
                return Ok(None);
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
                return Ok(None);
            }
            // Reuse existing measurements only when their L1 sidecar is valid for this input
            // (or the user explicitly asked for the legacy optimizer-target path). Every
            // candidate is tried in turn, so a stale shared file cannot shadow a valid one.
            let reuse = find_reusable_measurements(
                input_path,
                input_file,
                args,
                hlg_composer,
                false,
                &mut measurements_file,
                &mut l1_sidecar,
            );

            if reuse {
                if hlg {
                    // HLG's step count assumes a fresh analysis.
                    *total_steps -= 1;
                }
                progress::print_info("Using existing measurements file.");
                if args.boost_experimental {
                    progress::print_warn(
                        "Experimental boost requested, but using existing measurements.",
                    );
                }
            } else {
                match run_fresh_analysis(
                    input_file,
                    temp_dir,
                    args,
                    hdr_type,
                    hlg,
                    hlg_composer,
                    total_steps,
                    current_step,
                )? {
                    Some(measurements) => measurements_file = Some(measurements),
                    None => return Ok(None),
                }
            }
        }
        HdrFormat::Unsupported => {
            // Already handled above
            unreachable!();
        }
    }

    Ok(Some(Measured {
        hdr_type,
        measurements_file,
        hdr10plus_json,
        bl_source_file,
        l1_sidecar,
        hdr10plus_hybrid,
    }))
}

/// Tries every existing measurements candidate in turn and keeps the first whose L1 sidecar is
/// valid for this input (and, with `hybrid`, for `--hdr10plus-l1 hybrid`). Returns true when one
/// is reused.
pub(super) fn find_reusable_measurements(
    input_path: &Path,
    input_file: &str,
    args: &Args,
    hlg_composer: Option<Composer>,
    hybrid: bool,
    measurements_file: &mut Option<PathBuf>,
    l1_sidecar: &mut Option<metadata::L1Sidecar>,
) -> bool {
    let candidates = metadata::measurements_candidates(input_path);
    if args.legacy_madvr_l1 {
        *measurements_file = candidates.into_iter().next();
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
                Ok((sidecar, _)) if rejects_for_hybrid(&sidecar, &existing, hybrid) => {}
                Ok((sidecar, advisories)) => {
                    report_reused_sidecar(&sidecar, &existing, args);
                    print_sidecar_advisories(&advisories);
                    *l1_sidecar = Some(sidecar);
                    *measurements_file = Some(existing);
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
    }
}

/// Under `--hdr10plus-l1 hybrid` (`hybrid`: an HDR10+ input), only measurements that can carry
/// the hybrid's piece averages ([`hybrid::check_measurements`]) are reused; others are
/// re-analyzed. Returns true when the candidate was rejected (and says so).
fn rejects_for_hybrid(sidecar: &metadata::L1Sidecar, measurements: &Path, hybrid: bool) -> bool {
    if !hybrid {
        return false;
    }
    let Err(reason) = hybrid::check_measurements(sidecar) else {
        return false;
    };
    progress::print_warn(&format!(
        "Existing measurements '{}' cannot carry --hdr10plus-l1 hybrid ({reason}); re-analyzing.",
        measurements.display()
    ));
    true
}

/// Runs the analyzer for a fresh measurement. `None`: the phase printed why; `convert_file`
/// returns `Ok(false)`.
fn run_fresh_analysis(
    input_file: &str,
    temp_dir: &Path,
    args: &Args,
    hdr_type: HdrFormat,
    hlg: bool,
    hlg_composer: Option<Composer>,
    total_steps: &mut u8,
    current_step: &mut u8,
) -> Result<Option<PathBuf>> {
    // Generate them
    if hdr_type == HdrFormat::Hdr10WithMeasurements {
        *total_steps += 1;
    }
    *current_step += 1;
    progress::print_step(
        *current_step,
        *total_steps,
        if hlg {
            "Generating measurements (HLG via the Dolby Vision 8.4 mapping)..."
        } else {
            "Generating measurements..."
        },
    );

    let mut extra_args = Vec::new();
    if args.boost_experimental {
        progress::print_info("Experimental boost: using 'aggressive' optimizer.");
        extra_args.extend(["--optimizer-profile".to_string(), "aggressive".to_string()]);
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
        if !external::analyzer_lists_option(&analyzer_executable(), composer.luminance_mapping()) {
            progress::print_error(&format!(
                "--hlg-composer {} needs an hdr_analyzer_mvp that measures HLG \
                 through the spec 4:2:0 decode (sidecar mapping {}), and this \
                 analyzer's --help does not name it; rebuild or update \
                 hdr_analyzer_mvp.",
                composer.cli_name(),
                composer.luminance_mapping()
            ));
            return Ok(None);
        }
        extra_args.extend(["--hlg-composer".to_string(), composer.cli_name().into()]);
    }
    run_hdr_analyzer(input_file, temp_dir, &extra_args, args)
}
