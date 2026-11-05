//! [`AuMatcher`]: checks each AU a player reads against the manifest's AU table and says which L1
//! it carries. The player feeds AUs in decode order, as its extractor reads them, and calls
//! [`AuMatcher::reset`] at every seek or flush.
//!
//! - After a start or reset the matcher waits for an IRAP picture: AUs before it get
//!   [`AuVerdict::Fallback`] (Media3 drops them after a seek anyway), but their pts and size are
//!   still checked. The first IRAP must match by
//!   pts, size and CRC-32; from there every AU must be the table's next one.
//! - A picture whose presentation index lies in a coverage range gets the L1 of its shot, with
//!   `scene_refresh` on the shot's first picture only (mkvdovi parity). Other pictures and
//!   non-picture packets get the fallback, and are still checked.
//! - Any mismatch rejects the manifest for the rest of the playback; the rejection is sticky
//!   across resets.

use std::collections::HashMap;

use super::crc32::crc32;
use super::{Coverage, ManifestShot, PlaybackManifest, QualityClass};
use crate::builder::L1;
use crate::error::Result;

/// Why the matcher rejected the manifest. [`code`](Self::code) is stable across releases, for
/// callers across FFI; a new reason takes the next number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// The AU's pts is not in the table.
    UnknownPts,
    /// The AU's size differs from the table's.
    Size,
    /// The IRAP picture's CRC-32 differs from the table's.
    Crc,
    /// A continuous read reached an AU that is not the table's next one.
    Order,
    /// The AU came from a laced block, which the table does not describe.
    Laced,
    /// A continuous read went past the table's last AU.
    PastEnd,
}

impl RejectReason {
    pub fn code(self) -> i32 {
        match self {
            RejectReason::UnknownPts => 1,
            RejectReason::Size => 2,
            RejectReason::Crc => 3,
            RejectReason::Order => 4,
            RejectReason::Laced => 5,
            RejectReason::PastEnd => 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub reason: RejectReason,
    /// The table's decode index the AU was checked against, when there is one.
    pub decode_index: Option<u64>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuVerdict {
    /// Write this L1 into the AU's RPU.
    Inject {
        l1: L1,
        scene_refresh: bool,
        /// Index into the manifest's shots.
        shot: usize,
        class: QualityClass,
    },
    /// Use the player's own RPU: waiting for an IRAP, a non-picture packet, or not covered.
    Fallback,
    /// The manifest does not describe this stream.
    Rejected(Rejection),
}

#[derive(Debug, Clone)]
enum State {
    AwaitingIrap,
    Continuous { next: usize },
    Rejected(Rejection),
}

#[derive(Debug, Clone)]
pub struct AuMatcher {
    pts: Vec<i64>,
    size: Vec<u32>,
    index_by_pts: HashMap<i64, usize>,
    /// Presentation index per decode index; `None` for non-picture packets.
    rank: Vec<Option<u64>>,
    irap_crc: HashMap<usize, u32>,
    shots: Vec<ManifestShot>,
    coverage: Vec<Coverage>,
    state: State,
}

impl AuMatcher {
    /// Validates the manifest first ([`PlaybackManifest::validate`]): its fields are public, so a
    /// manifest built by hand reaches here without the parser's checks.
    pub fn new(manifest: &PlaybackManifest) -> Result<Self> {
        manifest.validate()?;
        let aus = &manifest.aus;
        let index_by_pts = aus
            .pts
            .iter()
            .enumerate()
            .map(|(index, &pts)| (pts, index))
            .collect();
        let mut pictures: Vec<usize> = (0..aus.pts.len())
            .filter(|index| aus.non_picture.binary_search(&(*index as u64)).is_err())
            .collect();
        pictures.sort_unstable_by_key(|&index| aus.pts[index]);
        let mut rank = vec![None; aus.pts.len()];
        for (presentation, &index) in pictures.iter().enumerate() {
            rank[index] = Some(presentation as u64);
        }
        Ok(Self {
            pts: aus.pts.clone(),
            size: aus.size.clone(),
            index_by_pts,
            rank,
            irap_crc: aus
                .irap_crc32
                .iter()
                .map(|&(index, crc)| (index as usize, crc))
                .collect(),
            shots: manifest.shots.clone(),
            coverage: manifest.coverage.clone(),
            state: State::AwaitingIrap,
        })
    }

    /// After a seek or a flush: wait for the next IRAP picture again. A rejection stays.
    pub fn reset(&mut self) {
        if !matches!(self.state, State::Rejected(_)) {
            self.state = State::AwaitingIrap;
        }
    }

    pub fn rejection(&self) -> Option<&Rejection> {
        match &self.state {
            State::Rejected(rejection) => Some(rejection),
            _ => None,
        }
    }

    /// The verdict for the next AU in decode order: its pts in the track's ticks, its bytes as
    /// the container stores them (header stripping undone), and whether it came from a laced
    /// block.
    pub fn next(&mut self, pts: i64, data: &[u8], laced: bool) -> AuVerdict {
        if let State::Rejected(rejection) = &self.state {
            return AuVerdict::Rejected(rejection.clone());
        }
        if laced {
            return self.reject(
                RejectReason::Laced,
                None,
                format!("AU at pts {pts} is laced"),
            );
        }
        let index = match self.locate(pts) {
            Ok(index) => index,
            Err(verdict) => return verdict,
        };
        // Every AU's size is checked, also the ones before the IRAP a seek waits for.
        if let Some(verdict) = self.check_bytes(index, data) {
            return verdict;
        }
        if matches!(self.state, State::AwaitingIrap) && !self.irap_crc.contains_key(&index) {
            return AuVerdict::Fallback;
        }
        self.state = State::Continuous { next: index + 1 };
        self.verdict(index)
    }

    /// The table's decode index for this AU.
    fn locate(&mut self, pts: i64) -> Result<usize, AuVerdict> {
        match self.state {
            State::Rejected(ref rejection) => Err(AuVerdict::Rejected(rejection.clone())),
            State::AwaitingIrap => {
                let Some(&index) = self.index_by_pts.get(&pts) else {
                    return Err(self.reject(
                        RejectReason::UnknownPts,
                        None,
                        format!("pts {pts} is not in the AU table"),
                    ));
                };
                Ok(index)
            }
            State::Continuous { next } => {
                if next >= self.pts.len() {
                    return Err(self.reject(
                        RejectReason::PastEnd,
                        None,
                        format!("pts {pts} comes after the table's last AU"),
                    ));
                }
                if self.pts[next] != pts {
                    let reason = if self.index_by_pts.contains_key(&pts) {
                        RejectReason::Order
                    } else {
                        RejectReason::UnknownPts
                    };
                    return Err(self.reject(
                        reason,
                        Some(next as u64),
                        format!("pts {pts}, the table's next AU has pts {}", self.pts[next]),
                    ));
                }
                Ok(next)
            }
        }
    }

    /// Size, and the CRC-32 of an IRAP picture.
    fn check_bytes(&mut self, index: usize, data: &[u8]) -> Option<AuVerdict> {
        let expected = self.size[index];
        if u32::try_from(data.len()).ok() != Some(expected) {
            return Some(self.reject(
                RejectReason::Size,
                Some(index as u64),
                format!("AU {index} has {} bytes, the table {expected}", data.len()),
            ));
        }
        if let Some(&crc) = self.irap_crc.get(&index) {
            let actual = crc32(data);
            if actual != crc {
                return Some(self.reject(
                    RejectReason::Crc,
                    Some(index as u64),
                    format!("IRAP AU {index} has CRC-32 {actual:08x}, the table {crc:08x}"),
                ));
            }
        }
        None
    }

    fn verdict(&self, index: usize) -> AuVerdict {
        let Some(presentation) = self.rank[index] else {
            return AuVerdict::Fallback;
        };
        let Some(range) = self
            .coverage
            .iter()
            .find(|range| range.start <= presentation && presentation < range.end)
        else {
            return AuVerdict::Fallback;
        };
        // Shots start at 0 and rise (`new` validates), so one starts at or before any index.
        let shot = self
            .shots
            .partition_point(|shot| shot.start <= presentation)
            .saturating_sub(1);
        AuVerdict::Inject {
            l1: self.shots[shot].l1,
            scene_refresh: self.shots[shot].start == presentation,
            shot,
            class: range.class,
        }
    }

    fn reject(
        &mut self,
        reason: RejectReason,
        decode_index: Option<u64>,
        detail: String,
    ) -> AuVerdict {
        let rejection = Rejection {
            reason,
            decode_index,
            detail,
        };
        self.state = State::Rejected(rejection.clone());
        AuVerdict::Rejected(rejection)
    }
}
