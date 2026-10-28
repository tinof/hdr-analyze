use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ffmpeg_next::{format, util::color};

use crate::analysis::frame::FrameAnalysisOptions;
use crate::analysis::hlg::PQ_MAPPING;
use crate::cli::{Cli, PeakDomain, PeakEstimator};
use crate::ffmpeg_io::{probe_crop, TransferFunction, VideoInfo};
use crate::l1_sidecar::{write_l1_sidecar, AnalysisMetadata, SidecarProvenance, SourceMetadata};
use crate::optimizer::{run_optimizer_pass, OptimizerProfile};
use crate::writer::write_measurement_file;

mod analyze;
mod crop_space;
mod scenes;

use self::analyze::*;
use self::crop_space::*;
use self::scenes::*;

fn peak_estimator_name(estimator: PeakEstimator) -> &'static str {
    match estimator {
        PeakEstimator::Max => "max",
        PeakEstimator::Percentile => "percentile",
        PeakEstimator::Robust => "robust",
    }
}

fn effective_downscale(requested: u32) -> u32 {
    match requested {
        1 | 2 | 4 => requested,
        other => {
            eprintln!(
                "Unsupported --downscale value {}. Falling back to 1 (no downscale). Allowed: 1,2,4.",
                other
            );
            1
        }
    }
}

/// This function orchestrates the complete HDR analysis pipeline using native FFmpeg
#[expect(clippy::too_many_lines, reason = "E13.9: split by phase")]
pub fn run(
    cli: &Cli,
    video_info: &VideoInfo,
    mut input_context: format::context::Input,
) -> Result<()> {
    let peak_domain = match video_info.transfer_function {
        TransferFunction::Pq | TransferFunction::Hlg | TransferFunction::Unknown => {
            cli.peak_domain.unwrap_or(PeakDomain::MaxRgb)
        }
        TransferFunction::Unsupported(name) => {
            anyhow::bail!(
                "transfer characteristic '{name}' is not an HDR transfer; this analyzer measures PQ (SMPTE 2084) and HLG signals only"
            );
        }
    };

    let (chroma_siting, chroma_modelled) =
        crate::ffmpeg_io::chroma_siting(video_info.chroma_location);
    match video_info.transfer_function {
        TransferFunction::Hlg => {
            println!(
                "Detected HLG transfer function. Measuring through the Dolby Vision Profile 8.4 decode (luma curve, chroma MMR, RPU matrix) of the '{}' composer.",
                cli.hlg_composer.cli_name()
            );
            if !chroma_modelled {
                eprintln!(
                    "Warning: chroma location {:?} is not modelled; the composed chroma is upsampled as for chroma location left.",
                    video_info.chroma_location
                );
            }
            println!(
                "HLG chroma location: {} (signalled {:?})",
                chroma_siting.name(),
                video_info.chroma_location
            );
        }
        TransferFunction::Unknown => {
            println!(
                "Transfer function unspecified; defaulting to PQ analysis path. Tag HLG sources as arib-std-b67 to measure them through the Dolby Vision Profile 8.4 curve."
            );
        }
        TransferFunction::Pq | TransferFunction::Unsupported(_) => {}
    }

    if video_info.color_range == color::Range::JPEG {
        eprintln!(
            "Warning: stream is tagged full range; samples are interpreted as limited range (64..940), so measured levels will be biased."
        );
    }
    // BT.2020 constant luminance is not BT.2020 NCL either: the PQ max-RGB and the DV 8.4
    // decode both assume NCL, so it warns too.
    if !matches!(
        video_info.color_space,
        color::Space::Unspecified | color::Space::BT2020NCL
    ) {
        eprintln!(
            "Warning: stream matrix is tagged {:?}; max-RGB peaks use BT.2020 non-constant-luminance coefficients.",
            video_info.color_space
        );
    }

    println!(
        "Direct peak domain: {}",
        match peak_domain {
            PeakDomain::MaxRgb => "max-rgb",
            PeakDomain::Luma => "luma",
        }
    );
    println!(
        "Peak estimator: {}{}",
        peak_estimator_name(cli.peak_estimator),
        if cli.peak_estimator == PeakEstimator::Percentile {
            format!(" (P{})", cli.peak_percentile)
        } else {
            String::new()
        }
    );

    if cli.scene_metric.to_lowercase() == "hybrid" {
        println!("Scene metric: hybrid (prototype, using histogram-only for now)");
    }

    let downscale = effective_downscale(cli.downscale);
    if cli.peak_estimator == PeakEstimator::Robust && downscale > 1 {
        eprintln!(
            "Warning: --peak-estimator robust is specified for --downscale 1. With --downscale {downscale} \
             the grain measurement and the pixel counts of its rule refer to analysis samples, \
             not source pixels."
        );
    }
    let input_path = cli
        .input_positional
        .as_deref()
        .or(cli.input_flag.as_deref())
        .context("input path is required")?;
    let initial_crop = if cli.no_crop || cli.crop_probes == 0 {
        None
    } else {
        println!(
            "Probing active video area at {} positions...",
            cli.crop_probes
        );
        match probe_crop(input_path, cli.crop_probes, downscale) {
            Ok(vote) => {
                println!(
                    "Crop probe accepted {}/{} frames across {} crop mode(s); modal support {}/{}.",
                    vote.candidate_count,
                    cli.crop_probes,
                    vote.cluster_count,
                    vote.modal_count,
                    vote.candidate_count
                );
                println!(
                    "Committed active video area: {}x{} at offset ({}, {})",
                    vote.rect.width, vote.rect.height, vote.rect.x, vote.rect.y
                );
                if vote.variable_ar {
                    println!(
                        "Variable active area detected during probing; using the union of observed modes so full-frame picture is preserved."
                    );
                }
                Some(vote.rect)
            }
            Err(error) => {
                eprintln!(
                    "Crop probe unavailable ({error:#}); falling back to in-stream detection."
                );
                None
            }
        }
    };

    let (
        mut scenes,
        mut frames,
        l1_measurements,
        frame_peak_stats,
        crop,
        gpu_active,
        stream_frames,
    ) = run_native_analysis_pipeline(
        cli,
        video_info,
        &mut input_context,
        downscale,
        initial_crop,
        &FrameAnalysisOptions {
            denoise_mode: &cli.pre_denoise,
            transfer_function: video_info.transfer_function,
            hlg_composer: cli.hlg_composer,
            chroma_siting,
            peak_domain,
            min_percentile: cli.min_percentile,
            peak_estimator: cli.peak_estimator,
            peak_percentile: cli.peak_percentile,
            dump_max_rgb: max_rgb_dump_dir().is_some(),
        },
    )?;

    if let Some(path) = &cli.dump_frame_stats {
        let (_, frame_indices, series) = detect_scene_cuts(
            &frames,
            cli.sample_rate,
            cli.scene_threshold,
            cli.min_scene_length,
        );
        write_frame_stats_csv(path, &frame_peak_stats, &frame_indices, &series, &scenes)?;
        println!("Wrote frame peak statistics: {}", path.display());
    }

    fix_scene_end_frames(&mut scenes, frames.len());

    // Smooth the histograms and the madVR frame averages with scene-aware resets. The L1
    // measurements stay unfiltered.
    if cli.hist_bin_ema_beta > 0.0 || cli.hist_temporal_median > 0 {
        apply_histogram_smoothing_pass(&scenes, &mut frames, cli, peak_domain)?;
    }

    precompute_scene_stats(&mut scenes, &frames);

    let optimizer_enabled = !cli.disable_optimizer;
    let mut selected_profile: Option<OptimizerProfile> = None;
    if optimizer_enabled {
        println!("Running intelligent optimizer pass...");
        let optimizer_profile = OptimizerProfile::from_name(&cli.optimizer_profile)?;
        run_optimizer_pass(&scenes, &mut frames, &optimizer_profile);
        selected_profile = Some(optimizer_profile);
    }

    // Optional post-optimization target_nits smoothing
    if optimizer_enabled && cli.target_smoother.to_lowercase() == "ema" {
        let alpha = cli.smoother_alpha.clamp(0.0, 1.0);
        let bidirectional = cli.smoother_bidirectional;
        let max_delta = selected_profile
            .map(|p| p.max_delta_per_frame)
            .unwrap_or(200);
        println!(
            "Applying target_nits EMA smoother (alpha={:.3}, bidirectional={})...",
            alpha, bidirectional
        );
        crate::optimizer::apply_target_smoother(
            &scenes,
            &mut frames,
            alpha,
            bidirectional,
            max_delta,
        );
        println!("Target_nits smoothing complete.");
    }

    let output_path = match &cli.output {
        Some(path) => path.clone(),
        None => {
            let input_path_obj = Path::new(
                cli.input_positional
                    .as_ref()
                    .unwrap_or(cli.input_flag.as_ref().unwrap()),
            );
            let stem = input_path_obj
                .file_stem()
                .context("Input file has no filename")?
                .to_str()
                .context("Invalid UTF-8 in filename")?;
            format!("{}_measurements.bin", stem)
        }
    };

    println!("Writing measurement file: {}", output_path);
    write_measurement_file(
        &output_path,
        &scenes,
        &frames,
        optimizer_enabled,
        cli.madvr_version as u32,
        cli.target_peak_nits,
        cli.header_peak_source.as_deref(),
    )?;
    let sidecar_path = write_l1_sidecar(
        Path::new(&output_path),
        &scenes,
        &frames,
        &l1_measurements,
        cli.min_percentile,
        &cli.pre_denoise,
        peak_domain,
        cli.peak_estimator,
        cli.peak_percentile,
        crop,
        &SidecarProvenance {
            source: SourceMetadata {
                file_name: Path::new(input_path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                size_bytes: std::fs::metadata(input_path)
                    .map(|meta| meta.len())
                    .unwrap_or(0),
                width: video_info.width,
                height: video_info.height,
                transfer_function: video_info.transfer_function.to_string(),
                stream_frames: stream_frames.stream,
                leading_skipped_frames: stream_frames.leading_skipped,
            },
            analysis: AnalysisMetadata {
                downscale,
                sample_rate: cli.sample_rate.max(1),
                gpu: gpu_active,
                no_crop: cli.no_crop,
                luminance_mapping: match video_info.transfer_function {
                    TransferFunction::Hlg => cli.hlg_composer.luminance_mapping(),
                    _ => PQ_MAPPING,
                }
                .to_owned(),
            },
        },
    )?;
    println!("Wrote L1 measurement sidecar: {}", sidecar_path.display());

    Ok(())
}

#[cfg(test)]
mod scene_cut_tests;

#[cfg(test)]
mod crop_space_tests;

/// `HDR_ANALYZER_DUMP_MAX_RGB=<dir>`: write every analyzed frame's per-pixel max-RGB values
/// there, for `scripts/validate_hlg_chroma_siting.sh` to compare the decode pixel by pixel.
fn max_rgb_dump_dir() -> Option<PathBuf> {
    std::env::var_os("HDR_ANALYZER_DUMP_MAX_RGB").map(PathBuf::from)
}

#[cfg(test)]
mod tests;
