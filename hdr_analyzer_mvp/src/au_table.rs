//! The AU table (`<measurements>.au.json`): every packet of the video stream in decode order,
//! for the playback manifest (`docs/PLAYBACK_MANIFEST.md`, ROADMAP P15).
//!
//! A player that injects this analyzer's L1 while it plays the file must find, for each access
//! unit it reads, the measured frame that AU becomes. The table gives it the AU's pts (in the
//! stream's time base), its size, CRC-32 for IRAP AUs, and the source and track identity. The
//! presentation index of a picture is its pts rank among the pictures; [`AuRecorder::finish`]
//! checks that rule against the pts of the frames the decoder output, so a table is only written
//! when the measured frames provably line up with it. The table is bound to the sidecar written
//! in the same run by the sidecar's SHA-256.
//!
//! `--help` advertises the capability `au-table-v1` (`cli.rs`); mkvdovi probes it before it
//! relies on the table. Version 1 covers one HEVC video track in Matroska. It refuses laced video blocks, packets
//! without a pts and two video packets with one pts; the measurements are unaffected by a refusal.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ffmpeg_next::{codec, format, media};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::leading_pictures::{first_vcl_nal_type, hevc_framing, LeadingPictureScan, NalFraming};

pub const AU_TABLE_VERSION: u32 = 1;
/// Bytes hashed at each end of the source for its identity.
pub const CHUNK_BYTES: u64 = 1 << 20;

const CRC32: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);
/// BLA_W_LP (16) to the reserved IRAP types (22, 23).
const IRAP: std::ops::RangeInclusive<u8> = 16..=23;

pub fn au_table_path(output_path: &Path) -> PathBuf {
    let mut path = output_path.as_os_str().to_owned();
    path.push(".au.json");
    PathBuf::from(path)
}

#[derive(Debug, Serialize)]
pub struct AuTable {
    pub version: u32,
    pub analyzer_version: String,
    /// SHA-256 of the L1 sidecar written in the same run; `None` for `--au-table-only`.
    pub sidecar_sha256: Option<String>,
    pub source: AuSource,
    pub track: AuTrack,
    pub aus: Aus,
    /// RASL pictures of the first IRAP picture that no decoder outputs (the sidecar's
    /// `leading_skipped_frames`).
    pub leading_skipped: u64,
    pub scene: SceneSettings,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AuSource {
    pub file_name: String,
    pub size_bytes: u64,
    /// SHA-256 of bytes `[0, min(1 MiB, size))`.
    pub head_sha256: String,
    /// SHA-256 of bytes `[max(0, size - 1 MiB), size)`.
    pub tail_sha256: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AuTrack {
    pub container: String,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// SHA-256 of the codec private data (`hvcC`).
    pub codec_private_sha256: String,
    /// The stream time base `[num, den]`; every pts is in these ticks.
    pub time_base: [i32; 2],
}

#[derive(Debug, Serialize)]
pub struct Aus {
    pub count: u64,
    /// pts of the first packet in decode order.
    pub pts_first: i64,
    /// `pts[k + 1] - pts[k]` in decode order (`count - 1` entries).
    pub pts_delta: Vec<i64>,
    /// Packet sizes in bytes, decode order.
    pub size: Vec<u32>,
    /// Decode indices of packets without a slice NAL unit (no picture).
    pub non_picture: Vec<u64>,
    /// `[decode index, CRC-32/ISO-HDLC of the whole packet]` for every IRAP picture.
    pub irap_crc32: Vec<(u64, u32)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SceneSettings {
    pub detector: String,
    pub metric: String,
    pub threshold: f64,
    pub min_length: u32,
}

/// Records the video packets of one analysis run.
#[derive(Debug)]
pub struct AuRecorder {
    framing: NalFraming,
    leading: LeadingPictureScan,
    source: AuSource,
    track: AuTrack,
    pts: Vec<i64>,
    size: Vec<u32>,
    non_picture: Vec<u64>,
    irap_crc32: Vec<(u64, u32)>,
    last_position: Option<i64>,
    output_pts: Vec<Option<i64>>,
    refusal: Option<String>,
}

/// The stream the analyzer measures: FFmpeg's best video stream.
pub fn measured_video_stream(input: &format::context::Input) -> Result<format::stream::Stream<'_>> {
    input
        .streams()
        .best(media::Type::Video)
        .context("No video stream found")
}

impl AuRecorder {
    /// Refuses, before any analysis, an input the table cannot describe.
    pub fn new(input_path: &Path, input: &format::context::Input) -> Result<Self> {
        let input_format = input.format();
        let container = input_format.name();
        if !container.split(',').any(|name| name == "matroska") {
            anyhow::bail!(
                "--au-table needs a Matroska input (this is '{container}'); the playback manifest covers MKV only"
            );
        }
        let videos = input
            .streams()
            .filter(|stream| {
                stream.parameters().medium() == media::Type::Video
                    && !stream
                        .disposition()
                        .contains(format::stream::Disposition::ATTACHED_PIC)
            })
            .count();
        if videos != 1 {
            anyhow::bail!(
                "--au-table needs exactly one video track; this file has {videos}, and the analyzer and mkvdovi could pick different ones"
            );
        }
        let stream = measured_video_stream(input)?;
        let parameters = stream.parameters();
        if parameters.id() != codec::Id::HEVC {
            anyhow::bail!("--au-table needs an HEVC video track");
        }
        let extradata = crate::ffmpeg_io::codec_extradata(&parameters);
        let framing = hevc_framing(&extradata);
        // SAFETY: parameters wraps a live AVCodecParameters; width and height are plain fields.
        let (width, height) = unsafe {
            let raw = parameters.as_ptr();
            ((*raw).width, (*raw).height)
        };
        let time_base = stream.time_base();
        Ok(Self::from_parts(
            framing,
            source_identity(input_path)?,
            AuTrack {
                container: "matroska".to_owned(),
                codec: "hevc".to_owned(),
                width: u32::try_from(width).unwrap_or(0),
                height: u32::try_from(height).unwrap_or(0),
                codec_private_sha256: hex_sha256(&extradata),
                time_base: [time_base.numerator(), time_base.denominator()],
            },
        ))
    }

    pub(crate) fn from_parts(framing: NalFraming, source: AuSource, track: AuTrack) -> Self {
        Self {
            framing,
            leading: LeadingPictureScan::new(framing),
            source,
            track,
            pts: Vec::new(),
            size: Vec::new(),
            non_picture: Vec::new(),
            irap_crc32: Vec::new(),
            last_position: None,
            output_pts: Vec::new(),
            refusal: None,
        }
    }

    /// Feeds the next video packet in decode order. `position` is the container byte position
    /// FFmpeg reports (-1 when unknown).
    pub fn observe_packet(&mut self, pts: Option<i64>, position: i64, data: &[u8]) {
        if self.refusal.is_some() {
            return;
        }
        let index = self.pts.len() as u64;
        let Some(pts) = pts else {
            self.refuse(format!("video packet {index} has no pts"));
            return;
        };
        // FFmpeg gives every lace of a block the block's position, so a laced block shows as
        // packets that do not advance. The player synthesizes lace times differently.
        if position < 0 {
            self.refuse(format!(
                "video packet {index} has no container position, so laced blocks cannot be ruled out"
            ));
            return;
        }
        if self.last_position.is_some_and(|last| position <= last) {
            self.refuse(format!(
                "video packet {index} shares its Matroska block with the previous one (laced video)"
            ));
            return;
        }
        self.last_position = Some(position);
        let Ok(size) = u32::try_from(data.len()) else {
            self.refuse(format!("video packet {index} is larger than 4 GiB"));
            return;
        };
        self.leading.observe_packet(data);
        match first_vcl_nal_type(data, self.framing) {
            None => self.non_picture.push(index),
            Some(kind) if IRAP.contains(&kind) => {
                self.irap_crc32.push((index, CRC32.checksum(data)));
            }
            Some(_) => {}
        }
        self.pts.push(pts);
        self.size.push(size);
    }

    fn refuse(&mut self, reason: String) {
        self.refusal = Some(reason);
        self.pts = Vec::new();
        self.size = Vec::new();
        self.non_picture = Vec::new();
        self.irap_crc32 = Vec::new();
    }

    /// The pts of the frames the decoder output, in output order.
    pub fn set_output_pts(&mut self, output_pts: Vec<Option<i64>>) {
        self.output_pts = output_pts;
    }

    /// Builds the table once every packet and output frame is observed. Refuses when the
    /// presentation rule does not hold.
    pub fn finish(self, sidecar_sha256: Option<String>, scene: SceneSettings) -> Result<AuTable> {
        if let Some(reason) = self.refusal {
            anyhow::bail!("{reason}");
        }
        let leading_skipped = self.leading.skipped();
        check_presentation_order(
            &self.pts,
            &self.non_picture,
            leading_skipped,
            &self.output_pts,
        )?;
        let pts_first = self
            .pts
            .first()
            .copied()
            .context("the video stream has no packets")?;
        Ok(AuTable {
            version: AU_TABLE_VERSION,
            analyzer_version: crate::cli::VERSION.to_owned(),
            sidecar_sha256,
            source: self.source,
            track: self.track,
            aus: Aus {
                count: self.pts.len() as u64,
                pts_first,
                pts_delta: self
                    .pts
                    .windows(2)
                    .map(|pair| pair[1].wrapping_sub(pair[0]))
                    .collect(),
                size: self.size,
                non_picture: self.non_picture,
                irap_crc32: self.irap_crc32,
            },
            leading_skipped,
            scene,
        })
    }
}

/// The decoder's output, in order, must be the pictures sorted by pts without the first
/// `leading_skipped` (the RASL pictures of the first IRAP, which precede every other picture in
/// output order). Only then is a picture's presentation index its pts rank.
fn check_presentation_order(
    pts: &[i64],
    non_picture: &[u64],
    leading_skipped: u64,
    output_pts: &[Option<i64>],
) -> Result<()> {
    // The matcher finds an AU by its pts, so it must be unique over every AU, pictures or not.
    let mut all = pts.to_vec();
    all.sort_unstable();
    if let Some(pair) = all.windows(2).find(|pair| pair[0] == pair[1]) {
        anyhow::bail!("two video packets have the same pts {}", pair[0]);
    }
    let mut pictures: Vec<i64> = pts
        .iter()
        .enumerate()
        .filter(|(index, _)| non_picture.binary_search(&(*index as u64)).is_err())
        .map(|(_, &pts)| pts)
        .collect();
    pictures.sort_unstable();
    let skip = usize::try_from(leading_skipped).unwrap_or(usize::MAX);
    let expected = pictures.get(skip..).unwrap_or_default();
    if output_pts.len() != expected.len() {
        anyhow::bail!(
            "the decoder output {} frames, the stream has {} pictures after {} skipped leading pictures",
            output_pts.len(),
            expected.len(),
            leading_skipped
        );
    }
    for (index, (output, &expected)) in output_pts.iter().zip(expected).enumerate() {
        match output {
            Some(output) if *output == expected => {}
            Some(output) => anyhow::bail!(
                "output frame {index} has pts {output}, but pts order says {expected}: presentation order is not pts order"
            ),
            None => anyhow::bail!("output frame {index} has no pts"),
        }
    }
    Ok(())
}

fn hex_sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hash_range(file: &mut File, start: u64, length: u64) -> Result<String> {
    file.seek(SeekFrom::Start(start))?;
    let mut buffer = Vec::new();
    file.take(length).read_to_end(&mut buffer)?;
    if buffer.len() as u64 != length {
        anyhow::bail!("short read: {} of {length} bytes", buffer.len());
    }
    Ok(hex_sha256(&buffer))
}

/// Size and the SHA-256 of the first and last MiB, read from the file itself.
pub fn source_identity(path: &Path) -> Result<AuSource> {
    let mut file =
        File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let size_bytes = file.metadata()?.len();
    let head = size_bytes.min(CHUNK_BYTES);
    let tail_start = size_bytes.saturating_sub(CHUNK_BYTES);
    Ok(AuSource {
        file_name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size_bytes,
        head_sha256: hash_range(&mut file, 0, head)
            .with_context(|| format!("Failed to hash the start of {}", path.display()))?,
        tail_sha256: hash_range(&mut file, tail_start, size_bytes - tail_start)
            .with_context(|| format!("Failed to hash the end of {}", path.display()))?,
    })
}

/// SHA-256 of a file's bytes (the sidecar the table is bound to).
pub fn file_sha256(path: &Path) -> Result<String> {
    let bytes =
        std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    Ok(hex_sha256(&bytes))
}

/// Writes through a temp file and a rename, so a reader never sees a partial table.
pub fn write_au_table(path: &Path, table: &AuTable) -> Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    let mut file = File::create(&temp)
        .with_context(|| format!("Failed to create AU table {}", temp.display()))?;
    serde_json::to_writer(&mut file, table)
        .with_context(|| format!("Failed to serialize AU table {}", temp.display()))?;
    file.flush()?;
    drop(file);
    std::fs::rename(&temp, path)
        .with_context(|| format!("Failed to move AU table into place at {}", path.display()))
}

/// Any run rewrites the sidecar, so a table left by an earlier run no longer describes it.
pub fn remove_stale_au_table(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            println!("Removed the AU table of an earlier run: {}", path.display());
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("Failed to remove the stale AU table {}", path.display())),
    }
}

#[cfg(test)]
#[path = "au_table_tests.rs"]
mod tests;
