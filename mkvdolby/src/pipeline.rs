use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result};
use colored::Colorize;

use crate::cli::{Args, CmVersion, Encoder, HwAccel, PeakSource};
use crate::external::{self, run_command_with_spinner};
use crate::metadata::{self, HdrFormat};
use crate::progress;

pub fn convert_file(input_file: &str, args: &Args) -> Result<bool> {
    let input_path = Path::new(input_file);
    if !input_path.exists() {
        progress::print_warn(&format!("Input file not found: {}", input_file));
        return Ok(false);
    }

    // Output filename: name.DV.mkv
    let stem = input_path.file_stem().unwrap().to_string_lossy();
    let dir = input_path.parent().unwrap_or(Path::new("."));
    let output_file = dir.join(format!("{}.DV.mkv", stem));

    if output_file.exists() {
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

    // Create temp directory
    let temp_dir_name = format!("mkvdolby_temp_{}", stem);
    let temp_dir = dir.join(&temp_dir_name);
    fs::create_dir_all(&temp_dir).context("Failed to create temp directory")?;

    // --- Step 1: Detect HDR format ---
    progress::print_step(1, 0, "Detecting HDR format...");
    let mut hdr_type = metadata::check_hdr_format(input_file);
    let mut measurements_file: Option<PathBuf> = None;
    let mut hdr10plus_json: Option<PathBuf> = None;
    let mut bl_source_file = PathBuf::from(input_file);

    let format_label = match hdr_type {
        HdrFormat::Hdr10Plus => "HDR10+",
        HdrFormat::Hlg => "HLG",
        HdrFormat::Hdr10WithMeasurements => "HDR10 (measurements found)",
        HdrFormat::Hdr10Unsupported => "HDR10",
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
        match extract_hdr10plus_metadata(input_file, &temp_dir) {
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
            match convert_hlg_to_pq(input_file, &temp_dir, args) {
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

    // Generate extra.json
    let extra_json_path = temp_dir.join("extra.json");
    let final_trims: Vec<u32> = args
        .trim_targets
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    if args.trim_from_details {
        if let Some(_details) = metadata::find_details_file(input_path) {
            // Logic to parse details for trims would go here
        }
    }

    metadata::generate_extra_json(
        &extra_json_path,
        &static_meta,
        &final_trims,
        cm_v40_config.as_ref(),
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
    )?;

    if rpu_path.is_none() {
        return Ok(false);
    }
    let rpu_path = rpu_path.unwrap();

    // --- Extract base layer ---
    current_step += 1;
    progress::print_step(current_step, total_steps, "Extracting base layer...");
    let bl_hevc = temp_dir.join("BL.hevc");

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

    if !run_command_with_spinner(
        &mut ffmpeg_cmd,
        &temp_dir.join("ffmpeg_extract_bl.log"),
        "Extracting base layer HEVC stream",
    )? {
        return Ok(false);
    }

    // --- Inject RPU ---
    current_step += 1;
    progress::print_step(
        current_step,
        total_steps,
        "Injecting RPU into base layer...",
    );
    let bl_rpu_hevc = temp_dir.join("BL_RPU.hevc");
    let dovi_tool_path =
        external::find_tool("dovi_tool").unwrap_or_else(|| PathBuf::from("dovi_tool"));
    let mut dovi_cmd = Command::new(fs::canonicalize(&dovi_tool_path).unwrap_or(dovi_tool_path));

    dovi_cmd.args([
        "inject-rpu",
        "-i",
        bl_hevc.to_str().unwrap(),
        "--rpu-in",
        rpu_path.to_str().unwrap(),
        "-o",
        bl_rpu_hevc.to_str().unwrap(),
    ]);

    if !run_command_with_spinner(
        &mut dovi_cmd,
        &temp_dir.join("dovi_inject.log"),
        "Injecting RPU into base layer",
    )? {
        return Ok(false);
    }

    // --- Mux ---
    current_step += 1;
    progress::print_step(current_step, total_steps, "Muxing final MKV...");
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

    if !run_command_with_spinner(
        &mut mkvmerge_cmd,
        &temp_dir.join("mkvmerge.log"),
        "Muxing final MKV",
    )? {
        return Ok(false);
    }

    // --- Optional verification ---
    if args.verify {
        progress::print_info("Running post-mux verification (--verify)...");
        let measurements_file_path = measurements_file.clone();
        let ok = crate::verify::verify_post_mux(
            input_file,
            &output_file,
            measurements_file_path.as_deref(),
            &temp_dir,
        );
        if !ok {
            progress::print_error("Inconsistencies detected during verification.");
            return Ok(false);
        }
        progress::print_info("Verification passed.");
    }

    // --- Cleanup ---
    let _ = fs::remove_dir_all(&temp_dir);

    // Delete source file unless --keep-source is set
    if !args.keep_source {
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

fn add_optimizer_args(args_vec: &mut Vec<String>, args: &Args) {
    args_vec.push("--optimizer-profile".to_string());
    args_vec.push(args.optimizer_profile.to_string());
}

fn run_hdr_analyzer(
    input: &str,
    temp_dir: &Path,
    extra_args: &[String],
    args: &Args,
) -> Result<Option<PathBuf>> {
    let tool_name = "hdr_analyzer_mvp";
    let mut exe = PathBuf::from(tool_name);
    if Path::new("target/release/hdr_analyzer_mvp").exists() {
        exe = Path::new("target/release/hdr_analyzer_mvp").to_path_buf();
    }

    let dir = Path::new(input).parent().unwrap_or(Path::new("."));
    let stem = Path::new(input).file_stem().unwrap().to_string_lossy();
    let out_path = dir.join(format!("{}_measurements.bin", stem));

    let mut cmd = Command::new(&exe);
    cmd.arg(input).arg("-o").arg(&out_path);
    // Fast mode defaults
    cmd.args(["--downscale", "2", "--sample-rate", "3"]);
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

fn extract_hdr10plus_metadata(input: &str, temp_dir: &Path) -> Result<Option<PathBuf>> {
    let hevc = temp_dir.join("video.hevc");
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

    if !run_command_with_spinner(
        &mut cmd,
        &temp_dir.join("ffmpeg_extract_hdr10p.log"),
        "Extracting HEVC stream",
    )? {
        return Ok(None);
    }

    let json_out = temp_dir.join("hdr10plus_metadata.json");
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
        return Ok(Some(json_out));
    }
    Ok(None)
}

fn convert_hlg_to_pq(input: &str, temp_dir: &Path, args: &Args) -> Result<PathBuf> {
    let out_path = temp_dir.join("HLG_to_PQ.mkv");
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

    if args.hwaccel == HwAccel::Cuda {
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

    if run_command_with_spinner(&mut cmd, &log_path, "Converting HLG to PQ (encoding)")?
        && out_path.exists()
    {
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
) -> Result<Option<PathBuf>> {
    let rpu_out = temp_dir.join("RPU.bin");
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
        Ok(Some(rpu_out))
    } else {
        Ok(None)
    }
}
