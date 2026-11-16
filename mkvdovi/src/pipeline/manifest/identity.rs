//! Source identity for the playback manifest: the size and the SHA-256 of the first and last MiB
//! (the same rule as the analyzer's AU table, `hdr_analyzer_mvp/src/au_table.rs`), the lookup key,
//! and the Matroska check that runs before any analysis.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Bytes hashed at each end of the source (the analyzer's `CHUNK_BYTES`).
pub(super) const CHUNK_BYTES: u64 = 1 << 20;

/// The EBML magic every Matroska file starts with.
const EBML_MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];
/// The EBML `DocType` element ID.
const DOC_TYPE_ID: [u8; 2] = [0x42, 0x82];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileIdentity {
    pub(super) size_bytes: u64,
    /// SHA-256 of bytes `[0, min(1 MiB, size))`.
    pub(super) head: [u8; 32],
    /// SHA-256 of bytes `[max(0, size - 1 MiB), size)`.
    pub(super) tail: [u8; 32],
}

impl FileIdentity {
    pub(super) fn of(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let size_bytes = file.metadata()?.len();
        let head = chunk_digest(&mut file, 0, size_bytes.min(CHUNK_BYTES))?;
        let tail_start = size_bytes.saturating_sub(CHUNK_BYTES);
        let tail = chunk_digest(&mut file, tail_start, size_bytes - tail_start)?;
        Ok(Self {
            size_bytes,
            head,
            tail,
        })
    }

    /// SHA-256 of (size as a big-endian u64 ‖ the 32 head digest bytes ‖ the 32 tail digest
    /// bytes), as lowercase hex: the name a plain HTTP folder can serve the manifest under.
    pub(super) fn lookup_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.size_bytes.to_be_bytes());
        hasher.update(self.head);
        hasher.update(self.tail);
        hex(&hasher.finalize())
    }
}

fn chunk_digest(file: &mut File, start: u64, len: u64) -> io::Result<[u8; 32]> {
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(len).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the file changed while it was read",
        ));
    }
    Ok(Sha256::digest(&bytes).into())
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether the file is Matroska: the EBML magic, then a `DocType` of exactly `matroska` in the
/// EBML header (WebM and anything else is not).
pub(super) fn is_matroska(path: &Path) -> io::Result<bool> {
    let mut header = Vec::with_capacity(64);
    File::open(path)?.take(64).read_to_end(&mut header)?;
    Ok(is_matroska_header(&header))
}

pub(super) fn is_matroska_header(header: &[u8]) -> bool {
    if !header.starts_with(&EBML_MAGIC) {
        return false;
    }
    header
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| *pair == DOC_TYPE_ID)
        .any(|(index, _)| {
            // A one-byte EBML size (0x80 | length) is what every muxer writes for a DocType.
            let Some(&size) = header.get(index + 2) else {
                return false;
            };
            if size & 0x80 == 0 {
                return false;
            }
            let length = usize::from(size & 0x7F);
            header.get(index + 3..index + 3 + length) == Some(b"matroska".as_slice())
        })
}

/// Whether `path` names the same file as `input`: equal after resolving symlinks and `..`, or
/// (on Unix) the same inode, which also catches a hard link. A path that does not exist yet is
/// compared through its resolved parent directory.
pub(super) fn is_same_file(path: &Path, input: &Path) -> bool {
    if same_inode(path, input) {
        return true;
    }
    match (resolve(path), resolve(input)) {
        (Some(path), Some(input)) => path == input,
        _ => false,
    }
}

#[cfg(unix)]
fn same_inode(path: &Path, input: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(path), std::fs::metadata(input)) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_inode(_path: &Path, _input: &Path) -> bool {
    false
}

fn resolve(path: &Path) -> Option<PathBuf> {
    if let Ok(resolved) = path.canonicalize() {
        return Some(resolved);
    }
    let name = path.file_name()?;
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    Some(parent.canonicalize().ok()?.join(name))
}
