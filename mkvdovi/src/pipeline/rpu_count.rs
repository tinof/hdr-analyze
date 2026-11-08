//! The output's RPU count against its video frames (ROADMAP E7.2).
//!
//! Every mux is checked: the output must carry one RPU per video frame. `--verify` compares the
//! same counts, but it is opt-in; this check runs on every conversion, after the timing check
//! (E7.3), whose packet read gives the video frame count. The RPUs are extracted from the output
//! with `dovi_tool extract-rpu` and counted, not parsed, so a passed-through RPU that the
//! `dolby_vision` parser would abort on cannot stop the run here.
//!
//! dovi_tool 2.3.4 and later read the Matroska file directly (`--dovi-input mkv`). That count is
//! accepted only when it equals the video's. Any other result is counted again from the output's
//! video stream extracted by ffmpeg, the reading `--verify` uses, and that count decides; so does
//! it alone under `--dovi-input raw`. The check fails closed: a count that differs, or one that
//! cannot be read, removes the output and fails the run, so the source is kept and the next run
//! muxes and counts again.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use crate::cli::{Args, DoviInput};
use crate::external::{self, run_command_with_progress, run_command_with_spinner};
use crate::metadata;
use crate::progress;
use crate::rpu_guard;

use super::timing::{self, Delivery};

/// The RPUs the check extracts into the temp dir; removed after each count.
pub(super) const RPU_FILE: &str = "output_rpu.bin";
/// The output's video stream for the second count; removed after it.
const VIDEO_FILE: &str = "output_video.hevc";
/// dovi_tool's error for a stream without RPUs (2.3.4), which writes no file.
const NO_RPU: &str = "No RPU was found";

/// A count, or why it could not be read.
type Count = Result<usize, String>;

/// Checks that `output_file` carries one RPU per video frame. `delivery` is what the timing check
/// found; without its frame count the output's frames are read here. False: the counts differ
/// or could not be read; the output was removed and the error reported.
pub(super) fn check(output_file: &Path, temp_dir: &Path, delivery: Delivery, args: &Args) -> bool {
    let started = Instant::now();
    progress::print_info("Checking the output's RPU count against its video frames...");
    let video_frames = match video_frames(output_file, delivery) {
        Ok(frames) => frames,
        Err(reason) => {
            return fail(
                output_file,
                temp_dir,
                &format!("The output's video frames could not be counted ({reason})"),
            )
        }
    };
    let counted = count_rpus(output_file, temp_dir, video_frames, args);
    let _ = fs::remove_file(temp_dir.join(RPU_FILE));
    match counted {
        Ok(rpus) if rpus == video_frames => {
            progress::print_info(&format!(
                "RPU count: {rpus} RPUs for {video_frames} video frames (checked in {}).",
                progress::format_duration_pub(started.elapsed())
            ));
            true
        }
        Ok(rpus) => fail(
            output_file,
            temp_dir,
            &format!("The output carries {rpus} RPUs for its {video_frames} video frames"),
        ),
        Err(reason) => fail(
            output_file,
            temp_dir,
            &format!("The output's RPUs could not be counted ({reason})"),
        ),
    }
}

fn fail(output_file: &Path, temp_dir: &Path, problem: &str) -> bool {
    timing::discard_output(output_file, temp_dir);
    progress::print_error(&format!(
        "{problem}. The output was removed and the source kept."
    ));
    false
}

/// The output's video frames: the timing check's packet count, else ffprobe, else MediaInfo
/// (mkvmerge writes the frame count into the track statistics, so it is exact here).
fn video_frames(output_file: &Path, delivery: Delivery) -> Count {
    if let Delivery::Checked { video_frames } = delivery {
        return Ok(video_frames);
    }
    match timing::read_video_times(output_file) {
        Ok(times) => Ok(times.frames.len()),
        Err(ffprobe) => output_file
            .to_str()
            .and_then(metadata::get_frame_count)
            .and_then(|frames| usize::try_from(frames).ok())
            .ok_or_else(|| format!("{ffprobe}; MediaInfo gives no frame count either")),
    }
}

/// The output's RPUs: the direct read when it matches `video_frames`, else the count from the
/// extracted video stream.
fn count_rpus(output_file: &Path, temp_dir: &Path, video_frames: usize, args: &Args) -> Count {
    if args.dovi_input == DoviInput::Mkv {
        let direct = extract_and_count(output_file, temp_dir, "check_rpu_count_mkv.log");
        if direct == Ok(video_frames) {
            return direct;
        }
        progress::print_warn(&format!(
            "dovi_tool's direct read of the output gave {}; counting again from its extracted \
             video stream.",
            match &direct {
                Ok(rpus) => format!("{rpus} RPUs"),
                Err(reason) => format!("no count ({reason})"),
            }
        ));
    }
    count_from_video_stream(output_file, temp_dir, args)
}

/// Extracts the output's video stream with ffmpeg, counts its RPUs and removes the stream.
fn count_from_video_stream(output_file: &Path, temp_dir: &Path, args: &Args) -> Count {
    let video = temp_dir.join(VIDEO_FILE);
    let log = temp_dir.join("check_rpu_count_extract.log");
    let mut ffmpeg = Command::new("ffmpeg");
    ffmpeg
        .args(["-hide_banner", "-loglevel", "error", "-stats", "-i"])
        .arg(output_file)
        .args(["-map", "0:v:0", "-c:v", "copy", "-f", "hevc", "-y"])
        .arg(&video);
    let total = fs::metadata(output_file)
        .ok()
        .map(|metadata| metadata.len());
    let extracted = run_command_with_progress(
        &mut ffmpeg,
        &log,
        "Extracting the output's video stream",
        &video,
        total,
        args.stall_timeout,
    );
    let counted = match extracted {
        Ok(true) => extract_and_count(&video, temp_dir, "check_rpu_count.log"),
        Ok(false) => Err(format!(
            "ffmpeg could not extract its video stream, see {}",
            log.display()
        )),
        Err(error) => Err(format!("ffmpeg could not run ({error:#})")),
    };
    let _ = fs::remove_file(&video);
    counted
}

/// `dovi_tool extract-rpu` of `input` into [`RPU_FILE`], counted.
fn extract_and_count(input: &Path, temp_dir: &Path, log_name: &str) -> Count {
    let rpu_file = temp_dir.join(RPU_FILE);
    // An RPU file of an earlier count or run must never stand in for this one.
    let _ = fs::remove_file(&rpu_file);
    let log = temp_dir.join(log_name);
    let mut command = external::dovi_tool_command();
    command
        .arg("extract-rpu")
        .arg("-i")
        .arg(input)
        .arg("-o")
        .arg(&rpu_file);
    let succeeded = run_command_with_spinner(&mut command, &log, "Counting the output's RPUs")
        .map_err(|error| format!("dovi_tool could not run ({error:#})"))?;
    let log_text = fs::read_to_string(&log).unwrap_or_default();
    let rpus = fs::read(&rpu_file).ok();
    count_of_extraction(succeeded, &log_text, rpus.as_deref())
        .ok_or_else(|| format!("dovi_tool extract-rpu failed, see {}", log.display()))
}

/// The RPU count of one `extract-rpu` run, from its exit status, its log and the file it wrote.
/// dovi_tool fails on a stream without RPUs and names it: that is a count of 0.
fn count_of_extraction(succeeded: bool, log: &str, rpu_file: Option<&[u8]>) -> Option<usize> {
    match (succeeded, rpu_file) {
        (true, Some(bytes)) => Some(rpu_guard::count_rpus(bytes)),
        (false, _) if log.contains(NO_RPU) => Some(0),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
