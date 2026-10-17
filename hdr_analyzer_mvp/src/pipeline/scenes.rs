use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use madvr_parse::{MadVRFrame, MadVRScene};

use crate::analysis::frame::FramePeakStats;
use crate::analysis::histogram::{
    apply_histogram_ema, apply_histogram_temporal_median, select_peak_pq,
};
use crate::analysis::scene::{scene_series, select_scene_cuts, SceneSeries};
use crate::cli::{Cli, PeakDomain};

/// Final scene cuts and the series they were chosen from.
///
/// Runs after the frame loop, on the unsmoothed histograms of the analyzed frames (with
/// `--sample-rate` the skipped frames only hold copies), so a decision can look ahead.
pub(super) fn detect_scene_cuts(
    frames: &[MadVRFrame],
    sample_rate: u32,
    threshold: f64,
    min_scene_length: u32,
) -> (Vec<u32>, Vec<u32>, SceneSeries) {
    let step = sample_rate.max(1) as usize;
    let frame_indices: Vec<u32> = (0..frames.len()).step_by(step).map(|i| i as u32).collect();
    let histograms: Vec<&[f64]> = frame_indices
        .iter()
        .map(|&index| frames[index as usize].lum_histogram.as_slice())
        .collect();
    let series = scene_series(&histograms);
    let cuts = select_scene_cuts(&series, &frame_indices, threshold, min_scene_length);
    (cuts, frame_indices, series)
}

pub(super) fn write_frame_stats_csv(
    path: &Path,
    stats: &[FramePeakStats],
    frame_indices: &[u32],
    series: &SceneSeries,
    scenes: &[MadVRScene],
) -> Result<()> {
    // Frames skipped by --sample-rate have no scene values of their own.
    let mut scene_values = vec![(0.0, 0.0, 0.0); stats.len()];
    for (position, &frame) in frame_indices.iter().enumerate() {
        if let Some(slot) = scene_values.get_mut(frame as usize) {
            *slot = (
                series.diff[position],
                series.score[position],
                series.baseline[position],
            );
        }
    }
    let cut_frames: std::collections::HashSet<u32> =
        scenes.iter().map(|scene| scene.start).collect();
    let file = File::create(path)
        .with_context(|| format!("Failed to create frame-stats CSV {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    writeln!(
        writer,
        "frame,selected_pq,raw_max_pq,percentile_pq,robust_pq,sigma_pq,correction_pq,n_eff,scene_diff,scene_score,scene_baseline,scene_start,avg_max_rgb_pq"
    )?;

    for (frame_index, stat) in stats.iter().enumerate() {
        let (scene_diff, scene_score, scene_baseline) = scene_values[frame_index];
        writeln!(
            writer,
            "{frame_index},{:.12},{:.12},{:.12},{:.12},{:.12},{:.12},{},{:.6},{:.6},{:.6},{},{:.12}",
            stat.selected_peak_pq,
            stat.raw_max_pq,
            stat.percentile_pq,
            stat.robust_pq,
            stat.sigma_pq,
            stat.correction_pq,
            stat.n_eff,
            scene_diff,
            scene_score,
            scene_baseline,
            u8::from(cut_frames.contains(&(frame_index as u32))),
            stat.avg_max_rgb_pq
        )?;
    }
    writer
        .flush()
        .with_context(|| format!("Failed to write frame-stats CSV {}", path.display()))
}

pub(super) fn fix_scene_end_frames(scenes: &mut [MadVRScene], total_frames: usize) {
    if scenes.is_empty() || total_frames == 0 {
        return;
    }

    let last_frame_idx = (total_frames - 1) as u32;

    for scene in scenes.iter_mut() {
        if scene.end == u32::MAX {
            scene.end = last_frame_idx;
        }

        if scene.end >= total_frames as u32 {
            scene.end = last_frame_idx;
        }

        if scene.start >= total_frames as u32 {
            scene.start = last_frame_idx;
        }

        if scene.start > scene.end {
            scene.start = 0;
            scene.end = last_frame_idx;
        }
    }
}

pub(super) fn smooth_average(
    current: f64,
    ema_state: &mut Option<f64>,
    history: &mut VecDeque<f64>,
    ema_beta: f64,
    temporal_window: usize,
) -> f64 {
    let mut smoothed = if ema_beta > 0.0 {
        let value = ema_state.map_or(current, |previous| {
            ema_beta * current + (1.0 - ema_beta) * previous
        });
        *ema_state = Some(value);
        value
    } else {
        current
    };

    if temporal_window > 0 {
        history.push_back(smoothed);
        if history.len() > temporal_window {
            history.pop_front();
        }
        let mut values: Vec<f64> = history.iter().copied().collect();
        values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        smoothed = values[values.len() / 2];
    }

    smoothed
}

pub(super) fn apply_histogram_smoothing_pass(
    scenes: &[MadVRScene],
    frames: &mut [MadVRFrame],
    cli: &Cli,
    peak_domain: PeakDomain,
) -> Result<()> {
    println!(
        "Applying histogram smoothing (EMA beta={}, temporal median window={})...",
        cli.hist_bin_ema_beta, cli.hist_temporal_median
    );

    let ema_beta = cli.hist_bin_ema_beta;
    let temporal_window = cli.hist_temporal_median;

    // A max-RGB domain defaults to its direct measurement. Histogram sources
    // remain available as explicit Y-based noise-robustness choices. The legacy
    // profile-dependent default applies only to the luma domain.
    let peak_source = cli.peak_source.as_deref().unwrap_or_else(|| {
        if peak_domain == PeakDomain::MaxRgb
            || cli.optimizer_profile.eq_ignore_ascii_case("conservative")
        {
            "max"
        } else {
            "histogram99"
        }
    });

    // Process each scene independently to reset all smoothing state at boundaries.
    for scene in scenes {
        let start_idx = scene.start as usize;
        let end_idx = ((scene.end + 1) as usize).min(frames.len());

        if start_idx >= frames.len() || start_idx >= end_idx {
            continue;
        }

        let mut ema_state = vec![0.0; 256];
        let mut temporal_history: VecDeque<Vec<f64>> = VecDeque::with_capacity(temporal_window);
        let mut luma_avg_ema_state: Option<f64> = None;
        let mut luma_avg_history: VecDeque<f64> = VecDeque::with_capacity(temporal_window);

        for frame in frames[start_idx..end_idx].iter_mut() {
            let direct_max_pq = frame.peak_pq_2020;
            let direct_luma_avg_pq = frame.avg_pq;

            if ema_beta > 0.0 {
                apply_histogram_ema(&mut frame.lum_histogram, &mut ema_state, ema_beta);
            }

            if temporal_window > 0 && !temporal_history.is_empty() {
                apply_histogram_temporal_median(
                    &mut frame.lum_histogram,
                    &temporal_history.iter().cloned().collect::<Vec<_>>(),
                );
            }

            if temporal_window > 0 {
                temporal_history.push_back(frame.lum_histogram.clone());
                if temporal_history.len() >= temporal_window {
                    temporal_history.pop_front();
                }
            }

            frame.peak_pq_2020 = select_peak_pq(&frame.lum_histogram, direct_max_pq, peak_source);

            // Only the madVR frame average is smoothed (it feeds the .bin and the optimizer).
            // The L1 sidecar keeps the unfiltered per-frame means in `FrameL1Measurement`: a
            // forward-only EMA pulls a scene mean toward the scene's first frames. The first
            // frame of every scene initializes the EMA without zero-state bias.
            frame.avg_pq = smooth_average(
                direct_luma_avg_pq,
                &mut luma_avg_ema_state,
                &mut luma_avg_history,
                ema_beta,
                temporal_window,
            );
        }
    }

    println!(
        "Histogram and average smoothing completed. Peak source: {}",
        peak_source
    );
    Ok(())
}

pub(super) fn precompute_scene_stats(scenes: &mut [MadVRScene], frames: &[MadVRFrame]) {
    println!("Computing scene-based statistics...");

    for scene in scenes.iter_mut() {
        let start_idx = scene.start as usize;
        let end_idx = ((scene.end + 1) as usize).min(frames.len());

        if start_idx < frames.len() && start_idx < end_idx {
            let scene_frames = &frames[start_idx..end_idx];

            let total_avg_pq: f64 = scene_frames.iter().map(|f| f.avg_pq).sum();
            scene.avg_pq = total_avg_pq / scene_frames.len() as f64;

            let max_peak_pq = scene_frames
                .iter()
                .map(|f| f.peak_pq_2020)
                .fold(0.0f64, f64::max);
            scene.peak_nits = crate::analysis::histogram::pq_to_nits(max_peak_pq) as u32;
        }
    }
}
