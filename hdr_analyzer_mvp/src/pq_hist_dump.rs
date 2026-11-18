//! The per-frame PQ histogram dump (`--dump-pq-hist`, format `pqhist` version 1), read by
//! `tools/display_sim` (ROADMAP WS7.1; schema in `docs/FORMAT_COMPATIBILITY.md`, "PQ histogram
//! dump"). A cross-binary contract: change the writer here and the reader in
//! `tools/display_sim/src/hist.rs` in the same PR, and bump the version.
//!
//! Little-endian. Header (64 bytes): magic `PQHIST`, u16 version, then u32 header bytes (64),
//! u32 bins (4096), u32 bytes per count (4), u32 peak domain (0 max-RGB, 1 luma), u32 transfer
//! (0 PQ, 1 HLG through the Profile 8.4 decode), u32 sample rate, u32 downscale, u32 reserved 0,
//! u64 frame count (`u64::MAX` until the run finishes), u64 leading skipped frames, 8 reserved
//! zero bytes. Then one record per measured frame: 4096 u32 pixel counts, bin `b` holding the
//! pixels whose peak-domain PQ value rounds to `b / 4095` (the histogram both backends build
//! before any smoothing). Record `i` is measured frame `i`, sidecar frame `i` and stream frame
//! `i + leading_skipped`. A frame that `--sample-rate` skips repeats the previous record. The
//! header names no backend, so CPU and CUDA dumps of one input are byte-identical.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::cli::PeakDomain;

pub const MAGIC: &[u8; 6] = b"PQHIST";
pub const VERSION: u16 = 1;
pub const HEADER_BYTES: usize = 64;
pub const BINS: usize = 4096;
const BYTES_PER_COUNT: usize = 4;
pub const RECORD_BYTES: usize = BINS * BYTES_PER_COUNT;
/// The frame count field until [`PqHistWriter::finish`] writes the real one, so a reader
/// refuses the dump of a run that did not finish.
pub const UNFINISHED: u64 = u64::MAX;
const FRAME_COUNT_OFFSET: u64 = 40;
const LEADING_OFFSET: u64 = 48;

/// What the records measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DumpHeader {
    pub peak_domain: PeakDomain,
    /// HLG measured through the Profile 8.4 decode; false for PQ (and an untagged input,
    /// which the analyzer measures as PQ).
    pub hlg: bool,
    pub sample_rate: u32,
    pub downscale: u32,
}

impl DumpHeader {
    /// The 64 header bytes, with the frame count still [`UNFINISHED`] and no leading frames.
    pub fn to_bytes(&self) -> [u8; HEADER_BYTES] {
        let mut bytes = [0u8; HEADER_BYTES];
        bytes[0..6].copy_from_slice(MAGIC);
        bytes[6..8].copy_from_slice(&VERSION.to_le_bytes());
        let peak_domain: u32 = match self.peak_domain {
            PeakDomain::MaxRgb => 0,
            PeakDomain::Luma => 1,
        };
        let words = [
            HEADER_BYTES as u32,
            BINS as u32,
            BYTES_PER_COUNT as u32,
            peak_domain,
            u32::from(self.hlg),
            self.sample_rate,
            self.downscale,
            0,
        ];
        for (index, word) in words.iter().enumerate() {
            let offset = 8 + index * 4;
            bytes[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        }
        bytes[40..48].copy_from_slice(&UNFINISHED.to_le_bytes());
        bytes
    }
}

/// Writes one dump, a record per measured frame.
pub struct PqHistWriter {
    out: BufWriter<File>,
    path: PathBuf,
    records: u64,
    /// The previous record, for frames `--sample-rate` skips; empty before the first.
    last: Vec<u8>,
}

impl PqHistWriter {
    pub fn create(path: &Path, header: &DumpHeader) -> Result<Self> {
        let file = File::create(path).with_context(|| {
            format!("Failed to create the PQ histogram dump {}", path.display())
        })?;
        let mut out = BufWriter::new(file);
        out.write_all(&header.to_bytes())
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(Self {
            out,
            path: path.to_path_buf(),
            records: 0,
            last: Vec::with_capacity(RECORD_BYTES),
        })
    }

    /// Appends a measured frame's histogram; a count above `u32::MAX` is an error.
    pub fn write_frame(&mut self, histogram: &[u64]) -> Result<()> {
        if histogram.len() != BINS {
            bail!(
                "PQ histogram dump: a frame histogram has {} bins, not {BINS}",
                histogram.len()
            );
        }
        self.last.clear();
        for (bin, &count) in histogram.iter().enumerate() {
            let count = u32::try_from(count).with_context(|| {
                format!("PQ histogram dump: bin {bin} counts {count} pixels, more than a u32 holds")
            })?;
            self.last.extend_from_slice(&count.to_le_bytes());
        }
        self.append_last()
    }

    /// Appends the previous record again, for a frame `--sample-rate` skips.
    pub fn repeat_last(&mut self) -> Result<()> {
        if self.last.is_empty() {
            bail!("PQ histogram dump: a skipped frame before the first measured frame");
        }
        self.append_last()
    }

    fn append_last(&mut self) -> Result<()> {
        self.out
            .write_all(&self.last)
            .with_context(|| format!("Failed to write {}", self.path.display()))?;
        self.records += 1;
        Ok(())
    }

    /// Records written so far.
    pub fn records(&self) -> u64 {
        self.records
    }

    /// Writes the frame count and the leading skipped frames into the header and returns the
    /// number of records.
    pub fn finish(self, leading_skipped: u64) -> Result<u64> {
        let records = self.records;
        let path = self.path;
        let mut file = self
            .out
            .into_inner()
            .map_err(|error| error.into_error())
            .with_context(|| format!("Failed to write {}", path.display()))?;
        for (offset, value) in [
            (FRAME_COUNT_OFFSET, records),
            (LEADING_OFFSET, leading_skipped),
        ] {
            file.seek(SeekFrom::Start(offset))
                .and_then(|_| file.write_all(&value.to_le_bytes()))
                .with_context(|| format!("Failed to finish {}", path.display()))?;
        }
        file.sync_all()
            .with_context(|| format!("Failed to finish {}", path.display()))?;
        Ok(records)
    }
}

/// Refuses a dump path that names another file of the run (`others`: a label and a path), so
/// the dump never overwrites the input or an output. Paths compare after
/// [`normalized_path`]; on Unix two existing paths of one file (a hard link) also match.
pub fn check_dump_path(dump: &Path, others: &[(&str, &Path)]) -> Result<()> {
    let dump_normalized = normalized_path(dump)?;
    for (label, other) in others {
        if normalized_path(other)? == dump_normalized || same_existing_file(dump, other) {
            bail!(
                "--dump-pq-hist {} is the {label} ({}); choose another path",
                dump.display(),
                other.display()
            );
        }
    }
    Ok(())
}

/// The absolute path `path` names: made absolute against the working directory, its deepest
/// existing ancestor canonicalized (symlinks resolved), and the rest, which does not exist yet,
/// re-attached with `.` and `..` collapsed lexically.
pub fn normalized_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to read the working directory")?
            .join(path)
    };
    let components: Vec<Component<'_>> = absolute.components().collect();
    // The longest prefix that exists; the root always does.
    let mut split = components.len();
    let mut normalized = loop {
        let prefix: PathBuf = components[..split].iter().collect();
        match prefix.canonicalize() {
            Ok(canonical) => break canonical,
            Err(_) if split > 1 => split -= 1,
            Err(_) => break prefix,
        }
    };
    for component in &components[split..] {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    Ok(normalized)
}

#[cfg(unix)]
fn same_existing_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_existing_file(_: &Path, _: &Path) -> bool {
    false
}

#[cfg(test)]
#[path = "pq_hist_dump_tests.rs"]
mod tests;
