//! Where a pass reads the raw HEVC from, and one pass over it: the fill-reader, the parser, the
//! progress bar and the stall warning, and the ffmpeg child's exit.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use hevc_parser::io::processor::{HevcProcessor, HevcProcessorOpts};
use hevc_parser::io::{IoFormat, IoProcessor};
use sha2::{Digest, Sha256};

use crate::progress::ByteProgress;

/// The raw Annex B HEVC stream an injection reads, once per pass.
pub enum Source {
    /// An ffmpeg command that writes the stream to its stdout. Each pass runs it afresh with
    /// stdin closed, stdout read here and stderr in `logs[pass]`.
    Ffmpeg {
        command: Command,
        logs: [PathBuf; 2],
    },
    /// A raw HEVC file.
    RawFile(PathBuf),
}

impl Source {
    /// The stream, and the ffmpeg child that writes it.
    fn open(&mut self, pass: usize) -> Result<(Box<dyn Read + Send>, Option<Child>)> {
        match self {
            Source::RawFile(path) => {
                let file = File::open(&*path)
                    .with_context(|| format!("opening the base layer {}", path.display()))?;
                Ok((Box::new(file), None))
            }
            Source::Ffmpeg { command, logs } => {
                let mut log = File::create(&logs[pass]).context("Failed to create log file")?;
                writeln!(log, "Running command: {command:?} |")?;
                command.stdin(Stdio::null());
                command.stdout(Stdio::piped());
                command.stderr(Stdio::from(log));
                let mut child = command.spawn().context("Failed to spawn ffmpeg")?;
                let Some(stdout) = child.stdout.take() else {
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!("ffmpeg has no stdout");
                };
                Ok((Box::new(stdout), Some(child)))
            }
        }
    }

    /// The failure of the ffmpeg of pass `pass` that exited with `status`.
    fn failure(&self, pass: usize, status: ExitStatus) -> String {
        match self {
            Source::Ffmpeg { logs, .. } => {
                format!(
                    "ffmpeg exited with {status} (log: {})",
                    logs[pass].display()
                )
            }
            Source::RawFile(_) => format!("the reader exited with {status}"),
        }
    }
}

/// Fills every buffer it is given unless the stream ends, as a file read does. `HevcProcessor`
/// takes any read shorter than its chunk for the last one, and a pipe returns at most its own
/// buffer (64 KB on Linux) per read, so without this a pipe would be cut into other chunks than
/// dovi_tool reads from a file. The processor also takes a read error for the end of the stream
/// (`while let Ok(n)`), so the first one is kept here for [`parse_stream`] to check.
pub(super) struct FillReader<'a, R> {
    inner: R,
    bytes: u64,
    hash: Sha256,
    error: Option<io::Error>,
    progress: Option<&'a AtomicU64>,
}

impl<'a, R: Read> FillReader<'a, R> {
    pub(super) fn new(inner: R) -> Self {
        Self {
            inner,
            bytes: 0,
            hash: Sha256::new(),
            error: None,
            progress: None,
        }
    }

    /// Also stores the byte count in `progress` after every read, for the progress bar.
    pub(super) fn counting_into(mut self, progress: &'a AtomicU64) -> Self {
        self.progress = Some(progress);
        self
    }
}

impl<R: Read> Read for FillReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut filled = 0;
        while filled < buf.len() {
            match self.inner.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => {
                    filled += n;
                    // Per inner read, so a slow pipe moves the bar and never reads as a stall
                    // while a chunk fills.
                    if let Some(progress) = self.progress {
                        progress.store(self.bytes + filled as u64, Ordering::Relaxed);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    let returned = io::Error::new(error.kind(), error.to_string());
                    self.error.get_or_insert(error);
                    return Err(returned);
                }
            }
        }
        self.hash.update(&buf[..filled]);
        self.bytes += filled as u64;
        if let Some(progress) = self.progress {
            progress.store(self.bytes, Ordering::Relaxed);
        }
        Ok(filled)
    }
}

/// What one pass read: its length and SHA-256, so the two passes can be compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StreamDigest {
    pub(super) bytes: u64,
    pub(super) sha256: Vec<u8>,
}

/// Parses `reader` to its end through `processor` in chunks of `chunk_size`. A read error, which
/// the processor takes for the end of the stream, fails the pass.
pub(super) fn parse_stream<R: Read>(
    reader: &mut FillReader<'_, R>,
    processor: &mut dyn IoProcessor,
    chunk_size: usize,
) -> Result<StreamDigest> {
    HevcProcessor::new(IoFormat::Raw, HevcProcessorOpts::default(), chunk_size)
        .process_io(reader, processor)?;
    if let Some(error) = &reader.error {
        bail!("reading the base layer failed: {error}");
    }
    Ok(StreamDigest {
        bytes: reader.bytes,
        sha256: reader.hash.clone().finalize().to_vec(),
    })
}

/// How a pass is shown: the bar's text, its expected total and the stall warning.
pub(super) struct Display<'a> {
    pub(super) label: &'a str,
    pub(super) total: Option<u64>,
    pub(super) stall_secs: u64,
}

/// One pass over `source` (`pass` is 0 or 1): [`parse_stream`] with a byte-progress bar, and the
/// ffmpeg child's exit, which must be success. Every way out drops the pipe, ends and reaps the
/// child and stops the bar's thread.
pub(super) fn run_pass(
    source: &mut Source,
    pass: usize,
    processor: &mut dyn IoProcessor,
    chunk_size: usize,
    display: &Display<'_>,
) -> Result<StreamDigest> {
    let (inner, child) = source.open(pass)?;
    let progress = AtomicU64::new(0);
    let done = AtomicBool::new(false);
    let bar = ByteProgress::new(display.label, display.total);
    let mut reader = FillReader::new(inner).counting_into(&progress);
    let parsed = thread::scope(|scope| {
        let watcher = scope.spawn(|| watch(&bar, &progress, &done, display.stall_secs));
        let parsed = parse_stream(&mut reader, processor, chunk_size);
        done.store(true, Ordering::Relaxed);
        watcher.thread().unpark();
        parsed
    });
    bar.set_bytes(reader.bytes);
    // Dropping the reader closes the pipe: an ffmpeg still writing fails on it instead of
    // blocking on a full pipe while this process waits for it.
    drop(reader);
    let exited = child.map(|mut child| {
        if parsed.is_err() {
            let _ = child.kill();
        }
        child.wait()
    });
    let outcome = match (parsed, exited) {
        (Ok(_), Some(Err(error))) => Err(error).context("waiting for ffmpeg"),
        (Ok(_), Some(Ok(status))) if !status.success() => {
            Err(anyhow!(source.failure(pass, status)))
        }
        // An ffmpeg that failed by itself (killed, it has no exit code) is the likelier cause.
        (Err(error), Some(Ok(status))) if status.code().is_some_and(|code| code != 0) => {
            Err(error.context(source.failure(pass, status)))
        }
        (parsed, _) => parsed,
    };
    match &outcome {
        Ok(_) => bar.finish_success(),
        Err(_) => bar.finish_error(None),
    }
    outcome
}

/// Updates `bar` from `progress` twice a second and shows a stall warning once it has not grown
/// for `stall_secs` (`0`: never), until `done`.
fn watch(bar: &ByteProgress, progress: &AtomicU64, done: &AtomicBool, stall_secs: u64) {
    let mut last = 0;
    let mut last_growth = Instant::now();
    let mut warned = false;
    while !done.load(Ordering::Relaxed) {
        let bytes = progress.load(Ordering::Relaxed);
        bar.set_bytes(bytes);
        if bytes > last {
            last = bytes;
            last_growth = Instant::now();
            if warned {
                bar.set_stall(None);
                warned = false;
            }
        } else if stall_secs > 0 && last_growth.elapsed().as_secs() >= stall_secs {
            bar.set_stall(Some(last_growth.elapsed()));
            warned = true;
        }
        thread::park_timeout(Duration::from_millis(500));
    }
}
