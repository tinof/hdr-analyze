//! Delivery phases of `convert_file`: the base layer with its RPU (in process, or extracted and
//! injected by dovi_tool) and the mux.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::cli::{Args, DoviInject};
use crate::external::{self, run_command_with_progress};
use crate::inject::{self, Outcome, Pass, Request, Rpus, Source};
use crate::metadata::{self, HdrFormat};
use crate::progress;
use crate::resume;

use super::measure::Measured;
use super::timing::MuxTiming;

/// The lossless SPS/VUI edit (slices untouched) that writes the HLG transfer into the bitstream:
/// without an HLG tag in it mkvmerge would give the Profile 8.4 track compatibility ID 2 (SDR)
/// instead of 4.
const HLG_VUI_EDIT: &str =
    "hevc_metadata=colour_primaries=9:transfer_characteristics=18:matrix_coefficients=9";

/// Whether the base layer of `input_file` gets [`HLG_VUI_EDIT`]: HLG tagged only in the
/// container. Probes the input once and says so when it does.
fn needs_hlg_vui_edit(input_file: &str, hdr_type: HdrFormat) -> bool {
    let edit = hdr_type == HdrFormat::Hlg && metadata::hlg_bitstream_lacks_transfer(input_file);
    if edit {
        progress::print_info(
            "HLG is tagged only in the container; writing the HLG transfer into the HEVC VUI.",
        );
    }
    edit
}

/// ffmpeg copying the first video stream of `bl_source_file` as raw HEVC, to `output` with a
/// `-stats` readout or, for `None`, to its stdout. Both injection paths read this stream.
fn base_layer_ffmpeg(bl_source_file: &Path, hlg_vui_edit: bool, output: Option<&Path>) -> Command {
    let mut ffmpeg_cmd = Command::new("ffmpeg");
    ffmpeg_cmd.args(["-hide_banner", "-loglevel", "error"]);
    if output.is_some() {
        ffmpeg_cmd.arg("-stats");
    }
    ffmpeg_cmd
        .arg("-i")
        .arg(bl_source_file)
        .args(["-map", "0:v:0", "-c:v", "copy"]);
    if hlg_vui_edit {
        ffmpeg_cmd.args(["-bsf:v", HLG_VUI_EDIT]);
    }
    ffmpeg_cmd.args(["-f", "hevc"]);
    match output {
        Some(path) => ffmpeg_cmd.arg("-y").arg(path),
        None => ffmpeg_cmd.arg("-"),
    };
    ffmpeg_cmd
}

/// The two steps that give `BL_RPU.hevc`, each with its step header (`steps`: the current step,
/// advanced here, and the total). By default the RPU is written in process (`crate::inject`,
/// ROADMAP E20.1) from the raw base layer, piped from ffmpeg unless it is a raw file already.
/// Under `--dovi-inject external`, and with a warning for an `RPU.bin` the in-process path
/// cannot read, ffmpeg extracts `BL.hevc` and `dovi_tool inject-rpu` writes the RPU. `None`: a
/// step failed and was reported; `convert_file` returns `Ok(false)`.
pub(super) fn inject_base_layer(
    input_file: &str,
    measured: &Measured,
    rpu_path: &Path,
    temp_dir: &Path,
    resume_enabled: bool,
    args: &Args,
    steps: (&mut u8, u8),
) -> Result<Option<PathBuf>> {
    let (current_step, total_steps) = steps;
    let mut step = |message: &str| {
        *current_step += 1;
        progress::print_step(*current_step, total_steps, message);
    };
    if args.dovi_inject == DoviInject::Internal {
        let bl_rpu_hevc = temp_dir.join("BL_RPU.hevc");
        if resume_enabled && resume::is_complete(&bl_rpu_hevc) {
            step("Reading base layer frame order...");
            step("Injecting RPU into base layer...");
            progress::print_info("Reusing RPU-injected base layer from a previous run.");
            return Ok(Some(bl_rpu_hevc));
        }
        match Rpus::prepare(rpu_path) {
            Ok(rpus) => {
                return inject_in_process(
                    input_file,
                    measured,
                    &rpus,
                    temp_dir,
                    resume_enabled,
                    args,
                    &mut step,
                )
            }
            Err(error) => progress::print_warn(&format!(
                "RPU injection: {error:#}. Writing the RPU with dovi_tool inject-rpu instead."
            )),
        }
    }

    step("Extracting base layer...");
    let Some(bl_hevc) = extract_base_layer(
        input_file,
        &measured.bl_source_file,
        temp_dir,
        measured.hdr_type,
        resume_enabled,
        args,
    )?
    else {
        return Ok(None);
    };
    step("Injecting RPU into base layer...");
    inject_rpu(
        &bl_hevc,
        rpu_path,
        temp_dir,
        measured.measurements_file.is_some(),
        resume_enabled,
        args,
    )
}

/// `inject::inject` into `BL_RPU.hevc`, which is sealed only when the injection succeeded and
/// removed otherwise. A measured RPU whose length differs from the pictures is refused before
/// anything is written.
fn inject_in_process(
    input_file: &str,
    measured: &Measured,
    rpus: &Rpus,
    temp_dir: &Path,
    resume_enabled: bool,
    args: &Args,
    step: &mut dyn FnMut(&str),
) -> Result<Option<PathBuf>> {
    step("Reading base layer frame order...");
    let bl_source_file = &measured.bl_source_file;
    let extracted = temp_dir.join("BL.hevc");
    let mut source = if bl_source_file.extension().is_some_and(|ext| ext == "hevc") {
        progress::print_info("Base layer is already a raw HEVC stream; reading it directly.");
        Source::RawFile(bl_source_file.clone())
    } else if resume_enabled && resume::is_complete(&extracted) {
        progress::print_info("Reusing extracted base layer from a previous run.");
        Source::RawFile(extracted)
    } else {
        let hlg_vui_edit = needs_hlg_vui_edit(input_file, measured.hdr_type);
        Source::Ffmpeg {
            command: base_layer_ffmpeg(bl_source_file, hlg_vui_edit, None),
            logs: [
                temp_dir.join("ffmpeg_inject_pass1.log"),
                temp_dir.join("ffmpeg_inject_pass2.log"),
            ],
        }
    };
    let size_hint = match &source {
        Source::RawFile(path) => fs::metadata(path),
        Source::Ffmpeg { .. } => fs::metadata(bl_source_file),
    }
    .ok()
    .map(|metadata| metadata.len());

    let bl_rpu_hevc = temp_dir.join("BL_RPU.hevc");
    let file = File::create(&bl_rpu_hevc)
        .with_context(|| format!("creating {}", bl_rpu_hevc.display()))?;
    let mut output = BufWriter::with_capacity(1 << 20, file);
    let outcome = inject::inject(
        Request {
            source: &mut source,
            rpus,
            refuse_mismatch: measured.measurements_file.is_some(),
            chunk_size: inject::CHUNK_SIZE,
            size_hint,
            stall_secs: args.stall_timeout,
            announce: &mut |pass| {
                if pass == Pass::Write {
                    step("Injecting RPU into base layer...");
                }
            },
        },
        &mut output,
    );
    drop(output);

    let report = match outcome {
        Ok(Outcome::Injected(report)) => report,
        Ok(Outcome::LengthMismatch { frames, rpus }) => {
            progress::print_error(&format!(
                "RPU injection: mismatched lengths. video {frames}, RPU {rpus}. The measured RPU does not match the video picture for picture; refusing to mux it."
            ));
            let _ = fs::remove_file(&bl_rpu_hevc);
            return Ok(None);
        }
        Err(error) => {
            progress::print_error(&format!(
                "RPU injection failed: {error:#}. --dovi-inject external writes the RPU with dovi_tool inject-rpu instead."
            ));
            let _ = fs::remove_file(&bl_rpu_hevc);
            return Ok(None);
        }
    };
    if report.frames != report.rpus {
        let rule = if report.rpus < report.frames {
            "the last RPU is repeated to the end"
        } else {
            "the RPUs past the last picture are dropped"
        };
        progress::print_warn(&format!(
            "RPU injection: mismatched lengths. video {}, RPU {}; {rule}.",
            report.frames, report.rpus
        ));
    }
    resume::mark_done(&bl_rpu_hevc)?;
    // As after dovi_tool's injection: a temp-dir input is superseded (resume never re-reads it
    // once BL_RPU.hevc is sealed); an input outside the temp dir is kept.
    if let Source::RawFile(path) = &source {
        if path.starts_with(temp_dir) {
            remove_superseded(path);
        }
    }
    Ok(Some(bl_rpu_hevc))
}

/// Removes a sealed temp-dir stream that `BL_RPU.hevc` supersedes, its sentinel first. A
/// sentinel left beside the missing file would seal whatever a later run starts to write there
/// and does not finish, and the run after that would resume from the partial stream.
fn remove_superseded(stream: &Path) {
    let _ = resume::invalidate(stream);
    let _ = fs::remove_file(stream);
}

/// `--dovi-inject external`, step 1: the raw HEVC base layer the RPU is injected into,
/// `bl_source_file` itself when it already is one, else `BL.hevc` extracted into `temp_dir`. `None`: the extraction failed and was reported;
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
            let hlg_vui_edit = needs_hlg_vui_edit(input_file, hdr_type);
            let mut ffmpeg_cmd = base_layer_ffmpeg(bl_source_file, hlg_vui_edit, Some(&bl_hevc));

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

/// `--dovi-inject external`, step 2: `dovi_tool inject-rpu` of the RPU into `bl_hevc`, giving
/// `BL_RPU.hevc`. A length mismatch is fatal for a
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
            remove_superseded(bl_hevc);
        }
    }
    Ok(Some(bl_rpu_hevc))
}

/// Muxes `bl_rpu_hevc`, timed by `timing`, with every non-video track of the input into
/// `output_file`. Returns false when mkvmerge failed.
pub(super) fn mux_output(
    input_file: &str,
    bl_rpu_hevc: &Path,
    output_file: &Path,
    temp_dir: &Path,
    resume_enabled: bool,
    args: &Args,
    timing: &MuxTiming,
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

        mkvmerge_cmd.args(timing.mkvmerge_args()).arg(bl_rpu_hevc);
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
