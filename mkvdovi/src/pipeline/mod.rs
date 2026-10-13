use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{bail, Result};
use colored::Colorize;
use dovi84_composer::Composer;

use crate::cli::{Args, CmVersion};
use crate::external::{self, run_command_with_progress};
use crate::metadata::{self, HdrFormat};
use crate::progress;
use crate::resume;

mod analyzer;
mod config;
mod dovi_steps;
mod hdr10plus;
mod measure;
mod temp_dir;

pub use self::analyzer::{analyzer_executable, resolve_auto_settings};
use self::config::{prepare_configuration, Configured};
pub use self::dovi_steps::DOVI_TOOL_MKV_INPUT_MIN;
use self::dovi_steps::{convert_mel_fast_path, generate_rpu};
use self::measure::{abort_unsupported, acquire_measurements, report_detection, total_steps_for};
use self::temp_dir::{locate_temp_dir, prepare_temp_dir, settle_leftover_temp_dir};

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
    let temp_dir = locate_temp_dir(dir, &temp_dir_name, &stem, resume_enabled);

    // With no leftover temp dir, an existing output means the file is already converted. This is
    // checked before format detection so that a finished batch is skipped without probing.
    if output_file.exists() && !(resume_enabled && temp_dir.exists()) {
        warn_output_exists(&output_file);
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

    let fingerprint =
        resume::Fingerprint::for_input(input_path, resume_settings(args, hlg_composer)).ok();
    let (temp_dir, resuming) = settle_leftover_temp_dir(
        temp_dir,
        dir.join(&temp_dir_name),
        resume_enabled,
        fingerprint.as_ref(),
        custom_composer,
        args.hlg_composer,
    );

    if output_file.exists() && !resuming {
        warn_output_exists(&output_file);
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
    prepare_temp_dir(&temp_dir, resuming, fingerprint.as_ref())?;

    // --- Step 1: Detect HDR format ---
    let original_hdr_type = detected_hdr_type;
    let level5_offsets = report_detection(
        input_file,
        &temp_dir,
        detected_hdr_type,
        &colour_warnings,
        custom_composer,
    );

    if abort_unsupported(detected_hdr_type, &temp_dir) {
        return Ok(false);
    }

    if detected_hdr_type == HdrFormat::DolbyVisionMel && !args.mdfix {
        return convert_mel_fast_path(
            input_file,
            &temp_dir,
            &output_file,
            args,
            resume_enabled,
            original_hdr_type,
            file_start,
        );
    }

    // Compute total steps based on the detected format
    let mut total_steps = total_steps_for(detected_hdr_type);

    let mut current_step: u8 = 1; // Step 1 (detect) already done

    let Some(mut measured) = acquire_measurements(
        input_file,
        &temp_dir,
        args,
        resume_enabled,
        hlg_composer,
        detected_hdr_type,
        &mut total_steps,
        &mut current_step,
    )?
    else {
        return Ok(false);
    };

    // --- Configuration step ---
    current_step += 1;
    progress::print_step(
        current_step,
        total_steps,
        "Preparing Dolby Vision configuration...",
    );

    let Some(Configured { source_range }) = prepare_configuration(
        input_file,
        &temp_dir,
        args,
        resume_enabled,
        hlg_composer,
        &mut measured,
        level5_offsets,
    )?
    else {
        return Ok(false);
    };

    // --- Generate RPU ---
    current_step += 1;
    progress::print_step(current_step, total_steps, "Generating Dolby Vision RPU...");
    let rpu_path = generate_rpu(
        measured.hdr_type,
        &temp_dir,
        args.peak_source,
        measured.hdr10plus_json.as_deref(),
        measured.measurements_file.as_deref(),
        measured.l1_sidecar.is_some(),
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
    let bl_hevc = if measured
        .bl_source_file
        .extension()
        .is_some_and(|ext| ext == "hevc")
    {
        progress::print_info("Base layer is already a raw HEVC stream; skipping re-extraction.");
        measured.bl_source_file.clone()
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
                measured.bl_source_file.to_str().unwrap(),
                "-map",
                "0:v:0",
                "-c:v",
                "copy",
            ]);
            if measured.hdr_type == HdrFormat::Hlg
                && metadata::hlg_bitstream_lacks_transfer(input_file)
            {
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

            let bl_total = fs::metadata(&measured.bl_source_file).ok().map(|m| m.len());
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
            if measured.measurements_file.is_some() {
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
        let measurements_file_path = measured.measurements_file.clone();
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
                l1_sidecar: measured
                    .l1_sidecar
                    .as_ref()
                    .filter(|_| measured.hdr_type != HdrFormat::Hdr10Plus),
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

fn warn_output_exists(output_file: &Path) {
    progress::print_warn(&format!(
        "Output file '{}' already exists. Skipping.",
        output_file.display()
    ));
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
