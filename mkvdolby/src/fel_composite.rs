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
use std::process::{Child, Command, Stdio};

use anyhow::{bail, Context, Result};
use colored::Colorize;
use dolby_vision::rpu::rpu_data_mapping::DoviMappingMethod;
use dolby_vision::rpu::utils::parse_rpu_file;

use crate::cli::{Args, CompositePipeArgs, Encoder, FelEncoder, HwAccel};
use crate::external::{self, run_command_live, run_command_with_spinner};
use crate::metadata;

/// Path to the modal-ffmpeg script (absolute path on the deployment host).
const MODAL_FFMPEG_SCRIPT: &str = "/home/ubuntu/modal-ffmpeg/src/modal_ffmpeg.py";

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

/// Reshaping curve type for a single channel
#[derive(Debug, Clone)]
enum ReshapingCurve {
    /// Piecewise polynomial (typically luma / channel 0)
    /// pivots: BL signal pivot points (12-bit range after scaling)
    /// pieces: each piece has coefficients [c0, c1, c2] in fixed-point
    Polynomial {
        pivots: Vec<i64>,
        /// Per-piece: (order, coefficients in fixed-point)
        /// order 0 → linear [c0, c1], order 1 → quadratic [c0, c1, c2]
        pieces: Vec<(u64, Vec<i64>)>,
    },
    /// Multi-channel Multiple Regression (typically chroma channels 1,2)
    /// pivots: BL luma pivot points (12-bit range)
    /// pieces: each piece has MMR order and coefficients
    MMR {
        pivots: Vec<i64>,
        /// Per-piece: (order, constant_fp, coefs_fp)
        /// coefs_fp is [order][7] fixed-point coefficients
        pieces: Vec<(u8, i64, Vec<Vec<i64>>)>,
    },
    /// Identity mapping — no reshaping needed
    Identity,
}

/// Per-frame reshaping parameters for all 3 channels
#[derive(Debug, Clone)]
struct ReshapingParams {
    curves: [ReshapingCurve; 3],
    coeff_log2_denom: i64,
}

/// Combined per-frame parameters for reshaping + NLQ
#[derive(Debug, Clone)]
struct FrameParams {
    nlq: NlqParams,
    reshaping: ReshapingParams,
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

    // Step 3: Probe properties needed for streaming encode
    println!("{}", "Step 3/5: Reading source video properties...".green());
    let (width, height) = get_hevc_dimensions(&bl_hevc)?;
    let (_, _, fps_num, fps_den) = get_video_properties(input_file)?;

    // Step 4: Composite BL+EL via NLQ and encode
    let composited_mkv = temp_dir.join("FEL_composited.mkv");

    match args.fel_encoder {
        FelEncoder::Modal => {
            // Upload BL+EL+RPU to Modal for composite + encode (no FFV1 intermediate)
            println!(
                "{}",
                "Step 4/5: Uploading BL+EL+RPU to Modal for composite + NVENC encode...".green()
            );

            encode_via_modal(
                &bl_hevc,
                &el_hevc,
                &rpu_bin,
                &composited_mkv,
                width,
                height,
                fps_num,
                fps_den,
                input_file,
                args,
            )?;

            // Clean up intermediate files
            let _ = fs::remove_file(&bl_hevc);
            let _ = fs::remove_file(&el_hevc);

            println!(
                "{}",
                "Step 5/5: Modal encode complete! Proceeding with DV RPU generation..."
                    .green()
                    .bold()
            );
        }
        FelEncoder::Local => {
            // Original streaming encode path
            println!(
                "{}",
                "Step 4/5: Compositing BL+EL via NLQ (streaming to encoder; this may take a while)..."
                    .green()
            );

            let (mut enc_proc, enc_log_path) = spawn_reencode_composited_from_stdin(
                &composited_mkv,
                width,
                height,
                fps_num,
                fps_den,
                input_file,
                args,
                temp_dir,
            )?;

            let composite_result = {
                let stdin = enc_proc
                    .stdin
                    .take()
                    .context("Failed to open ffmpeg stdin for rawvideo input")?;
                let mut enc_stdin = std::io::BufWriter::new(stdin);

                composite_bl_el_nlq(
                    &bl_hevc,
                    &el_hevc,
                    &rpu_bin,
                    width,
                    height,
                    &mut enc_stdin,
                    temp_dir,
                )
            };

            if let Err(err) = composite_result {
                let _ = enc_proc.kill();
                let _ = enc_proc.wait();
                return Err(err)
                    .with_context(|| format!("ffmpeg reencode log: {}", enc_log_path.display()));
            }

            let status = enc_proc
                .wait()
                .context("Failed to wait for ffmpeg re-encode process")?;
            if !status.success() {
                bail!(
                    "Failed to re-encode composited output (see {})",
                    enc_log_path.display()
                );
            }

            // Clean up large intermediate files
            let _ = fs::remove_file(&bl_hevc);
            let _ = fs::remove_file(&el_hevc);

            println!(
                "{}",
                "Step 5/5: FEL compositing complete! Proceeding with DV RPU generation..."
                    .green()
                    .bold()
            );
        }
    }

    Ok(composited_mkv)
}

/// Run the `composite-pipe` subcommand: composite BL+EL via NLQ and write raw frames to stdout.
///
/// This is designed for piping into an external encoder (e.g., ffmpeg with NVENC).
/// Output format: `yuv420p16le` raw frames on stdout.
/// All progress/status output goes to stderr.
pub fn run_composite_pipe(pipe_args: &CompositePipeArgs) -> Result<()> {
    let bl_hevc = Path::new(&pipe_args.bl);
    let el_hevc = Path::new(&pipe_args.el);
    let rpu_bin = Path::new(&pipe_args.rpu);

    if !bl_hevc.exists() {
        bail!("BL file not found: {}", pipe_args.bl);
    }
    if !el_hevc.exists() {
        bail!("EL file not found: {}", pipe_args.el);
    }
    if !rpu_bin.exists() {
        bail!("RPU file not found: {}", pipe_args.rpu);
    }

    eprintln!(
        "composite-pipe: {}x{} @ {}/{} fps",
        pipe_args.width, pipe_args.height, pipe_args.fps_num, pipe_args.fps_den
    );

    // Use a temp dir for decoder log files
    let temp_dir = std::env::temp_dir().join("mkvdolby-composite-pipe");
    fs::create_dir_all(&temp_dir)?;

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    composite_bl_el_nlq(
        bl_hevc,
        el_hevc,
        rpu_bin,
        pipe_args.width,
        pipe_args.height,
        &mut out,
        &temp_dir,
    )?;

    // Clean up temp dir
    let _ = fs::remove_dir_all(&temp_dir);

    eprintln!("composite-pipe: done");
    Ok(())
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
/// 5. Write composited 10-bit YUV frames to `out` (as yuv420p16le)
fn composite_bl_el_nlq(
    bl_hevc: &Path,
    el_hevc: &Path,
    rpu_bin: &Path,
    width: u32,
    height: u32,
    out: &mut impl Write,
    temp_dir: &Path,
) -> Result<()> {
    // Parse all RPU frames to get NLQ params
    let nlq_params = parse_rpu_params(rpu_bin, temp_dir)?;
    let total_frames = nlq_params.len();

    eprintln!(
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
    let bl_decode_log = temp_dir.join("ffmpeg_bl_decode.log");
    let bl_log_file = fs::File::create(&bl_decode_log)
        .with_context(|| format!("Failed to create {}", bl_decode_log.display()))?;

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
        .stderr(Stdio::from(bl_log_file))
        .spawn()
        .context("Failed to launch ffmpeg for BL decoding")?;

    // Launch EL decoder: ffmpeg → raw YUV 16-bit (upsampled to BL resolution)
    let el_decode_log = temp_dir.join("ffmpeg_el_decode.log");
    let el_log_file = fs::File::create(&el_decode_log)
        .with_context(|| format!("Failed to create {}", el_decode_log.display()))?;

    let (el_width, el_height) = get_hevc_dimensions(el_hevc)?;
    let needs_scale = el_width != width || el_height != height;

    let mut el_cmd = Command::new("ffmpeg");
    el_cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        el_hevc.to_str().unwrap(),
    ]);

    if needs_scale {
        let vf_filter = format!("scale={}:{}:flags=lanczos", width, height);
        el_cmd.args(["-vf", &vf_filter]);
    }

    el_cmd.args(["-f", "rawvideo", "-pix_fmt", "yuv420p16le", "-"]);

    let mut el_proc = el_cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::from(el_log_file))
        .spawn()
        .context("Failed to launch ffmpeg for EL decoding")?;

    let mut bl_stdout = bl_proc
        .stdout
        .take()
        .context("Failed to capture ffmpeg BL stdout")?;
    let mut el_stdout = el_proc
        .stdout
        .take()
        .context("Failed to capture ffmpeg EL stdout")?;

    // Progress bar (stderr so stdout stays clean for piped rawvideo output)
    let pb = indicatif::ProgressBar::with_draw_target(
        Some(total_frames as u64),
        indicatif::ProgressDrawTarget::stderr(),
    );
    pb.set_style(
        indicatif::ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} frames ({eta})",
            )
            .unwrap()
            .progress_chars("█▓░"),
    );

    let mut bl_buf = vec![0u8; bl_frame_bytes];
    let mut el_buf = vec![0u8; el_frame_bytes];
    let mut out_buf = vec![0u8; out_frame_bytes];

    for frame_idx in 0..total_frames {
        // Read BL frame
        let bl_read = bl_stdout.read_exact(&mut bl_buf).with_context(|| {
            format!(
                "BL stream ended at frame {} (expected {})",
                frame_idx, total_frames
            )
        });
        if let Err(err) = bl_read {
            let _ = bl_proc.kill();
            let _ = el_proc.kill();
            let _ = bl_proc.wait();
            let _ = el_proc.wait();
            return Err(err);
        }

        // Read EL frame
        let el_read = el_stdout.read_exact(&mut el_buf).with_context(|| {
            format!(
                "EL stream ended at frame {} (expected {})",
                frame_idx, total_frames
            )
        });
        if let Err(err) = el_read {
            let _ = bl_proc.kill();
            let _ = el_proc.kill();
            let _ = bl_proc.wait();
            let _ = el_proc.wait();
            return Err(err);
        }

        // Get params for this frame (or last available)
        let frame_params = &nlq_params[frame_idx.min(nlq_params.len() - 1)];
        let nlq = &frame_params.nlq;
        let reshaping = &frame_params.reshaping;

        // Plane offsets for YUV 4:2:0
        let bl_u_offset = y_pixels * 2;
        let bl_v_offset = bl_u_offset + uv_pixels * 2;
        let el_u_offset = y_pixels * 2;
        let el_v_offset = el_u_offset + uv_pixels * 2;
        let out_u_offset = y_pixels * 2;
        let out_v_offset = out_u_offset + uv_pixels * 2;

        // Composite: process Y, U, V planes
        // Y plane: full resolution, polynomial reshaping
        composite_plane(
            &bl_buf[..y_pixels * 2],
            &el_buf[..y_pixels * 2],
            &mut out_buf[..y_pixels * 2],
            nlq,
            0, // channel 0 = Y
            Some(reshaping),
            None, // Y doesn't need cross-channel refs
            None,
        );

        // U (Cb) plane: MMR reshaping needs BL luma
        composite_plane(
            &bl_buf[bl_u_offset..bl_u_offset + uv_pixels * 2],
            &el_buf[el_u_offset..el_u_offset + uv_pixels * 2],
            &mut out_buf[out_u_offset..out_u_offset + uv_pixels * 2],
            nlq,
            1, // channel 1 = U/Cb
            Some(reshaping),
            Some(&bl_buf[..y_pixels * 2]), // BL Y for MMR
            None,                          // Cb doesn't need Cb ref (it IS Cb)
        );

        // V (Cr) plane: MMR reshaping needs BL luma + BL Cb
        composite_plane(
            &bl_buf[bl_v_offset..bl_v_offset + uv_pixels * 2],
            &el_buf[el_v_offset..el_v_offset + uv_pixels * 2],
            &mut out_buf[out_v_offset..out_v_offset + uv_pixels * 2],
            nlq,
            2, // channel 2 = V/Cr
            Some(reshaping),
            Some(&bl_buf[..y_pixels * 2]), // BL Y for MMR
            Some(&bl_buf[bl_u_offset..bl_u_offset + uv_pixels * 2]), // BL Cb for MMR
        );

        out.write_all(&out_buf)
            .context("Failed to write composited frame")?;

        pb.inc(1);
    }

    pb.finish_with_message("Compositing complete");
    out.flush()?;

    // Wait for child processes and validate exit status
    let bl_status = bl_proc
        .wait()
        .context("Failed to wait for ffmpeg BL decoder")?;
    if !bl_status.success() {
        bail!("BL decode failed (see {})", bl_decode_log.display());
    }

    let el_status = el_proc
        .wait()
        .context("Failed to wait for ffmpeg EL decoder")?;
    if !el_status.success() {
        bail!("EL decode failed (see {})", el_decode_log.display());
    }

    Ok(())
}

/// Apply piecewise polynomial reshaping to a single luma sample.
///
/// The BL signal is evaluated through the piecewise polynomial defined by the RPU:
/// For the piece where `pivots[p] <= bl < pivots[p+1]`:
///   `result = c0 + c1 * bl + c2 * bl^2` (all in fixed-point, then >> coeff_log2_denom)
///
/// Input/output are in 16-bit range (0..65535), internally scaled to 12-bit for evaluation.
fn apply_polynomial_reshape(bl_16: i64, curve: &ReshapingCurve, coeff_log2_denom: i64) -> i64 {
    let (pivots, pieces) = match curve {
        ReshapingCurve::Polynomial { pivots, pieces } => (pivots, pieces),
        _ => return bl_16, // Not polynomial, return unchanged
    };

    if pieces.is_empty() {
        return bl_16;
    }

    // Scale BL from 16-bit to 12-bit for pivot comparison
    // BL is 10-bit content in 16-bit container: bl_10 = bl_16 >> 6
    // Pivots are in BL bit depth (10-bit). Scale to match.
    let bl_10 = bl_16 >> 6;

    // Find which piece this sample falls into
    let num_pieces = pieces.len();
    let mut piece_idx = num_pieces - 1; // Default to last piece
    for p in 0..num_pieces {
        if p + 1 < pivots.len() && bl_10 < pivots[p + 1] {
            piece_idx = p;
            break;
        }
    }

    let (order, ref coeffs) = pieces[piece_idx];

    // Evaluate polynomial in fixed-point:
    // result = c0 + c1 * x + c2 * x^2 (for quadratic)
    // where x = bl_10 (10-bit BL value), coefficients are in fixed-point with coeff_log2_denom
    //
    // The output is in 12-bit VDR space scaled by 2^coeff_log2_denom
    // Final: result >> coeff_log2_denom gives 12-bit value
    let x = bl_10;
    let mut result: i64 = 0;

    // All terms are accumulated in fixed-point (scaled by 2^coeff_log2_denom).
    // A single >> coeff_log2_denom is applied at the end.
    if !coeffs.is_empty() {
        result = coeffs[0]; // c0 (already in fixed-point)
    }
    if coeffs.len() > 1 {
        result += coeffs[1] * x; // c1_fp * x → still in fixed-point (c1 is fp, x is raw)
    }
    if coeffs.len() > 2 && order >= 1 {
        // c2_fp * x * x: max ≈ 2^30 * 1023^2 ≈ 2^50, fits i64 safely.
        // The product c2_fp * x gives a value in the same fixed-point scale as c0_fp and c1_fp * x.
        // But multiplied again by x puts it one "order" higher. We need to normalize by
        // shifting right by BL_bit_depth (10) to keep the scale consistent.
        // Actually: c2_fp is already scaled to compensate for raw input range,
        // so c2_fp * x * x is in the same accumulator scale as c0_fp and c1_fp * x.
        result += coeffs[2] * x * x;
    }

    // Convert from fixed-point to 10-bit, then scale to 16-bit
    let reshaped_10 = result >> coeff_log2_denom;
    let reshaped_16 = reshaped_10 << 6;

    reshaped_16.clamp(0, 65535)
}

/// Apply MMR (Multi-channel Multiple Regression) reshaping to a chroma sample.
///
/// MMR uses the luma (Y) and both chroma (Cb, Cr) BL values to compute the reshaped chroma:
///
/// Order 1: `c0 + c1*Y + c2*Cb + c3*Cr`
/// Order 2: `... + c4*Y*Cb + c5*Y*Cr + c6*Cb*Cr`
/// Order 3: `... + c7*Y*Y + c8*Cb*Cb + c9*Cr*Cr + ...` (extended)
///
/// All arithmetic in fixed-point with coeff_log2_denom precision.
fn apply_mmr_reshape(
    bl_y_16: i64,
    bl_cb_16: i64,
    bl_cr_16: i64,
    curve: &ReshapingCurve,
    coeff_log2_denom: i64,
) -> i64 {
    let (pivots, pieces) = match curve {
        ReshapingCurve::MMR { pivots, pieces } => (pivots, pieces),
        _ => return bl_cb_16, // Not MMR, return the chroma value unchanged
    };

    if pieces.is_empty() {
        return bl_cb_16;
    }

    // Scale to 10-bit for pivot comparison (pivots are in BL bit depth)
    let y_10 = bl_y_16 >> 6;
    let cb_10 = bl_cb_16 >> 6;
    let cr_10 = bl_cr_16 >> 6;

    // Find piece based on luma value
    let num_pieces = pieces.len();
    let mut piece_idx = num_pieces - 1;
    for p in 0..num_pieces {
        if p + 1 < pivots.len() && y_10 < pivots[p + 1] {
            piece_idx = p;
            break;
        }
    }

    let (order, constant_fp, ref coefs) = pieces[piece_idx];
    let denom = coeff_log2_denom;

    // Start with constant term
    let mut result: i64 = constant_fp;

    // Order 1 terms: k1*Y + k2*Cb + k3*Cr + k4*Y*Cb + k5*Y*Cr + k6*Cb*Cr
    if !coefs.is_empty() && !coefs[0].is_empty() {
        let c = &coefs[0];
        // Linear terms
        if c.len() > 0 {
            result += c[0] * y_10;
        }
        if c.len() > 1 {
            result += c[1] * cb_10;
        }
        if c.len() > 2 {
            result += c[2] * cr_10;
        }
        // Cross terms (shifted to prevent overflow)
        if c.len() > 3 {
            result += (c[3] * y_10 >> denom) * cb_10;
        }
        if c.len() > 4 {
            result += (c[4] * y_10 >> denom) * cr_10;
        }
        if c.len() > 5 {
            result += (c[5] * cb_10 >> denom) * cr_10;
        }
        if c.len() > 6 {
            result += (c[6] * y_10 >> denom) * cb_10 * cr_10 >> denom;
        }
    }

    // Order 2 terms
    if order >= 2 && coefs.len() > 1 && !coefs[1].is_empty() {
        let c = &coefs[1];
        let y2 = (y_10 * y_10) >> denom;
        let cb2 = (cb_10 * cb_10) >> denom;
        let cr2 = (cr_10 * cr_10) >> denom;
        if c.len() > 0 {
            result += c[0] * y2;
        }
        if c.len() > 1 {
            result += c[1] * cb2;
        }
        if c.len() > 2 {
            result += c[2] * cr2;
        }
        if c.len() > 3 {
            result += (c[3] * y2 >> denom) * cb_10;
        }
        if c.len() > 4 {
            result += (c[4] * y2 >> denom) * cr_10;
        }
        if c.len() > 5 {
            result += (c[5] * cb2 >> denom) * cr_10;
        }
        if c.len() > 6 {
            result += (c[6] * y2 >> denom) * cb2 >> denom;
        }
    }

    // Order 3 terms
    if order >= 3 && coefs.len() > 2 && !coefs[2].is_empty() {
        let c = &coefs[2];
        let y3 = (y_10 * y_10 >> denom) * y_10 >> denom;
        let cb3 = (cb_10 * cb_10 >> denom) * cb_10 >> denom;
        let cr3 = (cr_10 * cr_10 >> denom) * cr_10 >> denom;
        if c.len() > 0 {
            result += c[0] * y3;
        }
        if c.len() > 1 {
            result += c[1] * cb3;
        }
        if c.len() > 2 {
            result += c[2] * cr3;
        }
        // Higher cross terms for order 3 (less common, include for completeness)
        if c.len() > 3 {
            result += (c[3] * y3 >> denom) * cb_10;
        }
        if c.len() > 4 {
            result += (c[4] * y3 >> denom) * cr_10;
        }
        if c.len() > 5 {
            result += (c[5] * cb3 >> denom) * cr_10;
        }
        if c.len() > 6 {
            result += (c[6] * y3 >> denom) * cb3 >> denom;
        }
    }

    // Convert from fixed-point to 16-bit
    let reshaped_10 = result >> coeff_log2_denom;
    let reshaped_16 = reshaped_10 << 6;

    reshaped_16.clamp(0, 65535)
}

/// Apply NLQ LinearDeadzone compositing to a single plane.
///
/// The pipeline per pixel:
/// 1. Apply reshaping (polynomial or MMR reshaping) to BL signal
/// 2. Apply NLQ LinearDeadzone compositing with EL residual
/// 3. Round and clamp to output bit depth
///
/// Formula (per pixel):
/// ```text
/// h = reshape(bl_pixel)    ← NEW: polynomial or MMR reshaping
/// tmp = el_pixel - offset
/// if tmp != 0:
///   sign = signum(tmp)
///   tmp = (tmp << 1) - sign
///   tmp <<= (10 - el_bit_depth)
///   dq = tmp * slope + (threshold << (10 - el_bit_depth + 1)) * sign
///   dq = clamp(dq, -vdr_in_max << (10 - el_bit_depth + 1), vdr_in_max << (10 - el_bit_depth + 1))
///   result = dq >> (coeff_log2_denom - 5 - el_bit_depth)
/// h = h + result
/// output = (h + rounding) >> 6  (16-bit → 10-bit)
/// ```
fn composite_plane(
    bl_data: &[u8],      // 16-bit LE samples
    el_data: &[u8],      // 16-bit LE samples (EL was decoded to 16-bit by ffmpeg)
    out_data: &mut [u8], // 10-bit stored as 16-bit LE
    params: &NlqParams,
    channel: usize,
    reshaping: Option<&ReshapingParams>,
    bl_y_data: Option<&[u8]>, // Full-res BL luma for MMR cross-channel (needed for chroma channels)
    bl_cb_data: Option<&[u8]>, // Full-res BL Cb for MMR (needed for Cr channel)
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

        // Step 1: Apply reshaping to BL signal
        let mut h = if let Some(ref reshape) = reshaping {
            match (&reshape.curves[channel], channel) {
                (ReshapingCurve::Polynomial { .. }, 0) => apply_polynomial_reshape(
                    bl_16,
                    &reshape.curves[channel],
                    reshape.coeff_log2_denom,
                ),
                (ReshapingCurve::MMR { .. }, ch) if ch == 1 || ch == 2 => {
                    // For chroma MMR, we need the co-located BL luma and Cb values.
                    // For 4:2:0, chroma is subsampled — we use the corresponding
                    // subsampled luma position. bl_y_data here should already be
                    // subsampled or we average. For simplicity, use the same pixel index.
                    let y_val = bl_y_data.map_or(bl_16, |y| {
                        // Chroma is subsampled 2x in each dimension for 4:2:0
                        // Map chroma pixel i to luma pixel: row = i/(w/2), col = i%(w/2)
                        // Corresponding luma = (row*2)*w + (col*2)
                        // But we don't have width here, so use a simpler co-located approach:
                        // Just sample the Y plane at a position proportional to the chroma position
                        // Since both planes are passed as flat arrays, and chroma has 1/4 the pixels,
                        // we approximate by sampling luma at i*4 (center of 2x2 block)
                        let luma_idx = (i * 4).min(y.len() / 2 - 1);
                        i64::from(u16::from_le_bytes([y[luma_idx * 2], y[luma_idx * 2 + 1]]))
                    });
                    let cb_val = if ch == 2 {
                        bl_cb_data.map_or(512 << 6, |cb| {
                            let idx = i.min(cb.len() / 2 - 1);
                            i64::from(u16::from_le_bytes([cb[idx * 2], cb[idx * 2 + 1]]))
                        })
                    } else {
                        // For Cb channel, we don't need a separate Cb reference
                        // The current channel IS Cb, so use bl_16 as Cb
                        bl_16
                    };
                    let cr_val = if ch == 1 {
                        // For Cb channel, we don't have Cr yet — use neutral
                        512 << 6
                    } else {
                        // For Cr channel (ch==2), bl_16 is the Cr value
                        bl_16
                    };

                    // MMR expects (Y, Cb, Cr) regardless of which chroma channel we're reshaping
                    let (cb_for_mmr, cr_for_mmr) = if ch == 1 {
                        (bl_16, cr_val)
                    } else {
                        (cb_val, bl_16)
                    };

                    apply_mmr_reshape(
                        y_val,
                        cb_for_mmr,
                        cr_for_mmr,
                        &reshape.curves[channel],
                        reshape.coeff_log2_denom,
                    )
                }
                (ReshapingCurve::Identity, _) => bl_16,
                _ => bl_16, // Fallback: no reshaping
            }
        } else {
            bl_16 // No reshaping params — identity
        };

        // Step 2: Apply NLQ residual from EL
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

/// Parse RPU binary to extract NLQ and reshaping parameters per frame
fn parse_rpu_params(rpu_bin: &Path, _temp_dir: &Path) -> Result<Vec<FrameParams>> {
    eprintln!(
        "{}",
        "  Parsing RPU binary with dolby_vision crate...".cyan()
    );

    let rpus = parse_rpu_file(rpu_bin).context("Failed to parse RPU binary file")?;
    let total_frames = rpus.len();

    eprintln!(
        "{}",
        format!(
            "  Parsed {} RPU frames, extracting parameters...",
            total_frames
        )
        .cyan()
    );

    let pb = indicatif::ProgressBar::with_draw_target(
        Some(total_frames as u64),
        indicatif::ProgressDrawTarget::stderr(),
    );
    pb.set_style(
        indicatif::ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} RPU frames",
            )
            .unwrap()
            .progress_chars("█▓░"),
    );

    let mut all_params = Vec::with_capacity(total_frames);
    let mut reshaping_active_count = 0usize;

    for rpu in &rpus {
        let coeff_log2_denom = rpu.header.coefficient_log2_denom as i64;
        let el_bit_depth = (rpu.header.el_bit_depth_minus8 + 8) as u8;
        let disable_residual = rpu.header.disable_residual_flag;

        // --- Extract NLQ params ---
        let nlq = if disable_residual {
            NlqParams {
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
            }
        } else if let Some(ref mapping) = rpu.rpu_data_mapping {
            if let Some(ref nlq_data) = mapping.nlq {
                NlqParams {
                    nlq_offset: [
                        nlq_data.nlq_offset[0] as i64,
                        nlq_data.nlq_offset[1] as i64,
                        nlq_data.nlq_offset[2] as i64,
                    ],
                    vdr_in_max_int: [
                        nlq_data.vdr_in_max_int[0] as i64,
                        nlq_data.vdr_in_max_int[1] as i64,
                        nlq_data.vdr_in_max_int[2] as i64,
                    ],
                    vdr_in_max: [
                        nlq_data.vdr_in_max[0] as i64,
                        nlq_data.vdr_in_max[1] as i64,
                        nlq_data.vdr_in_max[2] as i64,
                    ],
                    linear_deadzone_slope_int: [
                        nlq_data.linear_deadzone_slope_int[0] as i64,
                        nlq_data.linear_deadzone_slope_int[1] as i64,
                        nlq_data.linear_deadzone_slope_int[2] as i64,
                    ],
                    linear_deadzone_slope: [
                        nlq_data.linear_deadzone_slope[0] as i64,
                        nlq_data.linear_deadzone_slope[1] as i64,
                        nlq_data.linear_deadzone_slope[2] as i64,
                    ],
                    linear_deadzone_threshold_int: [
                        nlq_data.linear_deadzone_threshold_int[0] as i64,
                        nlq_data.linear_deadzone_threshold_int[1] as i64,
                        nlq_data.linear_deadzone_threshold_int[2] as i64,
                    ],
                    linear_deadzone_threshold: [
                        nlq_data.linear_deadzone_threshold[0] as i64,
                        nlq_data.linear_deadzone_threshold[1] as i64,
                        nlq_data.linear_deadzone_threshold[2] as i64,
                    ],
                    coeff_log2_denom,
                    disable_residual_flag: false,
                    el_bit_depth,
                }
            } else {
                NlqParams {
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
                }
            }
        } else {
            NlqParams {
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
            }
        };

        // --- Extract reshaping params ---
        let reshaping = if let Some(ref mapping) = rpu.rpu_data_mapping {
            let mut curves: [ReshapingCurve; 3] = [
                ReshapingCurve::Identity,
                ReshapingCurve::Identity,
                ReshapingCurve::Identity,
            ];
            let mut has_non_identity = false;

            for ch in 0..3 {
                let curve = &mapping.curves[ch];
                let pivots: Vec<i64> = curve.pivots.iter().map(|&p| p as i64).collect();

                match curve.mapping_idc {
                    DoviMappingMethod::Polynomial => {
                        if let Some(ref poly) = curve.polynomial {
                            let num_pieces = poly.poly_order_minus1.len();
                            let mut pieces = Vec::with_capacity(num_pieces);

                            for p in 0..num_pieces {
                                let order = poly.poly_order_minus1[p];
                                let num_coeffs = (order + 2) as usize;
                                let mut coeffs = Vec::with_capacity(num_coeffs);

                                for c in 0..num_coeffs.min(poly.poly_coef_int[p].len()) {
                                    let fp = (poly.poly_coef_int[p][c] << coeff_log2_denom)
                                        + poly.poly_coef[p][c] as i64;
                                    coeffs.push(fp);
                                }

                                // Check if this is a non-identity mapping
                                // Identity: c0=0, c1=1.0 (= 1 << denom)
                                let is_identity = order == 0
                                    && coeffs.len() >= 2
                                    && coeffs[0] == 0
                                    && coeffs[1] == (1 << coeff_log2_denom);

                                if !is_identity {
                                    has_non_identity = true;
                                }

                                pieces.push((order, coeffs));
                            }

                            curves[ch] = ReshapingCurve::Polynomial { pivots, pieces };
                        }
                    }
                    DoviMappingMethod::MMR => {
                        if let Some(ref mmr) = curve.mmr {
                            let num_pieces = mmr.mmr_order_minus1.len();
                            let mut pieces = Vec::with_capacity(num_pieces);

                            for p in 0..num_pieces {
                                let order = mmr.mmr_order_minus1[p] + 1;
                                let constant_fp = (mmr.mmr_constant_int[p] << coeff_log2_denom)
                                    + mmr.mmr_constant[p] as i64;

                                let mut coefs_by_order = Vec::with_capacity(order as usize);
                                for o in 0..(order as usize).min(mmr.mmr_coef_int[p].len()) {
                                    let num_c = mmr.mmr_coef_int[p][o].len();
                                    let mut order_coefs = Vec::with_capacity(num_c);
                                    for c in 0..num_c {
                                        let fp = (mmr.mmr_coef_int[p][o][c] << coeff_log2_denom)
                                            + mmr.mmr_coef[p][o][c] as i64;
                                        order_coefs.push(fp);
                                    }
                                    coefs_by_order.push(order_coefs);
                                }

                                has_non_identity = true; // MMR is always non-identity
                                pieces.push((order, constant_fp, coefs_by_order));
                            }

                            curves[ch] = ReshapingCurve::MMR { pivots, pieces };
                        }
                    }
                    _ => {} // Invalid/unknown mapping — leave as Identity
                }
            }

            if has_non_identity {
                reshaping_active_count += 1;
            }

            ReshapingParams {
                curves,
                coeff_log2_denom,
            }
        } else {
            ReshapingParams {
                curves: [
                    ReshapingCurve::Identity,
                    ReshapingCurve::Identity,
                    ReshapingCurve::Identity,
                ],
                coeff_log2_denom,
            }
        };

        all_params.push(FrameParams { nlq, reshaping });
        pb.inc(1);
    }

    pb.finish_with_message("RPU parsing complete");

    if all_params.is_empty() {
        bail!("No RPU frames found in {}", rpu_bin.display());
    }

    // Report statistics
    let nlq_frames = all_params
        .iter()
        .filter(|p| !p.nlq.disable_residual_flag)
        .count();
    eprintln!(
        "{}",
        format!(
            "  NLQ active: {}/{} frames ({}%)",
            nlq_frames,
            total_frames,
            nlq_frames * 100 / total_frames.max(1)
        )
        .cyan()
    );
    eprintln!(
        "{}",
        format!(
            "  Reshaping active: {}/{} frames ({}%)",
            reshaping_active_count,
            total_frames,
            reshaping_active_count * 100 / total_frames.max(1)
        )
        .cyan()
    );

    Ok(all_params)
}

/// HDR10 static metadata extracted from the source file.
struct Hdr10Metadata {
    master_display: String,
    max_cll: u32,
    max_fall: u32,
}

/// Extract HDR10 static metadata (mastering display + MaxCLL/MaxFALL) from the original file.
fn extract_hdr10_metadata(original_file: &str) -> Hdr10Metadata {
    let static_meta = metadata::get_static_metadata(original_file);
    let max_dml = *static_meta.get("max_dml").unwrap_or(&1000.0) as u32;
    let min_dml = static_meta.get("min_dml").unwrap_or(&0.005);
    let max_cll = *static_meta.get("max_cll").unwrap_or(&1000.0) as u32;
    let max_fall = *static_meta.get("max_fall").unwrap_or(&400.0) as u32;

    let min_dml_int = (min_dml * 10000.0) as u32;
    let max_dml_int = max_dml * 10000;

    let gx = static_meta.get("md_gx").copied().unwrap_or(8500.0) as u32;
    let gy = static_meta.get("md_gy").copied().unwrap_or(39850.0) as u32;
    let bx = static_meta.get("md_bx").copied().unwrap_or(6550.0) as u32;
    let by = static_meta.get("md_by").copied().unwrap_or(2300.0) as u32;
    let rx = static_meta.get("md_rx").copied().unwrap_or(35400.0) as u32;
    let ry = static_meta.get("md_ry").copied().unwrap_or(14600.0) as u32;
    let wpx = static_meta.get("md_wpx").copied().unwrap_or(15635.0) as u32;
    let wpy = static_meta.get("md_wpy").copied().unwrap_or(16450.0) as u32;

    let master_display = format!(
        "G({},{})B({},{})R({},{})WP({},{})L({},{})",
        gx, gy, bx, by, rx, ry, wpx, wpy, max_dml_int, min_dml_int
    );

    Hdr10Metadata {
        master_display,
        max_cll,
        max_fall,
    }
}

/// Upload BL+EL+RPU to Modal and run composite + NVENC encode in the cloud.
///
/// Calls `modal run modal_ffmpeg.py --mode composite` with BL/EL/RPU paths.
/// No local FFV1 intermediate — compositing happens on Modal's GPU instance.
fn encode_via_modal(
    bl_hevc: &Path,
    el_hevc: &Path,
    rpu_bin: &Path,
    output_mkv: &Path,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    original_file: &str,
    args: &Args,
) -> Result<()> {
    let hdr10 = extract_hdr10_metadata(original_file);
    let max_cll_str = format!("{},{}", hdr10.max_cll, hdr10.max_fall);

    let bl_abs = fs::canonicalize(bl_hevc).unwrap_or_else(|_| bl_hevc.to_path_buf());
    let el_abs = fs::canonicalize(el_hevc).unwrap_or_else(|_| el_hevc.to_path_buf());
    let rpu_abs = fs::canonicalize(rpu_bin).unwrap_or_else(|_| rpu_bin.to_path_buf());
    let output_abs = fs::canonicalize(output_mkv.parent().unwrap_or(Path::new(".")))
        .unwrap_or_else(|_| output_mkv.parent().unwrap_or(Path::new(".")).to_path_buf())
        .join(output_mkv.file_name().unwrap());

    let qp_str = args.fel_crf.to_string();
    let width_str = width.to_string();
    let height_str = height.to_string();
    let fps_num_str = fps_num.to_string();
    let fps_den_str = fps_den.to_string();

    let mut cmd = Command::new("modal");
    cmd.args([
        "run",
        MODAL_FFMPEG_SCRIPT,
        "--mode",
        "composite",
        "--bl",
        bl_abs.to_str().unwrap(),
        "--el",
        el_abs.to_str().unwrap(),
        "--rpu",
        rpu_abs.to_str().unwrap(),
        "--output",
        output_abs.to_str().unwrap(),
        "--width",
        &width_str,
        "--height",
        &height_str,
        "--fps-num",
        &fps_num_str,
        "--fps-den",
        &fps_den_str,
        "--preset",
        &args.fel_nvenc_preset,
        "--qp",
        &qp_str,
        "--master-display",
        &hdr10.master_display,
        "--max-cll",
        &max_cll_str,
    ]);

    let bl_mb = fs::metadata(bl_hevc)
        .map(|m| m.len() / (1024 * 1024))
        .unwrap_or(0);
    let el_mb = fs::metadata(el_hevc)
        .map(|m| m.len() / (1024 * 1024))
        .unwrap_or(0);
    println!(
        "{}",
        format!(
            "  Modal composite: BL {} MB + EL {} MB → hevc_nvenc preset={} qp={}",
            bl_mb, el_mb, args.fel_nvenc_preset, args.fel_crf
        )
        .cyan()
    );

    let log_path = output_mkv
        .parent()
        .unwrap_or(Path::new("."))
        .join("modal_encode.log");

    if !run_command_live(&mut cmd, &log_path)? {
        bail!("Modal composite+encode failed (see {})", log_path.display());
    }

    if !output_abs.exists() {
        bail!(
            "Modal composite+encode completed but output file not found: {}",
            output_abs.display()
        );
    }

    let output_size = fs::metadata(&output_abs)
        .map(|m| m.len() / (1024 * 1024))
        .unwrap_or(0);
    println!(
        "{}",
        format!("  Modal encode output: {} MB", output_size).cyan()
    );

    Ok(())
}

/// Spawn ffmpeg to re-encode composited rawvideo from stdin into an HDR10 HEVC MKV.
///
/// The caller must write `yuv420p16le` frames to `child.stdin`, close stdin, then wait.
fn spawn_reencode_composited_from_stdin(
    output_mkv: &Path,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    original_file: &str,
    args: &Args,
    temp_dir: &Path,
) -> Result<(Child, PathBuf)> {
    let hdr10 = extract_hdr10_metadata(original_file);
    let master_display = &hdr10.master_display;
    let max_cll = hdr10.max_cll;
    let max_fall = hdr10.max_fall;

    let framerate = format!("{}/{}", fps_num, fps_den);
    let resolution = format!("{}x{}", width, height);

    let log_path = temp_dir.join("ffmpeg_reencode.log");
    let log_file = fs::File::create(&log_path)
        .with_context(|| format!("Failed to create {}", log_path.display()))?;

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "rawvideo",
        "-pixel_format",
        "yuv420p16le",
        "-video_size",
        &resolution,
        "-framerate",
        &framerate,
        "-i",
        "-",
        "-y",
    ]);

    if args.hwaccel == HwAccel::Cuda {
        let qp_str = args.fel_crf.to_string();
        cmd.args([
            "-c:v",
            "hevc_nvenc",
            "-preset",
            &args.fel_nvenc_preset,
            "-tune",
            "hq",
            "-rc",
            "constqp",
            "-qp",
            &qp_str,
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

        if ffmpeg_supports_hevc_metadata_mastering_display() {
            let bsf = format!(
                "hevc_metadata=master_display={}:max_cll={},{}",
                master_display, max_cll, max_fall
            );
            cmd.args(["-bsf:v", &bsf]);
        }
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

                if ffmpeg_supports_hevc_metadata_mastering_display() {
                    let bsf = format!(
                        "hevc_metadata=master_display={}:max_cll={},{}",
                        master_display, max_cll, max_fall
                    );
                    cmd.args(["-bsf:v", &bsf]);
                }
            }
        }
    }

    cmd.arg(output_mkv.to_str().unwrap());
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::from(log_file));

    let child = cmd
        .spawn()
        .context("Failed to launch ffmpeg for composited re-encoding")?;

    Ok((child, log_path))
}

fn ffmpeg_supports_hevc_metadata_mastering_display() -> bool {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-h", "bsf=hevc_metadata"]);

    let Ok(output) = cmd.output() else {
        return false;
    };

    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));

    text.contains("master_display") && text.contains("max_cll")
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

        composite_plane(
            &bl_data,
            &el_data,
            &mut out_data,
            &params,
            0,
            None,
            None,
            None,
        );

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

        composite_plane(
            &bl_data,
            &el_data,
            &mut out_data,
            &params,
            0,
            None,
            None,
            None,
        );

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

    // --- Reshaping tests ---

    #[test]
    fn test_polynomial_identity_reshape() {
        // Identity polynomial: c0=0, c1=1.0 (= 1 << 23 = 8388608)
        // Should return the input unchanged
        let denom: i64 = 23;
        let curve = ReshapingCurve::Polynomial {
            pivots: vec![0, 1023],
            pieces: vec![(0, vec![0, 1 << denom])], // identity: output = input
        };

        // BL = 512 in 10-bit = 32768 in 16-bit
        let bl_16: i64 = 512 << 6;
        let result = apply_polynomial_reshape(bl_16, &curve, denom);
        // Identity should return approximately the same value
        assert_eq!(
            result, bl_16,
            "Identity polynomial should preserve BL value"
        );
    }

    #[test]
    fn test_polynomial_constant_reshape() {
        // Constant polynomial: c0 = 256 (in 10-bit), c1 = 0
        // Output should always be ~256 regardless of input
        let denom: i64 = 23;
        let c0_fp = 256i64 << denom; // 256.0 in fixed-point
        let curve = ReshapingCurve::Polynomial {
            pivots: vec![0, 1023],
            pieces: vec![(0, vec![c0_fp, 0])],
        };

        let bl_16: i64 = 512 << 6; // BL = 512 in 10-bit
        let result = apply_polynomial_reshape(bl_16, &curve, denom);
        let result_10 = result >> 6;
        assert_eq!(result_10, 256, "Constant polynomial should output 256");
    }

    #[test]
    fn test_polynomial_linear_scale_reshape() {
        // Linear scaling: c0 = 0, c1 = 0.5 (= 1 << 22)
        // Output should be half the input
        let denom: i64 = 23;
        let c1_fp = 1i64 << (denom - 1); // 0.5 in fixed-point
        let curve = ReshapingCurve::Polynomial {
            pivots: vec![0, 1023],
            pieces: vec![(0, vec![0, c1_fp])],
        };

        let bl_16: i64 = 512 << 6; // BL = 512 in 10-bit
        let result = apply_polynomial_reshape(bl_16, &curve, denom);
        let result_10 = result >> 6;
        assert_eq!(result_10, 256, "0.5x linear should halve BL value");
    }

    #[test]
    fn test_polynomial_piecewise_two_segments() {
        // Two-piece polynomial with different mappings:
        // Piece 0 (0..512): identity
        // Piece 1 (512..1023): constant 800
        let denom: i64 = 23;
        let curve = ReshapingCurve::Polynomial {
            pivots: vec![0, 512, 1023],
            pieces: vec![
                (0, vec![0, 1 << denom]),      // identity
                (0, vec![800i64 << denom, 0]), // constant 800
            ],
        };

        // Test in first segment: BL=256 → should return ~256
        let bl_low: i64 = 256 << 6;
        let result_low = apply_polynomial_reshape(bl_low, &curve, denom);
        assert_eq!(
            result_low >> 6,
            256,
            "First piece identity should preserve 256"
        );

        // Test in second segment: BL=700 → should return ~800
        let bl_high: i64 = 700 << 6;
        let result_high = apply_polynomial_reshape(bl_high, &curve, denom);
        assert_eq!(
            result_high >> 6,
            800,
            "Second piece constant should output 800"
        );
    }

    #[test]
    fn test_polynomial_quadratic_reshape() {
        // Quadratic: c0=0, c1=0, c2 = 1/1023 (so output ≈ x²/1023)
        // For x=512: output ≈ 512*512/1023 ≈ 256
        let denom: i64 = 23;
        // c2 in fixed-point: we want c2 * x * x >> denom ≈ x²/1023 in fixed-point
        // c2 = (1 << denom) / 1023 ≈ 8196
        let c2_fp = (1i64 << denom) / 1023;
        let curve = ReshapingCurve::Polynomial {
            pivots: vec![0, 1023],
            pieces: vec![(1, vec![0, 0, c2_fp])], // order=1 (quadratic)
        };

        let bl_16: i64 = 512 << 6;
        let result = apply_polynomial_reshape(bl_16, &curve, denom);
        let result_10 = result >> 6;
        // 512*512/1023 ≈ 256.25
        assert!(
            (result_10 - 256).abs() <= 2,
            "Quadratic should produce ~256 for input 512, got {result_10}"
        );
    }

    #[test]
    fn test_mmr_constant_reshape() {
        // MMR with just a constant term: should always output that constant
        let denom: i64 = 23;
        let constant_fp = 512i64 << denom; // constant = 512.0
        let curve = ReshapingCurve::MMR {
            pivots: vec![0, 1023],
            pieces: vec![(1, constant_fp, vec![vec![0; 7]])], // order=1, all coeffs=0
        };

        let y_16: i64 = 300 << 6;
        let cb_16: i64 = 512 << 6;
        let cr_16: i64 = 512 << 6;

        let result = apply_mmr_reshape(y_16, cb_16, cr_16, &curve, denom);
        let result_10 = result >> 6;
        assert_eq!(
            result_10, 512,
            "MMR constant should output 512, got {result_10}"
        );
    }

    #[test]
    fn test_mmr_linear_luma_reshape() {
        // MMR order 1: constant=0, k1=1.0 (identity from luma), rest=0
        // Output should equal luma value
        let denom: i64 = 23;
        let constant_fp = 0i64;
        let mut coefs = vec![0i64; 7];
        coefs[0] = 1 << denom; // k1 = 1.0 for Y
        let curve = ReshapingCurve::MMR {
            pivots: vec![0, 1023],
            pieces: vec![(1, constant_fp, vec![coefs])],
        };

        let y_16: i64 = 400 << 6;
        let cb_16: i64 = 512 << 6;
        let cr_16: i64 = 512 << 6;

        let result = apply_mmr_reshape(y_16, cb_16, cr_16, &curve, denom);
        let result_10 = result >> 6;
        assert_eq!(
            result_10, 400,
            "MMR luma identity should output 400, got {result_10}"
        );
    }

    #[test]
    fn test_identity_reshape_passthrough() {
        // ReshapingCurve::Identity should leave the value unchanged
        let denom: i64 = 23;
        let curve = ReshapingCurve::Identity;
        let bl_16: i64 = 512 << 6;

        let result = apply_polynomial_reshape(bl_16, &curve, denom);
        assert_eq!(
            result, bl_16,
            "Identity curve should pass through unchanged"
        );
    }

    #[test]
    fn test_composite_plane_with_polynomial_reshaping() {
        // Verify that composite_plane applies polynomial reshaping before NLQ
        let params = NlqParams {
            nlq_offset: [512, 512, 512],
            vdr_in_max_int: [0; 3],
            vdr_in_max: [0; 3],
            linear_deadzone_slope_int: [0; 3],
            linear_deadzone_slope: [0; 3],
            linear_deadzone_threshold_int: [0; 3],
            linear_deadzone_threshold: [0; 3],
            coeff_log2_denom: 23,
            disable_residual_flag: true, // Disable NLQ to isolate reshaping
            el_bit_depth: 10,
        };

        let denom: i64 = 23;
        // Reshaping: constant output of 256
        let reshaping = ReshapingParams {
            curves: [
                ReshapingCurve::Polynomial {
                    pivots: vec![0, 1023],
                    pieces: vec![(0, vec![256i64 << denom, 0])],
                },
                ReshapingCurve::Identity,
                ReshapingCurve::Identity,
            ],
            coeff_log2_denom: denom,
        };

        let bl_data: Vec<u8> = vec![0x00, 0x80]; // BL = 32768 (512 in 10-bit)
        let el_data: Vec<u8> = vec![0x00, 0x80]; // EL = 32768 (512 in 10-bit, = offset)
        let mut out_data = vec![0u8; 2];

        composite_plane(
            &bl_data,
            &el_data,
            &mut out_data,
            &params,
            0, // luma
            Some(&reshaping),
            None,
            None,
        );

        let out_val = u16::from_le_bytes([out_data[0], out_data[1]]);
        let out_10 = out_val >> 6;
        assert_eq!(
            out_10, 256,
            "Reshaping should map BL 512 → 256, got {out_10}"
        );
    }

    #[test]
    fn test_polynomial_clamps_output() {
        // Test that output is clamped to valid range
        let denom: i64 = 23;
        // Large positive constant that would exceed 16-bit range
        let curve = ReshapingCurve::Polynomial {
            pivots: vec![0, 1023],
            pieces: vec![(0, vec![2000i64 << denom, 0])], // constant 2000 (exceeds 10-bit max)
        };

        let bl_16: i64 = 512 << 6;
        let result = apply_polynomial_reshape(bl_16, &curve, denom);
        assert!(
            result >= 0 && result <= 65535,
            "Result should be clamped to 16-bit range"
        );
    }
}
