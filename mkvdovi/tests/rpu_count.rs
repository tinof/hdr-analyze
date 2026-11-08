//! The output's RPU count against its video frames (ROADMAP E7.2). Every conversion counts the
//! RPUs of the muxed file and fails the run when they differ from its video frames, so the source
//! is kept. The expected count is the fixture's own: 48 frames, and an independent per-access-unit
//! read of the output (`rpu_per_access_unit`). The failing cases put a `dovi_tool` shim ahead of
//! the real one on PATH: one injects no RPU at all (a real output without RPUs), one makes
//! `extract-rpu` stop after 47 frames.
//!
//! Skipped (with a message) when ffmpeg with libx265, ffprobe, mkvmerge, mediainfo or dovi_tool is
//! missing, or when no current `hdr_analyzer_mvp` is built next to mkvdovi.

#[macro_use]
#[path = "support/playback.rs"]
mod playback;
#[path = "support/conversion.rs"]
mod conversion;

use std::path::Path;
use std::process::Command;

use self::conversion::*;
use self::playback::*;

/// The fixtures' frame count.
const FRAMES: usize = 48;

/// A `dovi_tool` that runs `body` (a shell fragment with the real tool in `$REAL`) and then
/// the real one, first on PATH in `<dir>/bin`. Returns the PATH value.
#[cfg(unix)]
fn dovi_tool_shim(dir: &Path, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;

    let real = run(Command::new("sh").arg("-c").arg("command -v dovi_tool"));
    let real = String::from_utf8_lossy(&real.stdout).trim().to_owned();
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let shim = bin.join("dovi_tool");
    std::fs::write(
        &shim,
        format!("#!/bin/sh\nREAL='{real}'\n{body}\nexec \"$REAL\" \"$@\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// `inject-rpu` copies the base layer unchanged: the output carries no RPU.
const INJECTS_NOTHING: &str = "if [ \"$1\" = inject-rpu ]; then\n  in= out= prev=\n  \
    for arg in \"$@\"; do\n    case \"$prev\" in -i) in=$arg ;; -o) out=$arg ;; esac\n    \
    prev=$arg\n  done\n  exec cp \"$in\" \"$out\"\nfi";

/// An `extract-rpu` whose arguments match the shell `pattern` drops the 48th RPU from the file
/// it wrote (`dovi_tool editor`). `-l` is no help: on a raw HEVC stream it stops at a chunk
/// boundary, not at the frame.
fn drops_the_last_rpu(pattern: &str) -> String {
    format!(
        "case \"$*\" in\n  {pattern})\n    \"$REAL\" \"$@\" || exit $?\n    eval out=\\${{$#}}\n    \
         printf '{{\"remove\":[\"47-47\"]}}' > \"$out.json\"\n    \
         \"$REAL\" editor -i \"$out\" -j \"$out.json\" -o \"$out.cut\" > /dev/null || exit $?\n    \
         exec mv \"$out.cut\" \"$out\" ;;\nesac"
    )
}

/// Every read of the RPU count check: the output's own and the extracted video stream's.
const EVERY_COUNT: &str = "extract-rpu*output_rpu.bin";

/// Only the check's direct read of the Matroska output.
const DIRECT_COUNT: &str = "extract-rpu*.mkv\\ -o\\ *output_rpu.bin";

/// `dovi_tool` refuses to read any Matroska file.
const READS_NO_MKV: &str = "case \"$*\" in\n  extract-rpu\\ -i\\ *.mkv\\ *) exit 1 ;;\nesac";

/// `--verify`'s RPU extraction fails, so the run stops after the mux and the checks.
const VERIFY_FAILS: &str = "case \"$*\" in\n  *verify_rpu.bin*) exit 1 ;;\nesac";

fn count_line(rpus: usize) -> String {
    format!("RPU count: {rpus} RPUs for {FRAMES} video frames")
}

fn assert_failed_and_kept(run: &std::process::Output, copy: &Path, output: &Path, problem: &str) {
    let log = log_of(run);
    assert!(!run.status.success(), "the run must fail:\n{log}");
    assert!(log.contains(problem), "{log}");
    assert!(
        log.contains("The output was removed and the source kept"),
        "{log}"
    );
    assert!(copy.exists(), "the source must be kept");
    assert!(!output.exists(), "the output must be removed");
    assert!(!log.contains("Deleting source file"), "{log}");
}

#[test]
fn every_conversion_counts_one_rpu_per_frame_before_the_source_is_deleted() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let source = pq_source(dir.path(), &[]);
    let (run, copy, output) = convert_copy(dir.path(), &source, &[], None);
    let log = log_of(&run);
    assert!(run.status.success(), "mkvdovi failed:\n{log}");
    assert!(log.contains(&count_line(FRAMES)), "{log}");
    assert!(!log.contains("direct read"), "{log}");
    let checked = log.find(&count_line(FRAMES)).unwrap();
    let deleted = log.find("Deleting source file").expect(&log);
    assert!(
        checked < deleted,
        "the count comes before the deletion:\n{log}"
    );
    assert!(!copy.exists());
    assert_eq!(rpu_per_access_unit(&output).len(), FRAMES);
}

#[cfg(unix)]
#[test]
fn raw_dovi_input_counts_from_the_extracted_video_stream() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let source = pq_source(dir.path(), &[]);
    let path = dovi_tool_shim(dir.path(), READS_NO_MKV);
    let (run, copy, _) = convert_copy(
        dir.path(),
        &source,
        &["--dovi-input", "raw", "--keep-source"],
        Some(&path),
    );
    let log = log_of(&run);
    assert!(run.status.success(), "mkvdovi failed:\n{log}");
    assert!(log.contains(&count_line(FRAMES)), "{log}");
    assert!(!log.contains("direct read"), "{log}");
    assert!(copy.exists());
}

#[cfg(unix)]
#[test]
fn an_output_without_rpus_fails_the_run_and_keeps_the_source() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let source = pq_source(dir.path(), &[]);
    let path = dovi_tool_shim(dir.path(), INJECTS_NOTHING);
    let (run, copy, output) = convert_copy(dir.path(), &source, &[], Some(&path));
    assert_failed_and_kept(
        &run,
        &copy,
        &output,
        "The output carries 0 RPUs for its 48 video frames",
    );
}

#[cfg(unix)]
#[test]
fn fewer_rpus_than_frames_fail_the_run_and_keep_the_source() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let source = pq_source(dir.path(), &[]);
    let path = dovi_tool_shim(dir.path(), &drops_the_last_rpu(EVERY_COUNT));
    let (run, copy, output) = convert_copy(dir.path(), &source, &[], Some(&path));
    assert_failed_and_kept(
        &run,
        &copy,
        &output,
        "The output carries 47 RPUs for its 48 video frames",
    );
    assert!(
        log_of(&run).contains("direct read of the output gave 47 RPUs"),
        "{}",
        log_of(&run)
    );
}

#[cfg(unix)]
#[test]
fn a_short_direct_read_is_counted_again_from_the_video_stream() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let source = pq_source(dir.path(), &[]);
    let path = dovi_tool_shim(dir.path(), &drops_the_last_rpu(DIRECT_COUNT));
    let (run, copy, output) = convert_copy(dir.path(), &source, &[], Some(&path));
    let log = log_of(&run);
    assert!(run.status.success(), "mkvdovi failed:\n{log}");
    assert!(
        log.contains("direct read of the output gave 47 RPUs"),
        "{log}"
    );
    assert!(log.contains(&count_line(FRAMES)), "{log}");
    assert!(
        !copy.exists(),
        "the recount passed, so the source is deleted"
    );
    assert!(output.exists());
}

/// A resumed run reuses the muxed output; the count still reads it, and an `output_rpu.bin`
/// left in the temp dir does not stand in for it.
#[cfg(unix)]
#[test]
fn a_reused_output_is_counted_again() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let source = pq_source(dir.path(), &[]);
    let path = dovi_tool_shim(dir.path(), VERIFY_FAILS);
    let flags = ["--keep-source", "--verify"];
    let (first, copy, output) = convert_copy(dir.path(), &source, &flags, Some(&path));
    let log = log_of(&first);
    assert!(
        !first.status.success(),
        "the first run stops at --verify:\n{log}"
    );
    assert!(log.contains(&count_line(FRAMES)), "{log}");
    let temp = dir.path().join("run").join("mkvdovi_temp_clip");
    assert!(temp.join("mux.done").exists(), "{log}");

    // The RPUs of the good output stay in the temp dir; the output becomes the source itself:
    // the same video timing, no RPU.
    run(Command::new("dovi_tool")
        .arg("extract-rpu")
        .arg("-i")
        .arg(&output)
        .arg("-o")
        .arg(temp.join("output_rpu.bin")));
    std::fs::copy(&copy, &output).unwrap();

    // The same run again on the same copy: a fresh copy would not resume the temp dir.
    let second = mkvdovi_cmd()
        .current_dir(dir.path().join("run"))
        .arg(SOURCE)
        .args(["--hwaccel", "none"])
        .args(flags)
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(
        log_of(&second).contains("Reusing muxed output"),
        "{}",
        log_of(&second)
    );
    assert_failed_and_kept(
        &second,
        &copy,
        &output,
        "The output carries 0 RPUs for its 48 video frames",
    );
    assert!(
        !temp.join("mux.done").exists(),
        "the next run must mux again"
    );
}

#[test]
fn profile7_mel_fast_path_counts_its_rpus() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let source = profile7_mel_source(dir.path());
    let (run, copy, output) = convert_copy(dir.path(), &source, &[], None);
    let log = log_of(&run);
    assert!(run.status.success(), "mkvdovi failed:\n{log}");
    assert!(log.contains("without rebuilding metadata"), "{log}");
    assert!(log.contains(&count_line(FRAMES)), "{log}");
    assert!(copy.exists(), "a Dolby Vision source is kept");
    assert_eq!(rpu_per_access_unit(&output).len(), FRAMES);
}

#[cfg(unix)]
#[test]
fn a_profile7_mel_output_with_fewer_rpus_fails_the_run() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let source = profile7_mel_source(dir.path());
    let path = dovi_tool_shim(dir.path(), &drops_the_last_rpu(EVERY_COUNT));
    let (run, copy, output) = convert_copy(dir.path(), &source, &[], Some(&path));
    assert!(
        log_of(&run).contains("without rebuilding metadata"),
        "{}",
        log_of(&run)
    );
    assert_failed_and_kept(
        &run,
        &copy,
        &output,
        "The output carries 47 RPUs for its 48 video frames",
    );
}
