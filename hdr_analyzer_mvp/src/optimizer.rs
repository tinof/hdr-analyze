use anyhow::Result;
use madvr_parse::{MadVRFrame, MadVRScene};
use std::collections::VecDeque;
use std::io::Write;

use crate::analysis::histogram::{find_highlight_knee_nits, pq_to_nits};

// --- Optimizer Profile Configuration ---
#[derive(Debug, Clone, Copy)]
pub struct OptimizerProfile {
    /// Maximum delta per frame for target_nits (lower = smoother, higher = more responsive)
    pub max_delta_per_frame: u16,
    /// Hard cap threshold for extreme peaks
    pub extreme_peak_threshold: u32,
    /// Dark scene target clamp range (min, max)
    pub dark_scene_clamp: (u32, u32),
    /// Medium scene target clamp range (min, max)
    pub medium_scene_clamp: (u32, u32),
    /// Bright scene target clamp range (min, max)
    pub bright_scene_clamp: (u32, u32),
    /// Knee multiplier for dark scenes (allow this much above knee)
    pub dark_knee_multiplier: f64,
    /// Knee multiplier for medium scenes
    pub medium_knee_multiplier: f64,
    /// Knee multiplier for bright scenes
    pub bright_knee_multiplier: f64,
    /// Rolling window for knee smoothing
    pub knee_smoothing_window: usize,
}

impl OptimizerProfile {
    pub fn conservative() -> Self {
        Self {
            max_delta_per_frame: 100,
            extreme_peak_threshold: 3500,
            dark_scene_clamp: (600, 1500),
            medium_scene_clamp: (500, 1200),
            bright_scene_clamp: (400, 900),
            dark_knee_multiplier: 1.1,
            medium_knee_multiplier: 1.05,
            bright_knee_multiplier: 1.0,
            knee_smoothing_window: 10,
        }
    }

    pub fn balanced() -> Self {
        Self {
            max_delta_per_frame: 200,
            extreme_peak_threshold: 4000,
            dark_scene_clamp: (800, 2000),
            medium_scene_clamp: (600, 1500),
            bright_scene_clamp: (400, 1000),
            dark_knee_multiplier: 1.2,
            medium_knee_multiplier: 1.1,
            bright_knee_multiplier: 1.0,
            knee_smoothing_window: 5,
        }
    }

    pub fn aggressive() -> Self {
        Self {
            max_delta_per_frame: 300,
            extreme_peak_threshold: 4500,
            dark_scene_clamp: (1000, 2500),
            medium_scene_clamp: (800, 2000),
            bright_scene_clamp: (500, 1200),
            dark_knee_multiplier: 1.3,
            medium_knee_multiplier: 1.15,
            bright_knee_multiplier: 1.05,
            knee_smoothing_window: 3,
        }
    }

    pub fn from_name(name: &str) -> Result<Self> {
        match name.to_lowercase().as_str() {
            "conservative" => Ok(Self::conservative()),
            "balanced" => Ok(Self::balanced()),
            "aggressive" => Ok(Self::aggressive()),
            _ => Err(anyhow::anyhow!(
                "Invalid optimizer profile: '{}'. Valid options: conservative, balanced, aggressive",
                name
            )),
        }
    }
}

/// Advanced optimizer with rolling averages and scene-aware heuristics.
///
/// This function implements the core optimization algorithm that generates
/// dynamic target nits for each frame. It uses:
/// - 240-frame rolling average for temporal smoothing
/// - 99th percentile highlight knee detection with per-scene smoothing
/// - Scene-aware heuristics based on average picture level
/// - Peak brightness analysis for tone mapping decisions
/// - Configurable profiles for conservative/balanced/aggressive optimization
///
/// The optimizer aims to preserve artistic intent while ensuring smooth
/// transitions and preventing blown highlights or crushed shadows.
///
/// # Arguments
/// * `scenes` - Scene metadata for scene-aware processing
/// * `frames` - Mutable slice of frame data to optimize
/// * `profile` - Optimizer profile configuration
pub fn run_optimizer_pass(
    scenes: &[MadVRScene],
    frames: &mut [MadVRFrame],
    profile: &OptimizerProfile,
) {
    const ROLLING_WINDOW_SIZE: usize = 240; // 240 frames as recommended by research

    let total_frames = frames.len();
    println!(
        "Applying advanced optimization heuristics with {}-frame rolling window (scene-aware)...",
        ROLLING_WINDOW_SIZE
    );

    let mut processed = 0usize;
    let mut prev_target: Option<u16> = None;

    for scene in scenes {
        let start = scene.start as usize;
        let end = ((scene.end + 1) as usize).min(frames.len());
        if start >= end {
            continue;
        }

        // Reset smoothing at scene boundary to avoid cross-scene lag
        let mut rolling_avg_queue: VecDeque<f64> = VecDeque::with_capacity(ROLLING_WINDOW_SIZE);
        // Dynamic clipping heuristic: smooth the highlight knee within the scene
        let mut knee_smoothing_queue: VecDeque<f64> =
            VecDeque::with_capacity(profile.knee_smoothing_window);

        let scene_avg_apl_nits = pq_to_nits(scene.avg_pq);

        for frame in frames.iter_mut().take(end).skip(start) {
            // Add current frame's avg_pq to rolling window
            rolling_avg_queue.push_back(frame.avg_pq);
            if rolling_avg_queue.len() > ROLLING_WINDOW_SIZE {
                rolling_avg_queue.pop_front();
            }

            // Rolling average PQ blended with scene average to be truly scene-aware
            let rolling_avg_pq =
                rolling_avg_queue.iter().sum::<f64>() / rolling_avg_queue.len() as f64;
            let rolling_apl_nits = pq_to_nits(rolling_avg_pq);
            let blended_apl_nits = 0.6 * rolling_apl_nits + 0.4 * scene_avg_apl_nits;

            // Convert peak PQ to nits for decision making
            let peak_nits = pq_to_nits(frame.peak_pq_2020) as u32;

            // Find highlight knee (99th percentile)
            let raw_highlight_knee_nits = find_highlight_knee_nits(&frame.lum_histogram);

            // Dynamic clipping heuristic: smooth the knee within the scene to avoid banding
            knee_smoothing_queue.push_back(raw_highlight_knee_nits);
            if knee_smoothing_queue.len() > profile.knee_smoothing_window {
                knee_smoothing_queue.pop_front();
            }
            let smoothed_knee_nits =
                knee_smoothing_queue.iter().sum::<f64>() / knee_smoothing_queue.len() as f64;

            // Apply heuristics with scene-aware APL and smoothed knee
            let raw_target = apply_advanced_heuristics(
                peak_nits,
                blended_apl_nits,
                smoothed_knee_nits,
                scene_avg_apl_nits,
                profile,
            );

            // Apply delta limiting for temporal smoothness
            let final_target =
                apply_delta_limit(prev_target, raw_target, profile.max_delta_per_frame);
            frame.target_nits = Some(final_target);
            prev_target = Some(final_target);

            processed += 1;
            if processed % 1000 == 0 {
                let progress = (processed as f64 / total_frames as f64) * 100.0;
                print!("\rOptimizer progress: {:.1}%", progress);
                std::io::stdout().flush().unwrap_or(());
            }
        }
    }

    println!("\rOptimizer completed: {} frames processed", total_frames);
}

/// Apply advanced optimization heuristics to determine target nits.
///
/// This function implements the core tone mapping logic using multiple
/// heuristics to determine the optimal target nits for a frame:
///
/// 1. Hard cap for extreme peaks (profile-dependent) to prevent flicker
/// 2. Scene-aware processing based on rolling average APL:
///    - Dark scenes: More aggressive, preserve shadow detail
///    - Medium scenes: Balanced approach
///    - Bright scenes: Conservative to prevent blown highlights
/// 3. Highlight knee respect to preserve detail in bright areas (with profile-specific multipliers)
///
/// # Arguments
/// * `peak_nits` - Peak brightness of the current frame
/// * `rolling_apl_nits` - Rolling average picture level in nits
/// * `highlight_knee_nits` - Smoothed 99th percentile brightness level
/// * `scene_avg_apl_nits` - Average APL for the current scene
/// * `profile` - Optimizer profile configuration
///
/// # Returns
/// Target nits value for tone mapping (as u16)
fn apply_advanced_heuristics(
    peak_nits: u32,
    rolling_apl_nits: f64,
    highlight_knee_nits: f64,
    scene_avg_apl_nits: f64,
    profile: &OptimizerProfile,
) -> u16 {
    // Heuristic 1: Hard cap for extreme peaks (prevents flicker and blown-out highlights)
    if peak_nits > profile.extreme_peak_threshold {
        return (highlight_knee_nits.min(profile.extreme_peak_threshold as f64)) as u16;
    }

    // Heuristic 2: Use rolling average to smooth transitions and prevent temporal artifacts
    // Blend rolling with scene average to stabilize classification
    let apl_ref = 0.7 * rolling_apl_nits + 0.3 * scene_avg_apl_nits;
    if apl_ref < 50.0 {
        // Dark scene - be more aggressive, allow brighter targets to preserve shadow detail
        // But still respect the highlight knee to prevent blown highlights
        let (min_clamp, max_clamp) = profile.dark_scene_clamp;
        let target = peak_nits.clamp(min_clamp, max_clamp);
        (target as f64).min(highlight_knee_nits * profile.dark_knee_multiplier) as u16
    } else if apl_ref < 150.0 {
        // Medium brightness scene - balanced approach
        let (min_clamp, max_clamp) = profile.medium_scene_clamp;
        let target = peak_nits.clamp(min_clamp, max_clamp);
        (target as f64).min(highlight_knee_nits * profile.medium_knee_multiplier) as u16
    } else {
        // Bright scene - be more conservative to prevent blown-out look
        let (min_clamp, max_clamp) = profile.bright_scene_clamp;
        let target = peak_nits.clamp(min_clamp, max_clamp);
        (target as f64).min(highlight_knee_nits * profile.bright_knee_multiplier) as u16
    }
}

/// Limit frame-to-frame change of target_nits to reduce flicker.
fn apply_delta_limit(prev: Option<u16>, target: u16, max_delta: u16) -> u16 {
    if let Some(p) = prev {
        use std::cmp::Ordering;
        match target.cmp(&p) {
            Ordering::Greater => p.saturating_add(max_delta).min(target),
            Ordering::Less => p.saturating_sub(max_delta).max(target),
            Ordering::Equal => target,
        }
    } else {
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_delta_limit() {
        // No previous: pass-through
        assert_eq!(apply_delta_limit(None, 800, 200), 800);
        // Limit positive jump
        assert_eq!(apply_delta_limit(Some(600), 1000, 200), 800);
        // Limit negative jump
        assert_eq!(apply_delta_limit(Some(900), 400, 200), 700);
        // Within delta: unchanged
        assert_eq!(apply_delta_limit(Some(700), 820, 200), 820);
    }

    #[test]
    fn test_optimizer_profile_from_name() {
        // Test profile parsing
        assert!(OptimizerProfile::from_name("conservative").is_ok());
        assert!(OptimizerProfile::from_name("balanced").is_ok());
        assert!(OptimizerProfile::from_name("aggressive").is_ok());
        assert!(OptimizerProfile::from_name("BALANCED").is_ok()); // Case insensitive
        assert!(OptimizerProfile::from_name("invalid").is_err());

        // Test profile properties
        let conservative = OptimizerProfile::from_name("conservative").unwrap();
        let aggressive = OptimizerProfile::from_name("aggressive").unwrap();
        assert!(
            conservative.max_delta_per_frame < aggressive.max_delta_per_frame,
            "Conservative should have lower delta limit"
        );
        assert!(
            conservative.knee_smoothing_window > aggressive.knee_smoothing_window,
            "Conservative should smooth more"
        );
    }
}
