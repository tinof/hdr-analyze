use std::cell::RefCell;

use serde_json::json;

use super::*;

/// ffprobe's JSON for a Matroska file with these packets (`pts`, `duration` in 1/1000 s).
fn ffprobe_json(packets: &[(i64, i64)]) -> Value {
    json!({
        "packets": packets
            .iter()
            .map(|&(pts, duration)| json!({"pts": pts, "duration": duration}))
            .collect::<Vec<_>>(),
        "streams": [{"time_base": "1/1000"}],
        "format": {"format_name": "matroska,webm"},
    })
}

#[test]
fn ffprobe_packets_give_sorted_frame_times_and_the_last_frames_end() {
    // Decode order with B-frames, as ffprobe lists them: the end is the latest pts + its duration.
    let json = ffprobe_json(&[(2000, 42), (2125, 42), (2083, 41), (2042, 42)]);
    let times = video_times_from_ffprobe(&json).unwrap();
    assert_eq!(times.frames, vec![2000.0, 2042.0, 2083.0, 2125.0]);
    assert_eq!(times.end, Some(2167.0));
}

#[test]
fn ffprobe_time_base_scales_ticks_to_milliseconds() {
    let mut json = ffprobe_json(&[(0, 0), (417_083, 0)]);
    json["streams"][0]["time_base"] = json!("1/10000000");
    let times = video_times_from_ffprobe(&json).unwrap();
    assert_eq!(times.frames, vec![0.0, 41.7083]);
    // Without a duration on the last frame its end is unknown.
    assert_eq!(times.end, None);
}

#[test]
fn ffprobe_json_that_is_not_readable_timing_is_refused_with_its_reason() {
    let mut mp4 = ffprobe_json(&[(0, 42)]);
    mp4["format"]["format_name"] = json!("mov,mp4,m4a,3gp,3g2,mj2");
    assert!(video_times_from_ffprobe(&mp4)
        .unwrap_err()
        .contains("not Matroska (mov,mp4"));

    let mut no_pts = ffprobe_json(&[(0, 42)]);
    no_pts["packets"]
        .as_array_mut()
        .unwrap()
        .push(json!({"duration": 42}));
    assert_eq!(
        video_times_from_ffprobe(&no_pts).unwrap_err(),
        "a video packet has no timestamp"
    );

    assert_eq!(
        video_times_from_ffprobe(&ffprobe_json(&[])).unwrap_err(),
        "the file has no video packets"
    );

    let mut no_stream = ffprobe_json(&[(0, 42)]);
    no_stream["streams"] = json!([]);
    assert_eq!(
        video_times_from_ffprobe(&no_stream).unwrap_err(),
        "the file has no video stream"
    );
}

#[test]
fn whole_millisecond_23_976_is_constant_rate_and_a_gap_or_rate_change_is_variable() {
    // 23.976 fps rounded to milliseconds: 0, 42, 83, 125, ... (k * 1001 / 24).
    let cfr: Vec<f64> = (0..48)
        .map(|k| (f64::from(k) * 1001.0 / 24.0).round())
        .collect();
    assert!(!is_variable_rate(&cfr));
    // 24 fps, then 30 fps after a gap (the VFR fixture of the integration tests).
    let vfr: Vec<f64> = (0..48)
        .map(|k| {
            if k < 24 {
                (f64::from(k) * 1000.0 / 24.0).round()
            } else {
                (1500.0 + f64::from(k - 24) * 1000.0 / 30.0).round()
            }
        })
        .collect();
    assert!(is_variable_rate(&vfr));
    // One frame 2 ms late: off the mean by more than the tolerance.
    let mut late = cfr.clone();
    late[10] += 2.0;
    assert!(is_variable_rate(&late));
    assert!(!is_variable_rate(&[]));
    assert!(!is_variable_rate(&[5.0]));
}

#[test]
fn compare_accepts_equal_times_within_the_tolerance_inclusive() {
    let source = [0.0, 41.708, 83.417, 125.125];
    assert_eq!(compare(&source, &source), Ok(()));
    // Another muxer rounds 41.708 up and 83.417 down: one millisecond apart is still equal.
    assert_eq!(compare(&source, &[0.0, 42.708, 82.417, 125.125]), Ok(()));
}

#[test]
fn compare_names_the_frame_count_the_start_or_the_first_later_frame() {
    let source = [2000.0, 2042.0, 2083.0];
    assert_eq!(
        compare(&source, &[2000.0, 2042.0]),
        Err("the output has 2 video frames, the source 3".to_owned())
    );
    assert_eq!(
        compare(&source, &[0.0, 42.0, 83.0]),
        Err("video starts at 0 ms, the source's at 2000 ms; frames 0 to 83 ms against 2000 to 2083 ms".to_owned())
    );
    assert_eq!(
        compare(&source, &[2000.0, 2042.0, 2084.5]),
        Err("frame 2 is at 2084.5 ms, the source's at 2083 ms; frames 2000 to 2084.5 ms against 2000 to 2083 ms".to_owned())
    );
}

#[test]
fn timestamps_v2_lists_every_frame_then_the_end_of_the_last() {
    let times = VideoTimes {
        frames: vec![2000.0, 2041.708333, 2083.0],
        end: Some(2125.0),
        frame_ms: None,
    };
    assert_eq!(
        timestamps_v2(&times),
        "# timestamp format v2\n2000\n2041.708333\n2083\n2125\n"
    );
    // An end that is unknown, or not after the last frame, would give it no duration.
    for end in [None, Some(42.0)] {
        let times = VideoTimes {
            frames: vec![0.0, 42.0],
            end,
            frame_ms: None,
        };
        assert_eq!(timestamps_v2(&times), "# timestamp format v2\n0\n42\n");
    }
}

#[test]
fn ffprobe_frame_rate_gives_the_frame_duration() {
    let mut json = ffprobe_json(&[(0, 42)]);
    json["streams"][0]["avg_frame_rate"] = json!("24000/1001");
    let frame_ms = video_times_from_ffprobe(&json).unwrap().frame_ms.unwrap();
    assert!((frame_ms - 41.708_333_333).abs() < 1e-9, "{frame_ms}");
    // ffprobe writes 0/0 when the container gives no rate.
    json["streams"][0]["avg_frame_rate"] = json!("0/0");
    assert_eq!(video_times_from_ffprobe(&json).unwrap().frame_ms, None);
}

#[test]
fn on_rate_grid_moves_whole_millisecond_frames_onto_the_frame_rate_grid() {
    // mkvmerge's 23.976 fps frame times (k * 41708333 ns, whole ms); frame 12 is at 500.499996.
    let frames = [0, 42, 83, 125, 167, 209, 250, 292, 334, 375, 417, 459, 500];
    let times = VideoTimes {
        frames: frames.iter().map(|&ms| f64::from(ms)).collect(),
        end: Some(542.0),
        frame_ms: Some(1001.0 / 24.0),
    };
    // k * 1001/24 to six decimals. Frame 12's grid time is 500.5, which mkvmerge would round
    // up to 501, so it keeps its own 500.
    assert_eq!(
        timestamps_v2(&on_rate_grid(&times)),
        "# timestamp format v2\n0\n41.708333\n83.416667\n125.125\n166.833333\n208.541667\n\
         250.25\n291.958333\n333.666667\n375.375\n417.083333\n458.791667\n500\n542.208333\n"
    );
    // A frame off the grid keeps its time; the grid starts at the first frame.
    let times = VideoTimes {
        frames: vec![2000.0, 2042.0, 2090.0],
        end: None,
        frame_ms: Some(1001.0 / 24.0),
    };
    assert_eq!(
        on_rate_grid(&times).frames,
        [2000.0, 2000.0 + 1001.0 / 24.0, 2090.0]
    );
    // Without a frame rate nothing moves.
    let times = VideoTimes {
        frame_ms: None,
        ..times
    };
    assert_eq!(on_rate_grid(&times), times);
}

#[test]
fn mkvmerge_args_shift_a_late_start_or_name_the_timestamps_file() {
    assert!(MuxTiming::Stream { sync_ms: 0 }.mkvmerge_args().is_empty());
    assert_eq!(
        MuxTiming::Stream { sync_ms: 2000 }.mkvmerge_args(),
        ["--sync", "0:2000"]
    );
    assert_eq!(
        MuxTiming::File(PathBuf::from("/t/video.timestamps.txt")).mkvmerge_args(),
        ["--timestamps", "0:/t/video.timestamps.txt"]
    );
}

/// Runs [`deliver`] with a fake mux that writes `outputs[n]` (an MKV made by mkvmerge) as the
/// output of its n-th call; returns the delivery and the timings it was called with.
fn deliver_with(
    source: &Path,
    temp: &Path,
    output: &Path,
    resume: bool,
    outputs: &[&Path],
) -> (Delivery, Vec<MuxTiming>) {
    let calls = RefCell::new(Vec::new());
    let delivery = deliver(
        source.to_str().unwrap(),
        &source.with_file_name("clip.hevc"),
        output,
        temp,
        resume,
        &mut |timing| {
            let mut calls = calls.borrow_mut();
            let next = outputs[calls.len()];
            calls.push(timing.clone());
            if !(resume && output.exists() && temp.join(MUX_MARKER).exists()) {
                fs::copy(next, output).unwrap();
                fs::write(temp.join(MUX_MARKER), b"").unwrap();
            }
            Ok(true)
        },
    )
    .unwrap();
    (delivery, calls.into_inner())
}

/// mkvmerge muxes of one x265 clip: (CFR from 0 ms, the same from 2000 ms, VFR from 0 ms).
/// `None` (after a skip message) when ffmpeg with libx265, ffprobe or mkvmerge is missing.
fn clips(dir: &Path, test: &str) -> Option<[PathBuf; 3]> {
    let tools = [
        ("ffmpeg", "-version"),
        ("ffprobe", "-version"),
        ("mkvmerge", "--version"),
    ];
    if tools
        .iter()
        .any(|(tool, arg)| Command::new(tool).arg(arg).output().is_err())
    {
        println!("Skipping {test}: ffmpeg, ffprobe or mkvmerge not found in PATH");
        return None;
    }
    let encoders = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .unwrap();
    if !String::from_utf8_lossy(&encoders.stdout).contains("libx265") {
        println!("Skipping {test}: ffmpeg has no libx265 encoder");
        return None;
    }
    let raw = dir.join("clip.hevc");
    let encoded = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("testsrc2=s=128x72:r=24000/1001")
        .args(["-frames:v", "12", "-c:v", "libx265", "-preset", "ultrafast"])
        .args(["-x265-params", "log-level=error:bframes=2:b-adapt=0"])
        .args(["-f", "hevc", "-y"])
        .arg(&raw)
        .status()
        .unwrap();
    assert!(encoded.success());
    let stamps = dir.join("v2.txt");
    let mut text = String::from("# timestamp format v2\n");
    for k in 0..12 {
        let _ = writeln!(text, "{}", if k < 6 { k * 40 } else { 400 + (k - 6) * 33 });
    }
    fs::write(&stamps, text).unwrap();
    let stamps = format!("0:{}", stamps.display());
    let mux = |name: &str, extra: &[&str]| {
        let out = dir.join(name);
        let status = Command::new("mkvmerge")
            .arg("-q")
            .arg("-o")
            .arg(&out)
            .args(extra)
            .arg(&raw)
            .status()
            .unwrap();
        assert!(status.success());
        out
    };
    Some([
        mux("cfr.mkv", &[]),
        mux("late.mkv", &["--sync", "0:2000"]),
        mux("vfr.mkv", &["--timestamps", &stamps]),
    ])
}

#[test]
fn deliver_shifts_a_late_start_and_checks_it() {
    let dir = tempfile::tempdir().unwrap();
    let Some([cfr, late, _]) = clips(dir.path(), "deliver_shifts_a_late_start_and_checks_it")
    else {
        return;
    };
    let (temp, output) = (dir.path().join("temp"), dir.path().join("out.mkv"));
    fs::create_dir(&temp).unwrap();
    let (delivery, calls) = deliver_with(&late, &temp, &output, false, &[&late]);
    assert_eq!(delivery, Delivery::Checked { video_frames: 12 });
    assert_eq!(calls, [MuxTiming::Stream { sync_ms: 2000 }]);
    assert!(!temp.join(TIMESTAMPS_FILE).exists());

    let (delivery, calls) = deliver_with(&cfr, &temp, &dir.path().join("out2.mkv"), false, &[&cfr]);
    assert_eq!(delivery, Delivery::Checked { video_frames: 12 });
    assert_eq!(calls, [MuxTiming::Stream { sync_ms: 0 }]);
}

#[test]
fn deliver_gives_a_variable_rate_source_its_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let Some([_, _, vfr]) = clips(
        dir.path(),
        "deliver_gives_a_variable_rate_source_its_timestamps",
    ) else {
        return;
    };
    let (temp, output) = (dir.path().join("temp"), dir.path().join("out.mkv"));
    fs::create_dir(&temp).unwrap();
    let (delivery, calls) = deliver_with(&vfr, &temp, &output, false, &[&vfr]);
    assert_eq!(delivery, Delivery::Checked { video_frames: 12 });
    assert_eq!(calls, [MuxTiming::File(temp.join(TIMESTAMPS_FILE))]);
    let written = fs::read_to_string(temp.join(TIMESTAMPS_FILE)).unwrap();
    assert!(
        written.starts_with("# timestamp format v2\n0\n40\n80\n"),
        "{written}"
    );
}

#[test]
fn deliver_retries_with_the_timestamps_file_then_fails_and_removes_the_output() {
    let dir = tempfile::tempdir().unwrap();
    let Some([cfr, late, _]) = clips(
        dir.path(),
        "deliver_retries_with_the_timestamps_file_then_fails_and_removes_the_output",
    ) else {
        return;
    };
    let (temp, output) = (dir.path().join("temp"), dir.path().join("out.mkv"));
    fs::create_dir(&temp).unwrap();
    // A mux that loses the late start; the retry with the timestamps file is right.
    let (delivery, calls) = deliver_with(&late, &temp, &output, false, &[&cfr, &late]);
    assert_eq!(delivery, Delivery::Checked { video_frames: 12 });
    assert_eq!(
        calls,
        [
            MuxTiming::Stream { sync_ms: 2000 },
            MuxTiming::File(temp.join(TIMESTAMPS_FILE))
        ]
    );
    // Both muxes lose it: the run fails, and neither the output nor its sentinel is left.
    let (delivery, calls) = deliver_with(&late, &temp, &output, false, &[&cfr, &cfr]);
    assert_eq!(delivery, Delivery::Failed);
    assert_eq!(calls.len(), 2);
    assert!(!output.exists());
    assert!(!temp.join(MUX_MARKER).exists());
}

/// The default duration (ns) that mkvmerge reports for a file's video track.
fn default_duration(mkv: &Path) -> Option<u64> {
    let output = Command::new("mkvmerge")
        .arg("-J")
        .arg(mkv)
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    json["tracks"]
        .as_array()?
        .iter()
        .find(|track| track["type"] == "video")?["properties"]["default_duration"]
        .as_u64()
}

#[test]
fn deliver_retry_with_the_timestamps_file_keeps_the_sources_frame_rate() {
    let dir = tempfile::tempdir().unwrap();
    let Some([cfr, late, _]) = clips(
        dir.path(),
        "deliver_retry_with_the_timestamps_file_keeps_the_sources_frame_rate",
    ) else {
        return;
    };
    let (temp, output) = (dir.path().join("temp"), dir.path().join("out.mkv"));
    fs::create_dir(&temp).unwrap();
    let raw = dir.path().join("clip.hevc");
    let mut calls = Vec::new();
    let delivery = deliver(
        late.to_str().unwrap(),
        &raw,
        &output,
        &temp,
        false,
        &mut |timing| {
            calls.push(timing.clone());
            if calls.len() == 1 {
                // A mux that loses the late start, so the timestamps file is muxed next.
                fs::copy(&cfr, &output).unwrap();
                return Ok(true);
            }
            let status = Command::new("mkvmerge")
                .arg("-q")
                .arg("-o")
                .arg(&output)
                .args(timing.mkvmerge_args())
                .arg(&raw)
                .status()
                .unwrap();
            Ok(status.success())
        },
    )
    .unwrap();
    assert_eq!(delivery, Delivery::Checked { video_frames: 12 });
    assert_eq!(
        calls,
        [
            MuxTiming::Stream { sync_ms: 2000 },
            MuxTiming::File(temp.join(TIMESTAMPS_FILE))
        ]
    );
    // 41708333 ns (23.976 fps) as in the source, not the 42 ms of a whole-millisecond file.
    let source = default_duration(&late);
    assert!(source.is_some());
    assert_eq!(default_duration(&output), source);
}

#[test]
fn deliver_remuxes_a_reused_wrong_mux_with_the_chosen_timing_first() {
    let dir = tempfile::tempdir().unwrap();
    let Some([cfr, late, _]) = clips(
        dir.path(),
        "deliver_remuxes_a_reused_wrong_mux_with_the_chosen_timing_first",
    ) else {
        return;
    };
    let (temp, output) = (dir.path().join("temp"), dir.path().join("out.mkv"));
    fs::create_dir(&temp).unwrap();
    // A finished mux from before the fix: the late start was lost.
    fs::copy(&cfr, &output).unwrap();
    fs::write(temp.join(MUX_MARKER), b"").unwrap();
    let (delivery, calls) = deliver_with(&late, &temp, &output, true, &[&cfr, &late]);
    assert_eq!(delivery, Delivery::Checked { video_frames: 12 });
    // Reused, then muxed afresh with --sync; the timestamps file is never needed.
    assert_eq!(
        calls,
        [
            MuxTiming::Stream { sync_ms: 2000 },
            MuxTiming::Stream { sync_ms: 2000 }
        ]
    );
}

#[test]
fn deliver_of_an_unreadable_source_muxes_unchecked() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("in.mp4");
    fs::write(&source, b"not a video").unwrap();
    let output = dir.path().join("out.mkv");
    let calls = RefCell::new(Vec::new());
    let delivery = deliver(
        source.to_str().unwrap(),
        &source,
        &output,
        dir.path(),
        false,
        &mut |timing| {
            calls.borrow_mut().push(timing.clone());
            Ok(true)
        },
    )
    .unwrap();
    assert_eq!(delivery, Delivery::Unchecked);
    assert_eq!(calls.into_inner(), [MuxTiming::Stream { sync_ms: 0 }]);

    let failed = deliver(
        source.to_str().unwrap(),
        &source,
        &output,
        dir.path(),
        false,
        &mut |_| Ok(false),
    )
    .unwrap();
    assert_eq!(failed, Delivery::Failed);
}

/// A constant-rate source from 0 ms: `n` frames of `frame_ms`, with the container's rate.
fn constant_rate(n: usize, frame_ms: f64) -> VideoTimes {
    VideoTimes {
        frames: (0..n).map(|k| k as f64 * frame_ms).collect(),
        end: Some(n as f64 * frame_ms),
        frame_ms: Some(frame_ms),
    }
}

#[test]
fn first_timing_takes_the_timestamps_file_when_the_stream_states_another_rate() {
    let file = MuxTiming::File(PathBuf::from("/t/video.timestamps.txt"));
    let source = constant_rate(10, 40.0);
    // The stream's VUI says 30 fps, the source runs at 25 fps (an RTX HDR encode).
    let (timing, reason) = first_timing(&source, Some(1000.0 / 30.0), &file);
    assert_eq!(timing, file);
    let reason = reason.unwrap();
    assert!(reason.contains("30.000 fps"), "{reason}");
    assert!(reason.contains("25.000 fps"), "{reason}");
    // 29.97 against 30 fps drifts 33 ms over 1000 frames: the stream's timing would fail.
    let ntsc = constant_rate(1000, 1001.0 / 30.0);
    assert_eq!(first_timing(&ntsc, Some(1000.0 / 30.0), &file).0, file);
}

#[test]
fn first_timing_keeps_the_streams_timing_when_the_rates_agree_or_one_is_unknown() {
    let file = MuxTiming::File(PathBuf::from("/t/video.timestamps.txt"));
    let film = constant_rate(10, 1001.0 / 24.0);
    // mkvmerge's 41708333 ns for 24000/1001 fps against the frames' 41.708333... ms.
    let (timing, reason) = first_timing(&film, Some(41.708_333), &file);
    assert_eq!(timing, MuxTiming::Stream { sync_ms: 0 });
    assert_eq!(reason, None);
    // 29.97 against 30 fps over 10 frames drifts 0.3 ms, within the check: stream timing.
    let short = constant_rate(10, 1001.0 / 30.0);
    assert_eq!(
        first_timing(&short, Some(1000.0 / 30.0), &file).0,
        MuxTiming::Stream { sync_ms: 0 }
    );
    // No prediction, or a single frame: today's behaviour.
    assert_eq!(
        first_timing(&film, None, &file).0,
        MuxTiming::Stream { sync_ms: 0 }
    );
    assert_eq!(
        first_timing(&constant_rate(1, 40.0), Some(1000.0 / 30.0), &file).0,
        MuxTiming::Stream { sync_ms: 0 }
    );
    // A late start is still kept with --sync.
    let mut late = constant_rate(10, 40.0);
    late.frames.iter_mut().for_each(|t| *t += 2000.0);
    assert_eq!(
        first_timing(&late, Some(40.0), &file).0,
        MuxTiming::Stream { sync_ms: 2000 }
    );
}

#[test]
fn first_timing_reads_the_rate_from_the_frames_not_ffprobes_estimate() {
    let file = MuxTiming::File(PathBuf::from("/t/video.timestamps.txt"));
    // A 23.976 fps track without a default duration: whole-millisecond frames 0, 42, 83, 125...
    // and ffprobe's avg_frame_rate estimate of 293/12 fps (40.956 ms), 1.8 % off.
    let frames: Vec<f64> = (0..240)
        .map(|k| (f64::from(k) * 1001.0 / 24.0).round())
        .collect();
    let source = VideoTimes {
        end: None,
        frame_ms: Some(12_000.0 / 293.0),
        frames,
    };
    let (timing, reason) = first_timing(&source, Some(41.708_333), &file);
    assert_eq!(timing, MuxTiming::Stream { sync_ms: 0 });
    assert_eq!(reason, None);
}

#[test]
fn first_timing_gives_a_variable_rate_source_its_timestamps_whatever_the_stream_says() {
    let file = MuxTiming::File(PathBuf::from("/t/video.timestamps.txt"));
    let mut vfr = constant_rate(10, 40.0);
    vfr.frames[5] += 15.0;
    let (timing, reason) = first_timing(&vfr, Some(40.0), &file);
    assert_eq!(timing, file);
    assert!(reason.unwrap().contains("variable frame rate"));
}

#[test]
fn mkvmerge_identification_gives_the_video_tracks_frame_duration() {
    let json = json!({"tracks": [
        {"type": "audio", "properties": {"default_duration": 23_219_954}},
        {"type": "video", "properties": {"default_duration": 33_333_333}},
    ]});
    let ms = stream_frame_ms_from_mkvmerge(&json).unwrap();
    assert!((ms - 33.333_333).abs() < 1e-6, "{ms}");
    // No VUI timing: mkvmerge muxes the raw stream at 25 fps.
    for properties in [json!({}), json!({"default_duration": null})] {
        let json = json!({"tracks": [{"type": "video", "properties": properties}]});
        assert_eq!(stream_frame_ms_from_mkvmerge(&json), Some(40.0));
    }
    assert_eq!(stream_frame_ms_from_mkvmerge(&json!({"tracks": []})), None);
    assert_eq!(
        stream_frame_ms_from_mkvmerge(&json!({"container": {}})),
        None
    );
}

#[test]
fn deliver_muxes_a_stream_whose_vui_rate_differs_with_the_timestamps_file_first() {
    let test = "deliver_muxes_a_stream_whose_vui_rate_differs_with_the_timestamps_file_first";
    let dir = tempfile::tempdir().unwrap();
    // `clips` checks the tools; its own clip is not used here.
    if clips(dir.path(), test).is_none() {
        return;
    }
    // A 30 fps stream (VUI timing) in a 25 fps container, as an RTX HDR encode delivers it.
    let raw = dir.path().join("vui30.hevc");
    let encoded = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("testsrc2=s=128x72:r=30")
        .args(["-frames:v", "10", "-c:v", "libx265", "-preset", "ultrafast"])
        .args(["-x265-params", "log-level=error", "-f", "hevc", "-y"])
        .arg(&raw)
        .status()
        .unwrap();
    assert!(encoded.success());
    let source = dir.path().join("source25.mkv");
    let muxed = Command::new("mkvmerge")
        .arg("-q")
        .arg("-o")
        .arg(&source)
        .args(["--default-duration", "0:25p"])
        .arg(&raw)
        .status()
        .unwrap();
    assert!(muxed.success());
    assert_eq!(default_duration(&source), Some(40_000_000));
    assert_eq!(
        predicted_stream_frame_ms(&raw).map(|ms| ms.round()),
        Some(33.0)
    );

    let (temp, output) = (dir.path().join("temp"), dir.path().join("out.mkv"));
    fs::create_dir(&temp).unwrap();
    let mut calls = Vec::new();
    let delivery = deliver(
        source.to_str().unwrap(),
        &raw,
        &output,
        &temp,
        false,
        &mut |timing| {
            calls.push(timing.clone());
            let status = Command::new("mkvmerge")
                .arg("-q")
                .arg("-o")
                .arg(&output)
                .args(timing.mkvmerge_args())
                .arg(&raw)
                .status()
                .unwrap();
            Ok(status.success())
        },
    )
    .unwrap();
    assert_eq!(delivery, Delivery::Checked { video_frames: 10 });
    // One mux, with the source's timestamps: the stream's 30 fps is never muxed.
    assert_eq!(calls, [MuxTiming::File(temp.join(TIMESTAMPS_FILE))]);
    assert_eq!(default_duration(&output), Some(40_000_000));
}

#[test]
fn first_timing_keeps_whole_millisecond_frames_of_a_matching_rate_on_the_stream_timing() {
    let file = MuxTiming::File(PathBuf::from("/t/video.timestamps.txt"));
    // 24000/1001 fps in whole milliseconds, as Matroska stores it, against mkvmerge's truncated
    // 41708333 ns, at 24 consecutive lengths near 100k and 200k frames, from 0 and from a
    // fractional start: the rounding of both ends must never read as another rate.
    for start in [0.0, 0.4, 1000.6] {
        for n in (99_990..100_014).chain(199_990..200_014) {
            let frames: Vec<f64> = (0..n)
                .map(|k| (start + f64::from(k) * 1001.0 / 24.0).round())
                .collect();
            let source = VideoTimes {
                end: None,
                frame_ms: Some(1001.0 / 24.0),
                frames,
            };
            let (timing, reason) = first_timing(&source, Some(41.708_333), &file);
            assert!(
                matches!(timing, MuxTiming::Stream { .. }),
                "{n} frames from {start} ms: {reason:?}"
            );
        }
    }
}
