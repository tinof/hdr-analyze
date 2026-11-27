//! The frame order pass 1 reads from the stream: for each picture in decode order, its
//! presentation number (which RPU it carries) and its first slice's type (which AUD it gets).

use anyhow::{bail, Result};
use hevc_parser::hevc::Frame;

/// One picture of the stream, found by its decoded number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Entry {
    /// Its position in display order: the RPU with this index is written into its access unit.
    pub(super) presentation: usize,
    /// `hevc_parser`'s type of its first slice (2 = I, 1 = P, 0 = B), as `aud_for_frame` reads it.
    pub(super) frame_type: u64,
}

/// Every picture of the stream, indexed by decoded number.
#[derive(Debug)]
pub(super) struct FrameIndex(Vec<Entry>);

impl FrameIndex {
    /// The index of `parser.ordered_frames()`. dovi_tool looks each picture up by decoded number
    /// with `find` and takes the first match; this refuses instead unless the decoded numbers and
    /// the presentation numbers are each exactly `0..n`, so no lookup can be ambiguous or missing.
    pub(super) fn from_frames(frames: &[Frame]) -> Result<Self> {
        let count = frames.len();
        let mut entries: Vec<Option<Entry>> = vec![None; count];
        let mut presented = vec![false; count];
        for frame in frames {
            let decoded = usize::try_from(frame.decoded_number).unwrap_or(usize::MAX);
            let presentation = usize::try_from(frame.presentation_number).unwrap_or(usize::MAX);
            if decoded >= count || presentation >= count {
                bail!(
                    "picture {} (presentation {}) is outside the {count} pictures of the stream",
                    frame.decoded_number,
                    frame.presentation_number
                );
            }
            if entries[decoded].is_some() {
                bail!("two pictures carry decoded number {decoded}");
            }
            if presented[presentation] {
                bail!("two pictures carry presentation number {presentation}");
            }
            presented[presentation] = true;
            entries[decoded] = Some(Entry {
                presentation,
                frame_type: frame.frame_type,
            });
        }
        // `count` distinct decoded numbers below `count` fill every slot.
        Ok(Self(entries.into_iter().flatten().collect()))
    }

    /// The picture with decoded number `decoded`.
    pub(super) fn get(&self, decoded: u64) -> Option<Entry> {
        usize::try_from(decoded)
            .ok()
            .and_then(|decoded| self.0.get(decoded).copied())
    }

    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
}

/// The RPU dovi_tool writes into the picture with presentation number `presentation`: the one
/// at that index, else, when the RPU and the stream differ in length, the last one. `None`
/// (dovi_tool's "No RPU found for presentation frame") otherwise.
pub(super) fn rpu_index(presentation: usize, rpu_count: usize, mismatched: bool) -> Option<usize> {
    if presentation < rpu_count {
        Some(presentation)
    } else if mismatched && rpu_count > 0 {
        Some(rpu_count - 1)
    } else {
        None
    }
}
