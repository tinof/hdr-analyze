use madvr_parse::MadVRScene;

/// Calculate histogram difference using Sum of Absolute Differences for scene detection.
///
/// # Arguments
/// * `hist1` - First histogram
/// * `hist2` - Second histogram
///
/// # Returns
/// Difference score (higher values indicate more significant changes)
pub fn calculate_histogram_difference(hist1: &[f64], hist2: &[f64]) -> f64 {
    // Chi-squared distance (symmetric form) with small epsilon to avoid div-by-zero
    let mut dist = 0.0f64;
    let len = hist1.len().min(hist2.len());
    for i in 0..len {
        let a = hist1[i];
        let b = hist2[i];
        let denom = a + b + 1e-6;
        let diff = a - b;
        dist += (diff * diff) / denom;
    }
    dist
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
    use std::collections::VecDeque;

    #[test]
    fn test_histogram_diff_smoothing_behaves() {
        // A simple increasing sequence; smoothing should be lower than last value for window>1
        let diffs = [0.1, 0.2, 0.5, 1.0];
        let mut dq: VecDeque<f64> = VecDeque::with_capacity(3);
        let mut smoothed = Vec::new();
        for d in diffs {
            dq.push_back(d);
            if dq.len() > 3 {
                dq.pop_front();
            }
            smoothed.push(dq.iter().sum::<f64>() / dq.len() as f64)
        }
        assert!(
            smoothed[3] < 1.0,
            "smoothed value should be below last raw value"
        );
        assert!((smoothed[0] - 0.1_f64).abs() < 1e-9);
    }

    #[test]
    fn test_cut_allowed_min_len() {
        // First cut at frame 10 not allowed if min len 24
        assert!(!cut_allowed(Some(0), 10, 24));
        // Cut at frame 24 allowed
        assert!(cut_allowed(Some(0), 24, 24));
        // Subsequent cut needs another 24 frames
        assert!(!cut_allowed(Some(24), 40, 24));
        assert!(cut_allowed(Some(24), 48, 24));
    }

    #[test]
    fn test_histogram_diff_identical() {
        // Identical histograms should have zero difference
        let hist1 = vec![1.0; 256];
        let hist2 = vec![1.0; 256];
        let diff = calculate_histogram_difference(&hist1, &hist2);
        assert!(
            diff.abs() < 1e-9,
            "Identical histograms should have zero diff"
        );
    }

    #[test]
    fn test_histogram_diff_opposite() {
        // Completely different histograms should have high difference
        let mut hist1 = vec![0.0; 256];
        hist1[0] = 100.0; // All black
        let mut hist2 = vec![0.0; 256];
        hist2[255] = 100.0; // All white
        let diff = calculate_histogram_difference(&hist1, &hist2);
        assert!(
            diff > 0.5,
            "Opposite histograms should have high diff, got {}",
            diff
        );
    }

    #[test]
    fn test_scene_detection_threshold() {
        // Verify cut_allowed respects scene threshold logic
        // (threshold comparison happens in run_native_analysis_pipeline,
        // but we test the min_scene_length guard here)
        assert!(cut_allowed(None, 100, 24)); // First cut always allowed
        assert!(!cut_allowed(Some(100), 110, 24)); // Too close
        assert!(cut_allowed(Some(100), 124, 24)); // Exactly min distance
    }
}
