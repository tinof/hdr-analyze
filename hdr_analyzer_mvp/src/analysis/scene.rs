use madvr_parse::MadVRScene;

/// How many frames before and after a candidate are compared across it. A cut has to separate
/// every such pair; a flash shorter than this is bridged by one of them at both of its ends and
/// rejected.
const LOOKAROUND_FRAMES: usize = 4;

/// Half-width, in analyzed frames, of the window whose median frame-to-frame distance is the
/// local activity level (grain, motion) a cut has to stand out from.
const BASELINE_HALF_WINDOW: usize = 12;

/// A cut's distance has to exceed the local activity level by this factor.
const CONTRAST_RATIO: f64 = 16.0;

/// Symmetric chi-squared distance between two luminance histograms.
///
/// The histograms are percentages (sum 100), so the result ranges from 0 (identical) to about
/// 200 (disjoint).
pub fn calculate_histogram_difference(hist1: &[f64], hist2: &[f64]) -> f64 {
    let mut dist = 0.0f64;
    let len = hist1.len().min(hist2.len());
    for i in 0..len {
        let a = hist1[i];
        let b = hist2[i];
        // Small epsilon avoids a division by zero on empty bins.
        let denom = a + b + 1e-6;
        let diff = a - b;
        dist += (diff * diff) / denom;
    }
    dist
}

/// Per-frame scene-detection values, one entry per analyzed frame.
#[derive(Debug, Clone, Default)]
pub struct SceneSeries {
    /// Distance to the previous analyzed frame (0 for the first).
    pub diff: Vec<f64>,
    /// Smallest distance over all pairs that straddle the frame within `LOOKAROUND_FRAMES`.
    /// Stays high at a cut and drops at a flash, where the picture comes back.
    pub score: Vec<f64>,
    /// Local activity level: the median `diff` of the surrounding frames, the frame and its
    /// direct neighbours excluded.
    pub baseline: Vec<f64>,
}

/// Compute the scene-detection series from the luminance histograms of the analyzed frames.
pub fn scene_series(histograms: &[&[f64]]) -> SceneSeries {
    let count = histograms.len();
    let Some(last) = count.checked_sub(1) else {
        return SceneSeries::default();
    };

    let diff: Vec<f64> = (0..count)
        .map(|index| match index {
            0 => 0.0,
            _ => calculate_histogram_difference(histograms[index], histograms[index - 1]),
        })
        .collect();

    let score: Vec<f64> = (0..count)
        .map(|index| {
            let mut lowest = diff[index];
            for before in 1..=LOOKAROUND_FRAMES {
                let earlier = histograms[index.saturating_sub(before)];
                for after in 0..=LOOKAROUND_FRAMES {
                    let later = histograms[(index + after).min(last)];
                    lowest = lowest.min(calculate_histogram_difference(earlier, later));
                }
            }
            lowest
        })
        .collect();

    // `diff[0]` has no previous frame and is left out.
    let mut neighbours = Vec::with_capacity(2 * BASELINE_HALF_WINDOW);
    let baseline: Vec<f64> = (0..count)
        .map(|index| {
            neighbours.clear();
            let first = index.saturating_sub(BASELINE_HALF_WINDOW).max(1);
            let end = (index + BASELINE_HALF_WINDOW).min(last);
            neighbours.extend(
                (first..=end)
                    .filter(|other| other.abs_diff(index) > 1)
                    .map(|other| diff[other]),
            );
            median(&mut neighbours).unwrap_or(0.0)
        })
        .collect();

    SceneSeries {
        diff,
        score,
        baseline,
    }
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable_by(f64::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2.0
    })
}

/// Choose scene cuts from the series.
///
/// A frame is a candidate when its score exceeds both `threshold` and `CONTRAST_RATIO` times
/// the local baseline. Candidates are accepted strongest first, so a weak candidate shortly
/// before a real cut cannot suppress it; an accepted cut blocks candidates closer than
/// `min_scene_length`.
///
/// `frame_indices[i]` is the source frame number of analyzed frame `i` (they differ with
/// `--sample-rate`); the returned cuts and `min_scene_length` are in source frames.
pub fn select_scene_cuts(
    series: &SceneSeries,
    frame_indices: &[u32],
    threshold: f64,
    min_scene_length: u32,
) -> Vec<u32> {
    let mut candidates: Vec<(f64, u32)> = (1..series.score.len().min(frame_indices.len()))
        .filter(|&index| {
            let score = series.score[index];
            score > threshold && score > CONTRAST_RATIO * series.baseline[index]
        })
        .map(|index| {
            let strength = series.score[index] / (series.baseline[index] + threshold);
            (strength, frame_indices[index])
        })
        .collect();
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));

    let mut cuts: Vec<u32> = Vec::new();
    for (_, frame) in candidates {
        if cut_allowed(None, frame, min_scene_length)
            && cuts
                .iter()
                .all(|&cut| cut.abs_diff(frame) >= min_scene_length)
        {
            cuts.push(frame);
        }
    }
    cuts.sort_unstable();
    cuts
}

/// Decide whether a candidate cut is allowed given the last accepted cut and minimum scene length.
pub fn cut_allowed(last_cut: Option<u32>, candidate_frame: u32, min_scene_len: u32) -> bool {
    match last_cut {
        None => candidate_frame >= min_scene_len,
        Some(prev) => candidate_frame.saturating_sub(prev) >= min_scene_len,
    }
}

/// Convert scene cuts to MadVRScene structures.
///
/// # Arguments
/// * `scene_cuts` - Vector of frame indices where scene cuts occur
/// * `total_frames` - Total number of frames processed
///
/// # Returns
/// Vector of MadVRScene structures
pub fn convert_scene_cuts_to_scenes(
    mut scene_cuts: Vec<u32>,
    total_frames: u32,
) -> Vec<MadVRScene> {
    let mut scenes = Vec::new();
    let mut start_frame = 0u32;

    // Sort scene cuts to ensure proper ordering
    scene_cuts.sort_unstable();

    for &cut_frame in &scene_cuts {
        scenes.push(MadVRScene {
            start: start_frame,
            end: cut_frame.saturating_sub(1),
            peak_nits: 0, // Will be calculated later
            avg_pq: 0.0,  // Will be calculated later
            ..Default::default()
        });
        start_frame = cut_frame;
    }

    // Add final scene
    if !scene_cuts.is_empty() {
        scenes.push(MadVRScene {
            start: start_frame,
            end: total_frames.saturating_sub(1), // Use actual last frame index
            peak_nits: 0,
            avg_pq: 0.0,
            ..Default::default()
        });
    } else {
        // No scene cuts detected, create single scene
        scenes.push(MadVRScene {
            start: 0,
            end: total_frames.saturating_sub(1),
            peak_nits: 0,
            avg_pq: 0.0,
            ..Default::default()
        });
    }

    scenes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A histogram with all weight around `center`, plus `noise` moved to a neighbouring bin.
    fn hist(center: usize, noise: f64) -> Vec<f64> {
        let mut h = vec![0.0; 256];
        h[center] = 100.0 - noise;
        h[center + 1] = noise;
        h
    }

    /// Run the detector over histograms, one analyzed frame per source frame.
    fn detect(hists: &[Vec<f64>], threshold: f64, min_len: u32) -> Vec<u32> {
        let refs: Vec<&[f64]> = hists.iter().map(Vec::as_slice).collect();
        let indices: Vec<u32> = (0..hists.len() as u32).collect();
        select_scene_cuts(&scene_series(&refs), &indices, threshold, min_len)
    }

    /// `len` frames around `center` whose noise share alternates, like grain.
    fn shot(center: usize, len: usize) -> Vec<Vec<f64>> {
        (0..len)
            .map(|i| hist(center, if i % 2 == 0 { 10.0 } else { 14.0 }))
            .collect()
    }

    #[test]
    fn test_cut_allowed_min_len() {
        assert!(!cut_allowed(Some(0), 10, 24));
        assert!(cut_allowed(Some(0), 24, 24));
        assert!(!cut_allowed(Some(24), 40, 24));
        assert!(cut_allowed(Some(24), 48, 24));
        assert!(cut_allowed(None, 100, 24));
        assert!(!cut_allowed(None, 10, 24));
    }

    #[test]
    fn test_histogram_diff_identical() {
        let hist1 = vec![1.0; 256];
        let hist2 = vec![1.0; 256];
        assert!(calculate_histogram_difference(&hist1, &hist2).abs() < 1e-9);
    }

    #[test]
    fn test_histogram_diff_disjoint_is_full_scale() {
        let diff = calculate_histogram_difference(&hist(0, 0.0), &hist(200, 0.0));
        assert!((diff - 200.0).abs() < 1e-3, "got {diff}");
    }

    #[test]
    fn noisy_static_shot_has_no_cut() {
        // The frame-to-frame distance is far above an absolute threshold of 0.01 on every
        // frame; only the contrast against the local level keeps this from cutting.
        let frames = shot(50, 120);
        let refs: Vec<&[f64]> = frames.iter().map(Vec::as_slice).collect();
        assert!(scene_series(&refs).diff[1..].iter().all(|d| *d > 0.05));
        assert_eq!(detect(&frames, 0.01, 12), Vec::<u32>::new());
    }

    #[test]
    fn hard_cut_is_found_at_its_frame() {
        let mut frames = shot(50, 37);
        frames.extend(shot(150, 61));
        assert_eq!(detect(&frames, 3.0, 12), vec![37]);
    }

    #[test]
    fn flashes_shorter_than_the_lookaround_are_rejected() {
        for flash_len in 1..LOOKAROUND_FRAMES {
            let mut frames = shot(50, 40);
            frames.extend(shot(220, flash_len));
            frames.extend(shot(50, 40));
            assert_eq!(
                detect(&frames, 3.0, 12),
                Vec::<u32>::new(),
                "flash of {flash_len} frames"
            );
        }
    }

    #[test]
    fn flash_before_a_cut_does_not_suppress_the_cut() {
        // Flash at 40-41, real cut at 46: closer together than the minimum scene length.
        let mut frames = shot(50, 40);
        frames.extend(shot(220, 2));
        frames.extend(shot(50, 4));
        frames.extend(shot(150, 50));
        assert_eq!(detect(&frames, 3.0, 12), vec![46]);
    }

    #[test]
    fn stronger_candidate_wins_inside_the_minimum_length() {
        // Two real changes 6 frames apart; the larger one is kept, although it comes second.
        let mut frames = shot(50, 40);
        frames.extend(shot(51, 6));
        frames.extend(shot(180, 50));
        assert_eq!(detect(&frames, 3.0, 12), vec![46]);
    }

    #[test]
    fn slow_fade_is_not_a_cut() {
        // One bin per frame: every step looks like its neighbours, so nothing stands out.
        let frames: Vec<Vec<f64>> = (0..100).map(|i| hist(20 + i, 10.0)).collect();
        assert_eq!(detect(&frames, 3.0, 12), Vec::<u32>::new());
    }

    #[test]
    fn no_cut_before_the_minimum_length() {
        let mut frames = shot(50, 5);
        frames.extend(shot(150, 60));
        assert_eq!(detect(&frames, 3.0, 12), Vec::<u32>::new());
    }

    #[test]
    fn sampled_frames_report_source_frame_numbers() {
        // Every third frame analyzed: analyzed frame 13 is source frame 39.
        let mut frames = shot(50, 13);
        frames.extend(shot(150, 20));
        let refs: Vec<&[f64]> = frames.iter().map(Vec::as_slice).collect();
        let indices: Vec<u32> = (0..frames.len() as u32).map(|i| i * 3).collect();
        let cuts = select_scene_cuts(&scene_series(&refs), &indices, 3.0, 12);
        assert_eq!(cuts, vec![39]);
    }

    #[test]
    fn empty_and_single_frame_inputs() {
        assert!(scene_series(&[]).diff.is_empty());
        let one = hist(10, 0.0);
        assert_eq!(detect(&[one], 3.0, 12), Vec::<u32>::new());
    }
}
