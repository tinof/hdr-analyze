use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use dovi84_composer::Composer;

use crate::cli::{Args, DoviInput, PeakSource};
use crate::external::{self, run_command_with_progress, run_command_with_spinner, ToolVersion};
use crate::metadata::HdrFormat;
use crate::progress;
use crate::resume;

pub const DOVI_TOOL_MKV_INPUT_MIN: ToolVersion = (2, 3, 4);

pub(super) fn resolve_dovi_input(requested: DoviInput, version: Option<ToolVersion>) -> DoviInput {
    match requested {
        DoviInput::Auto => {
            if version >= Some(DOVI_TOOL_MKV_INPUT_MIN) {
                DoviInput::Mkv
            } else {
                DoviInput::Raw
            }
        }
        DoviInput::Raw => DoviInput::Raw,
        DoviInput::Mkv => DoviInput::Mkv,
    }
}

pub(crate) fn feed_mkv_to_dovi_tool(mode: DoviInput, raw_sealed: bool) -> bool {
    mode == DoviInput::Mkv && !raw_sealed
}

fn run_dovi_video_step(
    input_file: &str,
    raw_hevc: &Path,
    output: &Path,
    temp_dir: &Path,
    args: &Args,
    resume_enabled: bool,
    extract_message: &str,
    log_stem: &str,
    message: &str,
    build: impl Fn(&Path) -> Command,
) -> Result<bool> {
    let raw_sealed = resume_enabled && resume::is_complete(raw_hevc);
    if feed_mkv_to_dovi_tool(args.dovi_input, raw_sealed) {
        let mut mkv_cmd = build(Path::new(input_file));
        let mkv_log = temp_dir.join(format!("{log_stem}_mkv.log"));
        let total = fs::metadata(input_file).ok().map(|m| m.len());
        let success = run_command_with_progress(
            &mut mkv_cmd,
            &mkv_log,
            message,
            output,
            total,
            args.stall_timeout,
        )?;
        let non_empty =
            output.exists() && fs::metadata(output).map(|m| m.len() > 0).unwrap_or(false);
        if success && non_empty {
            return Ok(true);
        }
        let _ = fs::remove_file(output);
        progress::print_warn(&format!(
            "dovi_tool could not read the MKV directly (see {}); falling back to ffmpeg extraction.",
            mkv_log.display()
        ));
    }

    extract_video_hevc(
        input_file,
        raw_hevc,
        temp_dir,
        extract_message,
        resume_enabled,
        args.stall_timeout,
    )?;
    let mut raw_cmd = build(raw_hevc);
    let raw_log = temp_dir.join(format!("{log_stem}.log"));
    let total = fs::metadata(raw_hevc).ok().map(|m| m.len());
    let success = run_command_with_progress(
        &mut raw_cmd,
        &raw_log,
        message,
        output,
        total,
        args.stall_timeout,
    )?;
    let non_empty = output.exists() && fs::metadata(output).map(|m| m.len() > 0).unwrap_or(false);
    Ok(success && non_empty)
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

/// Produce the Dolby Vision-clean base layer for the repair paths. When a previous run
/// already sealed `BL_clean.hevc`, skip the raw extraction entirely — the intermediate
/// `DV_raw.hevc` is deleted once the clean BL is complete, so re-extracting it on resume
/// would waste a full-size disk pass for nothing.
pub(super) fn extract_clean_base_layer(
    input_file: &str,
    temp_dir: &Path,
    message: &str,
    args: &Args,
    resume_enabled: bool,
) -> Result<PathBuf> {
    let clean_bl = temp_dir.join("BL_clean.hevc");
    if resume_enabled && resume::is_complete(&clean_bl) {
        progress::print_info("Reusing Dolby Vision-clean base layer from a previous run.");
        return Ok(clean_bl);
    }

    let raw_hevc = temp_dir.join("DV_raw.hevc");
    let success = run_dovi_video_step(
        input_file,
        &raw_hevc,
        &clean_bl,
        temp_dir,
        args,
        resume_enabled,
        message,
        "dovi_remove",
        "Removing existing Dolby Vision metadata",
        |input| {
            let mut command = external::dovi_tool_command();
            command.args([
                "remove",
                "-i",
                input.to_str().unwrap(),
                "-o",
                clean_bl.to_str().unwrap(),
            ]);
            command
        },
    )?;

    if success {
        resume::mark_done(&clean_bl)?;
        let _ = fs::remove_file(&raw_hevc);
        let _ = fs::remove_file(resume::marker_path(&raw_hevc));
        Ok(clean_bl)
    } else {
        anyhow::bail!("Failed to remove Dolby Vision metadata from base layer")
    }
}

pub(super) fn convert_mel_to_profile81(
    input_file: &str,
    temp_dir: &Path,
    output_file: &Path,
    args: &Args,
    resume_enabled: bool,
) -> Result<bool> {
    let raw_hevc = temp_dir.join("DV_raw.hevc");
    let converted_hevc = temp_dir.join("P81_discard.hevc");

    if resume_enabled && resume::is_complete(&converted_hevc) {
        progress::print_info("Reusing converted Profile 8.1 HEVC from a previous run.");
    } else {
        let success = run_dovi_video_step(
            input_file,
            &raw_hevc,
            &converted_hevc,
            temp_dir,
            args,
            resume_enabled,
            "Extracting Profile 7 MEL HEVC stream",
            "dovi_convert_discard",
            "Converting MEL RPU to Profile 8.1 and discarding EL",
            |input| {
                let mut command = external::dovi_tool_command();
                command.args([
                    "-m",
                    "2",
                    "convert",
                    "--discard",
                    "-i",
                    input.to_str().unwrap(),
                    "-o",
                    converted_hevc.to_str().unwrap(),
                ]);
                command
            },
        )?;

        if !success {
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

pub(super) fn generate_rpu(
    hdr_type: HdrFormat,
    temp_dir: &Path,
    peak_source: PeakSource,
    meta_file: Option<&Path>,
    meas_file: Option<&Path>,
    embedded_l1_shots: bool,
    custom_composer: Option<Composer>,
    resume: bool,
) -> Result<Option<PathBuf>> {
    let rpu_out = temp_dir.join("RPU.bin");
    if resume && resume::is_complete(&rpu_out) {
        progress::print_info("Reusing generated RPU from a previous run.");
        return Ok(Some(rpu_out));
    }
    let extra_json = temp_dir.join("extra.json");
    let mut cmd = external::dovi_tool_command();
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
            // With embedded shots, the config already carries measured per-scene L1;
            // passing --madvr-file would override it (madVR-derived L1 wins in dovi_tool).
            if !embedded_l1_shots {
                cmd.arg("--madvr-file").arg(meas_file.unwrap());
                cmd.arg("--use-custom-targets");
            }
        }
        _ => return Ok(None),
    }

    let log_path = temp_dir.join("dovi_gen.log");
    if !run_command_with_spinner(&mut cmd, &log_path, "Generating Dolby Vision RPU")? {
        return Ok(None);
    }
    // dovi_tool always writes the preset composer. Install the selected one before the sentinel,
    // so an interrupted or failed rewrite is never reused as a finished RPU.
    if let Some(composer) = custom_composer {
        let frames = dovi84_composer::rewrite_rpu_file(&rpu_out, composer).with_context(|| {
            format!(
                "installing the {} HLG composer into {}",
                composer.cli_name(),
                rpu_out.display()
            )
        })?;
        progress::print_info(&format!(
            "Installed the {} HLG composer ({}) on {frames} RPU frames.",
            composer.cli_name(),
            composer.luminance_mapping()
        ));
    }
    resume::mark_done(&rpu_out)?;
    Ok(Some(rpu_out))
}
