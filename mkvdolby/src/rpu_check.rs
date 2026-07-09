use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use colored::Colorize;
use dolby_vision::rpu::extension_metadata::blocks::ExtMetadataBlock;
use dolby_vision::rpu::rpu_data_nlq::DoviELType;
use dolby_vision::rpu::utils::parse_rpu_file;

use crate::external::{self, run_command_live, run_command_with_spinner};

const CEILING_EPSILON_PQ: u16 = 6;
const CLIPPED_FRAME_RATIO: f64 = 0.01;
const STATIC_VARIANCE_PQ: f64 = 1.0;
const DEGENERATE_EPSILON_PQ: u16 = 4;
const DEGENERATE_SCENE_RATIO: f64 = 0.90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpuFormatKind {
    Profile7Mel,
    Profile7Fel,
    Profile8,
    OtherDolbyVision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level5Offsets {
    pub left: u16,
    pub right: u16,
    pub top: u16,
    pub bottom: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L1Stats {
    pub min_pq: u16,
    pub avg_pq: u16,
    pub max_pq: u16,
}

#[derive(Debug, Clone)]
pub struct RpuReport {
    pub frame_count: usize,
    pub scene_count: usize,
    pub mastering_peak_pq: Option<u16>,
    pub clipped_frame_count: usize,
    pub clipped_frame_ratio: f64,
    pub clipped_scene_count: usize,
    pub clipped_scene_ratio: f64,
    pub max_pq_variance: f64,
    pub degenerate_frame_count: usize,
    pub degenerate_frame_ratio: f64,
    pub degenerate_scene_count: usize,
    pub degenerate_scene_ratio: f64,
    pub suspicious: bool,
    pub reasons: Vec<String>,
}

pub fn inspect_file(input: &str) -> Result<()> {
    let temp_dir = make_temp_dir("mkvdolby_inspect")?;
    let rpu_path = temp_dir.join("RPU.bin");
    let result = (|| {
        extract_rpu(input, &rpu_path, None)?;
        let report = analyze_rpu_file(&rpu_path)?;
        print_report(&report);
        Ok(())
    })();
    let _ = fs::remove_dir_all(&temp_dir);
    result
}

pub fn extract_rpu(input: &str, output: &Path, limit: Option<u32>) -> Result<()> {
    let dovi_tool_path =
        external::find_tool("dovi_tool").unwrap_or_else(|| PathBuf::from("dovi_tool"));
    let dovi_abs = fs::canonicalize(&dovi_tool_path).unwrap_or(dovi_tool_path);
    let mut cmd = Command::new(dovi_abs);
    cmd.arg("extract-rpu").arg(input).arg("-o").arg(output);
    if let Some(limit) = limit {
        cmd.arg("-l").arg(limit.to_string());
    }

    if run_command_with_spinner(
        &mut cmd,
        &output.with_extension("extract.log"),
        "Extracting RPU",
    )? && output.exists()
        && fs::metadata(output)?.len() > 0
    {
        Ok(())
    } else {
        anyhow::bail!("Failed to extract Dolby Vision RPU")
    }
}

pub fn try_extract_rpu_quiet(input: &str, output: &Path, limit: Option<u32>) -> bool {
    let _ = fs::remove_file(output);
    let Some(dovi_tool_path) = external::find_tool("dovi_tool") else {
        return false;
    };
    let dovi_abs = fs::canonicalize(&dovi_tool_path).unwrap_or(dovi_tool_path);
    let mut cmd = Command::new(dovi_abs);
    cmd.arg("extract-rpu")
        .arg(input)
        .arg("-o")
        .arg(output)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(limit) = limit {
        cmd.arg("-l").arg(limit.to_string());
    }

    cmd.status().map(|status| status.success()).unwrap_or(false)
        && output.exists()
        && fs::metadata(output)
            .map(|meta| meta.len() > 0)
            .unwrap_or(false)
}

pub fn extract_rpu_sample(input: &str, temp_dir: &Path) -> Result<PathBuf> {
    let sample_hevc = temp_dir.join("dv_probe.hevc");
    let sample_rpu = temp_dir.join("dv_probe_RPU.bin");

    let mut ffmpeg_cmd = Command::new("ffmpeg");
    ffmpeg_cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-ss",
        "60",
        "-t",
        "20",
        "-i",
        input,
        "-map",
        "0:v:0",
        "-c:v",
        "copy",
        "-bsf:v",
        "hevc_mp4toannexb",
        "-f",
        "hevc",
        "-y",
        sample_hevc.to_str().unwrap(),
    ]);

    if !run_command_live(&mut ffmpeg_cmd, &temp_dir.join("ffmpeg_dv_probe.log"))?
        || !sample_hevc.exists()
        || fs::metadata(&sample_hevc)?.len() == 0
    {
        extract_rpu(input, &sample_rpu, Some(240))?;
        return Ok(sample_rpu);
    }

    extract_rpu(sample_hevc.to_str().unwrap(), &sample_rpu, Some(240))?;
    Ok(sample_rpu)
}

/// RPU data gathered from several short windows spread across the file.
#[derive(Debug, Clone)]
pub struct SampledRpu {
    pub l1_frames: Vec<L1Stats>,
    pub mastering_peak_pq: Option<u16>,
    pub level5: Option<Level5Offsets>,
    pub windows_sampled: usize,
}

/// Sample short windows spread across the file so the auto-inspect verdict is
/// not dominated by dark openings (studio logos, title cards).
pub fn sample_rpu_windows(input: &str, temp_dir: &Path) -> Result<SampledRpu> {
    let offsets = window_offsets(probe_duration_secs(input));
    let mut l1_frames = Vec::new();
    let mut mastering_peak_pq = None;
    let mut level5 = None;
    let mut windows_sampled = 0usize;

    for (idx, offset) in offsets.iter().enumerate() {
        let Some(rpu_path) = extract_rpu_window(input, temp_dir, *offset, idx) else {
            continue;
        };
        let Ok(rpus) = parse_rpu_file(&rpu_path) else {
            continue;
        };
        l1_frames.extend(rpus.iter().filter_map(l1_from_rpu));
        if mastering_peak_pq.is_none() {
            mastering_peak_pq = rpus.iter().find_map(mastering_peak_from_rpu);
        }
        if level5.is_none() {
            level5 = rpus.iter().find_map(level5_from_rpu);
        }
        windows_sampled += 1;
    }

    if windows_sampled == 0 || l1_frames.is_empty() {
        anyhow::bail!("No Dolby Vision RPU data found in sampled windows");
    }

    Ok(SampledRpu {
        l1_frames,
        mastering_peak_pq,
        level5,
        windows_sampled,
    })
}

fn extract_rpu_window(
    input: &str,
    temp_dir: &Path,
    offset_secs: u32,
    idx: usize,
) -> Option<PathBuf> {
    let sample_hevc = temp_dir.join(format!("dv_window_{idx}.hevc"));
    let sample_rpu = temp_dir.join(format!("dv_window_{idx}_RPU.bin"));

    let mut ffmpeg_cmd = Command::new("ffmpeg");
    ffmpeg_cmd
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-ss")
        .arg(offset_secs.to_string())
        .arg("-t")
        .arg("20")
        .arg("-i")
        .arg(input)
        .arg("-map")
        .arg("0:v:0")
        .arg("-c:v")
        .arg("copy")
        .arg("-bsf:v")
        .arg("hevc_mp4toannexb")
        .arg("-f")
        .arg("hevc")
        .arg("-y")
        .arg(&sample_hevc)
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let ffmpeg_ok = ffmpeg_cmd.status().map(|s| s.success()).unwrap_or(false)
        && fs::metadata(&sample_hevc).map(|m| m.len()).unwrap_or(0) > 0;
    if !ffmpeg_ok {
        let _ = fs::remove_file(&sample_hevc);
        return None;
    }

    let ok = try_extract_rpu_quiet(sample_hevc.to_str()?, &sample_rpu, None);
    let _ = fs::remove_file(&sample_hevc);
    ok.then_some(sample_rpu)
}

fn probe_duration_secs(input: &str) -> Option<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
            input,
        ])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Window start offsets: spread across the runtime when known, otherwise fixed
/// offsets that skip the opening. Short files fall back to a single window at 0.
fn window_offsets(duration_secs: Option<f64>) -> Vec<u32> {
    match duration_secs {
        Some(d) if d >= 240.0 => vec![(d * 0.15) as u32, (d * 0.45) as u32, (d * 0.75) as u32],
        Some(_) => vec![0],
        None => vec![60, 600, 1800],
    }
}

pub fn classify_rpu_format(rpu_path: &Path) -> Result<RpuFormatKind> {
    let rpus = parse_rpu_file(rpu_path).context("Failed to parse Dolby Vision RPU")?;
    let Some(rpu) = rpus.first() else {
        anyhow::bail!("No RPU frames found");
    };

    Ok(match rpu.dovi_profile {
        7 => match rpu.el_type.as_ref() {
            Some(DoviELType::MEL) => RpuFormatKind::Profile7Mel,
            Some(DoviELType::FEL) => RpuFormatKind::Profile7Fel,
            None => RpuFormatKind::Profile7Fel,
        },
        8 => RpuFormatKind::Profile8,
        _ => RpuFormatKind::OtherDolbyVision,
    })
}

pub fn analyze_rpu_file(rpu_path: &Path) -> Result<RpuReport> {
    let rpus = parse_rpu_file(rpu_path).context("Failed to parse Dolby Vision RPU")?;
    analyze_rpus(
        rpus.iter().filter_map(l1_from_rpu).collect(),
        rpus.iter().find_map(mastering_peak_from_rpu),
    )
}

pub fn print_report(report: &RpuReport) {
    println!("{}", "Dolby Vision RPU inspection".cyan().bold());
    println!("Frames: {}", report.frame_count);
    println!("Scenes: {}", report.scene_count);
    if let Some(mastering_peak_pq) = report.mastering_peak_pq {
        println!("L6 mastering peak PQ: {}", mastering_peak_pq);
    }
    println!(
        "Clipped-at-ceiling frames: {} ({:.1}%)",
        report.clipped_frame_count,
        report.clipped_frame_ratio * 100.0
    );
    println!(
        "Clipped-at-ceiling scenes: {} ({:.1}%)",
        report.clipped_scene_count,
        report.clipped_scene_ratio * 100.0
    );
    println!("L1 max variance: {:.2}", report.max_pq_variance);
    println!(
        "Degenerate avg≈max frames: {} ({:.1}%)",
        report.degenerate_frame_count,
        report.degenerate_frame_ratio * 100.0
    );
    println!(
        "Degenerate avg≈max scenes: {} ({:.1}%)",
        report.degenerate_scene_count,
        report.degenerate_scene_ratio * 100.0
    );

    if report.suspicious {
        println!(
            "{}",
            format!(
                "Verdict: suspicious DV metadata ({})",
                report.reasons.join(", ")
            )
            .yellow()
            .bold()
        );
    } else {
        println!(
            "{}",
            "Verdict: no obvious RPU mismatch pattern found".green()
        );
    }
}

pub fn warning_summary(report: &RpuReport) -> Option<String> {
    if report.suspicious {
        Some(report.reasons.join(", "))
    } else {
        None
    }
}

pub fn analyze_rpus(l1_frames: Vec<L1Stats>, mastering_peak_pq: Option<u16>) -> Result<RpuReport> {
    if l1_frames.is_empty() {
        anyhow::bail!("No L1 metadata found in RPU");
    }

    let scenes = collapse_scenes(&l1_frames);
    let frame_count = l1_frames.len();
    let scene_count = scenes.len();

    let clipped_frame_count = mastering_peak_pq
        .map(|peak| {
            l1_frames
                .iter()
                .filter(|frame| frame.max_pq.abs_diff(peak) <= CEILING_EPSILON_PQ)
                .count()
        })
        .unwrap_or(0);
    let clipped_frame_ratio = clipped_frame_count as f64 / frame_count as f64;

    let clipped_scene_count = mastering_peak_pq
        .map(|peak| {
            scenes
                .iter()
                .filter(|scene| scene.max_pq.abs_diff(peak) <= CEILING_EPSILON_PQ)
                .count()
        })
        .unwrap_or(0);
    let clipped_scene_ratio = clipped_scene_count as f64 / scene_count as f64;

    let mean = scenes.iter().map(|scene| scene.max_pq as f64).sum::<f64>() / scene_count as f64;
    let max_pq_variance = scenes
        .iter()
        .map(|scene| {
            let delta = scene.max_pq as f64 - mean;
            delta * delta
        })
        .sum::<f64>()
        / scene_count as f64;

    let degenerate_frame_count = l1_frames
        .iter()
        .filter(|frame| frame.avg_pq.abs_diff(frame.max_pq) <= DEGENERATE_EPSILON_PQ)
        .count();
    let degenerate_frame_ratio = degenerate_frame_count as f64 / frame_count as f64;

    let degenerate_scene_count = scenes
        .iter()
        .filter(|scene| scene.avg_pq.abs_diff(scene.max_pq) <= DEGENERATE_EPSILON_PQ)
        .count();
    let degenerate_scene_ratio = degenerate_scene_count as f64 / scene_count as f64;

    let mut reasons = Vec::new();
    if mastering_peak_pq.is_some() && clipped_frame_ratio >= CLIPPED_FRAME_RATIO {
        reasons.push(format!(
            "clipped-at-ceiling {:.1}% of frames",
            clipped_frame_ratio * 100.0
        ));
    }
    if scene_count >= 3 && max_pq_variance <= STATIC_VARIANCE_PQ {
        reasons.push("static L1 max".to_string());
    }
    if degenerate_scene_ratio >= DEGENERATE_SCENE_RATIO {
        reasons.push(format!(
            "avg≈max in {:.1}% of scenes",
            degenerate_scene_ratio * 100.0
        ));
    }

    Ok(RpuReport {
        frame_count,
        scene_count,
        mastering_peak_pq,
        clipped_frame_count,
        clipped_frame_ratio,
        clipped_scene_count,
        clipped_scene_ratio,
        max_pq_variance,
        degenerate_frame_count,
        degenerate_frame_ratio,
        degenerate_scene_count,
        degenerate_scene_ratio,
        suspicious: !reasons.is_empty(),
        reasons,
    })
}

fn l1_from_rpu(rpu: &dolby_vision::rpu::dovi_rpu::DoviRpu) -> Option<L1Stats> {
    match rpu.vdr_dm_data.as_ref()?.get_block(1)? {
        ExtMetadataBlock::Level1(level1) => Some(L1Stats {
            min_pq: level1.min_pq,
            avg_pq: level1.avg_pq,
            max_pq: level1.max_pq,
        }),
        _ => None,
    }
}

fn level5_from_rpu(rpu: &dolby_vision::rpu::dovi_rpu::DoviRpu) -> Option<Level5Offsets> {
    match rpu.vdr_dm_data.as_ref()?.get_block(5)? {
        ExtMetadataBlock::Level5(level5) => Some(Level5Offsets {
            left: level5.active_area_left_offset,
            right: level5.active_area_right_offset,
            top: level5.active_area_top_offset,
            bottom: level5.active_area_bottom_offset,
        }),
        _ => None,
    }
}

fn mastering_peak_from_rpu(rpu: &dolby_vision::rpu::dovi_rpu::DoviRpu) -> Option<u16> {
    match rpu.vdr_dm_data.as_ref()?.get_block(6)? {
        ExtMetadataBlock::Level6(level6) => Some(nits_to_pq_12_bit(
            level6.max_display_mastering_luminance as f64,
        )),
        _ => None,
    }
}

fn collapse_scenes(frames: &[L1Stats]) -> Vec<L1Stats> {
    let mut scenes = Vec::new();
    for frame in frames {
        if scenes.last() != Some(frame) {
            scenes.push(*frame);
        }
    }
    scenes
}

fn nits_to_pq_12_bit(nits: f64) -> u16 {
    let normalized = (nits / 10_000.0).clamp(0.0, 1.0);
    let m1 = 2610.0 / 16384.0;
    let m2 = 2523.0 / 32.0;
    let c1 = 3424.0 / 4096.0;
    let c2 = 2413.0 / 128.0;
    let c3 = 2392.0 / 128.0;
    let y = normalized.powf(m1);
    (((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2) * 4095.0).round() as u16
}

fn make_temp_dir(prefix: &str) -> Result<PathBuf> {
    let mut path = std::env::temp_dir();
    path.push(format!("{}_{}", prefix, std::process::id()));
    fs::create_dir_all(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_offsets_spread_across_known_duration() {
        assert_eq!(window_offsets(Some(4000.0)), vec![600, 1800, 3000]);
    }

    #[test]
    fn window_offsets_short_file_single_window() {
        assert_eq!(window_offsets(Some(120.0)), vec![0]);
    }

    #[test]
    fn window_offsets_unknown_duration_fixed_fallback() {
        assert_eq!(window_offsets(None), vec![60, 600, 1800]);
    }

    #[test]
    fn detects_clipped_ceiling() {
        let frames = vec![
            L1Stats {
                min_pq: 0,
                avg_pq: 1200,
                max_pq: 3079,
            },
            L1Stats {
                min_pq: 0,
                avg_pq: 1300,
                max_pq: 3078,
            },
            L1Stats {
                min_pq: 0,
                avg_pq: 900,
                max_pq: 2500,
            },
        ];
        let report = analyze_rpus(frames, Some(3079)).unwrap();
        assert!(report.suspicious);
        assert_eq!(report.clipped_scene_count, 2);
    }

    #[test]
    fn detects_static_l1() {
        let frames = vec![
            L1Stats {
                min_pq: 0,
                avg_pq: 1200,
                max_pq: 2500,
            },
            L1Stats {
                min_pq: 1,
                avg_pq: 1201,
                max_pq: 2500,
            },
            L1Stats {
                min_pq: 2,
                avg_pq: 1202,
                max_pq: 2500,
            },
        ];
        let report = analyze_rpus(frames, None).unwrap();
        assert!(report.suspicious);
        assert!(report.reasons.iter().any(|r| r.contains("static")));
    }

    #[test]
    fn healthy_l1_is_not_suspicious() {
        let frames = vec![
            L1Stats {
                min_pq: 0,
                avg_pq: 900,
                max_pq: 2200,
            },
            L1Stats {
                min_pq: 0,
                avg_pq: 1300,
                max_pq: 2800,
            },
            L1Stats {
                min_pq: 0,
                avg_pq: 1500,
                max_pq: 3300,
            },
        ];
        let report = analyze_rpus(frames, Some(3696)).unwrap();
        assert!(!report.suspicious);
    }

    #[test]
    fn detects_low_ratio_frame_weighted_ceiling_clip() {
        let mut frames = vec![
            L1Stats {
                min_pq: 0,
                avg_pq: 1000,
                max_pq: 2500,
            };
            807
        ];
        frames.extend(vec![
            L1Stats {
                min_pq: 0,
                avg_pq: 1200,
                max_pq: 3079,
            };
            12
        ]);

        let report = analyze_rpus(frames, Some(3079)).unwrap();
        assert!(report.suspicious);
        assert!(report.clipped_frame_ratio > 0.01);
    }
}
