//! The source's video timing in the output (ROADMAP E7.3).
//!
//! mkvmerge times a raw HEVC stream from the stream's frame rate, starting at 0. So the mux
//! shifts a late start with `--sync`, and gives a variable-rate source its own timestamps with
//! `--timestamps`. mkvmerge takes the track's default duration from the timestamps file's first
//! frame intervals, and `--default-duration` does not override it (mkvmerge v82). So the file
//! puts every frame that lies on the source's frame-rate grid at its exact grid time: in whole
//! milliseconds, 23.976 fps would get 42 ms, which players read as 23.81 fps.
//!
//! Every mux is then checked: the output's video frame times must equal the source's, which
//! also fixes the start and the duration. A mismatch retries once with the timestamps file and
//! then fails the run, so the source is kept.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::external;
use crate::progress;

/// Largest difference between a source and an output frame time that still counts as equal:
/// mkvmerge and the source's muxer may round one frame time to different milliseconds.
pub(super) const TOLERANCE_MS: f64 = 1.0;

/// The timestamps v2 file written for a `--timestamps` mux, in the temp dir.
pub(super) const TIMESTAMPS_FILE: &str = "video.timestamps.txt";

/// The mux's completion sentinel in the temp dir (`deliver::mux_output`, the MEL fast path).
const MUX_MARKER: &str = "mux.done";

/// Largest distance from a frame time to its frame-rate grid time that [`on_rate_grid`] closes.
/// Below half a millisecond, with a margin for the file's six decimals, the output's whole
/// millisecond is still the source frame's own (mkvmerge rounds 500.5 up to 501).
const GRID_SNAP_MS: f64 = 0.499;

/// How the mux times the video track.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum MuxTiming {
    /// mkvmerge's timestamps from the stream's frame rate, shifted by `sync_ms` (`--sync`).
    Stream { sync_ms: i64 },
    /// The source's timestamps, from a timestamps v2 file (`--timestamps`).
    File(PathBuf),
}

impl MuxTiming {
    /// The mkvmerge options placed before the HEVC file, whose video is its track 0.
    pub(super) fn mkvmerge_args(&self) -> Vec<String> {
        match self {
            Self::Stream { sync_ms: 0 } => Vec::new(),
            Self::Stream { sync_ms } => vec!["--sync".to_owned(), format!("0:{sync_ms}")],
            Self::File(path) => vec!["--timestamps".to_owned(), format!("0:{}", path.display())],
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Stream { sync_ms: 0 } => "with the stream's frame timing".to_owned(),
            Self::Stream { sync_ms } => format!("with the stream's frame timing from {sync_ms} ms"),
            Self::File(_) => "with the source's timestamps".to_owned(),
        }
    }
}

/// The frame times of a file's first video track in milliseconds, in presentation order, the
/// end of the last frame when the container gives its duration, and the track's frame duration
/// when the container gives a frame rate (ffprobe's `avg_frame_rate`; Matroska's default
/// duration).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct VideoTimes {
    pub(super) frames: Vec<f64>,
    pub(super) end: Option<f64>,
    pub(super) frame_ms: Option<f64>,
}

/// What the mux delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Delivery {
    /// mkvmerge failed, or the output's video timing differs from the source's (reported).
    Failed,
    /// The output's video frame times equal the source's; it has `video_frames` frames, which
    /// the RPU count check (ROADMAP E7.2) takes from here.
    Checked { video_frames: usize },
    /// The source's timing could not be read, so the output was not checked (reported).
    Unchecked,
}

/// Muxes with `mux` so that the output's video timing equals the source's, and checks it.
/// `mux` runs mkvmerge with the given timing (or reuses a finished mux on resume) and returns
/// false when mkvmerge failed, which it reports. A mismatch removes the output and its sentinel.
pub(super) fn deliver(
    input_file: &str,
    output_file: &Path,
    temp_dir: &Path,
    resume_enabled: bool,
    mux: &mut dyn FnMut(&MuxTiming) -> Result<bool>,
) -> Result<Delivery> {
    progress::print_info("Reading the source's video timestamps...");
    let source = match read_video_times(Path::new(input_file)) {
        Ok(times) => times,
        Err(reason) => {
            progress::print_warn(&format!(
                "The source's video timestamps could not be read: {reason}. The output keeps \
                 the stream's own timing, it is not checked against the source, and the source \
                 is kept."
            ));
            let muxed = mux(&MuxTiming::Stream { sync_ms: 0 })?;
            return Ok(if muxed {
                Delivery::Unchecked
            } else {
                Delivery::Failed
            });
        }
    };

    let file = MuxTiming::File(temp_dir.join(TIMESTAMPS_FILE));
    let chosen = if is_variable_rate(&source.frames) {
        progress::print_info(
            "The source has a variable frame rate; the output takes its timestamps.",
        );
        file.clone()
    } else {
        MuxTiming::Stream {
            sync_ms: source.frames[0].round() as i64,
        }
    };
    // A finished mux of an earlier run is checked as well; when it fails, the chosen timing is
    // muxed afresh before the timestamps file.
    let reused = resume_enabled && output_file.exists() && temp_dir.join(MUX_MARKER).exists();
    let mut plan = vec![chosen.clone()];
    if reused {
        plan.push(chosen.clone());
    }
    if chosen != file {
        plan.push(file);
    }

    for (attempt, timing) in plan.iter().enumerate() {
        if let MuxTiming::File(path) = timing {
            fs::write(path, timestamps_v2(&on_rate_grid(&source)))
                .context("Failed to write the video timestamps file")?;
        }
        if !mux(timing)? {
            return Ok(Delivery::Failed);
        }
        progress::print_info("Checking the output's video timestamps against the source's...");
        let mismatch = match read_video_times(output_file) {
            Ok(output) => match compare(&source.frames, &output.frames) {
                Ok(()) => {
                    return Ok(Delivery::Checked {
                        video_frames: output.frames.len(),
                    })
                }
                Err(mismatch) => mismatch,
            },
            Err(reason) => format!("the output's video timestamps could not be read: {reason}"),
        };
        discard_output(output_file, temp_dir);
        match plan.get(attempt + 1) {
            Some(next) => progress::print_warn(&format!(
                "The output's video timing differs from the source's ({mismatch}); muxing again {}.",
                next.describe()
            )),
            None => progress::print_error(&format!(
                "The output's video timing differs from the source's ({mismatch}). The output \
                 was removed and the source kept."
            )),
        }
    }
    Ok(Delivery::Failed)
}

/// Removes the output and its mux sentinel, so a resumed run muxes afresh.
pub(super) fn discard_output(output_file: &Path, temp_dir: &Path) {
    let _ = fs::remove_file(output_file);
    let _ = fs::remove_file(temp_dir.join(MUX_MARKER));
}

/// The video frame times of a Matroska file, read by ffprobe. `Err` says why they could not be
/// read.
pub(super) fn read_video_times(file: &Path) -> Result<VideoTimes, String> {
    let mut command = Command::new("ffprobe");
    command
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .arg("packet=pts,duration:stream=time_base,avg_frame_rate:format=format_name")
        .args(["-of", "json"])
        .arg(file);
    let output = external::get_command_output(&mut command)
        .map_err(|error| format!("ffprobe could not read the file ({error})"))?;
    let json: Value = serde_json::from_str(&output)
        .map_err(|error| format!("ffprobe's output is not JSON ({error})"))?;
    video_times_from_ffprobe(&json)
}

/// [`read_video_times`] from ffprobe's JSON. Only Matroska is read: mkvmerge, which muxes the
/// output, may read the start of another container differently from ffprobe.
pub(super) fn video_times_from_ffprobe(json: &Value) -> Result<VideoTimes, String> {
    let format = json
        .pointer("/format/format_name")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if !format.split(',').any(|name| name == "matroska") {
        return Err(format!("the file is not Matroska ({format})"));
    }
    // `24000/1001` frames per second is 1001/24000 s per frame; ffprobe writes `0/0` for none.
    let frame_ms = json
        .pointer("/streams/0/avg_frame_rate")
        .and_then(Value::as_str)
        .and_then(|rate| rate.split_once('/'))
        .and_then(|(num, den)| ms_per_tick(&format!("{den}/{num}")));
    let ms_per_tick = json
        .pointer("/streams/0/time_base")
        .and_then(Value::as_str)
        .and_then(ms_per_tick)
        .ok_or("the file has no video stream")?;
    let packets = json
        .get("packets")
        .and_then(Value::as_array)
        .filter(|packets| !packets.is_empty())
        .ok_or("the file has no video packets")?;

    let mut frames = Vec::with_capacity(packets.len());
    let mut last: Option<(i64, i64)> = None;
    for packet in packets {
        let pts = packet
            .get("pts")
            .and_then(Value::as_i64)
            .ok_or("a video packet has no timestamp")?;
        let duration = packet.get("duration").and_then(Value::as_i64).unwrap_or(0);
        if last.is_none_or(|(latest, _)| pts > latest) {
            last = Some((pts, duration));
        }
        frames.push(pts as f64 * ms_per_tick);
    }
    frames.sort_by(f64::total_cmp);
    let end = last
        .filter(|&(_, duration)| duration > 0)
        .map(|(pts, duration)| (pts + duration) as f64 * ms_per_tick);
    Ok(VideoTimes {
        frames,
        end,
        frame_ms,
    })
}

/// Milliseconds per tick of an ffprobe time base such as `1/1000`.
fn ms_per_tick(time_base: &str) -> Option<f64> {
    let (num, den) = time_base.split_once('/')?;
    let (num, den) = (num.parse::<f64>().ok()?, den.parse::<f64>().ok()?);
    (num > 0.0 && den > 0.0).then(|| num * 1000.0 / den)
}

/// True when a frame interval differs from the mean interval by more than [`TOLERANCE_MS`].
/// 23.976 fps in whole milliseconds (41 and 42 ms around a mean of 41.708) is a constant rate.
pub(super) fn is_variable_rate(frames: &[f64]) -> bool {
    let [first, .., last] = frames else {
        return false;
    };
    let mean = (last - first) / (frames.len() - 1) as f64;
    frames
        .windows(2)
        .any(|pair| (pair[1] - pair[0] - mean).abs() > TOLERANCE_MS)
}

/// `Err` names the first difference between the output's and the source's frame times
/// (presentation order): the frame count, the start, or a later frame.
pub(super) fn compare(source: &[f64], output: &[f64]) -> Result<(), String> {
    if source.len() != output.len() {
        return Err(format!(
            "the output has {} video frames, the source {}",
            output.len(),
            source.len()
        ));
    }
    let Some(index) = source
        .iter()
        .zip(output)
        .position(|(source, output)| (source - output).abs() > TOLERANCE_MS)
    else {
        return Ok(());
    };
    let span = |frames: &[f64]| {
        format!(
            "{} to {} ms",
            format_ms(frames[0]),
            format_ms(frames[frames.len() - 1])
        )
    };
    let detail = if index == 0 {
        format!(
            "video starts at {} ms, the source's at {} ms",
            format_ms(output[0]),
            format_ms(source[0])
        )
    } else {
        format!(
            "frame {index} is at {} ms, the source's at {} ms",
            format_ms(output[index]),
            format_ms(source[index])
        )
    };
    Err(format!(
        "{detail}; frames {} against {}",
        span(output),
        span(source)
    ))
}

/// `times` with every frame (and the end, as frame `frames.len()`) that lies within
/// [`GRID_SNAP_MS`] of its frame-rate grid time, `frames[0] + k * frame_ms`, moved onto it. The
/// other frames keep their times; without a frame duration nothing moves.
pub(super) fn on_rate_grid(times: &VideoTimes) -> VideoTimes {
    let (Some(frame_ms), Some(&first)) = (times.frame_ms, times.frames.first()) else {
        return times.clone();
    };
    let snap = |k: usize, time: f64| {
        let grid = first + k as f64 * frame_ms;
        if (time - grid).abs() <= GRID_SNAP_MS {
            grid
        } else {
            time
        }
    };
    VideoTimes {
        frames: times
            .frames
            .iter()
            .enumerate()
            .map(|(k, &time)| snap(k, time))
            .collect(),
        end: times.end.map(|end| snap(times.frames.len(), end)),
        frame_ms: times.frame_ms,
    }
}

/// A timestamps v2 file for mkvmerge `--timestamps`: one line per frame in presentation order,
/// then the end of the last frame when it is known, which keeps the last frame's duration.
pub(super) fn timestamps_v2(times: &VideoTimes) -> String {
    let mut text = String::from("# timestamp format v2\n");
    for frame in &times.frames {
        let _ = writeln!(text, "{}", format_ms(*frame));
    }
    let last = times.frames.last().copied().unwrap_or(f64::NEG_INFINITY);
    if let Some(end) = times.end.filter(|&end| end > last) {
        let _ = writeln!(text, "{}", format_ms(end));
    }
    text
}

/// Milliseconds with up to six decimals and no trailing zeros (`41.708333`, `2000`).
fn format_ms(ms: f64) -> String {
    let text = format!("{ms:.6}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests;
