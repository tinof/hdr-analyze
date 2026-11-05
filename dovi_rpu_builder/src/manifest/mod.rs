//! The playback manifest (`docs/PLAYBACK_MANIFEST.md`, ROADMAP P15): measured per-shot L1 for one
//! encode, with what a player needs to prove, AU by AU, that the manifest describes the stream it
//! plays. mkvdovi writes it ([`PlaybackManifest::to_json`]); the NuvioTV fork reads it
//! ([`PlaybackManifest::parse`]) and asks an [`AuMatcher`] for each AU's L1.
//!
//! The parser is strict and never panics: every refusal is [`Error::Manifest`]. A parsed manifest
//! is internally consistent (AU table, shots, coverage), its `rpu` builds with
//! [`RpuBuilder::new`], and its composer matches the mapping the analysis measured through. No
//! digest is computed here: SHA-256 values are compared as hex strings, computed by the producer
//! and the player.
//!
//! [`Error::Manifest`]: crate::Error::Manifest
//! [`RpuBuilder::new`]: crate::RpuBuilder::new

mod crc32;
mod json;
mod matcher;
mod rpu_json;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use dovi84_composer::Composer;
use serde_json::{json, Map, Value};

use self::json::{
    array_at, check_digest, check_rising, digest_at, fail, field, i64_at, object, str_at, u64_at,
    uint_at, uints,
};
use crate::builder::{RpuBuilder, L1};
use crate::composer::HlgComposer;
use crate::error::Result;
use crate::params::{Profile, RpuParams};

pub use self::crc32::crc32;
pub use self::matcher::{AuMatcher, AuVerdict, RejectReason, Rejection};

pub const MANIFEST_FORMAT: &str = "hdr-analyze.playback-manifest";
pub const MANIFEST_VERSION: u64 = 1;
/// `analysis.luminance_mapping` of a PQ measurement (the analyzer's `PQ_MAPPING`).
pub const PQ_MAPPING: &str = "pq";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Producer {
    pub mkvdovi_version: String,
    pub analyzer_version: String,
    pub sidecar_version: u64,
    pub au_table_version: u64,
    pub rpu_builder_version: String,
    /// SHA-256 of the L1 sidecar the shots come from.
    pub sidecar_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub file_name: String,
    pub size_bytes: u64,
    pub head_sha256: String,
    pub tail_sha256: String,
    /// SHA-256 of (size as big-endian u64 ‖ head digest ‖ tail digest).
    pub lookup_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub container: String,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub codec_private_sha256: String,
    /// `[num, den]`; every pts is in these ticks.
    pub time_base: (i32, i32),
}

/// The AU table: every video packet in decode order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aus {
    pub pts: Vec<i64>,
    pub size: Vec<u32>,
    /// Decode indices of packets without a picture.
    pub non_picture: Vec<u64>,
    /// `(decode index, CRC-32)` of every IRAP picture.
    pub irap_crc32: Vec<(u64, u32)>,
}

impl Aus {
    pub fn picture_count(&self) -> u64 {
        self.pts.len().saturating_sub(self.non_picture.len()) as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L1Policy {
    Measured,
    RpuBin,
}

/// A shot from presentation index `start` (stream frames) to the next shot's start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestShot {
    pub start: u64,
    /// Unclamped, as `extra.json` gets it; the builder clamps.
    pub l1: L1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityClass {
    Full,
    Approximate,
}

impl QualityClass {
    fn name(self) -> &'static str {
        match self {
            QualityClass::Full => "full",
            QualityClass::Approximate => "approximate",
        }
    }
}

/// Presentation indices `start .. end` carry this class; others carry no manifest L1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coverage {
    pub start: u64,
    pub end: u64,
    pub class: QualityClass,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackManifest {
    pub producer: Producer,
    pub source: Source,
    pub track: Track,
    pub aus: Aus,
    pub leading_skipped: u64,
    /// Analysis provenance; `luminance_mapping` is required and checked against the composer.
    pub analysis: Map<String, Value>,
    pub rpu: RpuParams,
    pub l1_policy: L1Policy,
    pub shots: Vec<ManifestShot>,
    pub coverage: Vec<Coverage>,
}

impl PlaybackManifest {
    /// Parses and validates a manifest file's text.
    pub fn parse(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text)
            .map_err(|error| crate::Error::Manifest(format!("not JSON: {error}")))?;
        Self::from_value(&value)
    }

    pub fn from_value(value: &Value) -> Result<Self> {
        let root = object(value, "manifest")?;
        let format = str_at(root, "format", "manifest")?;
        if format != MANIFEST_FORMAT {
            return fail(format!("format '{format}' is not {MANIFEST_FORMAT}"));
        }
        let version = u64_at(root, "version", "manifest")?;
        if version != MANIFEST_VERSION {
            return fail(format!("version {version} is not {MANIFEST_VERSION}"));
        }
        let manifest = Self {
            producer: read_producer(field(root, "producer", "manifest")?)?,
            source: read_source(field(root, "source", "manifest")?)?,
            track: read_track(field(root, "track", "manifest")?)?,
            aus: read_aus(field(root, "aus", "manifest")?)?,
            leading_skipped: u64_at(root, "leading_skipped", "manifest")?,
            analysis: object(field(root, "analysis", "manifest")?, "analysis")?.clone(),
            rpu: RpuParams::from_manifest_json(field(root, "rpu", "manifest")?)?,
            l1_policy: match str_at(root, "l1_policy", "manifest")? {
                "measured" => L1Policy::Measured,
                "rpu-bin" => L1Policy::RpuBin,
                other => return fail(format!("l1_policy '{other}' is not measured or rpu-bin")),
            },
            shots: read_shots(field(root, "shots", "manifest")?)?,
            coverage: read_coverage(array_at(root, "coverage", "manifest")?)?,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Every cross-field rule. [`parse`](Self::parse) runs it; a producer runs it before writing.
    pub fn validate(&self) -> Result<()> {
        self.validate_identity()?;
        self.validate_aus()?;
        self.validate_shots_and_coverage()?;
        let mapping = self.luminance_mapping()?;
        let expected = match self.rpu.profile {
            Profile::P81 { .. } => PQ_MAPPING,
            Profile::P84 { composer } => composer_mapping(composer)?,
        };
        if mapping != expected {
            return fail(format!(
                "analysis.luminance_mapping '{mapping}' does not fit the rpu ('{expected}')"
            ));
        }
        RpuBuilder::new(&self.rpu)
            .map_err(|error| crate::Error::Manifest(format!("rpu cannot be built: {error}")))?;
        Ok(())
    }

    /// The track kind version 1 covers, and the digests' syntax; the parser reads the digests with
    /// the same check.
    fn validate_identity(&self) -> Result<()> {
        let track = &self.track;
        if (track.container.as_str(), track.codec.as_str()) != ("matroska", "hevc") {
            return fail(format!(
                "track is {}/{}; version 1 covers matroska/hevc",
                track.container, track.codec
            ));
        }
        for (digest, path) in [
            (&self.producer.sidecar_sha256, "producer.sidecar_sha256"),
            (&self.source.head_sha256, "source.head_sha256"),
            (&self.source.tail_sha256, "source.tail_sha256"),
            (&self.source.lookup_key, "source.lookup_key"),
            (&track.codec_private_sha256, "track.codec_private_sha256"),
        ] {
            check_digest(digest, path)?;
        }
        Ok(())
    }

    fn validate_aus(&self) -> Result<()> {
        let aus = &self.aus;
        let count = aus.pts.len() as u64;
        if count == 0 {
            return fail("aus.count is 0");
        }
        if aus.size.len() != aus.pts.len() {
            return fail("aus.size and the pts differ in length");
        }
        check_rising(&aus.non_picture, count, "aus.non_picture")?;
        let irap: Vec<u64> = aus.irap_crc32.iter().map(|entry| entry.0).collect();
        check_rising(&irap, count, "aus.irap_crc32")?;
        if let Some(index) = irap
            .iter()
            .find(|index| aus.non_picture.binary_search(index).is_ok())
        {
            return fail(format!("aus.irap_crc32 names non-picture AU {index}"));
        }
        let mut seen = HashSet::with_capacity(aus.pts.len());
        if let Some(pts) = aus.pts.iter().find(|pts| !seen.insert(**pts)) {
            return fail(format!("two AUs have pts {pts}"));
        }
        if self.leading_skipped > aus.picture_count() {
            return fail("leading_skipped is above the picture count");
        }
        if self.track.time_base.0 <= 0 || self.track.time_base.1 <= 0 {
            return fail("track.time_base is not positive");
        }
        Ok(())
    }

    fn validate_shots_and_coverage(&self) -> Result<()> {
        let pictures = self.aus.picture_count();
        let Some(first) = self.shots.first() else {
            return fail("shots is empty");
        };
        if first.start != 0 {
            return fail("the first shot does not start at 0");
        }
        for (index, pair) in self.shots.windows(2).enumerate() {
            if pair[1].start <= pair[0].start {
                return fail(format!(
                    "shot {} does not start after shot {index}",
                    index + 1
                ));
            }
        }
        if let Some(last) = self.shots.last().filter(|shot| shot.start >= pictures) {
            return fail(format!(
                "a shot starts at {}, not below {pictures} pictures",
                last.start
            ));
        }
        if let Some(shot) = self.shots.iter().find(|shot| {
            let l1 = shot.l1;
            l1.min_pq > 4095 || l1.avg_pq > 4095 || l1.max_pq > 4095
        }) {
            return fail(format!("the shot at {} has an L1 above 4095", shot.start));
        }
        if self.coverage.is_empty() {
            return fail("coverage is empty");
        }
        let is_start = |value: u64| {
            self.shots
                .binary_search_by_key(&value, |shot| shot.start)
                .is_ok()
        };
        let mut previous_end = 0;
        for (index, range) in self.coverage.iter().enumerate() {
            if range.start >= range.end || range.end > pictures {
                return fail(format!(
                    "coverage[{index}] is empty or beyond {pictures} pictures"
                ));
            }
            if index > 0 && range.start < previous_end {
                return fail(format!("coverage[{index}] overlaps or is not sorted"));
            }
            // No shot may be partly covered: it would mix manifest L1 and the fallback.
            if !is_start(range.start) || !(range.end == pictures || is_start(range.end)) {
                return fail(format!(
                    "coverage[{index}] does not start and end on shot boundaries"
                ));
            }
            previous_end = range.end;
        }
        Ok(())
    }

    pub fn luminance_mapping(&self) -> Result<&str> {
        str_at(&self.analysis, "luminance_mapping", "analysis")
    }

    /// Refuses a manifest whose 8.4 composer is not the one the player writes.
    pub fn check_composer(&self, expected: HlgComposer) -> Result<()> {
        match self.rpu.profile {
            Profile::P84 { composer } if composer == expected => Ok(()),
            Profile::P84 { composer } => fail(format!(
                "the manifest's composer is {composer:?}, the player writes {expected:?}"
            )),
            Profile::P81 { .. } => fail("the manifest is profile 8.1, not 8.4"),
        }
    }

    /// The candidate step: size and both chunk digests of the file the player opened.
    pub fn check_source(
        &self,
        size_bytes: u64,
        head_sha256: &str,
        tail_sha256: &str,
    ) -> Result<()> {
        let source = &self.source;
        if size_bytes != source.size_bytes {
            return fail(format!(
                "source size {size_bytes} is not {}",
                source.size_bytes
            ));
        }
        if head_sha256 != source.head_sha256 || tail_sha256 != source.tail_sha256 {
            return fail("source chunk digests differ");
        }
        Ok(())
    }

    /// The track the player opened: the CodecPrivate digest, the dimensions and the time base.
    pub fn check_track(
        &self,
        codec_private_sha256: &str,
        width: u32,
        height: u32,
        time_base: (i32, i32),
    ) -> Result<()> {
        let track = &self.track;
        if codec_private_sha256 != track.codec_private_sha256 {
            return fail("the CodecPrivate digest differs");
        }
        if (width, height) != (track.width, track.height) {
            return fail(format!(
                "the track is {width}x{height}, the manifest {}x{}",
                track.width, track.height
            ));
        }
        if time_base != track.time_base {
            return fail(format!(
                "the time base is {}/{}, the manifest {}/{}",
                time_base.0, time_base.1, track.time_base.0, track.time_base.1
            ));
        }
        Ok(())
    }

    /// The manifest as JSON. Refuses a manifest that does not [`validate`](Self::validate).
    pub fn to_json(&self) -> Result<Value> {
        self.validate()?;
        let aus = &self.aus;
        let pts_delta: Vec<i64> = aus
            .pts
            .windows(2)
            .map(|pair| pair[1].wrapping_sub(pair[0]))
            .collect();
        Ok(json!({
            "format": MANIFEST_FORMAT,
            "version": MANIFEST_VERSION,
            "producer": {
                "mkvdovi_version": self.producer.mkvdovi_version,
                "analyzer_version": self.producer.analyzer_version,
                "sidecar_version": self.producer.sidecar_version,
                "au_table_version": self.producer.au_table_version,
                "rpu_builder_version": self.producer.rpu_builder_version,
                "sidecar_sha256": self.producer.sidecar_sha256,
            },
            "source": {
                "file_name": self.source.file_name,
                "size_bytes": self.source.size_bytes,
                "head_sha256": self.source.head_sha256,
                "tail_sha256": self.source.tail_sha256,
                "lookup_key": self.source.lookup_key,
            },
            "track": {
                "container": self.track.container,
                "codec": self.track.codec,
                "width": self.track.width,
                "height": self.track.height,
                "codec_private_sha256": self.track.codec_private_sha256,
                "time_base": [self.track.time_base.0, self.track.time_base.1],
            },
            "aus": {
                "count": aus.pts.len(),
                "pts_first": aus.pts[0],
                "pts_delta": pts_delta,
                "size": aus.size,
                "non_picture": aus.non_picture,
                "irap_crc32": aus.irap_crc32.iter().map(|&(k, crc)| json!([k, crc])).collect::<Vec<_>>(),
            },
            "leading_skipped": self.leading_skipped,
            "analysis": self.analysis,
            "rpu": self.rpu.to_manifest_json()?,
            "l1_policy": match self.l1_policy {
                L1Policy::Measured => "measured",
                L1Policy::RpuBin => "rpu-bin",
            },
            "shots": {
                "start": self.shots.iter().map(|shot| shot.start).collect::<Vec<_>>(),
                "min": self.shots.iter().map(|shot| shot.l1.min_pq).collect::<Vec<_>>(),
                "avg": self.shots.iter().map(|shot| shot.l1.avg_pq).collect::<Vec<_>>(),
                "max": self.shots.iter().map(|shot| shot.l1.max_pq).collect::<Vec<_>>(),
            },
            "coverage": self.coverage.iter().map(|range| json!({
                "start": range.start,
                "end": range.end,
                "class": range.class.name(),
            })).collect::<Vec<_>>(),
        }))
    }
}

fn composer_mapping(composer: HlgComposer) -> Result<&'static str> {
    match composer {
        HlgComposer::Preset => Ok(Composer::Preset.luminance_mapping()),
        HlgComposer::Bt2100 => Ok(Composer::Bt2100V1.luminance_mapping()),
        HlgComposer::Canary { .. } => fail("the canary composer has no measurement mapping"),
    }
}

fn read_producer(value: &Value) -> Result<Producer> {
    let map = object(value, "producer")?;
    let path = "producer";
    Ok(Producer {
        mkvdovi_version: str_at(map, "mkvdovi_version", path)?.to_owned(),
        analyzer_version: str_at(map, "analyzer_version", path)?.to_owned(),
        sidecar_version: u64_at(map, "sidecar_version", path)?,
        au_table_version: u64_at(map, "au_table_version", path)?,
        rpu_builder_version: str_at(map, "rpu_builder_version", path)?.to_owned(),
        sidecar_sha256: digest_at(map, "sidecar_sha256", path)?,
    })
}

fn read_source(value: &Value) -> Result<Source> {
    let map = object(value, "source")?;
    let path = "source";
    Ok(Source {
        file_name: str_at(map, "file_name", path)?.to_owned(),
        size_bytes: u64_at(map, "size_bytes", path)?,
        head_sha256: digest_at(map, "head_sha256", path)?,
        tail_sha256: digest_at(map, "tail_sha256", path)?,
        lookup_key: digest_at(map, "lookup_key", path)?,
    })
}

fn read_track(value: &Value) -> Result<Track> {
    let map = object(value, "track")?;
    let path = "track";
    let container = str_at(map, "container", path)?;
    let codec = str_at(map, "codec", path)?;
    if (container, codec) != ("matroska", "hevc") {
        return fail(format!(
            "track is {container}/{codec}; version 1 covers matroska/hevc"
        ));
    }
    let time_base = array_at(map, "time_base", path)?;
    let [num, den] = time_base.as_slice() else {
        return fail("track.time_base is not [num, den]");
    };
    let part = |value: &Value| value.as_i64().and_then(|value| i32::try_from(value).ok());
    let (Some(num), Some(den)) = (part(num), part(den)) else {
        return fail("track.time_base is not two 32-bit integers");
    };
    Ok(Track {
        container: container.to_owned(),
        codec: codec.to_owned(),
        width: uint_at(map, "width", path)?,
        height: uint_at(map, "height", path)?,
        codec_private_sha256: digest_at(map, "codec_private_sha256", path)?,
        time_base: (num, den),
    })
}

fn read_aus(value: &Value) -> Result<Aus> {
    let map = object(value, "aus")?;
    let path = "aus";
    let count = u64_at(map, "count", path)?;
    let deltas = array_at(map, "pts_delta", path)?;
    if count == 0 || deltas.len() as u64 != count - 1 {
        return fail(format!(
            "aus.pts_delta has {} entries for {count} AUs",
            deltas.len()
        ));
    }
    let mut pts = Vec::with_capacity(deltas.len() + 1);
    pts.push(i64_at(map, "pts_first", path)?);
    for (index, delta) in deltas.iter().enumerate() {
        let delta = delta.as_i64().ok_or_else(|| {
            crate::Error::Manifest(format!("aus.pts_delta[{index}] is not an integer"))
        })?;
        pts.push(pts[index].wrapping_add(delta));
    }
    let size: Vec<u32> = uints(array_at(map, "size", path)?, "aus.size")?;
    if size.len() as u64 != count {
        return fail(format!(
            "aus.size has {} entries for {count} AUs",
            size.len()
        ));
    }
    let irap_crc32 = array_at(map, "irap_crc32", path)?
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let pair = entry.as_array().filter(|pair| pair.len() == 2);
            let parsed = pair
                .and_then(|pair| Some((pair[0].as_u64()?, u32::try_from(pair[1].as_u64()?).ok()?)));
            parsed.ok_or_else(|| {
                crate::Error::Manifest(format!("aus.irap_crc32[{index}] is not [index, crc32]"))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Aus {
        pts,
        size,
        non_picture: uints(array_at(map, "non_picture", path)?, "aus.non_picture")?,
        irap_crc32,
    })
}

fn read_shots(value: &Value) -> Result<Vec<ManifestShot>> {
    let map = object(value, "shots")?;
    let column = |key: &str| -> Result<Vec<u64>> {
        uints(array_at(map, key, "shots")?, &format!("shots.{key}"))
    };
    let start = column("start")?;
    let [min, avg, max] = ["min", "avg", "max"].map(|key| {
        array_at(map, key, "shots").and_then(|values| uints::<u16>(values, &format!("shots.{key}")))
    });
    let (min, avg, max) = (min?, avg?, max?);
    if [min.len(), avg.len(), max.len()] != [start.len(); 3] {
        return fail("the shot columns differ in length");
    }
    Ok((0..start.len())
        .map(|index| ManifestShot {
            start: start[index],
            l1: L1::new(min[index], avg[index], max[index]),
        })
        .collect())
}

fn read_coverage(values: &[Value]) -> Result<Vec<Coverage>> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let path = format!("coverage[{index}]");
            let map = object(value, &path)?;
            Ok(Coverage {
                start: u64_at(map, "start", &path)?,
                end: u64_at(map, "end", &path)?,
                class: match str_at(map, "class", &path)? {
                    "full" => QualityClass::Full,
                    "approximate" => QualityClass::Approximate,
                    other => {
                        return fail(format!("{path}.class '{other}' is not full or approximate"))
                    }
                },
            })
        })
        .collect()
}
