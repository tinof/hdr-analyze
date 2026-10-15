use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use colored::Colorize;
use dolby_vision::rpu::dovi_rpu::DoviRpu;
use dolby_vision::rpu::extension_metadata::blocks::{
    ExtMetadataBlock, ExtMetadataBlockLevel2, ExtMetadataBlockLevel3, ExtMetadataBlockLevel8,
};
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
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
    let temp_dir = make_temp_dir("mkvdovi_inspect")?;
    let rpu_path = temp_dir.join("RPU.bin");
    let result = (|| {
        extract_rpu(input, &rpu_path, None)?;
        let format = classify_rpu_format(&rpu_path)?;
        let report = analyze_rpu_file(&rpu_path)?;
        println!("Format: {}", rpu_format_label(format));
        print_report(&report);
        Ok(())
    })();
    let _ = fs::remove_dir_all(&temp_dir);
    result
}

fn rpu_format_label(format: RpuFormatKind) -> &'static str {
    match format {
        RpuFormatKind::Profile7Mel => "Dolby Vision Profile 7 MEL",
        RpuFormatKind::Profile7Fel => "Dolby Vision Profile 7 FEL",
        RpuFormatKind::Profile8 => "Dolby Vision Profile 8",
        RpuFormatKind::OtherDolbyVision => "other Dolby Vision profile",
    }
}

pub fn extract_rpu(input: &str, output: &Path, limit: Option<u32>) -> Result<()> {
    let mut cmd = external::dovi_tool_command();
    cmd.arg("extract-rpu")
        .arg("-i")
        .arg(input)
        .arg("-o")
        .arg(output);
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
    if external::find_tool("dovi_tool").is_none() {
        return false;
    }
    let mut cmd = external::dovi_tool_command();
    cmd.arg("extract-rpu")
        .arg("-i")
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
    pub authored: AuthoredLevels,
}

/// Authored metadata seen in the sampled windows that a `--mdfix` rebuild does not keep as it is.
/// An absence means only "not seen in the sample".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthoredLevels {
    /// Windows whose RPUs were parsed.
    pub windows: usize,
    /// `target_max_pq` of every L2 block with a non-neutral trim.
    pub l2_trims: BTreeSet<u16>,
    /// A non-neutral L3 block.
    pub l3: bool,
    pub l4: bool,
    /// Distinct L5 active areas.
    pub l5_values: BTreeSet<Level5Offsets>,
    pub l6: bool,
    /// `target_display_index` of every L8 block with a non-neutral trim or vector field.
    pub l8_trims: BTreeSet<u8>,
    /// Source primary index of each L9 block.
    pub l9: BTreeSet<u8>,
    /// `target_display_index` -> `target_max_pq` of every L10 block (custom target displays).
    pub l10_targets: BTreeMap<u8, u16>,
    /// Distinct L11 (content type, whitepoint, reference mode flag).
    pub l11: BTreeSet<(u8, u8, bool)>,
    /// Distinct L254 (DM mode, DM version index).
    pub l254: BTreeSet<(u8, u8)>,
    pub l255: bool,
}

const NEUTRAL_TRIM: u16 = 2048;
const NEUTRAL_L8_VECTOR: u8 = 128;

fn l2_is_neutral(b: &ExtMetadataBlockLevel2) -> bool {
    [
        b.trim_slope,
        b.trim_offset,
        b.trim_power,
        b.trim_chroma_weight,
        b.trim_saturation_gain,
    ]
    .iter()
    .all(|&v| v == NEUTRAL_TRIM)
        && b.ms_weight == NEUTRAL_TRIM as i16
}

fn l3_is_neutral(b: &ExtMetadataBlockLevel3) -> bool {
    [b.min_pq_offset, b.max_pq_offset, b.avg_pq_offset]
        .iter()
        .all(|&v| v == NEUTRAL_TRIM)
}

/// `length` and `target_display_index` are not trims; fields a short block does not carry are
/// parsed as these neutral values.
fn l8_is_neutral(b: &ExtMetadataBlockLevel8) -> bool {
    [
        b.trim_slope,
        b.trim_offset,
        b.trim_power,
        b.trim_chroma_weight,
        b.trim_saturation_gain,
        b.ms_weight,
        b.target_mid_contrast,
        b.clip_trim,
    ]
    .iter()
    .all(|&v| v == NEUTRAL_TRIM)
        && [
            b.saturation_vector_field0,
            b.saturation_vector_field1,
            b.saturation_vector_field2,
            b.saturation_vector_field3,
            b.saturation_vector_field4,
            b.saturation_vector_field5,
            b.hue_vector_field0,
            b.hue_vector_field1,
            b.hue_vector_field2,
            b.hue_vector_field3,
            b.hue_vector_field4,
            b.hue_vector_field5,
        ]
        .iter()
        .all(|&v| v == NEUTRAL_L8_VECTOR)
}

/// Adds the authored levels of one window's RPUs to `acc`. Does not count the window.
pub fn record_authored_levels(acc: &mut AuthoredLevels, rpus: &[DoviRpu]) {
    for dm in rpus.iter().filter_map(|rpu| rpu.vdr_dm_data.as_ref()) {
        for level in [2, 3, 4, 5, 6, 8, 9, 10, 11, 254, 255] {
            for block in dm.level_blocks_iter(level) {
                match block {
                    ExtMetadataBlock::Level2(b) if !l2_is_neutral(b) => {
                        acc.l2_trims.insert(b.target_max_pq);
                    }
                    ExtMetadataBlock::Level3(b) if !l3_is_neutral(b) => acc.l3 = true,
                    ExtMetadataBlock::Level4(_) => acc.l4 = true,
                    ExtMetadataBlock::Level5(b) => {
                        acc.l5_values.insert(Level5Offsets {
                            left: b.active_area_left_offset,
                            right: b.active_area_right_offset,
                            top: b.active_area_top_offset,
                            bottom: b.active_area_bottom_offset,
                        });
                    }
                    ExtMetadataBlock::Level6(_) => acc.l6 = true,
                    ExtMetadataBlock::Level8(b) if !l8_is_neutral(b) => {
                        acc.l8_trims.insert(b.target_display_index);
                    }
                    ExtMetadataBlock::Level9(b) => {
                        acc.l9.insert(b.source_primary_index);
                    }
                    ExtMetadataBlock::Level10(b) => {
                        acc.l10_targets
                            .insert(b.target_display_index, b.target_max_pq);
                    }
                    ExtMetadataBlock::Level11(b) => {
                        acc.l11
                            .insert((b.content_type, b.whitepoint, b.reference_mode_flag));
                    }
                    ExtMetadataBlock::Level254(b) => {
                        acc.l254.insert((b.dm_mode, b.dm_version_index));
                    }
                    ExtMetadataBlock::Level255(_) => acc.l255 = true,
                    _ => {}
                }
            }
        }
    }
}

/// Sample short windows spread across the file so the auto-inspect verdict is
/// not dominated by dark openings (studio logos, title cards).
pub fn sample_rpu_windows(input: &str, temp_dir: &Path) -> Result<SampledRpu> {
    let offsets = window_offsets(probe_duration_secs(input));
    let mut l1_frames = Vec::new();
    let mut mastering_peak_pq = None;
    let mut level5 = None;
    let mut windows_sampled = 0usize;
    let mut authored = AuthoredLevels::default();

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
        record_authored_levels(&mut authored, &rpus);
        windows_sampled += 1;
    }
    authored.windows = windows_sampled;

    if windows_sampled == 0 || l1_frames.is_empty() {
        anyhow::bail!("No Dolby Vision RPU data found in sampled windows");
    }

    Ok(SampledRpu {
        l1_frames,
        mastering_peak_pq,
        level5,
        windows_sampled,
        authored,
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

fn l1_from_rpu(rpu: &DoviRpu) -> Option<L1Stats> {
    match rpu.vdr_dm_data.as_ref()?.get_block(1)? {
        ExtMetadataBlock::Level1(level1) => Some(L1Stats {
            min_pq: level1.min_pq,
            avg_pq: level1.avg_pq,
            max_pq: level1.max_pq,
        }),
        _ => None,
    }
}

fn level5_from_rpu(rpu: &DoviRpu) -> Option<Level5Offsets> {
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

fn mastering_peak_from_rpu(rpu: &DoviRpu) -> Option<u16> {
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
    use dolby_vision::rpu::extension_metadata::blocks::{
        ExtMetadataBlockLevel1, ExtMetadataBlockLevel10, ExtMetadataBlockLevel11,
        ExtMetadataBlockLevel4, ExtMetadataBlockLevel5,
    };
    use dolby_vision::rpu::generate::{GenerateConfig, VideoShot};
    use dolby_vision::rpu::vdr_dm_data::CmVersion as RpuCmVersion;

    /// RPUs as `dovi_tool generate` writes them, one shot of two frames per block list. Generated
    /// RPUs always carry L5 and L6; CM v4.0 adds a neutral L3, L9 0, L11 (1, 0, false) and L254.
    fn generated(cm_version: RpuCmVersion, shots: Vec<Vec<ExtMetadataBlock>>) -> Vec<DoviRpu> {
        let shots: Vec<VideoShot> = shots
            .into_iter()
            .enumerate()
            .map(|(i, mut blocks)| {
                blocks.push(ExtMetadataBlock::Level1(ExtMetadataBlockLevel1::new(
                    0, 2800, 1200,
                )));
                VideoShot {
                    start: i * 2,
                    duration: 2,
                    metadata_blocks: blocks,
                    ..Default::default()
                }
            })
            .collect();
        GenerateConfig {
            cm_version,
            length: shots.len() * 2,
            shots,
            ..Default::default()
        }
        .generate_rpu_list()
        .unwrap()
    }

    fn authored(rpus: &[DoviRpu]) -> AuthoredLevels {
        let mut acc = AuthoredLevels::default();
        record_authored_levels(&mut acc, rpus);
        acc
    }

    fn retail_l2(target_max_pq: u16) -> ExtMetadataBlock {
        ExtMetadataBlock::Level2(ExtMetadataBlockLevel2 {
            target_max_pq,
            trim_slope: 1602,
            trim_power: 1550,
            trim_saturation_gain: 1929,
            ms_weight: 0,
            ..Default::default()
        })
    }

    fn l5(top: u16) -> ExtMetadataBlock {
        ExtMetadataBlock::Level5(ExtMetadataBlockLevel5 {
            active_area_top_offset: top,
            active_area_bottom_offset: top,
            ..Default::default()
        })
    }

    #[test]
    fn authored_levels_of_a_cm_v29_source() {
        let rpus = generated(
            RpuCmVersion::V29,
            vec![vec![
                retail_l2(2081),
                ExtMetadataBlock::Level4(ExtMetadataBlockLevel4 {
                    anchor_pq: 1200,
                    anchor_power: 2000,
                }),
            ]],
        );
        let levels = authored(&rpus);
        assert_eq!(levels.l2_trims, BTreeSet::from([2081]));
        assert!(levels.l4);
        assert!(levels.l6);
        assert_eq!(levels.l5_values.len(), 1);
        assert!(!levels.l3);
        assert!(levels.l8_trims.is_empty());
        assert!(levels.l9.is_empty() && levels.l11.is_empty() && levels.l254.is_empty());
        assert!(levels.l10_targets.is_empty() && !levels.l255);
    }

    #[test]
    fn authored_levels_of_a_cm_v40_source() {
        let rpus = generated(
            RpuCmVersion::V40,
            vec![vec![
                ExtMetadataBlock::Level2(ExtMetadataBlockLevel2 {
                    target_max_pq: 2081,
                    ..Default::default()
                }),
                ExtMetadataBlock::Level3(ExtMetadataBlockLevel3 {
                    min_pq_offset: 2060,
                    ..Default::default()
                }),
                ExtMetadataBlock::Level8(ExtMetadataBlockLevel8 {
                    trim_slope: 2077,
                    ..Default::default()
                }),
            ]],
        );
        let levels = authored(&rpus);
        // A neutral L2 is not authored metadata the rebuild loses.
        assert!(levels.l2_trims.is_empty(), "{levels:?}");
        assert!(levels.l3);
        assert_eq!(levels.l8_trims, BTreeSet::from([1]));
        assert!(!levels.l4);
        assert_eq!(levels.l9, BTreeSet::from([0]));
        assert_eq!(levels.l11, BTreeSet::from([(1, 0, false)]));
        assert_eq!(levels.l254, BTreeSet::from([(0, 2)]));
    }

    #[test]
    fn neutral_cm_v40_defaults_are_not_authored() {
        let levels = authored(&generated(RpuCmVersion::V40, vec![vec![]]));
        assert!(!levels.l3 && levels.l8_trims.is_empty() && levels.l2_trims.is_empty());
    }

    #[test]
    fn distinct_l5_and_l11_values_are_all_kept() {
        let rpus = generated(
            RpuCmVersion::V40,
            vec![
                vec![
                    l5(0),
                    ExtMetadataBlock::Level11(ExtMetadataBlockLevel11::default_reference_cinema()),
                ],
                vec![
                    l5(276),
                    ExtMetadataBlock::Level11(ExtMetadataBlockLevel11 {
                        content_type: 1,
                        whitepoint: 0,
                        reference_mode_flag: false,
                        ..Default::default()
                    }),
                ],
            ],
        );
        let levels = authored(&rpus);
        assert_eq!(levels.l5_values.len(), 2);
        assert_eq!(levels.l11.len(), 2, "{levels:?}");
        assert!(levels.l11.contains(&(1, 0, false)));
    }

    #[test]
    fn custom_l8_targets_come_with_their_l10() {
        let rpus = generated(
            RpuCmVersion::V40,
            vec![vec![
                ExtMetadataBlock::Level10(ExtMetadataBlockLevel10 {
                    target_display_index: 5,
                    target_max_pq: 2851,
                    ..Default::default()
                }),
                ExtMetadataBlock::Level8(ExtMetadataBlockLevel8 {
                    target_display_index: 5,
                    trim_power: 1900,
                    ..Default::default()
                }),
            ]],
        );
        let levels = authored(&rpus);
        assert_eq!(levels.l8_trims, BTreeSet::from([5]));
        assert_eq!(levels.l10_targets.get(&5), Some(&2851));
    }

    #[test]
    fn windows_accumulate_into_one_summary() {
        let mut acc = AuthoredLevels::default();
        record_authored_levels(
            &mut acc,
            &generated(RpuCmVersion::V29, vec![vec![retail_l2(2081), l5(0)]]),
        );
        record_authored_levels(
            &mut acc,
            &generated(RpuCmVersion::V29, vec![vec![retail_l2(2851), l5(140)]]),
        );
        assert_eq!(acc.l2_trims, BTreeSet::from([2081, 2851]));
        assert_eq!(acc.l5_values.len(), 2);
    }

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

    /// `rpus` in the `RPU.bin` layout `dovi_tool` writes: a 4-byte start code, then the
    /// unspec62 NAL without its 2-byte NAL header.
    fn write_rpu_bin(dir: &Path, name: &str, rpus: &[DoviRpu]) -> PathBuf {
        let mut bytes = Vec::new();
        for rpu in rpus {
            let nal = rpu.write_hevc_unspec62_nalu().unwrap();
            bytes.extend_from_slice(&[0, 0, 0, 1]);
            bytes.extend_from_slice(&nal[2..]);
        }
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    /// Generated Profile 8.1 RPUs converted the way `dovi_tool editor` mode 1 does: the EL flags
    /// set and a zero (MEL) NLQ, which makes the header Profile 7 with a 12-bit VDR.
    fn profile7_mel() -> Vec<DoviRpu> {
        let mut rpus = generated(RpuCmVersion::V40, vec![vec![]]);
        for rpu in &mut rpus {
            rpu.convert_with_mode(1u8).unwrap();
        }
        rpus
    }

    #[test]
    fn classify_rpu_format_tells_profile7_mel_fel_profile8_and_other_profiles_apart() {
        let dir = tempfile::tempdir().unwrap();
        let mel = profile7_mel();
        // One non-zero NLQ offset is a residual the enhancement layer must carry: FEL.
        let mut fel = profile7_mel();
        for rpu in &mut fel {
            let nlq = rpu.rpu_data_mapping.as_mut().unwrap().nlq.as_mut().unwrap();
            nlq.nlq_offset[0] = 64;
        }
        let profile5 = GenerateConfig {
            length: 2,
            shots: vec![VideoShot {
                duration: 2,
                ..Default::default()
            }],
            profile: dolby_vision::rpu::generate::GenerateProfile::Profile5,
            ..Default::default()
        }
        .generate_rpu_list()
        .unwrap();
        for (name, rpus, expected) in [
            (
                "p8.bin",
                generated(RpuCmVersion::V40, vec![vec![]]),
                RpuFormatKind::Profile8,
            ),
            ("mel.bin", mel, RpuFormatKind::Profile7Mel),
            ("fel.bin", fel, RpuFormatKind::Profile7Fel),
            ("p5.bin", profile5, RpuFormatKind::OtherDolbyVision),
        ] {
            let path = write_rpu_bin(dir.path(), name, &rpus);
            assert_eq!(classify_rpu_format(&path).unwrap(), expected, "{name}");
        }
    }

    #[test]
    fn classify_rpu_format_rejects_a_file_without_rpus() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rpu_bin(dir.path(), "empty.bin", &[]);
        assert!(classify_rpu_format(&path).is_err());
    }
}
