//! Profile 7 FEL (Full Enhancement Layer) compositing and conversion to Profile 8.1.
//!
//! This module implements the NLQ (Non-Linear Quantization) compositing algorithm
//! to combine BL (Base Layer) and EL (Enhancement Layer) into a single 12-bit output,
//! then re-encodes it as a standard HDR10 stream suitable for Profile 8.1 RPU injection.
//!
//! The NLQ LinearDeadzone compositing formula is derived from the Dolby Vision
//! specification and the reference implementation in quietvoid's vs-nlq plugin.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use colored::Colorize;
use dolby_vision::rpu::utils::parse_rpu_file;

use crate::cli::{Args, Encoder, HwAccel};
use crate::external::{self, run_command_live, run_command_with_spinner};
use crate::metadata;

/// Information extracted from the Profile 7 FEL RPU
#[derive(Debug)]
#[allow(dead_code)]
pub struct FelInfo {
    pub profile: u8,
    pub el_type: String, // "FEL" or "MEL"
    pub frame_count: u32,
    pub scene_count: u32,
    pub source_min_pq: u16,
    pub source_max_pq: u16,
}

/// NLQ parameters for a single frame, per channel
#[derive(Debug, Clone)]
struct NlqParams {
    nlq_offset: [i64; 3],
    vdr_in_max_int: [i64; 3],
    vdr_in_max: [i64; 3],
    linear_deadzone_slope_int: [i64; 3],
    linear_deadzone_slope: [i64; 3],
    linear_deadzone_threshold_int: [i64; 3],
    linear_deadzone_threshold: [i64; 3],
    coeff_log2_denom: i64,
    disable_residual_flag: bool,
    el_bit_depth: u8,
}

/// Check if a file is Profile 7 FEL using dovi_tool
#[allow(dead_code)]
pub fn detect_profile7_fel(input_file: &str, temp_dir: &Path) -> Result<Option<FelInfo>> {
    // Extract RPU to analyze
    let rpu_path = temp_dir.join("detect_rpu.bin");
    let dovi_tool_path =
        external::find_tool("dovi_tool").unwrap_or_else(|| PathBuf::from("dovi_tool"));
    let dovi_abs = fs::canonicalize(&dovi_tool_path).unwrap_or(dovi_tool_path);

    // Extract RPU (limit to 1 frame for quick detection)
    let mut cmd = Command::new(&dovi_abs);
    cmd.args([
        "extract-rpu",
        "-i",
        input_file,
        "-o",
        rpu_path.to_str().unwrap(),
        "-l",
        "1",
    ]);

    let output = cmd
        .output()
        .context("Failed to run dovi_tool extract-rpu")?;
    if !output.status.success() {
        // Not a DV file or no RPU found
        return Ok(None);
    }

    // Get info summary
    let mut info_cmd = Command::new(&dovi_abs);
    info_cmd.args(["info", "-i", rpu_path.to_str().unwrap(), "-s"]);

    let info_output = external::get_command_output(&mut info_cmd);
    let _ = fs::remove_file(&rpu_path);

    match info_output {
        Ok(info_text) => {
            // Parse the summary text
            if !info_text.contains("Profile 7") {
                return Ok(None);
            }

            let is_fel = info_text.contains("FEL");
            if !is_fel {
                return Ok(None);
            }

            // Extract basic info
            let frame_count = extract_number_after(&info_text, "Frames:").unwrap_or(0.0) as u32;
            let scene_count = extract_number_after(&info_text, "Scene/Cuts:").unwrap_or(0.0) as u32;

            Ok(Some(FelInfo {
                profile: 7,
                el_type: "FEL".to_string(),
                frame_count,
                scene_count,
                source_min_pq: 0,
                source_max_pq: 0,
            }))
        }
        Err(_) => Ok(None),
    }
}

/// Main FEL conversion pipeline: demux → composite → re-encode → return path to composited HEVC
pub fn convert_fel_to_hdr10(input_file: &str, temp_dir: &Path, args: &Args) -> Result<PathBuf> {
    println!(
        "{}",
        "Profile 7 FEL detected! Starting BL+EL compositing pipeline..."
            .cyan()
            .bold()
    );

    // Step 1: Extract raw HEVC from MKV
    let raw_hevc = temp_dir.join("raw_dual_layer.hevc");
    println!("{}", "Step 1/5: Extracting HEVC bitstream...".green());
    extract_hevc_from_mkv(input_file, &raw_hevc, temp_dir)?;

    // Step 2: Demux into BL and EL
    let bl_hevc = temp_dir.join("FEL_BL.hevc");
    let el_hevc = temp_dir.join("FEL_EL.hevc");
    let rpu_bin = temp_dir.join("FEL_RPU.bin");
    println!("{}", "Step 2/5: Demuxing BL and EL layers...".green());
    demux_dual_layer(&raw_hevc, &bl_hevc, &el_hevc, &rpu_bin, temp_dir)?;

    // Clean up the large raw HEVC to save disk space
    let _ = fs::remove_file(&raw_hevc);

    // Step 3: Composite BL+EL using NLQ
    let composited_yuv = temp_dir.join("composited.yuv");
    println!(
        "{}",
        "Step 3/5: Compositing BL+EL via NLQ (this may take a while)...".green()
    );
    composite_bl_el_nlq(&bl_hevc, &el_hevc, &rpu_bin, &composited_yuv, temp_dir)?;

    // Step 4: Get BL video properties for encoding
    let (width, height, fps_num, fps_den) = get_video_properties(input_file)?;

    // Step 5: Re-encode composited output as HDR10 HEVC
    let composited_mkv = temp_dir.join("FEL_composited.mkv");
    println!("{}", "Step 4/5: Re-encoding composited output...".green());
    reencode_composited(
        &composited_yuv,
        &composited_mkv,
        width,
        height,
        fps_num,
        fps_den,
        input_file,
        args,
        temp_dir,
    )?;

    // Clean up large intermediate files
    let _ = fs::remove_file(&composited_yuv);
    let _ = fs::remove_file(&bl_hevc);
    let _ = fs::remove_file(&el_hevc);

    println!(
        "{}",
        "Step 5/5: FEL compositing complete! Proceeding with DV RPU generation..."
            .green()
            .bold()
    );

    Ok(composited_mkv)
}

// --- Internal helpers ---

fn extract_hevc_from_mkv(input: &str, output: &Path, temp_dir: &Path) -> Result<()> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-stats",
        "-i",
        input,
        "-map",
        "0:v:0",
        "-c:v",
        "copy",
        "-f",
        "hevc",
        "-y",
        output.to_str().unwrap(),
    ]);

    if !run_command_live(&mut cmd, &temp_dir.join("ffmpeg_extract_hevc.log"))? {
        bail!("Failed to extract HEVC bitstream from MKV");
    }
    Ok(())
}

fn demux_dual_layer(
    raw_hevc: &Path,
    bl_out: &Path,
    el_out: &Path,
    rpu_out: &Path,
    temp_dir: &Path,
) -> Result<()> {
    let dovi_tool_path =
        external::find_tool("dovi_tool").unwrap_or_else(|| PathBuf::from("dovi_tool"));
    let dovi_abs = fs::canonicalize(&dovi_tool_path).unwrap_or(dovi_tool_path);

    let mut cmd = Command::new(&dovi_abs);
    cmd.args([
        "demux",
        "-i",
        raw_hevc.to_str().unwrap(),
        "-b",
        bl_out.to_str().unwrap(),
        "-e",
        el_out.to_str().unwrap(),
    ]);

    if !run_command_with_spinner(
        &mut cmd,
        &temp_dir.join("dovi_demux.log"),
        "Demuxing dual-layer HEVC",
    )? {
        bail!("Failed to demux BL/EL with dovi_tool");
    }

    // Also extract RPU separately
    let mut rpu_cmd = Command::new(&dovi_abs);
    rpu_cmd.args([
        "extract-rpu",
        "-i",
        raw_hevc.to_str().unwrap(),
        "-o",
        rpu_out.to_str().unwrap(),
    ]);

    if !run_command_with_spinner(
        &mut rpu_cmd,
        &temp_dir.join("dovi_extract_rpu.log"),
        "Extracting RPU data",
    )? {
        bail!("Failed to extract RPU with dovi_tool");
    }

    Ok(())
}

/// Composite BL + EL using the NLQ LinearDeadzone algorithm.
///
/// Pipeline:
/// 1. Decode BL via ffmpeg → raw YUV 4:2:0 16-bit
/// 2. Decode EL via ffmpeg → raw YUV 4:2:0 10-bit (upsampled to BL resolution)
/// 3. Parse RPU for NLQ params per frame
/// 4. Apply NLQ compositing formula pixel-by-pixel
/// 5. Output 10-bit YUV (dithered from 12-bit internal)
fn composite_bl_el_nlq(
    bl_hevc: &Path,
    el_hevc: &Path,
    rpu_bin: &Path,
    output_yuv: &Path,
    temp_dir: &Path,
) -> Result<()> {
    // Get BL dimensions
    let (width, height) = get_hevc_dimensions(bl_hevc)?;

    // Parse all RPU frames to get NLQ params
    let nlq_params = parse_rpu_nlq_params(rpu_bin, temp_dir)?;
    let total_frames = nlq_params.len();

    println!(
        "{}",
        format!(
            "  Compositing {} frames at {}x{} (BL 16-bit + EL 10-bit → 10-bit output)",
            total_frames, width, height
        )
        .cyan()
    );

    // Frame sizes
    // YUV 4:2:0: Y = w*h, U = w*h/4, V = w*h/4 → total = w*h*3/2
    let y_pixels = (width * height) as usize;
    let uv_pixels = y_pixels / 4;
    let bl_frame_bytes = y_pixels * 2 + uv_pixels * 2 * 2; // 16-bit per component
    let el_frame_bytes = y_pixels * 2 + uv_pixels * 2 * 2; // 10-bit stored as 16-bit (output of ffmpeg)
    let out_frame_bytes = y_pixels * 2 + uv_pixels * 2 * 2; // 10-bit stored as 16-bit

    // Launch BL decoder: ffmpeg → raw YUV 16-bit
    let mut bl_proc = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            bl_hevc.to_str().unwrap(),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p16le",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to launch ffmpeg for BL decoding")?;

    // Launch EL decoder: ffmpeg → raw YUV 16-bit (upsampled to BL resolution)
    let vf_filter = format!("scale={}:{}:flags=lanczos", width, height);
    let mut el_proc = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            el_hevc.to_str().unwrap(),
            "-vf",
            &vf_filter,
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p16le",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to launch ffmpeg for EL decoding")?;

    let mut bl_stdout = bl_proc.stdout.take().unwrap();
    let mut el_stdout = el_proc.stdout.take().unwrap();

    // Output file
    let mut out_file = std::io::BufWriter::new(
        fs::File::create(output_yuv).context("Failed to create output YUV")?,
    );

    // Progress bar
    let pb = indicatif::ProgressBar::new(total_frames as u64);
    pb.set_style(
        indicatif::ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} frames ({eta})")
            .unwrap()
            .progress_chars("█▓░"),
    );

    let mut bl_buf = vec![0u8; bl_frame_bytes];
    let mut el_buf = vec![0u8; el_frame_bytes];
    let mut out_buf = vec![0u8; out_frame_bytes];

    for frame_idx in 0..total_frames {
        // Read BL frame
        if bl_stdout.read_exact(&mut bl_buf).is_err() {
            println!(
                "{}",
                format!(
                    "  BL stream ended at frame {} (expected {})",
                    frame_idx, total_frames
                )
                .yellow()
            );
            break;
        }

        // Read EL frame
        if el_stdout.read_exact(&mut el_buf).is_err() {
            println!(
                "{}",
                format!(
                    "  EL stream ended at frame {} (expected {})",
                    frame_idx, total_frames
                )
                .yellow()
            );
            break;
        }

        // Get NLQ params for this frame (or last available)
        let params = &nlq_params[frame_idx.min(nlq_params.len() - 1)];

        // Composite: process Y, U, V planes
        // Y plane: full resolution
        composite_plane(
            &bl_buf[..y_pixels * 2],
            &el_buf[..y_pixels * 2],
            &mut out_buf[..y_pixels * 2],
            params,
            0, // channel 0 = Y
        );

        // U plane
        let bl_u_offset = y_pixels * 2;
        let el_u_offset = y_pixels * 2;
        let out_u_offset = y_pixels * 2;
        composite_plane(
            &bl_buf[bl_u_offset..bl_u_offset + uv_pixels * 2],
            &el_buf[el_u_offset..el_u_offset + uv_pixels * 2],
            &mut out_buf[out_u_offset..out_u_offset + uv_pixels * 2],
            params,
            1, // channel 1 = U
        );

        // V plane
        let bl_v_offset = bl_u_offset + uv_pixels * 2;
        let el_v_offset = el_u_offset + uv_pixels * 2;
        let out_v_offset = out_u_offset + uv_pixels * 2;
        composite_plane(
            &bl_buf[bl_v_offset..bl_v_offset + uv_pixels * 2],
            &el_buf[el_v_offset..el_v_offset + uv_pixels * 2],
            &mut out_buf[out_v_offset..out_v_offset + uv_pixels * 2],
            params,
            2, // channel 2 = V
        );

        out_file
            .write_all(&out_buf)
            .context("Failed to write composited frame")?;

        pb.inc(1);
    }

    pb.finish_with_message("Compositing complete");
    out_file.flush()?;

    // Wait for child processes
    let _ = bl_proc.wait();
    let _ = el_proc.wait();

    Ok(())
}

/// Apply NLQ LinearDeadzone compositing to a single plane.
///
/// Formula (per pixel):
/// ```text
/// tmp = el_pixel - offset
/// if tmp != 0:
///   sign = signum(tmp)
///   tmp = (tmp << 1) - sign
///   tmp <<= (10 - el_bit_depth)
///   dq = tmp * slope + (threshold << (10 - el_bit_depth + 1)) * sign
///   dq = clamp(dq, -vdr_in_max << (10 - el_bit_depth + 1), vdr_in_max << (10 - el_bit_depth + 1))
///   result = dq >> (coeff_log2_denom - 5 - el_bit_depth)
/// h = bl_pixel_16bit + result
/// output = (h + rounding) >> 6  (16-bit → 10-bit)
/// ```
fn composite_plane(
    bl_data: &[u8],      // 16-bit LE samples
    el_data: &[u8],      // 16-bit LE samples (EL was decoded to 16-bit by ffmpeg)
    out_data: &mut [u8], // 10-bit stored as 16-bit LE
    params: &NlqParams,
    channel: usize,
) {
    let pixel_count = bl_data.len() / 2;
    let coeff_log2_denom = params.coeff_log2_denom;
    let el_bit_depth = params.el_bit_depth as i64;

    // Assemble fixed-point params
    let fp_slope = (params.linear_deadzone_slope_int[channel] << coeff_log2_denom)
        + params.linear_deadzone_slope[channel];
    let fp_threshold = (params.linear_deadzone_threshold_int[channel] << coeff_log2_denom)
        + params.linear_deadzone_threshold[channel];
    let fp_in_max =
        (params.vdr_in_max_int[channel] << coeff_log2_denom) + params.vdr_in_max[channel];
    let nlq_offset = params.nlq_offset[channel];

    // EL from ffmpeg is 16-bit, but represents 10-bit content scaled up.
    // We need to scale it back to 10-bit range for NLQ.
    let el_scale_shift: u32 = 6; // 16-bit → 10-bit

    let out_bit_depth: i64 = 10; // Output 10-bit (practical for re-encoding)
                                 // Shift from 16-bit BL to output: 16 - out_bit_depth = 6
    let out_shift = 16 - out_bit_depth;
    let out_round = 1i64 << (out_shift - 1);
    let out_max = (1i64 << out_bit_depth) - 1;

    for i in 0..pixel_count {
        let bl_16 = i64::from(u16::from_le_bytes([bl_data[i * 2], bl_data[i * 2 + 1]]));
        let el_16 = u16::from_le_bytes([el_data[i * 2], el_data[i * 2 + 1]]);
        // Scale EL back to 10-bit
        let el_10 = i64::from(el_16 >> el_scale_shift);

        let mut h = bl_16;

        if !params.disable_residual_flag {
            let tmp = el_10 - nlq_offset;

            if tmp != 0 {
                let sign: i64 = if tmp < 0 { -1 } else { 1 };
                let mut val = (tmp << 1) - sign;
                val <<= 10 - el_bit_depth;

                let mut dq = val * fp_slope;
                let tt = (fp_threshold << (10 - el_bit_depth + 1)) * sign;
                dq += tt;

                let rr = fp_in_max << (10 - el_bit_depth + 1);
                dq = dq.clamp(-rr, rr);

                let result = dq >> (coeff_log2_denom - 5 - el_bit_depth);
                h += result;
            }
        }

        // Round and shift to output bit depth
        h = ((h + out_round) >> out_shift).clamp(0, out_max);

        // Store as 16-bit LE (10-bit value in 16-bit container)
        let h16 = (h as u16) << el_scale_shift; // Scale back to 16-bit for yuv420p16le output
        let bytes = h16.to_le_bytes();
        out_data[i * 2] = bytes[0];
        out_data[i * 2 + 1] = bytes[1];
    }
}

/// Parse RPU binary to extract NLQ parameters per frame using the dolby_vision crate
fn parse_rpu_nlq_params(rpu_bin: &Path, _temp_dir: &Path) -> Result<Vec<NlqParams>> {
    println!(
        "{}",
        "  Parsing RPU binary with dolby_vision crate...".cyan()
    );

    let rpus = parse_rpu_file(rpu_bin).context("Failed to parse RPU binary file")?;
    let total_frames = rpus.len();

    println!(
        "{}",
        format!("  Parsed {} RPU frames, extracting NLQ parameters...", total_frames).cyan()
    );

    let pb = indicatif::ProgressBar::new(total_frames as u64);
    pb.set_style(
        indicatif::ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} RPU frames",
            )
            .unwrap()
            .progress_chars("█▓░"),
    );

    let mut all_params = Vec::with_capacity(total_frames);

    for rpu in &rpus {
        let coeff_log2_denom = rpu.header.coefficient_log2_denom as i64;
        let el_bit_depth = (rpu.header.el_bit_depth_minus8 + 8) as u8;
        let disable_residual = rpu.header.disable_residual_flag;

        if disable_residual {
            all_params.push(NlqParams {
                nlq_offset: [0; 3],
                vdr_in_max_int: [0; 3],
                vdr_in_max: [0; 3],
                linear_deadzone_slope_int: [0; 3],
                linear_deadzone_slope: [0; 3],
                linear_deadzone_threshold_int: [0; 3],
                linear_deadzone_threshold: [0; 3],
                coeff_log2_denom,
                disable_residual_flag: true,
                el_bit_depth,
            });
        } else if let Some(ref mapping) = rpu.rpu_data_mapping {
            if let Some(ref nlq) = mapping.nlq {
                let params = NlqParams {
                    nlq_offset: [
                        nlq.nlq_offset[0] as i64,
                        nlq.nlq_offset[1] as i64,
                        nlq.nlq_offset[2] as i64,
                    ],
                    vdr_in_max_int: [
                        nlq.vdr_in_max_int[0] as i64,
                        nlq.vdr_in_max_int[1] as i64,
                        nlq.vdr_in_max_int[2] as i64,
                    ],
                    vdr_in_max: [
                        nlq.vdr_in_max[0] as i64,
                        nlq.vdr_in_max[1] as i64,
                        nlq.vdr_in_max[2] as i64,
                    ],
                    linear_deadzone_slope_int: [
                        nlq.linear_deadzone_slope_int[0] as i64,
                        nlq.linear_deadzone_slope_int[1] as i64,
                        nlq.linear_deadzone_slope_int[2] as i64,
                    ],
                    linear_deadzone_slope: [
                        nlq.linear_deadzone_slope[0] as i64,
                        nlq.linear_deadzone_slope[1] as i64,
                        nlq.linear_deadzone_slope[2] as i64,
                    ],
                    linear_deadzone_threshold_int: [
                        nlq.linear_deadzone_threshold_int[0] as i64,
                        nlq.linear_deadzone_threshold_int[1] as i64,
                        nlq.linear_deadzone_threshold_int[2] as i64,
                    ],
                    linear_deadzone_threshold: [
                        nlq.linear_deadzone_threshold[0] as i64,
                        nlq.linear_deadzone_threshold[1] as i64,
                        nlq.linear_deadzone_threshold[2] as i64,
                    ],
                    coeff_log2_denom,
                    disable_residual_flag: false,
                    el_bit_depth,
                };
                all_params.push(params);
            } else {
                // Mapping exists but no NLQ data — treat as identity
                all_params.push(NlqParams {
                    nlq_offset: [0; 3],
                    vdr_in_max_int: [0; 3],
                    vdr_in_max: [0; 3],
                    linear_deadzone_slope_int: [0; 3],
                    linear_deadzone_slope: [0; 3],
                    linear_deadzone_threshold_int: [0; 3],
                    linear_deadzone_threshold: [0; 3],
                    coeff_log2_denom,
                    disable_residual_flag: true,
                    el_bit_depth,
                });
            }
        } else {
            // No mapping data at all — treat as identity
            all_params.push(NlqParams {
                nlq_offset: [0; 3],
                vdr_in_max_int: [0; 3],
                vdr_in_max: [0; 3],
                linear_deadzone_slope_int: [0; 3],
                linear_deadzone_slope: [0; 3],
                linear_deadzone_threshold_int: [0; 3],
                linear_deadzone_threshold: [0; 3],
                coeff_log2_denom,
                disable_residual_flag: true,
                el_bit_depth,
            });
        }

        pb.inc(1);
    }

    pb.finish_with_message("RPU parsing complete");

    if all_params.is_empty() {
        bail!("No RPU frames found in {}", rpu_bin.display());
    }

    // Report NLQ statistics
    let nlq_frames = all_params.iter().filter(|p| !p.disable_residual_flag).count();
    println!(
        "{}",
        format!(
            "  NLQ active: {}/{} frames ({}%)",
            nlq_frames,
            total_frames,
            nlq_frames * 100 / total_frames.max(1)
        )
        .cyan()
    );

    Ok(all_params)
}

/// Re-encode the composited YUV output as HDR10 HEVC in MKV container
fn reencode_composited(
    input_yuv: &Path,
    output_mkv: &Path,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    original_file: &str,
    args: &Args,
    temp_dir: &Path,
) -> Result<()> {
    let static_meta = metadata::get_static_metadata(original_file);
    let max_dml = *static_meta.get("max_dml").unwrap_or(&1000.0) as u32;
    let min_dml = static_meta.get("min_dml").unwrap_or(&0.005);
    let max_cll = *static_meta.get("max_cll").unwrap_or(&1000.0) as u32;
    let max_fall = *static_meta.get("max_fall").unwrap_or(&400.0) as u32;

    let min_dml_int = (min_dml * 10000.0) as u32;
    let max_dml_int = max_dml * 10000;

    let master_display = format!(
        "G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L({},{})",
        max_dml_int, min_dml_int
    );

    let framerate = format!("{}/{}", fps_num, fps_den);
    let resolution = format!("{}x{}", width, height);

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-stats",
        "-f",
        "rawvideo",
        "-pixel_format",
        "yuv420p16le",
        "-video_size",
        &resolution,
        "-framerate",
        &framerate,
        "-i",
        input_yuv.to_str().unwrap(),
        "-y",
    ]);

    if args.hwaccel == HwAccel::Cuda {
        cmd.args([
            "-c:v",
            "hevc_nvenc",
            "-preset",
            "p7",
            "-tune",
            "hq",
            "-rc",
            "constqp",
            "-qp",
            "18",
            "-pix_fmt",
            "yuv420p10le",
            "-profile:v",
            "main10",
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "smpte2084",
            "-colorspace",
            "bt2020nc",
            "-color_range",
            "tv",
        ]);
    } else {
        let x265_params = format!(
            "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:master-display={}:max-cll={},{}:hdr-opt=1:repeat-headers=1",
            master_display, max_cll, max_fall
        );

        match args.encoder {
            Encoder::Libx265 => {
                let crf_str = args.fel_crf.to_string();
                cmd.args([
                    "-c:v",
                    "libx265",
                    "-preset",
                    &args.fel_preset,
                    "-crf",
                    &crf_str,
                    "-pix_fmt",
                    "yuv420p10le",
                    "-profile:v",
                    "main10",
                    "-x265-params",
                    &x265_params,
                ]);
            }
            Encoder::HevcVideotoolbox => {
                cmd.args([
                    "-c:v",
                    "hevc_videotoolbox",
                    "-allow_sw",
                    "1",
                    "-profile:v",
                    "main10",
                    "-pix_fmt",
                    "p010le",
                    "-color_primaries",
                    "bt2020",
                    "-color_trc",
                    "smpte2084",
                    "-colorspace",
                    "bt2020nc",
                    "-q:v",
                    "65",
                ]);
            }
        }
    }

    cmd.arg(output_mkv.to_str().unwrap());

    if !run_command_live(&mut cmd, &temp_dir.join("ffmpeg_reencode.log"))? {
        bail!("Failed to re-encode composited output");
    }

    Ok(())
}

/// Get video dimensions and framerate from input file
fn get_video_properties(input_file: &str) -> Result<(u32, u32, u32, u32)> {
    let mut cmd = Command::new("ffprobe");
    cmd.args([
        "-v",
        "quiet",
        "-print_format",
        "json",
        "-show_streams",
        "-select_streams",
        "v:0",
        input_file,
    ]);

    let output = external::get_command_output(&mut cmd)?;
    let json: serde_json::Value = serde_json::from_str(&output)?;

    let stream = json
        .get("streams")
        .and_then(|s| s.as_array())
        .and_then(|a| a.first())
        .context("No video stream found")?;

    let width = stream
        .get("width")
        .and_then(|v| v.as_u64())
        .context("No width found")? as u32;
    let height = stream
        .get("height")
        .and_then(|v| v.as_u64())
        .context("No height found")? as u32;

    // Parse r_frame_rate which is "num/den"
    let fps_str = stream
        .get("r_frame_rate")
        .and_then(|v| v.as_str())
        .unwrap_or("24000/1001");

    let parts: Vec<&str> = fps_str.split('/').collect();
    let fps_num = parts[0].parse::<u32>().unwrap_or(24000);
    let fps_den = if parts.len() > 1 {
        parts[1].parse::<u32>().unwrap_or(1001)
    } else {
        1
    };

    Ok((width, height, fps_num, fps_den))
}

/// Get HEVC stream dimensions
fn get_hevc_dimensions(hevc_path: &Path) -> Result<(u32, u32)> {
    let mut cmd = Command::new("ffprobe");
    cmd.args([
        "-v",
        "quiet",
        "-print_format",
        "json",
        "-show_streams",
        "-select_streams",
        "v:0",
        hevc_path.to_str().unwrap(),
    ]);

    let output = external::get_command_output(&mut cmd)?;
    let json: serde_json::Value = serde_json::from_str(&output)?;

    let stream = json
        .get("streams")
        .and_then(|s| s.as_array())
        .and_then(|a| a.first())
        .context("No video stream found in HEVC")?;

    let width = stream
        .get("width")
        .and_then(|v| v.as_u64())
        .context("No width in HEVC")? as u32;
    let height = stream
        .get("height")
        .and_then(|v| v.as_u64())
        .context("No height in HEVC")? as u32;

    Ok((width, height))
}

/// Extract a number after a label in text output
fn extract_number_after(text: &str, label: &str) -> Option<f64> {
    let idx = text.find(label)?;
    let after = &text[idx + label.len()..];
    let trimmed = after.trim();
    // Take chars while digit or dot
    let num_str: String = trimmed
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num_str.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nlq_identity_composite() {
        // When disable_residual_flag is true, output should equal BL
        let params = NlqParams {
            nlq_offset: [512, 512, 512],
            vdr_in_max_int: [0; 3],
            vdr_in_max: [1048576, 1048576, 1048576],
            linear_deadzone_slope_int: [0; 3],
            linear_deadzone_slope: [2048, 2048, 2048],
            linear_deadzone_threshold_int: [0; 3],
            linear_deadzone_threshold: [0; 3],
            coeff_log2_denom: 23,
            disable_residual_flag: true,
            el_bit_depth: 10,
        };

        // BL = 32768 (mid-range 16-bit), EL = anything
        let bl_data: Vec<u8> = vec![0x00, 0x80]; // 32768 LE
        let el_data: Vec<u8> = vec![0x00, 0x40]; // 16384 LE
        let mut out_data = vec![0u8; 2];

        composite_plane(&bl_data, &el_data, &mut out_data, &params, 0);

        let out_val = u16::from_le_bytes([out_data[0], out_data[1]]);
        // With identity (disabled residual), BL 32768 → 10-bit 512 → 16-bit 32768
        assert_eq!(out_val, 32768);
    }

    #[test]
    fn test_nlq_with_residual() {
        // When residual is enabled and EL differs from offset, should modify BL
        let params = NlqParams {
            nlq_offset: [512, 512, 512],
            vdr_in_max_int: [0; 3],
            vdr_in_max: [1048576, 1048576, 1048576],
            linear_deadzone_slope_int: [0; 3],
            linear_deadzone_slope: [2048, 2048, 2048],
            linear_deadzone_threshold_int: [0; 3],
            linear_deadzone_threshold: [0; 3],
            coeff_log2_denom: 23,
            disable_residual_flag: false,
            el_bit_depth: 10,
        };

        // BL = 32768, EL = 33792 (offset 512 in 10-bit = 32768 in 16-bit, EL = 528 in 10-bit)
        let bl_data: Vec<u8> = vec![0x00, 0x80]; // 32768
        let el_data: Vec<u8> = vec![0x00, 0x84]; // 33792 → 10-bit: 528
        let mut out_data = vec![0u8; 2];

        composite_plane(&bl_data, &el_data, &mut out_data, &params, 0);

        let out_val = u16::from_le_bytes([out_data[0], out_data[1]]);
        // Output should differ from BL since EL has a residual contribution
        // Exact value depends on NLQ formula, just verify it changed
        assert_ne!(out_val, 32768, "Residual should modify the output");
    }

    #[test]
    fn test_extract_number_after() {
        assert_eq!(
            extract_number_after("Frames: 2908", "Frames:"),
            Some(2908.0)
        );
        assert_eq!(
            extract_number_after("Scene/Cuts: 34", "Scene/Cuts:"),
            Some(34.0)
        );
        assert_eq!(extract_number_after("no match here", "Frames:"), None);
    }
}
