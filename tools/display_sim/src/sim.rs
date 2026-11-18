//! The comparison: each frame's histogram through the curve set up for the reference L1 and for
//! the candidate L1, on one display. Everything is in displayed nits (libplacebo's PQ EOTF of
//! the curve output): changes are candidate / reference - 1, the frame's light is the
//! histogram-weighted sum of displayed nits, and a shot or run aggregates light over all its
//! pixels (sum of candidate light / sum of reference light - 1) besides the per-frame spread.

use anyhow::Result;

use crate::curves::{nits_to_pq, Curve, Display, Mapping, Metadata};
use crate::hist::BINS;

/// Source luminance levels whose displayed change is reported.
pub const LEVELS_NITS: [f64; 3] = [10.0, 100.0, 200.0];

/// One metadata setting's display response.
#[derive(Debug)]
pub struct Response {
    /// Displayed nits of each bin's PQ value (`b / 4095`).
    pub bins: Vec<f64>,
    /// Displayed nits of [`LEVELS_NITS`].
    pub levels: [f64; 3],
    /// The first clipped bin: bins whose lower edge `b - 0.5` lies above the curve's clip
    /// ceiling (`4095 x ceiling`) display as the ceiling does. `BINS` when none is.
    pub first_clipped: usize,
    /// The renderer skips the curve (`pl_tone_map_params_noop`): the display clips the source.
    pub noop: bool,
}

impl Response {
    pub fn new(curve: Curve, display: &Display, metadata: &Metadata) -> Result<Self> {
        let mapping = Mapping::new(curve, display, metadata)?;
        let bins = (0..BINS)
            .map(|bin| mapping.displayed_nits(bin as f64 / 4095.0))
            .collect();
        let levels = LEVELS_NITS.map(|nits| mapping.displayed_nits(nits_to_pq(nits)));
        Ok(Self {
            bins,
            levels,
            first_clipped: first_clipped_bin(mapping.clip_ceiling()),
            noop: mapping.is_noop(),
        })
    }
}

/// The lowest bin `b` with `b - 0.5 > 4095 x ceiling`.
pub fn first_clipped_bin(ceiling: f64) -> usize {
    let edge = 4095.0 * ceiling + 0.5;
    ((edge.floor() as usize) + 1).min(BINS)
}

/// `candidate / reference - 1`, or `None` when the reference is not positive.
pub fn change(candidate: f64, reference: f64) -> Option<f64> {
    (reference > 0.0 && reference.is_finite() && candidate.is_finite())
        .then(|| candidate / reference - 1.0)
}

/// One frame on one curve and display.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameResult {
    pub level_reference: [f64; 3],
    pub level_candidate: [f64; 3],
    /// The highest non-empty bin.
    pub peak_bin: Option<usize>,
    pub peak_change: Option<f64>,
    /// Sum over pixels of displayed nits.
    pub light_reference: f64,
    pub light_candidate: f64,
    pub pixels: u64,
    pub clipped_reference: u64,
    pub clipped_candidate: u64,
    /// The curve is a no-op for the reference, for the candidate.
    pub noop: (bool, bool),
}

impl FrameResult {
    pub fn level_changes(&self) -> [Option<f64>; 3] {
        std::array::from_fn(|level| {
            change(self.level_candidate[level], self.level_reference[level])
        })
    }

    pub fn light_change(&self) -> Option<f64> {
        change(self.light_candidate, self.light_reference)
    }
}

pub fn compare_frame(counts: &[u32], reference: &Response, candidate: &Response) -> FrameResult {
    let mut result = FrameResult {
        level_reference: reference.levels,
        level_candidate: candidate.levels,
        peak_bin: None,
        peak_change: None,
        light_reference: 0.0,
        light_candidate: 0.0,
        pixels: 0,
        clipped_reference: 0,
        clipped_candidate: 0,
        noop: (reference.noop, candidate.noop),
    };
    for (bin, &count) in counts.iter().enumerate().filter(|(_, &count)| count > 0) {
        let count = u64::from(count);
        result.pixels += count;
        result.light_reference += count as f64 * reference.bins[bin];
        result.light_candidate += count as f64 * candidate.bins[bin];
        if bin >= reference.first_clipped {
            result.clipped_reference += count;
        }
        if bin >= candidate.first_clipped {
            result.clipped_candidate += count;
        }
        result.peak_bin = Some(bin);
    }
    result.peak_change = result
        .peak_bin
        .and_then(|bin| change(candidate.bins[bin], reference.bins[bin]));
    result
}

/// Frame-to-frame steps of displayed luminance at the levels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Steps {
    /// Frame pairs compared (each at three levels).
    pub pairs: usize,
    pub max_reference: f64,
    pub max_candidate: f64,
    /// Steps above the threshold.
    pub above_reference: usize,
    pub above_candidate: usize,
    /// Steps above the threshold in the candidate where the reference has none.
    pub added: usize,
}

/// `current / previous - 1` per level of one side; `None` where the previous value is 0.
pub fn level_steps(previous: &[f64; 3], current: &[f64; 3]) -> [Option<f64>; 3] {
    std::array::from_fn(|level| change(current[level], previous[level]))
}

impl Steps {
    pub fn add(&mut self, previous: &FrameResult, current: &FrameResult, threshold: f64) {
        self.pairs += 1;
        let reference = level_steps(&previous.level_reference, &current.level_reference);
        let candidate = level_steps(&previous.level_candidate, &current.level_candidate);
        for (reference, candidate) in reference.into_iter().zip(candidate) {
            let reference = reference.map_or(0.0, f64::abs);
            let candidate = candidate.map_or(0.0, f64::abs);
            self.max_reference = self.max_reference.max(reference);
            self.max_candidate = self.max_candidate.max(candidate);
            self.above_reference += usize::from(reference > threshold);
            self.above_candidate += usize::from(candidate > threshold);
            self.added += usize::from(candidate > threshold && reference <= threshold);
        }
    }
}

/// Values of one metric across frames.
#[derive(Clone, Debug, Default)]
pub struct Spread {
    values: Vec<f64>,
}

impl Spread {
    pub fn push(&mut self, value: Option<f64>) {
        if let Some(value) = value {
            self.values.push(value);
        }
    }

    /// Minimum, median (the lower middle value) and maximum.
    pub fn digest(&self) -> Option<[f64; 3]> {
        let mut sorted = self.values.clone();
        sorted.sort_by(f64::total_cmp);
        let middle = *sorted.get((sorted.len().max(1) - 1) / 2)?;
        Some([sorted[0], middle, sorted[sorted.len() - 1]])
    }
}

/// A shot or the run on one curve and display.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub frames: usize,
    pub light_reference: f64,
    pub light_candidate: f64,
    pub frame_light: Spread,
    pub levels: [Spread; 3],
    pub peak: Spread,
    pub pixels: u64,
    pub clipped_reference: u64,
    pub clipped_candidate: u64,
    /// Frames on which the curve is a no-op, reference and candidate.
    pub noop_reference: usize,
    pub noop_candidate: usize,
    pub steps: Steps,
}

impl Summary {
    pub fn add(&mut self, frame: &FrameResult) {
        self.frames += 1;
        self.light_reference += frame.light_reference;
        self.light_candidate += frame.light_candidate;
        self.frame_light.push(frame.light_change());
        for (spread, value) in self.levels.iter_mut().zip(frame.level_changes()) {
            spread.push(value);
        }
        self.peak.push(frame.peak_change);
        self.pixels += frame.pixels;
        self.clipped_reference += frame.clipped_reference;
        self.clipped_candidate += frame.clipped_candidate;
        self.noop_reference += usize::from(frame.noop.0);
        self.noop_candidate += usize::from(frame.noop.1);
    }

    /// Light-weighted change over all pixels of all frames.
    pub fn light_change(&self) -> Option<f64> {
        change(self.light_candidate, self.light_reference)
    }

    /// Clipped share of the pixels, reference and candidate.
    pub fn clipped_shares(&self) -> Option<(f64, f64)> {
        (self.pixels > 0).then(|| {
            let pixels = self.pixels as f64;
            (
                self.clipped_reference as f64 / pixels,
                self.clipped_candidate as f64 / pixels,
            )
        })
    }
}

#[cfg(test)]
#[path = "sim_tests.rs"]
mod tests;
