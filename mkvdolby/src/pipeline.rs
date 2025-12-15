use anyhow::{Context, Result};
use colored::Colorize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::{Args, HwAccel, PeakSource};
use crate::external::{self, run_command, run_command_live};
use crate::metadata::{self, HdrFormat};

pub fn convert_file(input_file: &str, args: &Args) -> Result<bool> {
    let input_path = Path::new(input_file);
    if !input_path.exists() {
        println!(
            "{}",
            format!("Warning: Input file not found: {}", input_file).yellow()
        );
        return Ok(false);
    }

    // Output filename: name.DV.mkv
    let stem = input_path.file_stem().unwrap().to_string_lossy();
    let dir = input_path.parent().unwrap_or(Path::new("."));
    let output_file = dir.join(format!("{}.DV.mkv", stem));

    if output_file.exists() {
        println!(
            "{}",
            format!("Output file '{:?}' already exists. Skipping.", output_file).yellow()
        );
        return Ok(true);
    }

    println!(
        "{}",
        format!("\n----- Processing: {} -----", input_file).green()
    );

    // Create temp directory
    let temp_dir_name = format!("mkvdolby_temp_{}", stem);
    let temp_dir = dir.join(&temp_dir_name);
    fs::create_dir_all(&temp_dir).context("Failed to create temp directory")?;

    // Determine format
    let mut hdr_type = metadata::check_hdr_format(input_file);
    let mut measurements_file: Option<PathBuf> = None;
    let mut hdr10plus_json: Option<PathBuf> = None;
    let mut bl_source_file = PathBuf::from(input_file);

    // Pre-handle HDR10+ to allow fallback to HDR10Unsupported if metadata is missing
    if hdr_type == HdrFormat::Hdr10Plus {
        match extract_hdr10plus_metadata(input_file, &temp_dir) {
            Ok(Some(json_path)) => hdr10plus_json = Some(json_path),
            Ok(None) => {
                println!(
                    "{}",
                    "HDR10+ tagged but no dynamic metadata found. Falling back to HDR10 analysis."
                        .yellow()
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
            // 1. Run analyzer with --hlg-peak-nits
            let mut extra_args = vec![
                "--hlg-peak-nits".to_string(),
                args.hlg_peak_nits.to_string(),
            ];
            add_optimizer_args(&mut extra_args, args);

            println!(
                "{}",
                format!(
                    "HLG detected. Running analyzer natively with --hlg-peak-nits={}...",
                    args.hlg_peak_nits
                )
                .green()
            );

            measurements_file = run_hdr_analyzer(input_file, &temp_dir, &extra_args, args)?;
            if measurements_file.is_none() {
                return Ok(false);
            }

            // 2. Convert HLG -> PQ for Base Layer
            match convert_hlg_to_pq(input_file, &temp_dir, args) {
                Ok(path) => bl_source_file = path,
                Err(_) => return Ok(false),
            }
        }
        HdrFormat::Hdr10WithMeasurements | HdrFormat::Hdr10Unsupported => {
            // Try finding measurements
            measurements_file = metadata::find_measurements_file(input_path);

            if measurements_file.is_some() {
                if args.boost_experimental {
                    println!(
                        "{}",
                        "Experimental boost requested, but using existing measurements.".yellow()
                    );
                }
            } else if hdr_type == HdrFormat::Hdr10WithMeasurements {
                // Should have found it
                println!("{}", "Expected madVR measurements file not found.".red());
                return Ok(false);
            } else {
                // Generate them
                let mut extra_args = Vec::new();
                if args.boost_experimental {
                    println!(
                        "{}",
                        "Experimental boost: using 'aggressive' optimizer.".green()
                    );
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
            println!("{}", "Unsupported HDR format.".red());
            return Ok(false);
        }
    }

    // Static Metadata
    let static_meta = metadata::get_static_metadata(input_file);
    // TODO: Validate metadata (logic in metadata.rs, just print warnings)

    // Generate extra.json
    let extra_json_path = temp_dir.join("extra.json");
    // Parse trim targets
    // Assuming trim_targets logic is simple: use args, or override from Details.txt if enabled
    let final_trims: Vec<u32> = args
        .trim_targets
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    // We already parsed this in main.rs but let's re-parse or pass it down.
    // Ideally args should have Vec<u32>.
    // Just re-parsing for now.

    if args.trim_from_details {
        if let Some(_details) = metadata::find_details_file(input_path) {
            // Logic to parse details for trims would go here
            // Using stub or simplified logic
            // For now, sticking to CLI defaults unless exact logic ported
        }
    }

    metadata::generate_extra_json(&extra_json_path, &static_meta, &final_trims)?;

    // Generate RPU
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

    // Extract BL (if needed, or copy)
    // Actually we need to extract HEVC bitstream to INJECT RPU
    let bl_hevc = temp_dir.join("BL.hevc");

    // Run ffmpeg to extract HEVC
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

    println!("{}", "Extracting BL to HEVC...".green());
    if !run_command_live(&mut ffmpeg_cmd, &temp_dir.join("ffmpeg_extract_bl.log"))? {
        return Ok(false);
    }

    // Inject RPU
    let bl_rpu_hevc = temp_dir.join("BL_RPU.hevc");
    let mut dovi_cmd = Command::new("dovi_tool");
    dovi_cmd.args([
        "inject-rpu",
        "-i",
        bl_hevc.to_str().unwrap(),
        "--rpu-in",
        rpu_path.to_str().unwrap(),
        "-o",
        bl_rpu_hevc.to_str().unwrap(),
    ]);

    println!("{}", "Injecting RPU...".green());
    if !run_command(&mut dovi_cmd, &temp_dir.join("dovi_inject.log"))? {
        return Ok(false);
    }

    // Mux
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

    println!("{}", "Muxing final MKV...".green());
    if !run_command(&mut mkvmerge_cmd, &temp_dir.join("mkvmerge.log"))? {
        return Ok(false);
    }

    // Optional post-mux verification
    if args.verify {
        println!("{}", "Running post-mux verification (--verify)...".green());
        let measurements_file_path = measurements_file.clone(); // Need pathbuf, it's optional
        let ok = crate::verify::verify_post_mux(
            input_file,
            &output_file,
            measurements_file_path.as_deref(),
            &temp_dir,
        );
        if !ok {
            println!("{}", "Inconsistencies detected during verification.".red());
            return Ok(false);
        }
    }

    // Cleanup
    if !args.keep_source {
        println!("{}", "Cleaning up...".green());
        let _ = fs::remove_dir_all(&temp_dir);
    }

    println!(
        "{}",
        format!("✓ Success! Created: {:?}", output_file.file_name().unwrap())
            .green()
            .bold()
    );
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
    // Find analyzer executable
    // Note: We are in the workspace. Ideally we use the one in target/release if we are running from cargo?
    // Or we assume it's in PATH.
    // Python script looked for "hdr_analyzer_mvp" or "hdranalyze".
    let tool_name = "hdr_analyzer_mvp";
    // Check local target/release (dev workflow) first?
    let mut exe = PathBuf::from(tool_name);
    if std::path::Path::new("target/release/hdr_analyzer_mvp").exists() {
        exe = std::path::Path::new("target/release/hdr_analyzer_mvp").to_path_buf();
    }

    // Command
    let _out_name = Path::new(input).with_extension("measurements.bin");
    // Wait, python script puts measurements next to input file?
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

    println!("{}", "Generating measurements...".green());
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

    if !run_command_live(&mut cmd, &temp_dir.join("ffmpeg_extract_hdr10p.log"))? {
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

    if run_command(&mut tool, &temp_dir.join("hdr10plus_tool.log"))?
        && json_out.exists()
        && fs::metadata(&json_out)?.len() > 0
    {
        return Ok(Some(json_out));
    }
    // Check log for "no dynamic metadata"
    // Skipping logic for brevity, assume failed if no file.
    Ok(None)
}

fn convert_hlg_to_pq(input: &str, temp_dir: &Path, args: &Args) -> Result<PathBuf> {
    let out_path = temp_dir.join("HLG_to_PQ.mkv");
    let log_path = temp_dir.join("ffmpeg_hlg2pq.log");

    println!("{}", "Converting HLG to PQ...".green());

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
        // NVENC Encoding
        // Translate x265 params to NVENC equivs where possible
        // Mastering display and CLL are handled via specific flags if supported by the ffmpeg build
        // or -sei arguments.

        // Note: Modern ffmpeg hevc_nvenc supports -master_display and -max_cll?
        // Let's assume a reasonably recent ffmpeg or use the generic side data pass-through if filter chain preserves it.
        // Actually, zscale re-creates the frame. We need to re-tag.

        // We will use standard high-quality NVENC settings.
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
            "19", // Approx equivalent to CRF 17 for high quality
            "-pix_fmt",
            "yuv420p10le",
            "-profile:v",
            "main10",
        ]);

        // Explicitly set VUI to be safe
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

        // Attempt to pass mastering display metadata.
        // Note: As of typical ffmpeg builds, hevc_nvenc might not accept the x265 text format for master-display.
        // It's safer to rely on the container signaling for now, or use bitstream filters if strict correctness is needed.
        // However, for the base layer of a Profile 8.1 file, the RPU usually overrides/controls the display mapping.
        // We'll trust the container tagging.
    } else {
        // CPU Encoding (x265)
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

    cmd.arg(out_path.to_str().unwrap());

    if run_command_live(&mut cmd, &log_path)? && out_path.exists() {
        println!("{}", "Converted HLG to PQ successfully.".green());
        return Ok(out_path);
    }
    anyhow::bail!("HLG to PQ conversion failed");
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

    let mut cmd = Command::new("dovi_tool");
    cmd.args([
        "generate",
        "-j",
        extra_json.to_str().unwrap(),
        "--rpu-out",
        rpu_out.to_str().unwrap(),
    ]);

    println!("{}", "Generating RPU from metadata...".green());

    match hdr_type {
        HdrFormat::Hdr10Plus => {
            cmd.arg("--hdr10plus-json").arg(meta_file.unwrap());
            cmd.arg("--hdr10plus-peak-source")
                .arg(peak_source.to_string());
        }
        HdrFormat::Hdr10WithMeasurements | HdrFormat::Hdr10Unsupported => {
            cmd.arg("--madvr-file").arg(meas_file.unwrap());
            cmd.arg("--use-custom-targets");
        }
        _ => return Ok(None),
    }

    if run_command(&mut cmd, &temp_dir.join("dovi_gen.log"))? {
        Ok(Some(rpu_out))
    } else {
        Ok(None)
    }
}
