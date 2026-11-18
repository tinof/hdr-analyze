//! Reader of the analyzer's `--dump-pq-hist` file, format `pqhist` version 1. The writer is
//! `hdr_analyzer_mvp/src/pq_hist_dump.rs`; schema in `docs/FORMAT_COMPATIBILITY.md` ("PQ
//! histogram dump"). Both sides change in the same PR.
//!
//! The header is checked field by field before any record is read, and the file length must be
//! exactly 64 + frames x 16384 bytes. Records are then read one at a time.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

pub const BINS: usize = 4096;
pub const HEADER_BYTES: usize = 64;
const RECORD_BYTES: u64 = (BINS * 4) as u64;
/// The frame count of a run that did not finish.
const UNFINISHED: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeakDomain {
    MaxRgb,
    Luma,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub peak_domain: PeakDomain,
    /// HLG measured through the Profile 8.4 decode; false: PQ.
    pub hlg: bool,
    pub sample_rate: u32,
    pub downscale: u32,
    pub frames: u64,
    /// Undecodable leading pictures before measured frame 0: record `i` is stream frame
    /// `i + leading_skipped`.
    pub leading_skipped: u64,
}

fn u32_at(bytes: &[u8; HEADER_BYTES], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("4 bytes"))
}

fn u64_at(bytes: &[u8; HEADER_BYTES], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("8 bytes"))
}

/// Checks the 64 header bytes against a file of `file_len` bytes.
pub fn parse_header(bytes: &[u8; HEADER_BYTES], file_len: u64) -> Result<Header> {
    ensure!(
        &bytes[..6] == b"PQHIST",
        "not a pqhist dump (no PQHIST magic)"
    );
    let version = u16::from_le_bytes([bytes[6], bytes[7]]);
    ensure!(
        version == 1,
        "pqhist version {version} is not supported (this display_sim reads version 1)"
    );
    for (offset, name, expected) in [
        (8, "header size", HEADER_BYTES as u32),
        (12, "bin count", BINS as u32),
        (16, "bytes per count", 4),
    ] {
        let value = u32_at(bytes, offset);
        ensure!(
            value == expected,
            "pqhist {name} is {value}, not {expected}"
        );
    }
    let peak_domain = match u32_at(bytes, 20) {
        0 => PeakDomain::MaxRgb,
        1 => PeakDomain::Luma,
        other => bail!("pqhist peak domain {other} is unknown (0 max-RGB, 1 luma)"),
    };
    let hlg = match u32_at(bytes, 24) {
        0 => false,
        1 => true,
        other => bail!("pqhist transfer {other} is unknown (0 PQ, 1 HLG)"),
    };
    let (sample_rate, downscale) = (u32_at(bytes, 28), u32_at(bytes, 32));
    ensure!(sample_rate >= 1, "pqhist sample rate is 0");
    ensure!(downscale >= 1, "pqhist downscale is 0");
    ensure!(
        u32_at(bytes, 36) == 0 && bytes[56..].iter().all(|&byte| byte == 0),
        "pqhist reserved header fields are not zero"
    );
    let frames = u64_at(bytes, 40);
    ensure!(
        frames != UNFINISHED,
        "the pqhist dump has no frame count: the analyzer run that wrote it did not finish"
    );
    ensure!(frames > 0, "the pqhist dump holds no frames");
    let expected = frames
        .checked_mul(RECORD_BYTES)
        .and_then(|records| records.checked_add(HEADER_BYTES as u64))
        .context("pqhist frame count overflows a file size")?;
    ensure!(
        file_len == expected,
        "the pqhist dump is {file_len} bytes; {frames} frames need {expected}"
    );
    Ok(Header {
        peak_domain,
        hlg,
        sample_rate,
        downscale,
        frames,
        leading_skipped: u64_at(bytes, 48),
    })
}

/// A dump opened for reading, one record at a time.
pub struct Dump {
    pub header: Header,
    reader: BufReader<File>,
    read: u64,
    buffer: Vec<u8>,
}

impl Dump {
    pub fn open(path: &Path) -> Result<Self> {
        let file =
            File::open(path).with_context(|| format!("opening the dump {}", path.display()))?;
        let file_len = file
            .metadata()
            .with_context(|| format!("reading the size of {}", path.display()))?
            .len();
        let mut reader = BufReader::with_capacity(RECORD_BYTES as usize, file);
        let mut bytes = [0u8; HEADER_BYTES];
        reader
            .read_exact(&mut bytes)
            .with_context(|| format!("{} is shorter than a pqhist header", path.display()))?;
        let header = parse_header(&bytes, file_len)
            .with_context(|| format!("reading {}", path.display()))?;
        Ok(Self {
            header,
            reader,
            read: 0,
            buffer: vec![0; RECORD_BYTES as usize],
        })
    }

    /// The next record into `counts`; false after the last.
    pub fn next_frame(&mut self, counts: &mut [u32; BINS]) -> Result<bool> {
        if self.read == self.header.frames {
            return Ok(false);
        }
        self.reader
            .read_exact(&mut self.buffer)
            .with_context(|| format!("reading pqhist record {}", self.read))?;
        let (words, _) = self.buffer.as_chunks::<4>();
        for (count, bytes) in counts.iter_mut().zip(words) {
            *count = u32::from_le_bytes(*bytes);
        }
        self.read += 1;
        Ok(true)
    }
}

#[cfg(test)]
#[path = "hist_tests.rs"]
mod tests;
