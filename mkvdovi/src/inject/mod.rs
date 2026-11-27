//! In-process RPU injection (ROADMAP E20.1): writes `RPU.bin` into the raw HEVC base layer byte
//! for byte as `dovi_tool inject-rpu` 2.3.4 does, so the base layer can come from an ffmpeg pipe
//! and the full-size `BL.hevc` is never written.
//!
//! It drives the parser dovi_tool uses (`hevc_parser`, pinned to 0.6.12 in `Cargo.toml`) the way
//! dovi_tool does: `IoFormat::Raw`, chunks of [`CHUNK_SIZE`] bytes, two passes over the stream.
//! Pass 1 reads the frame order ([`frame_index`]); pass 2 writes every access unit with an AUD
//! first and its RPU last ([`interleave`]). A pipe is read through a fill-reader
//! ([`source::FillReader`]) so the parser sees the same chunks as from a file. The output is
//! accepted only when both passes read the same bytes (length and SHA-256), the same NAL units
//! and pictures, every ffmpeg exited 0, and one RPU went into every picture.
//!
//! There is no fallback in here: the caller chooses the external path (`pipeline::deliver`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, ensure, Context, Result};
use hevc_parser::hevc::NALUnit;
use hevc_parser::io::IoProcessor;
use hevc_parser::HevcParser;

use crate::rpu_guard;

mod frame_index;
mod interleave;
mod source;

use self::frame_index::FrameIndex;
use self::interleave::Interleaver;
pub use self::source::Source;
use self::source::{run_pass, Display, StreamDigest};

/// dovi_tool's chunk size for both passes (`rpu_injector.rs`, `chunk_size = 100_000`).
pub const CHUNK_SIZE: usize = 100_000;

/// The RPUs of an `RPU.bin`, each serialised as dovi_tool writes it into the stream: the
/// `7C 01` NAL header and the emulation-prevented payload, without a start code.
pub struct Rpus(Vec<Vec<u8>>);

impl std::fmt::Debug for Rpus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rpus({} RPUs)", self.0.len())
    }
}

impl Rpus {
    /// Parses `path` behind the abort guard (`rpu_guard::parse_source_rpu_file`) and serialises
    /// every RPU. An error means the in-process path cannot write this file.
    pub fn prepare(path: &Path) -> Result<Self> {
        rpu_guard::parse_source_rpu_file(path)?
            .iter()
            .enumerate()
            .map(|(index, rpu)| {
                rpu.write_hevc_unspec62_nalu()
                    .with_context(|| format!("RPU {index} does not serialise"))
            })
            .collect::<Result<_>>()
            .map(Self)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// A pass about to start, for the caller's step header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Pass 1: the frame order.
    Index,
    /// Pass 2: the output.
    Write,
}

/// One injection.
pub struct Request<'a> {
    pub source: &'a mut Source,
    pub rpus: &'a Rpus,
    /// Refuse, before anything is written, when the RPU count differs from the pictures (an RPU
    /// built from measurements). Otherwise dovi_tool's rule applies: the last RPU is repeated, or
    /// the RPUs past the last picture are dropped.
    pub refuse_mismatch: bool,
    /// [`CHUNK_SIZE`]; the tests read other sizes too.
    pub chunk_size: usize,
    /// The expected size of the stream, for the first pass's progress bar.
    pub size_hint: Option<u64>,
    /// `--stall-timeout`: warn after this many seconds without a byte read (0: never).
    pub stall_secs: u64,
    /// Called before each pass starts.
    pub announce: &'a mut dyn FnMut(Pass),
}

/// A finished injection.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub frames: usize,
    pub rpus: usize,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub index_secs: f64,
    pub write_secs: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Injected(Report),
    /// Refused under [`Request::refuse_mismatch`]; nothing was written.
    LengthMismatch {
        frames: usize,
        rpus: usize,
    },
}

/// Injects `request.rpus` into the stream of `request.source`, writing the result to `output`.
/// On an error `output` may hold part of a stream; the caller discards it.
pub fn inject(request: Request<'_>, output: &mut dyn Write) -> Result<Outcome> {
    let Request {
        source,
        rpus,
        refuse_mismatch,
        chunk_size,
        size_hint,
        stall_secs,
        announce,
    } = request;

    announce(Pass::Index);
    let started = Instant::now();
    let mut indexer = Indexer::default();
    let display = Display {
        label: "Reading the base layer's frame order",
        total: size_hint,
        stall_secs,
    };
    let first = run_pass(source, 0, &mut indexer, chunk_size, &display).context("pass 1")?;
    let index_secs = started.elapsed().as_secs_f64();
    let Some(index) = indexer.index else {
        bail!("pass 1 ended without a frame order");
    };
    let frames = index.len();
    ensure!(frames > 0, "the base layer holds no pictures");
    let mismatched = frames != rpus.len();
    if mismatched && refuse_mismatch {
        return Ok(Outcome::LengthMismatch {
            frames,
            rpus: rpus.len(),
        });
    }

    announce(Pass::Write);
    let started = Instant::now();
    let mut interleaver = Interleaver::new(&index, &rpus.0, mismatched, output);
    let display = Display {
        label: "Injecting RPU into base layer",
        total: Some(first.bytes),
        stall_secs,
    };
    let second = run_pass(source, 1, &mut interleaver, chunk_size, &display).context("pass 2")?;
    check_passes_agree(&first, indexer.nals, frames, &second, &interleaver)?;
    Ok(Outcome::Injected(Report {
        frames,
        rpus: rpus.len(),
        bytes_read: second.bytes,
        bytes_written: interleaver.bytes_written,
        index_secs,
        write_secs: started.elapsed().as_secs_f64(),
    }))
}

/// The output stands only when pass 2 read what pass 1 indexed and wrote one RPU per picture.
fn check_passes_agree(
    first: &StreamDigest,
    first_nals: u64,
    frames: usize,
    second: &StreamDigest,
    written: &Interleaver<'_>,
) -> Result<()> {
    ensure!(
        first == second,
        "the base layer changed between the two passes: {} bytes, then {} bytes with another SHA-256",
        first.bytes,
        second.bytes
    );
    ensure!(
        first_nals == written.nals && frames == written.stream_frames,
        "the two passes parsed different streams: {first_nals} NAL units and {frames} pictures, then {} and {}",
        written.nals,
        written.stream_frames
    );
    ensure!(
        written.frames_written == frames,
        "{} access units were written with an RPU for {frames} pictures",
        written.frames_written
    );
    Ok(())
}

/// The pass-1 processor: counts the NAL units and keeps the frame order of the whole stream.
#[derive(Default)]
struct Indexer {
    nals: u64,
    index: Option<FrameIndex>,
    input: PathBuf,
}

impl IoProcessor for Indexer {
    fn input(&self) -> &PathBuf {
        &self.input
    }

    fn update_progress(&mut self, _delta: u64) {}

    fn process_nals(
        &mut self,
        _parser: &HevcParser,
        nals: &[NALUnit],
        _chunk: &[u8],
    ) -> Result<()> {
        self.nals += nals.len() as u64;
        Ok(())
    }

    fn finalize(&mut self, parser: &HevcParser) -> Result<()> {
        self.index = Some(FrameIndex::from_frames(parser.ordered_frames())?);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
