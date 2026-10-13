//! Delivery phases of `convert_file`: base-layer extraction, RPU injection and the mux.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;

use crate::cli::Args;
use crate::external::{self, run_command_with_progress};
use crate::metadata::{self, HdrFormat};
use crate::progress;
use crate::resume;

/// The raw HEVC base layer the RPU is injected into: `bl_source_file` itself when it already is
/// one, else `BL.hevc` extracted into `temp_dir`. `None`: the extraction failed and was reported;
/// `convert_file` returns `Ok(false)`.
pub(super) fn extract_base_layer(
    input_file: &str,
    bl_source_file: &Path,
    temp_dir: &Path,
    hdr_type: HdrFormat,
    resume_enabled: bool,
    args: &Args,
) -> Result<Option<PathBuf>> {
    // When the BL source is already a raw Annex B HEVC stream in the temp dir (mdfix paths),
    // re-extracting it with ffmpeg would just duplicate ~the full video size on disk.
    let bl_hevc = if bl_source_file.extension().is_some_and(|ext| ext == "hevc") {
        progress::print_info("Base layer is already a raw HEVC stream; skipping re-extraction.");
        bl_source_file.to_path_buf()
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

            let bl_total = fs::metadata(bl_source_file).ok().map(|m| m.len());
            if !run_command_with_progress(
                &mut ffmpeg_cmd,
                &temp_dir.join("ffmpeg_extract_bl.log"),
                "Extracting base layer HEVC stream",
                &bl_hevc,
                bl_total,
                args.stall_timeout,
            )? {
                return Ok(None);
            }
            resume::mark_done(&bl_hevc)?;
        }

        bl_hevc
    };
    Ok(Some(bl_hevc))
}

/// Injects the RPU into `bl_hevc`, giving `BL_RPU.hevc`. A length mismatch is fatal for a
/// measured RPU (`has_measurements`). `None`: the step failed and was reported; `convert_file`
/// returns `Ok(false)`.
pub(super) fn inject_rpu(
    bl_hevc: &Path,
    rpu_path: &Path,
    temp_dir: &Path,
    has_measurements: bool,
    resume_enabled: bool,
    args: &Args,
) -> Result<Option<PathBuf>> {
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

        let inject_total = fs::metadata(bl_hevc).ok().map(|m| m.len());
        if !run_command_with_progress(
            &mut dovi_cmd,
            &temp_dir.join("dovi_inject.log"),
            "Injecting RPU into base layer",
            &bl_rpu_hevc,
            inject_total,
            args.stall_timeout,
        )? {
            return Ok(None);
        }
        // dovi_tool repeats or drops RPUs at the end when the RPU and the stream differ in
        // length, so measurements no longer line up with the pictures they were taken from.
        let inject_log = fs::read_to_string(temp_dir.join("dovi_inject.log")).unwrap_or_default();
        if let Some(line) = inject_log
            .lines()
            .find(|line| line.contains("mismatched lengths"))
        {
            if has_measurements {
                progress::print_error(&format!(
                    "dovi_tool inject-rpu: {}. The measured RPU does not match the video picture for picture; refusing to mux it.",
                    line.trim().trim_start_matches("Warning: ")
                ));
                let _ = fs::remove_file(&bl_rpu_hevc);
                return Ok(None);
            }
            progress::print_warn(&format!("dovi_tool inject-rpu: {}", line.trim()));
        }
        resume::mark_done(&bl_rpu_hevc)?;

        // The injected stream supersedes its temp-dir input; drop it early so the mux step
        // does not need both full-size streams on disk. Resume never re-reads it once
        // BL_RPU.hevc is sealed. Inputs outside the temp dir (e.g. the source MKV) are kept.
        if bl_hevc.starts_with(temp_dir) {
            let _ = fs::remove_file(bl_hevc);
        }
    }
    Ok(Some(bl_rpu_hevc))
}

/// Muxes `bl_rpu_hevc` with every non-video track of the input into `output_file`. Returns false
/// when mkvmerge failed.
pub(super) fn mux_output(
    input_file: &str,
    bl_rpu_hevc: &Path,
    output_file: &Path,
    temp_dir: &Path,
    resume_enabled: bool,
    args: &Args,
) -> Result<bool> {
    // The mux sentinel lives inside the temp dir (the output file is outside it), so cleanup
    // removes it and no stray marker is left beside the final `.DV.mkv`.
    let mux_marker = temp_dir.join("mux.done");

    if resume_enabled && output_file.exists() && mux_marker.exists() {
        progress::print_info("Reusing muxed output from a previous run.");
    } else {
        let mut mkvmerge_cmd = Command::new("mkvmerge");
        mkvmerge_cmd.arg("-q").arg("-o").arg(output_file);
        if args.drop_tags {
            mkvmerge_cmd.arg("--no-global-tags");
        }
        if args.drop_chapters {
            mkvmerge_cmd.arg("--no-chapters");
        }

        mkvmerge_cmd.arg(bl_rpu_hevc);
        mkvmerge_cmd.arg("--no-video").arg(input_file);

        let mux_total = fs::metadata(input_file).ok().map(|m| m.len());
        if !run_command_with_progress(
            &mut mkvmerge_cmd,
            &temp_dir.join("mkvmerge.log"),
            "Muxing final MKV",
            output_file,
            mux_total,
            args.stall_timeout,
        )? {
            return Ok(false);
        }
        let _ = fs::write(&mux_marker, b"");
    }
    Ok(true)
}
