use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result};
use colored::Colorize;
use serde_json::Value;

use crate::cli::{AnalysisQuality, Args, CmVersion, Encoder, HwAccel, PeakSource};
use crate::external::{self, run_command_with_progress, run_command_with_spinner};
use crate::fel_composite;
use crate::metadata::{self, HdrFormat};
use crate::progress;
use crate::resume;
use crate::rpu_check::{self, Level5Offsets};

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
    // `mkvdolby_temp_*` directory when no new-style one exists. Remove after one release.
    if resume_enabled && !temp_dir.exists() {
        let legacy_temp_dir = dir.join(format!("mkvdolby_temp_{}", stem));
        if legacy_temp_dir.exists() {
            temp_dir = legacy_temp_dir;
        }
    }

    // A leftover temp dir means a previous run for this file was interrupted. With resume
    // enabled we reuse its completed steps; otherwise we discard it and start clean.
    let resuming = resume_enabled && temp_dir.exists();

    if output_file.exists() && !resuming {
        progress::print_warn(&format!(
            "Output file '{}' already exists. Skipping.",
            output_file.display()
        ));
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
    if resuming {
        progress::print_info("Resuming from a previous run — completed steps will be reused.");
    }

    // --- Step 1: Detect HDR format ---
    progress::print_step(1, 0, "Detecting HDR format...");
    let mut hdr_type = metadata::check_hdr_format(input_file);
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

    if hdr_type == HdrFormat::Unsupported {
        progress::print_error("Unsupported HDR format. Cannot process this file.");
        let _ = fs::remove_dir_all(&temp_dir);
        return Ok(false);
    }

    // Compute total steps based on the detected format
    let total_steps: u8 = match hdr_type {
        HdrFormat::Hdr10Plus => 7, // detect, extract HEVC, extract meta, config, RPU, inject, mux
        HdrFormat::Hlg => 8,       // detect, analyze, HLG→PQ, config, RPU, extract BL, inject, mux
        HdrFormat::Hdr10WithMeasurements => 6, // detect, config, RPU, extract BL, inject, mux
        HdrFormat::Hdr10Unsupported => 7, // detect, analyze, config, RPU, extract BL, inject, mux
        HdrFormat::DolbyVisionMel => 7,
        HdrFormat::DolbyVisionFel => 8,
        HdrFormat::DolbyVisionP8 => 7,
        HdrFormat::Unsupported => 0, // unreachable — handled above
    };

    let mut current_step: u8 = 1; // Step 1 (detect) already done

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
            let raw_hevc = temp_dir.join("DV_raw.hevc");
            extract_video_hevc(
                input_file,
                &raw_hevc,
                &temp_dir,
                "Extracting Dolby Vision HEVC stream",
                resume_enabled,
                args.stall_timeout,
            )?;
            let clean_bl = remove_dolby_vision_metadata(&raw_hevc, &temp_dir, resume_enabled)?;
            bl_source_file = clean_bl.clone();

            let mut extra_args = Vec::new();
            add_optimizer_args(&mut extra_args, args);
            measurements_file =
                run_hdr_analyzer(clean_bl.to_str().unwrap(), &temp_dir, &extra_args, args)?;
            if measurements_file.is_none() {
                return Ok(false);
            }
            hdr_type = HdrFormat::Hdr10WithMeasurements;
        }
        HdrFormat::DolbyVisionFel => {
            progress::print_info("Compositing Profile 7 FEL into a Profile 8.1 base layer.");
            let composited = fel_composite::convert_fel_to_hdr10(input_file, &temp_dir, args)?;
            bl_source_file = composited.clone();

            let mut extra_args = Vec::new();
            add_optimizer_args(&mut extra_args, args);
            measurements_file =
                run_hdr_analyzer(composited.to_str().unwrap(), &temp_dir, &extra_args, args)?;
            if measurements_file.is_none() {
                progress::print_error(
                    "Failed to generate measurements from the composited FEL output.",
                );
                return Ok(false);
            }
            hdr_type = HdrFormat::Hdr10WithMeasurements;
        }
        HdrFormat::DolbyVisionP8 => {
            if !args.mdfix {
                progress::print_warn(
                    "Profile 8 input is already converted; use `inspect` or --mdfix to rebuild metadata.",
                );
                return Ok(false);
            }

            progress::print_info(
                "Rebuilding Profile 8 metadata from fresh base-layer measurements (--mdfix).",
            );
            let raw_hevc = temp_dir.join("DV_raw.hevc");
            extract_video_hevc(
                input_file,
                &raw_hevc,
                &temp_dir,
                "Extracting Profile 8 HEVC stream",
                resume_enabled,
                args.stall_timeout,
            )?;
            let clean_bl = remove_dolby_vision_metadata(&raw_hevc, &temp_dir, resume_enabled)?;
            bl_source_file = clean_bl.clone();

            let mut extra_args = Vec::new();
            add_optimizer_args(&mut extra_args, args);
            measurements_file =
                run_hdr_analyzer(clean_bl.to_str().unwrap(), &temp_dir, &extra_args, args)?;
            if measurements_file.is_none() {
                return Ok(false);
            }
            hdr_type = HdrFormat::Hdr10WithMeasurements;
        }
        HdrFormat::Hlg => {
            // HLG Logic
            current_step += 1;
            progress::print_step(
                current_step,
                total_steps,
                &format!(
                    "Generating measurements (HLG, --hlg-peak-nits={})...",
                    args.hlg_peak_nits
                ),
            );

            let mut extra_args = vec![
                "--hlg-peak-nits".to_string(),
                args.hlg_peak_nits.to_string(),
            ];
            add_optimizer_args(&mut extra_args, args);

            measurements_file = run_hdr_analyzer(input_file, &temp_dir, &extra_args, args)?;
            if measurements_file.is_none() {
                return Ok(false);
            }

            // Convert HLG -> PQ for Base Layer
            current_step += 1;
            progress::print_step(current_step, total_steps, "Converting HLG to PQ...");
            match convert_hlg_to_pq(input_file, &temp_dir, args, resume_enabled) {
                Ok(path) => bl_source_file = path,
                Err(_) => return Ok(false),
            }
        }
        HdrFormat::Hdr10WithMeasurements | HdrFormat::Hdr10Unsupported => {
            // Try finding measurements
            measurements_file = metadata::find_measurements_file(input_path);

            if measurements_file.is_some() {
                progress::print_info("Using existing measurements file.");
                if args.boost_experimental {
                    progress::print_warn(
                        "Experimental boost requested, but using existing measurements.",
                    );
                }
            } else if hdr_type == HdrFormat::Hdr10WithMeasurements {
                // Should have found it
                progress::print_error("Expected madVR measurements file not found.");
                return Ok(false);
            } else {
                // Generate them
                current_step += 1;
                progress::print_step(current_step, total_steps, "Generating measurements...");

                let mut extra_args = Vec::new();
                if args.boost_experimental {
                    progress::print_info("Experimental boost: using 'aggressive' optimizer.");
                    extra_args
                        .extend(["--optimizer-profile".to_string(), "aggressive".to_string()]);
                } else {
                    add_optimizer_args(&mut extra_args, args);
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
    let static_meta = metadata::get_static_metadata(input_file);

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

    metadata::generate_extra_json(
        &extra_json_path,
        &static_meta,
        &final_trims,
        cm_v40_config.as_ref(),
        level5_offsets,
    )?;
    progress::print_info("Configuration written.");

    // --- Generate RPU ---
    current_step += 1;
    progress::print_step(current_step, total_steps, "Generating Dolby Vision RPU...");
    let rpu_path = generate_rpu(
        hdr_type,
        &temp_dir,
        args.peak_source,
        hdr10plus_json.as_deref(),
        measurements_file.as_deref(),
        resume_enabled,
    )?;

    if rpu_path.is_none() {
        return Ok(false);
    }
    let rpu_path = rpu_path.unwrap();

    // --- Extract base layer ---
    current_step += 1;
    progress::print_step(current_step, total_steps, "Extracting base layer...");
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
            "-f",
            "hevc",
            "-y",
            bl_hevc.to_str().unwrap(),
        ]);

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
        let dovi_tool_path =
            external::find_tool("dovi_tool").unwrap_or_else(|| PathBuf::from("dovi_tool"));
        let mut dovi_cmd =
            Command::new(fs::canonicalize(&dovi_tool_path).unwrap_or(dovi_tool_path));

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
        resume::mark_done(&bl_rpu_hevc)?;
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

fn dovi_tool_command() -> Command {
    let dovi_tool_path =
        external::find_tool("dovi_tool").unwrap_or_else(|| PathBuf::from("dovi_tool"));
    Command::new(fs::canonicalize(&dovi_tool_path).unwrap_or(dovi_tool_path))
}

fn extract_video_hevc(
    input: &str,
    output: &Path,
    temp_dir: &Path,
    message: &str,
    resume_enabled: bool,
    stall_timeout: u64,
) -> Result<()> {
    if resume_enabled && resume::is_complete(output) {
        progress::print_info(&format!(
            "Reusing {} from a previous run.",
            output.display()
        ));
        return Ok(());
    }

    let mut command = Command::new("ffmpeg");
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-stats",
        "-i",
        input,
        "-map",
        "0:v:0",
        "-c:v",
        "copy",
        "-bsf:v",
        "hevc_mp4toannexb",
        "-f",
        "hevc",
        "-y",
        output.to_str().unwrap(),
    ]);

    let total = fs::metadata(input).ok().map(|metadata| metadata.len());
    if run_command_with_progress(
        &mut command,
        &temp_dir.join("ffmpeg_extract_dv.log"),
        message,
        output,
        total,
        stall_timeout,
    )? && output.exists()
    {
        resume::mark_done(output)?;
        Ok(())
    } else {
        anyhow::bail!("Failed to extract HEVC bitstream")
    }
}

fn remove_dolby_vision_metadata(
    input_hevc: &Path,
    temp_dir: &Path,
    resume_enabled: bool,
) -> Result<PathBuf> {
    let clean_bl = temp_dir.join("BL_clean.hevc");
    if resume_enabled && resume::is_complete(&clean_bl) {
        progress::print_info("Reusing Dolby Vision-clean base layer from a previous run.");
        return Ok(clean_bl);
    }

    let mut command = dovi_tool_command();
    command.args([
        "remove",
        "-i",
        input_hevc.to_str().unwrap(),
        "-o",
        clean_bl.to_str().unwrap(),
    ]);

    if run_command_with_spinner(
        &mut command,
        &temp_dir.join("dovi_remove.log"),
        "Removing existing Dolby Vision metadata",
    )? && clean_bl.exists()
    {
        resume::mark_done(&clean_bl)?;
        Ok(clean_bl)
    } else {
        anyhow::bail!("Failed to remove Dolby Vision metadata from base layer")
    }
}

fn convert_mel_to_profile81(
    input_file: &str,
    temp_dir: &Path,
    output_file: &Path,
    args: &Args,
    resume_enabled: bool,
) -> Result<bool> {
    let raw_hevc = temp_dir.join("DV_raw.hevc");
    let converted_hevc = temp_dir.join("P81_discard.hevc");

    extract_video_hevc(
        input_file,
        &raw_hevc,
        temp_dir,
        "Extracting Profile 7 MEL HEVC stream",
        resume_enabled,
        args.stall_timeout,
    )?;

    if resume_enabled && resume::is_complete(&converted_hevc) {
        progress::print_info("Reusing converted Profile 8.1 HEVC from a previous run.");
    } else {
        let mut command = dovi_tool_command();
        command.args([
            "-m",
            "2",
            "convert",
            "--discard",
            "-i",
            raw_hevc.to_str().unwrap(),
            "-o",
            converted_hevc.to_str().unwrap(),
        ]);

        if !run_command_with_spinner(
            &mut command,
            &temp_dir.join("dovi_convert_discard.log"),
            "Converting MEL RPU to Profile 8.1 and discarding EL",
        )? || !converted_hevc.exists()
        {
            return Ok(false);
        }
        resume::mark_done(&converted_hevc)?;
    }

    mux_hevc_with_original(
        input_file,
        &converted_hevc,
        output_file,
        temp_dir,
        args,
        resume_enabled,
    )
}

fn mux_hevc_with_original(
    input_file: &str,
    hevc_file: &Path,
    output_file: &Path,
    temp_dir: &Path,
    args: &Args,
    resume_enabled: bool,
) -> Result<bool> {
    let marker = temp_dir.join("mux.done");
    if resume_enabled && output_file.exists() && marker.exists() {
        progress::print_info("Reusing muxed output from a previous run.");
        return Ok(true);
    }

    let mut command = Command::new("mkvmerge");
    command.arg("-q").arg("-o").arg(output_file);
    if args.drop_tags {
        command.arg("--no-global-tags");
    }
    if args.drop_chapters {
        command.arg("--no-chapters");
    }
    command.arg(hevc_file).arg("--no-video").arg(input_file);

    let total = fs::metadata(input_file).ok().map(|metadata| metadata.len());
    let success = run_command_with_progress(
        &mut command,
        &temp_dir.join("mkvmerge.log"),
        "Muxing final MKV",
        output_file,
        total,
        args.stall_timeout,
    )?;
    if success {
        fs::write(marker, b"")?;
    }
    Ok(success)
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

/// Locate the hdr_analyzer_mvp binary, preferring a fresh local release build.
pub fn analyzer_executable() -> PathBuf {
    let local = Path::new("target/release/hdr_analyzer_mvp");
    if local.exists() {
        local.to_path_buf()
    } else {
        PathBuf::from("hdr_analyzer_mvp")
    }
}

/// Resolve `auto` settings to concrete values for this machine, once at startup.
/// `--hwaccel auto` becomes `cuda` when an NVIDIA GPU is detected (else `none`);
/// `--analysis-quality auto` becomes `accurate` only when GPU analysis is actually
/// available (CUDA resolved + analyzer built with the cuda feature), because
/// full-resolution every-frame analysis on the CPU would be slower than today's
/// balanced default.
pub fn resolve_auto_settings(args: &mut Args) {
    if args.hwaccel == HwAccel::Auto {
        if external::detect_nvidia_gpu() {
            args.hwaccel = HwAccel::Cuda;
            progress::print_info(
                "Auto-detected NVIDIA GPU: CUDA acceleration enabled (decode + analysis; NVENC for re-encodes).",
            );
        } else {
            args.hwaccel = HwAccel::None;
            progress::print_info("No NVIDIA GPU detected: using the CPU pipeline.");
        }
    }
    if args.analysis_quality == AnalysisQuality::Auto {
        let gpu_analysis = args.hwaccel == HwAccel::Cuda
            && external::analyzer_has_cuda_feature(&analyzer_executable());
        args.analysis_quality = if gpu_analysis {
            progress::print_info(
                "GPU analysis available: using accurate (full-resolution) analysis quality.",
            );
            AnalysisQuality::Accurate
        } else {
            AnalysisQuality::Balanced
        };
    }
}

fn run_hdr_analyzer(
    input: &str,
    temp_dir: &Path,
    extra_args: &[String],
    args: &Args,
) -> Result<Option<PathBuf>> {
    let exe = analyzer_executable();

    let dir = Path::new(input).parent().unwrap_or(Path::new("."));
    let stem = Path::new(input).file_stem().unwrap().to_string_lossy();
    let out_path = dir.join(format!("{}_measurements.bin", stem));

    let (downscale, sample_rate) = analysis_quality_args(args.analysis_quality);

    let mut cmd = Command::new(&exe);
    cmd.arg(input).arg("-o").arg(&out_path);
    cmd.args(["--downscale", downscale, "--sample-rate", sample_rate]);
    cmd.args(extra_args);

    if args.hwaccel != HwAccel::None {
        cmd.arg("--hwaccel").arg(args.hwaccel.to_string());
    }

    // Use inherit_stderr so indicatif progress bar works correctly (detects TTY)
    if external::run_command_inherit_stderr(&mut cmd, &temp_dir.join("analyzer.log"))?
        && out_path.exists()
    {
        return Ok(Some(out_path));
    }
    Ok(None)
}

fn analysis_quality_args(quality: AnalysisQuality) -> (&'static str, &'static str) {
    match quality {
        // Auto is resolved to a concrete value at startup; map it defensively.
        AnalysisQuality::Auto | AnalysisQuality::Balanced => ("2", "1"),
        AnalysisQuality::Fast => ("2", "3"),
        AnalysisQuality::Accurate => ("1", "1"),
    }
}

#[derive(Debug, PartialEq)]
struct Hdr10PlusPeakStats {
    max_peak_nits: f64,
    outlier_scene_count: usize,
}

fn inspect_hdr10plus_scene_peaks(
    metadata_path: &Path,
    peak_source: PeakSource,
    outlier_threshold_nits: f64,
) -> Result<Option<Hdr10PlusPeakStats>> {
    let metadata: Value = serde_json::from_reader(
        File::open(metadata_path).context("Failed to open extracted HDR10+ metadata JSON")?,
    )
    .context("Failed to parse extracted HDR10+ metadata JSON")?;
    hdr10plus_scene_peak_stats(&metadata, peak_source, outlier_threshold_nits)
}

fn hdr10plus_scene_peak_stats(
    metadata: &Value,
    peak_source: PeakSource,
    outlier_threshold_nits: f64,
) -> Result<Option<Hdr10PlusPeakStats>> {
    let scene_info = metadata
        .get("SceneInfo")
        .and_then(Value::as_array)
        .context("HDR10+ metadata is missing SceneInfo")?;
    let first_frame_indices = metadata
        .pointer("/SceneInfoSummary/SceneFirstFrameIndex")
        .and_then(Value::as_array)
        .context("HDR10+ metadata is missing SceneInfoSummary.SceneFirstFrameIndex")?;
    let first_frame_offset = first_frame_indices
        .first()
        .and_then(Value::as_u64)
        .context("HDR10+ metadata has no scene first-frame indices")?;

    let mut outlier_scenes = HashSet::new();
    let mut max_peak_nits = 0.0_f64;
    for scene_index in first_frame_indices {
        let source_index = scene_index
            .as_u64()
            .context("HDR10+ scene first-frame index is not an integer")?;
        let relative_index = source_index
            .checked_sub(first_frame_offset)
            .context("HDR10+ scene first-frame index precedes the first scene")?;
        let scene = scene_info
            .get(relative_index as usize)
            .context("HDR10+ scene first-frame index is out of range")?;
        let peak_nits = hdr10plus_peak_nits(scene, peak_source)
            .context("HDR10+ scene is missing peak-brightness metadata")?;

        if peak_nits > outlier_threshold_nits {
            outlier_scenes.insert(source_index);
            max_peak_nits = max_peak_nits.max(peak_nits);
        }
    }

    if outlier_scenes.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Hdr10PlusPeakStats {
            max_peak_nits,
            outlier_scene_count: outlier_scenes.len(),
        }))
    }
}

fn hdr10plus_peak_nits(scene: &Value, peak_source: PeakSource) -> Option<f64> {
    let luminance = scene.get("LuminanceParameters")?;
    let tenths_of_a_nit = match peak_source {
        PeakSource::Histogram => luminance
            .pointer("/LuminanceDistributions/DistributionValues")?
            .as_array()?
            .iter()
            .filter_map(Value::as_u64)
            .max()? as f64,
        PeakSource::Histogram99 => luminance
            .pointer("/LuminanceDistributions/DistributionValues")?
            .as_array()?
            .last()?
            .as_u64()? as f64,
        PeakSource::MaxScl => luminance
            .get("MaxScl")?
            .as_array()?
            .iter()
            .filter_map(Value::as_u64)
            .max()? as f64,
        PeakSource::MaxSclLuminance => {
            let max_scl = luminance.get("MaxScl")?.as_array()?;
            let [r, g, b] = max_scl.as_slice() else {
                return None;
            };
            (0.2627 * r.as_u64()? as f64)
                + (0.678 * g.as_u64()? as f64)
                + (0.0593 * b.as_u64()? as f64)
        }
    };

    Some(tenths_of_a_nit / 10.0)
}

fn extract_hdr10plus_metadata(
    input: &str,
    temp_dir: &Path,
    resume: bool,
    stall_secs: u64,
) -> Result<Option<PathBuf>> {
    let hevc = temp_dir.join("video.hevc");
    if resume && resume::is_complete(&hevc) {
        progress::print_info("Reusing extracted HEVC stream from a previous run.");
    } else {
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            input,
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-f",
            "hevc",
            "-y",
            hevc.to_str().unwrap(),
        ]);

        let total = fs::metadata(input).ok().map(|m| m.len());
        if !run_command_with_progress(
            &mut cmd,
            &temp_dir.join("ffmpeg_extract_hdr10p.log"),
            "Extracting HEVC stream",
            &hevc,
            total,
            stall_secs,
        )? {
            return Ok(None);
        }
        resume::mark_done(&hevc)?;
    }

    let json_out = temp_dir.join("hdr10plus_metadata.json");
    if resume && resume::is_complete(&json_out) {
        progress::print_info("Reusing extracted HDR10+ metadata from a previous run.");
        return Ok(Some(json_out));
    }
    let mut tool = Command::new("hdr10plus_tool");
    tool.args([
        "extract",
        "-i",
        hevc.to_str().unwrap(),
        "-o",
        json_out.to_str().unwrap(),
    ]);

    if run_command_with_spinner(
        &mut tool,
        &temp_dir.join("hdr10plus_tool.log"),
        "Extracting HDR10+ metadata",
    )? && json_out.exists()
        && fs::metadata(&json_out)?.len() > 0
    {
        resume::mark_done(&json_out)?;
        return Ok(Some(json_out));
    }
    Ok(None)
}

fn convert_hlg_to_pq(input: &str, temp_dir: &Path, args: &Args, resume: bool) -> Result<PathBuf> {
    let out_path = temp_dir.join("HLG_to_PQ.mkv");
    if resume && resume::is_complete(&out_path) {
        progress::print_info("Reusing HLG\u{2192}PQ base layer from a previous run.");
        return Ok(out_path);
    }
    let log_path = temp_dir.join("ffmpeg_hlg2pq.log");

    let static_meta = metadata::get_static_metadata(input);
    let max_dml = *static_meta.get("max_dml").unwrap_or(&1000.0) as u32;
    let min_dml = static_meta.get("min_dml").unwrap_or(&0.005);
    let max_cll = *static_meta.get("max_cll").unwrap_or(&1000.0) as u32;
    let max_fall = *static_meta.get("max_fall").unwrap_or(&400.0) as u32;

    let min_dml_int = (min_dml * 10000.0) as u32;
    let max_dml_int = max_dml * 10000;

    let master_display = format!(
        "G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L({},{})",
        max_dml_int, min_dml_int
    );

    let x265_params = format!(
        "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:master-display={}:max-cll={},{}:hdr-opt=1:repeat-headers=1",
        master_display, max_cll, max_fall
    );

    let npl = args.hlg_peak_nits;
    let vf = format!(
        "zscale=transferin=arib-std-b67:transfer=smpte2084:primaries=bt2020:matrix=bt2020nc:rangein=tv:range=tv:npl={},format=yuv420p10le",
        npl
    );

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-stats",
        "-i",
        input,
        "-y",
        "-map",
        "0:v:0",
        "-an",
        "-sn",
        "-vf",
        &vf,
    ]);

    if args.hwaccel == HwAccel::Cuda && external::ffmpeg_has_encoder("hevc_nvenc") {
        cmd.args([
            "-c:v",
            "hevc_nvenc",
            "-preset",
            "p7",
            "-tune",
            "hq",
            "-rc",
            "constqp",
            "-qp",
            "19",
            "-pix_fmt",
            "yuv420p10le",
            "-profile:v",
            "main10",
        ]);

        cmd.args([
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "smpte2084",
            "-colorspace",
            "bt2020nc",
            "-color_range",
            "tv",
        ]);
    } else {
        if args.hwaccel == HwAccel::Cuda {
            progress::print_warn(
                "ffmpeg lacks hevc_nvenc; using the configured software encoder instead.",
            );
        }
        match args.encoder {
            Encoder::Libx265 => {
                cmd.args([
                    "-c:v",
                    "libx265",
                    "-preset",
                    &args.hlg_preset,
                    "-crf",
                    &args.hlg_crf.to_string(),
                    "-pix_fmt",
                    "yuv420p10le",
                    "-profile:v",
                    "main10",
                    "-x265-params",
                    &x265_params,
                ]);
            }
            Encoder::HevcVideotoolbox => {
                cmd.args([
                    "-c:v",
                    "hevc_videotoolbox",
                    "-allow_sw",
                    "1",
                    "-profile:v",
                    "main10",
                    "-pix_fmt",
                    "p010le",
                    "-color_primaries",
                    "bt2020",
                    "-color_trc",
                    "smpte2084",
                    "-colorspace",
                    "bt2020nc",
                    "-q:v",
                    "65",
                ]);
            }
        }
    }

    cmd.arg(out_path.to_str().unwrap());

    if run_command_with_progress(
        &mut cmd,
        &log_path,
        "Converting HLG to PQ (encoding)",
        &out_path,
        None,
        args.stall_timeout,
    )? && out_path.exists()
    {
        resume::mark_done(&out_path)?;
        return Ok(out_path);
    }
    anyhow::bail!("HLG to PQ conversion failed")
}

fn generate_rpu(
    hdr_type: HdrFormat,
    temp_dir: &Path,
    peak_source: PeakSource,
    meta_file: Option<&Path>,
    meas_file: Option<&Path>,
    resume: bool,
) -> Result<Option<PathBuf>> {
    let rpu_out = temp_dir.join("RPU.bin");
    if resume && resume::is_complete(&rpu_out) {
        progress::print_info("Reusing generated RPU from a previous run.");
        return Ok(Some(rpu_out));
    }
    let extra_json = temp_dir.join("extra.json");
    // Resolve tool path
    let dovi_tool_path =
        external::find_tool("dovi_tool").unwrap_or_else(|| PathBuf::from("dovi_tool"));
    let dovi_abs = fs::canonicalize(&dovi_tool_path).unwrap_or(dovi_tool_path);

    let mut cmd = Command::new(&dovi_abs);
    cmd.args([
        "generate",
        "-j",
        extra_json.to_str().unwrap(),
        "--rpu-out",
        rpu_out.to_str().unwrap(),
    ]);

    match hdr_type {
        HdrFormat::Hdr10Plus => {
            cmd.arg("--hdr10plus-json").arg(meta_file.unwrap());
            cmd.arg("--hdr10plus-peak-source")
                .arg(peak_source.to_string());
        }
        HdrFormat::Hdr10WithMeasurements | HdrFormat::Hdr10Unsupported | HdrFormat::Hlg => {
            cmd.arg("--madvr-file").arg(meas_file.unwrap());
            cmd.arg("--use-custom-targets");
        }
        _ => return Ok(None),
    }

    let log_path = temp_dir.join("dovi_gen.log");
    if run_command_with_spinner(&mut cmd, &log_path, "Generating Dolby Vision RPU")? {
        resume::mark_done(&rpu_out)?;
        Ok(Some(rpu_out))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn analysis_quality_maps_to_analyzer_sampling_args() {
        assert_eq!(analysis_quality_args(AnalysisQuality::Fast), ("2", "3"));
        assert_eq!(analysis_quality_args(AnalysisQuality::Balanced), ("2", "1"));
        assert_eq!(analysis_quality_args(AnalysisQuality::Accurate), ("1", "1"));
        assert_eq!(analysis_quality_args(AnalysisQuality::Auto), ("2", "1"));
    }

    #[test]
    fn mdfix_output_does_not_collide_with_dv_input() {
        assert_eq!(
            output_path_for(Path::new("episode.DV.mkv"), true),
            PathBuf::from("episode.mdfix.DV.mkv")
        );
        assert_eq!(
            output_path_for(Path::new("episode.mkv"), false),
            PathBuf::from("episode.DV.mkv")
        );
    }

    #[test]
    fn hdr10plus_outlier_stats_use_selected_scene_peak_source() {
        let metadata = json!({
            "SceneInfo": [
                {
                    "LuminanceParameters": {
                        "LuminanceDistributions": { "DistributionValues": [100, 35000] },
                        "MaxScl": [1000, 1100, 1200]
                    }
                },
                {
                    "LuminanceParameters": {
                        "LuminanceDistributions": { "DistributionValues": [100, 20000] },
                        "MaxScl": [1000, 1100, 1200]
                    }
                }
            ],
            "SceneInfoSummary": { "SceneFirstFrameIndex": [0, 1] }
        });

        assert_eq!(
            hdr10plus_scene_peak_stats(&metadata, PeakSource::Histogram, 3000.0).unwrap(),
            Some(Hdr10PlusPeakStats {
                max_peak_nits: 3500.0,
                outlier_scene_count: 1,
            })
        );
        assert_eq!(
            hdr10plus_scene_peak_stats(&metadata, PeakSource::MaxScl, 3000.0).unwrap(),
            None
        );
    }

    #[test]
    fn hdr10plus_max_scl_luminance_matches_upstream_weighting() {
        let scene = json!({
            "LuminanceParameters": {
                "LuminanceDistributions": { "DistributionValues": [100] },
                "MaxScl": [1000, 2000, 3000]
            }
        });

        let peak_nits = hdr10plus_peak_nits(&scene, PeakSource::MaxSclLuminance).unwrap();

        assert!((peak_nits - 179.66).abs() < 1e-9);
    }
}
