//! The analyzer's AU table (`<measurements>.au.json`, version 1) as mkvdovi reads it: a
//! cross-binary contract with `hdr_analyzer_mvp/src/au_table.rs`, defined on each side with no
//! crate link (like the L1 sidecar). Schema: `docs/PLAYBACK_MANIFEST.md` "AU table".
//!
//! [`Pair`] is a table with the sidecar bytes it is bound to. Every check here runs before the
//! pair is used, so a broken or stale pair is re-analyzed (or refused), never written into a
//! manifest.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use super::identity::{hex, sha256_hex, FileIdentity};
use crate::metadata::L1Sidecar;

/// The only table version this build reads.
pub(super) const AU_TABLE_VERSION: u64 = 1;
/// What the analyzer's `--help` names when it writes AU table version 1.
pub(super) const AU_TABLE_CAPABILITY: &str = "au-table-v1";

/// `<measurements>.au.json` (the analyzer's `au_table_path`).
pub(super) fn au_table_path(measurements: &Path) -> PathBuf {
    let mut path = measurements.as_os_str().to_owned();
    path.push(".au.json");
    PathBuf::from(path)
}

#[derive(Debug, Deserialize)]
pub(super) struct AuTable {
    /// Checked on the raw JSON before the rest is parsed (`Pair::from_bytes`).
    pub(super) analyzer_version: String,
    pub(super) sidecar_sha256: Option<String>,
    pub(super) source: TableSource,
    pub(super) track: TableTrack,
    pub(super) aus: TableAus,
    pub(super) leading_skipped: u64,
    /// Detector, metric, threshold and minimum length as run; copied into the manifest.
    pub(super) scene: Value,
}

#[derive(Debug, Deserialize)]
pub(super) struct TableSource {
    pub(super) file_name: String,
    pub(super) size_bytes: u64,
    pub(super) head_sha256: String,
    pub(super) tail_sha256: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct TableTrack {
    pub(super) container: String,
    pub(super) codec: String,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) codec_private_sha256: String,
    pub(super) time_base: [i32; 2],
}

#[derive(Debug, Deserialize)]
pub(super) struct TableAus {
    pub(super) count: u64,
    pub(super) pts_first: i64,
    pub(super) pts_delta: Vec<i64>,
    pub(super) size: Vec<u32>,
    pub(super) non_picture: Vec<u64>,
    pub(super) irap_crc32: Vec<(u64, u32)>,
}

impl TableAus {
    /// The pts of every AU in decode order (the manifest parser's reconstruction).
    pub(super) fn pts(&self) -> Vec<i64> {
        let mut pts = Vec::with_capacity(self.pts_delta.len() + 1);
        pts.push(self.pts_first);
        for (index, delta) in self.pts_delta.iter().enumerate() {
            pts.push(pts[index].wrapping_add(*delta));
        }
        pts
    }

    pub(super) fn picture_count(&self) -> u64 {
        self.count.saturating_sub(self.non_picture.len() as u64)
    }

    /// The decode index of each picture, by presentation index: the pictures (AUs not in
    /// `non_picture`) ranked by pts, the rank `AuMatcher` uses. The pts are unique once
    /// [`check_structure`] passed.
    pub(super) fn decode_index_by_presentation(&self) -> Vec<u64> {
        let pts = self.pts();
        let mut pictures: Vec<u64> = (0..self.count)
            .filter(|index| self.non_picture.binary_search(index).is_err())
            .collect();
        pictures.sort_by_key(|&index| pts[index as usize]);
        pictures
    }
}

impl AuTable {
    /// Reads a table that need not be bound to a sidecar (`--au-table-only`, for an imported
    /// `RPU.bin`), with the version and structure checks of a bound one.
    pub(super) fn read_unbound(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("{} cannot be read ({error})", path.display()))?;
        let table = Self::parse(&bytes)?;
        check_structure(&table)?;
        Ok(table)
    }

    /// The version first, so a later layout is named as such rather than as a parse error.
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let raw: Value = serde_json::from_slice(bytes)
            .map_err(|error| format!("the AU table is not JSON ({error})"))?;
        match raw.get("version").and_then(Value::as_u64) {
            Some(AU_TABLE_VERSION) => {}
            Some(version) => {
                return Err(format!(
                "the AU table is version {version}; this mkvdovi reads version {AU_TABLE_VERSION}"
            ))
            }
            None => return Err("the AU table has no version".into()),
        }
        serde_json::from_value(raw)
            .map_err(|error| format!("the AU table does not parse ({error})"))
    }

    /// The table describes `input` as it is now: its file name, size and both chunk digests.
    pub(super) fn check_source(&self, input: &Path, identity: &FileIdentity) -> Result<(), String> {
        let source = &self.source;
        let name = input.file_name().map(|name| name.to_string_lossy());
        if name.as_deref() != Some(source.file_name.as_str()) {
            return Err(format!(
                "the AU table describes '{}', not '{}'",
                source.file_name,
                name.unwrap_or_default()
            ));
        }
        if source.size_bytes != identity.size_bytes
            || source.head_sha256 != hex(&identity.head)
            || source.tail_sha256 != hex(&identity.tail)
        {
            return Err("the input's size or chunk digests differ from the AU table's".into());
        }
        Ok(())
    }
}

/// An AU table and the bytes of the L1 sidecar it names by digest.
#[derive(Debug)]
pub(super) struct Pair {
    pub(super) table: AuTable,
    /// The sidecar as JSON, for the provenance the manifest copies.
    pub(super) sidecar: Value,
    pub(super) sidecar_sha256: String,
}

impl Pair {
    /// Reads the table at `table_path` and the sidecar at `sidecar_path`, and checks the table's
    /// version, its structure and its binding to these sidecar bytes.
    pub(super) fn read(table_path: &Path, sidecar_path: &Path) -> Result<Self, String> {
        let table_bytes = std::fs::read(table_path)
            .map_err(|error| format!("{} cannot be read ({error})", table_path.display()))?;
        let sidecar_bytes = std::fs::read(sidecar_path)
            .map_err(|error| format!("{} cannot be read ({error})", sidecar_path.display()))?;
        let sidecar = serde_json::from_slice(&sidecar_bytes)
            .map_err(|error| format!("{} is not JSON ({error})", sidecar_path.display()))?;
        Self::from_bytes(&table_bytes, &sidecar_bytes, sidecar)
    }

    pub(super) fn from_bytes(
        table_bytes: &[u8],
        sidecar_bytes: &[u8],
        sidecar: Value,
    ) -> Result<Self, String> {
        let table = AuTable::parse(table_bytes)?;
        let Some(bound) = table.sidecar_sha256.as_deref() else {
            return Err(
                "the AU table names no sidecar (written by --au-table-only), so its measurements cannot be bound to it"
                    .into(),
            );
        };
        let sidecar_sha256 = sha256_hex(sidecar_bytes);
        if bound != sidecar_sha256 {
            return Err(format!(
                "the AU table is bound to sidecar {bound}, but the sidecar next to it is {sidecar_sha256}"
            ));
        }
        check_structure(&table)?;
        Ok(Self {
            table,
            sidecar,
            sidecar_sha256,
        })
    }

    /// [`AuTable::check_source`] of the pair's table.
    pub(super) fn check_source(&self, input: &Path, identity: &FileIdentity) -> Result<(), String> {
        self.table.check_source(input, identity)
    }

    /// The table and the loaded sidecar count the same pictures and leading pictures.
    pub(super) fn check_sidecar(&self, sidecar: &L1Sidecar) -> Result<(), String> {
        let table = &self.table;
        if table.leading_skipped != sidecar.leading_frames() {
            return Err(format!(
                "the AU table has {} leading skipped pictures, the sidecar {}",
                table.leading_skipped,
                sidecar.leading_frames()
            ));
        }
        let pictures = table.aus.picture_count();
        if pictures != sidecar.stream_frame_count() {
            return Err(format!(
                "the AU table has {pictures} pictures, the sidecar covers {}",
                sidecar.stream_frame_count()
            ));
        }
        Ok(())
    }
}

/// The rules of the manifest parser's AU checks (`dovi_rpu_builder::manifest`), applied before
/// the pair is used so that a broken cached table is re-analyzed instead of refused at the end.
pub(super) fn check_structure(table: &AuTable) -> Result<(), String> {
    let track = &table.track;
    if (track.container.as_str(), track.codec.as_str()) != ("matroska", "hevc") {
        return Err(format!(
            "the AU table's track is {}/{}, not matroska/hevc",
            track.container, track.codec
        ));
    }
    if track.time_base[0] <= 0 || track.time_base[1] <= 0 {
        return Err("the AU table's time base is not positive".into());
    }
    let aus = &table.aus;
    if aus.count == 0 {
        return Err("the AU table has no AUs".into());
    }
    if aus.pts_delta.len() as u64 != aus.count - 1 || aus.size.len() as u64 != aus.count {
        return Err(format!(
            "the AU table's arrays do not fit its {} AUs ({} pts deltas, {} sizes)",
            aus.count,
            aus.pts_delta.len(),
            aus.size.len()
        ));
    }
    check_rising(&aus.non_picture, aus.count, "non_picture")?;
    let irap: Vec<u64> = aus.irap_crc32.iter().map(|entry| entry.0).collect();
    check_rising(&irap, aus.count, "irap_crc32")?;
    if irap.is_empty() {
        return Err("the AU table has no IRAP picture".into());
    }
    if let Some(index) = irap
        .iter()
        .find(|index| aus.non_picture.binary_search(index).is_ok())
    {
        return Err(format!(
            "the AU table's irap_crc32 names non-picture AU {index}"
        ));
    }
    let mut seen = HashSet::with_capacity(aus.size.len());
    if let Some(pts) = aus.pts().into_iter().find(|pts| !seen.insert(*pts)) {
        return Err(format!("two AUs of the AU table have pts {pts}"));
    }
    if table.leading_skipped > aus.picture_count() {
        return Err("the AU table's leading_skipped is above its picture count".into());
    }
    Ok(())
}

fn check_rising(indices: &[u64], count: u64, name: &str) -> Result<(), String> {
    if indices.windows(2).any(|pair| pair[1] <= pair[0]) {
        return Err(format!("the AU table's {name} is not strictly rising"));
    }
    if indices.last().is_some_and(|&last| last >= count) {
        return Err(format!("the AU table's {name} names an AU beyond {count}"));
    }
    Ok(())
}
