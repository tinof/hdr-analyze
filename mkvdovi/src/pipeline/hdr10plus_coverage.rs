//! How many pictures of an HDR10+ stream carry their own HDR10+ payload (ROADMAP P9.4).
//!
//! `hdr10plus_tool extract` gives a picture without a payload the previous picture's, and
//! FFmpeg's decoder carries the last payload forward too, so neither shows partial coverage.
//! The scan reads the Annex B stream that the HDR10+ path already extracts. A picture counts
//! when a prefix SEI before its first slice holds a `user_data_registered_itu_t_t35` message
//! with the HDR10+ identity, the message `hdr10plus_tool` 1.7.2 picks (the port in
//! `tools/rpu_builder_check/src/payloads.rs`). A scan that cannot be trusted only warns.

use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::Path;

use dolby_vision::utils::clear_start_code_emulation_prevention_3_byte;
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::progress;
use crate::resume;

use super::hdr10plus::HDR10PLUS_HEVC;
use super::measure::hybrid_refusal;

/// The sealed count in the temp directory, reused on resume.
pub(super) const COVERAGE_FILE: &str = "hdr10plus_coverage.json";
const COVERAGE_VERSION: u32 = 1;
/// The longest SEI NAL unit the scan keeps; a longer one makes the scan untrusted.
const SEI_CAP: usize = 256 * 1024;
const CHUNK: usize = 1 << 20;

const PREFIX_SEI: u8 = 39;
const SUFFIX_SEI: u8 = 40;
const USER_DATA_REGISTERED_ITU_T_T35: usize = 4;
/// Country code (United States), provider code (Samsung), provider-oriented code,
/// application identifier and application version of an HDR10+ (ST 2094-40) message.
const HDR10PLUS_IDENTITY: [u8; 7] = [0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04, 0x01];

/// Pictures in decode order and how many of them carry their own HDR10+ payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Coverage {
    pub(super) access_units: u64,
    pub(super) own: u64,
    /// Runs of pictures without their own payload.
    pub(super) gaps: u64,
    pub(super) longest_gap: u64,
    /// Decode index of the first picture without its own payload.
    pub(super) first_gap: Option<u64>,
    /// HDR10+ payloads that no picture takes: in a suffix SEI, between the slices of a picture,
    /// or after the last picture.
    pub(super) ignored_payloads: u64,
}

#[derive(Serialize, Deserialize)]
struct Sealed {
    version: u32,
    coverage: Coverage,
}

/// The decision on a count next to the metadata `hdr10plus_tool` extracted.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Full(String),
    Partial(String),
    Refused(String),
    NotCounted(String),
}

/// The decision on a count when `hdr10plus_tool` extracted nothing.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Unextracted {
    /// No payload, or no count: the HDR10 fallback as before.
    NoPayloads,
    /// Payloads that the tool could not extract: the fallback, with this warning.
    Unreadable(String),
    /// The same under an explicit hybrid: this refusal reason.
    Refused(String),
}

/// Counts the HDR10+ payloads of `reader`, an Annex B HEVC stream.
pub(super) fn scan(reader: impl Read) -> Result<Coverage, String> {
    scan_with_cap(reader, SEI_CAP)
}

fn scan_with_cap(mut reader: impl Read, cap: usize) -> Result<Coverage, String> {
    let mut scanner = Scanner::new(cap);
    let mut buffer = vec![0; CHUNK];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("the stream could not be read ({error})")),
        };
        scanner.feed(&buffer[..read])?;
    }
    scanner.finish()
}

/// A streaming Annex B reader: it keeps the first three bytes of each NAL unit and the whole
/// unit only for SEI.
struct Scanner {
    cap: usize,
    /// Zero bytes seen and not yet given to a NAL unit: they end it if a start code follows.
    zeros: usize,
    in_nal: bool,
    nal_len: usize,
    header: [u8; 3],
    sei: Option<Vec<u8>>,
    overflow: bool,
    tally: Tally,
}

impl Scanner {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            zeros: 0,
            in_nal: false,
            nal_len: 0,
            header: [0; 3],
            sei: None,
            overflow: false,
            tally: Tally::default(),
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<(), String> {
        for &byte in bytes {
            if byte == 0 {
                self.zeros += 1;
                continue;
            }
            if byte == 1 && self.zeros >= 2 {
                if self.in_nal {
                    self.end_nal()?;
                }
                self.in_nal = true;
                self.nal_len = 0;
                self.sei = None;
            } else if self.in_nal {
                self.push_zeros();
                self.push(byte);
            }
            self.zeros = 0;
        }
        Ok(())
    }

    fn push_zeros(&mut self) {
        if self.sei.is_none() && self.nal_len >= 3 {
            self.nal_len += self.zeros;
            return;
        }
        for _ in 0..self.zeros {
            self.push(0);
        }
    }

    fn push(&mut self, byte: u8) {
        let index = self.nal_len;
        self.nal_len += 1;
        if index < 3 {
            self.header[index] = byte;
        }
        if index == 1 {
            let (kind, layer) = self.kind_and_layer();
            if layer == 0 && (kind == PREFIX_SEI || kind == SUFFIX_SEI) {
                self.sei = Some(vec![self.header[0], self.header[1]]);
            }
            return;
        }
        if let Some(sei) = &mut self.sei {
            if sei.len() >= self.cap {
                self.overflow = true;
                self.sei = None;
            } else {
                sei.push(byte);
            }
        }
    }

    fn kind_and_layer(&self) -> (u8, u8) {
        let kind = (self.header[0] >> 1) & 0x3F;
        let layer = ((self.header[0] & 1) << 5) | (self.header[1] >> 3);
        (kind, layer)
    }

    fn end_nal(&mut self) -> Result<(), String> {
        self.in_nal = false;
        if self.overflow {
            return Err(format!("an SEI NAL unit is longer than {} bytes", self.cap));
        }
        if self.nal_len < 2 {
            return Ok(());
        }
        let (kind, layer) = self.kind_and_layer();
        if layer != 0 {
            return Ok(());
        }
        match kind {
            0..=9 | 16..=21 => {
                if self.nal_len < 3 {
                    return Err("a slice NAL unit ends inside its header".into());
                }
                self.tally.slice(self.header[2] & 0x80 != 0)
            }
            PREFIX_SEI | SUFFIX_SEI => {
                let sei = self.sei.take().unwrap_or_default();
                if carries_hdr10plus(&sei)? {
                    if kind == PREFIX_SEI {
                        self.tally.pending = true;
                    } else {
                        self.tally.ignored_payloads += 1;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn finish(mut self) -> Result<Coverage, String> {
        if self.in_nal {
            self.end_nal()?;
        }
        Ok(self.tally.finish())
    }
}

#[derive(Default)]
struct Tally {
    access_units: u64,
    own: u64,
    gaps: u64,
    longest_gap: u64,
    first_gap: Option<u64>,
    run: u64,
    ignored_payloads: u64,
    /// A prefix SEI with an HDR10+ payload waits for the slice that decides its picture.
    pending: bool,
    in_picture: bool,
}

impl Tally {
    fn slice(&mut self, first_in_picture: bool) -> Result<(), String> {
        if !first_in_picture {
            if !self.in_picture {
                return Err("a slice continues a picture whose first slice is missing".into());
            }
            // A payload between the slices of one picture belongs to no picture.
            if self.pending {
                self.ignored_payloads += 1;
                self.pending = false;
            }
            return Ok(());
        }
        let index = self.access_units;
        self.access_units += 1;
        self.in_picture = true;
        if std::mem::take(&mut self.pending) {
            self.own += 1;
            self.run = 0;
        } else {
            if self.run == 0 {
                self.gaps += 1;
                self.first_gap.get_or_insert(index);
            }
            self.run += 1;
            self.longest_gap = self.longest_gap.max(self.run);
        }
        Ok(())
    }

    fn finish(self) -> Coverage {
        Coverage {
            access_units: self.access_units,
            own: self.own,
            gaps: self.gaps,
            longest_gap: self.longest_gap,
            first_gap: self.first_gap,
            ignored_payloads: self.ignored_payloads + u64::from(self.pending),
        }
    }
}

/// Whether the SEI NAL unit `nal` (header included) holds an HDR10+ message. The walk must end
/// at the RBSP trailing bits; a message that runs past the unit, or a unit that ends without
/// them, makes the scan untrusted.
fn carries_hdr10plus(nal: &[u8]) -> Result<bool, String> {
    let rbsp = clear_start_code_emulation_prevention_3_byte(nal);
    let mut position = 2;
    let mut found = false;
    loop {
        match rbsp.len().checked_sub(position) {
            None | Some(0) => return Err("an SEI NAL unit ends without its trailing bits".into()),
            Some(1) if rbsp[position] == 0x80 => return Ok(found),
            Some(_) => {}
        }
        let payload_type = read_coded(&rbsp, &mut position)?;
        let size = read_coded(&rbsp, &mut position)?;
        let end = position
            .checked_add(size)
            .filter(|&end| end <= rbsp.len())
            .ok_or("an SEI message runs past its NAL unit")?;
        if payload_type == USER_DATA_REGISTERED_ITU_T_T35
            && size >= HDR10PLUS_IDENTITY.len()
            && rbsp[position..position + HDR10PLUS_IDENTITY.len()] == HDR10PLUS_IDENTITY
        {
            found = true;
        }
        position = end;
    }
}

/// An SEI payload type or size: `0xFF` bytes, each adding 255, then the last byte.
fn read_coded(rbsp: &[u8], position: &mut usize) -> Result<usize, String> {
    let mut value = 0;
    loop {
        let byte = *rbsp
            .get(*position)
            .ok_or("an SEI message header runs past its NAL unit")?;
        *position += 1;
        value += usize::from(byte);
        if byte != 0xFF {
            return Ok(value);
        }
    }
}

/// The count for the stream in `temp_dir`: sealed from an earlier run when `resume`, else
/// scanned and sealed. An error seals nothing.
pub(super) fn count_sealed(temp_dir: &Path, resume: bool) -> Result<Coverage, String> {
    let sealed = temp_dir.join(COVERAGE_FILE);
    if resume && resume::is_complete(&sealed) {
        let reused = fs::read(&sealed)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Sealed>(&bytes).ok())
            .filter(|sealed| sealed.version == COVERAGE_VERSION);
        if let Some(reused) = reused {
            return Ok(reused.coverage);
        }
    }
    let hevc = temp_dir.join(HDR10PLUS_HEVC);
    let spinner = progress::Spinner::new("Counting HDR10+ payloads");
    let counted = File::open(&hevc)
        .map_err(|error| format!("{} could not be opened ({error})", hevc.display()))
        .and_then(scan);
    match &counted {
        Ok(coverage) => {
            spinner.finish_success();
            let record = Sealed {
                version: COVERAGE_VERSION,
                coverage: coverage.clone(),
            };
            // A seal that cannot be written only costs a new scan on the next run.
            if serde_json::to_vec(&record)
                .ok()
                .is_some_and(|bytes| fs::write(&sealed, bytes).is_ok())
            {
                let _ = resume::mark_done(&sealed);
            }
        }
        Err(_) => spinner.finish_error(None),
    }
    counted
}

/// The number of frames in `hdr10plus_tool`'s metadata, without parsing them.
pub(super) fn scene_info_frames(hdr10plus_json: &Path) -> Option<u64> {
    #[derive(Deserialize)]
    struct Frames {
        #[serde(rename = "SceneInfo")]
        scene_info: Vec<IgnoredAny>,
    }
    let file = File::open(hdr10plus_json).ok()?;
    let frames: Frames = serde_json::from_reader(BufReader::new(file)).ok()?;
    Some(frames.scene_info.len() as u64)
}

fn gap_detail(coverage: &Coverage) -> String {
    let mut detail = format!(
        "{} gap(s) in decode order, the longest {} frame(s), the first at frame {}",
        coverage.gaps,
        coverage.longest_gap,
        coverage.first_gap.unwrap_or(0)
    );
    detail.push_str(&ignored_detail(coverage));
    detail
}

fn ignored_detail(coverage: &Coverage) -> String {
    match coverage.ignored_payloads {
        0 => String::new(),
        n => format!(
            "; {n} more HDR10+ payload(s) sit where no frame takes them (a suffix SEI, between \
             slices or after the last frame)"
        ),
    }
}

/// The decision when `hdr10plus_tool` extracted `scene_frames` frames. Only a trusted partial
/// count refuses, and only under an explicit hybrid.
pub(super) fn judge_extracted(
    counted: &Result<Coverage, String>,
    scene_frames: Option<u64>,
    explicit_hybrid: bool,
) -> Verdict {
    let coverage = match counted {
        Ok(coverage) => coverage,
        Err(reason) => return Verdict::NotCounted(reason.clone()),
    };
    let Some(frames) = scene_frames else {
        return Verdict::NotCounted("the HDR10+ metadata has no SceneInfo list".into());
    };
    if coverage.access_units != frames {
        return Verdict::NotCounted(format!(
            "the stream has {} frames, the HDR10+ metadata {frames}",
            coverage.access_units
        ));
    }
    if coverage.own == 0 {
        return Verdict::NotCounted("the stream scan found no HDR10+ payload".into());
    }
    if coverage.own == frames {
        return Verdict::Full(format!(
            "HDR10+ payloads: all {frames} frames carry their own{}.",
            ignored_detail(coverage)
        ));
    }
    let missing = frames - coverage.own;
    if explicit_hybrid {
        return Verdict::Refused(format!(
            "only {} of {frames} frames carry their own HDR10+ payload ({})",
            coverage.own,
            gap_detail(coverage)
        ));
    }
    Verdict::Partial(format!(
        "HDR10+ payloads: {} of {frames} frames carry their own; hdr10plus_tool repeats the \
         previous frame's for the other {missing} ({}). L1 peaks there come from earlier metadata.",
        coverage.own,
        gap_detail(coverage)
    ))
}

/// The decision when `hdr10plus_tool` extracted nothing from a stream with payloads.
pub(super) fn judge_unextracted(
    counted: &Result<Coverage, String>,
    explicit_hybrid: bool,
    tool_log: &Path,
) -> Unextracted {
    let Ok(coverage) = counted else {
        return Unextracted::NoPayloads;
    };
    if coverage.own == 0 {
        return Unextracted::NoPayloads;
    }
    let counts = format!(
        "{} of {} frames carry their own HDR10+ payload, but hdr10plus_tool could not extract \
         them (see {})",
        coverage.own,
        coverage.access_units,
        tool_log.display()
    );
    if explicit_hybrid {
        return Unextracted::Refused(counts);
    }
    Unextracted::Unreadable(format!(
        "{counts}. Falling back to HDR10 analysis without the HDR10+ metadata."
    ))
}

/// The refusal of an explicit hybrid after `hdr10plus_tool` extracted nothing. Unlike
/// `hybrid_refusal`, it names no `--hdr10plus-l1 hdr10plus`: that mode has no metadata here
/// either and falls back to HDR10 analysis.
pub(super) fn unextracted_refusal(reason: &str) -> String {
    format!(
        "--hdr10plus-l1 hybrid cannot be used here: {reason}. Without --hdr10plus-l1 hybrid the \
         file converts from HDR10 analysis, without the HDR10+ metadata."
    )
}

/// Counts the stream next to `hdr10plus_json` and prints the decision. `false`: refused.
pub(super) fn check_extracted(
    temp_dir: &Path,
    hdr10plus_json: &Path,
    resume: bool,
    explicit_hybrid: bool,
) -> bool {
    let counted = count_sealed(temp_dir, resume);
    match judge_extracted(&counted, scene_info_frames(hdr10plus_json), explicit_hybrid) {
        Verdict::Full(message) => progress::print_info(&message),
        Verdict::Partial(message) => progress::print_warn(&message),
        Verdict::NotCounted(reason) => progress::print_warn(&format!(
            "HDR10+ payload coverage was not counted: {reason}."
        )),
        Verdict::Refused(reason) => {
            progress::print_error(&hybrid_refusal(&reason));
            return false;
        }
    }
    true
}

/// Counts the stream after `hdr10plus_tool` extracted nothing. Without an extracted stream
/// there is nothing to count.
pub(super) fn check_unextracted(
    temp_dir: &Path,
    resume: bool,
    explicit_hybrid: bool,
) -> Unextracted {
    if !resume::is_complete(&temp_dir.join(HDR10PLUS_HEVC)) {
        return Unextracted::NoPayloads;
    }
    let counted = count_sealed(temp_dir, resume);
    if let Err(reason) = &counted {
        progress::print_warn(&format!(
            "HDR10+ payload coverage was not counted: {reason}."
        ));
    }
    judge_unextracted(
        &counted,
        explicit_hybrid,
        &temp_dir.join(super::hdr10plus::HDR10PLUS_TOOL_LOG),
    )
}

#[cfg(test)]
mod tests;
