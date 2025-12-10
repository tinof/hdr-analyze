use anyhow::{Context, Result};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{codec, format, media, util::color};
use std::fmt;

/// Video transfer function reported by FFmpeg metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferFunction {
    Pq,
    Hlg,
    Unknown,
}

impl fmt::Display for TransferFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransferFunction::Pq => write!(f, "PQ (SMPTE 2084)"),
            TransferFunction::Hlg => write!(f, "HLG (ARIB STD-B67)"),
            TransferFunction::Unknown => write!(f, "Unspecified"),
        }
    }
}

impl From<color::TransferCharacteristic> for TransferFunction {
    fn from(value: color::TransferCharacteristic) -> Self {
        use color::TransferCharacteristic::*;
        match value {
            SMPTE2084 | BT2020_10 | BT2020_12 => TransferFunction::Pq,
            ARIB_STD_B67 => TransferFunction::Hlg,
            _ => TransferFunction::Unknown,
        }
    }
}

/// Basic metadata about the input video stream needed by the analyzer pipeline.
#[derive(Clone, Copy, Debug)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub total_frames: Option<u32>,
    pub transfer_function: TransferFunction,
}

/// Native video information extraction using ffmpeg-next.
///
/// This function replaces the external ffprobe process with native FFmpeg library calls
/// to extract essential video metadata needed for analysis.
///
/// # Arguments
/// * `input_path` - Path to the input video file
///
/// # Returns
/// `Result<(VideoInfo, format::context::Input)>` - (video metadata, input_context)
pub fn get_native_video_info(input_path: &str) -> Result<(VideoInfo, format::context::Input)> {
    // Initialize FFmpeg
    ffmpeg::init().context("Failed to initialize FFmpeg")?;

    // Open input file
    let input_context = format::input(input_path).context("Failed to open input video file")?;

    // Find the best video stream
    let video_stream = input_context
        .streams()
        .best(media::Type::Video)
        .context("No video stream found in input file")?;

    let decoder_context = codec::context::Context::from_parameters(video_stream.parameters())
        .context("Failed to create decoder context")?;
    let transfer_characteristic =
        unsafe { color::TransferCharacteristic::from((*decoder_context.as_ptr()).color_trc) };
    let decoder = decoder_context
        .decoder()
        .video()
        .context("Failed to create video decoder")?;
    let width = decoder.width();
    let height = decoder.height();

    // Try multiple methods to estimate frame count
    let frame_count = {
        // Method 1: Try to get nb_frames directly from the stream
        let nb_frames = video_stream.frames();
        if nb_frames > 0 {
            Some(nb_frames as u32)
        } else {
            // Method 2: Calculate from stream duration and frame rate
            let stream_duration = video_stream.duration();
            if stream_duration != ffmpeg::ffi::AV_NOPTS_VALUE && stream_duration > 0 {
                let time_base = video_stream.time_base();
                let avg_frame_rate = video_stream.avg_frame_rate();

                if avg_frame_rate.numerator() > 0 && avg_frame_rate.denominator() > 0 {
                    let duration_seconds = (stream_duration as f64) * f64::from(time_base);
                    let fps =
                        avg_frame_rate.numerator() as f64 / avg_frame_rate.denominator() as f64;
                    Some((duration_seconds * fps) as u32)
                } else {
                    None
                }
            } else {
                // Method 3: Calculate from container duration and frame rate
                let container_duration = input_context.duration();
                if container_duration > 0 {
                    let avg_frame_rate = video_stream.avg_frame_rate();
                    if avg_frame_rate.numerator() > 0 && avg_frame_rate.denominator() > 0 {
                        // Duration is in AV_TIME_BASE units (microseconds)
                        let duration_seconds = container_duration as f64 / 1_000_000.0;
                        let fps =
                            avg_frame_rate.numerator() as f64 / avg_frame_rate.denominator() as f64;
                        Some((duration_seconds * fps) as u32)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        }
    };

    let transfer_function = TransferFunction::from(transfer_characteristic);
    let transfer_label = transfer_characteristic
        .name()
        .unwrap_or("unspecified")
        .to_string();

    println!("Native video info: {}x{}", width, height);
    if let Some(frames) = frame_count {
        println!("Estimated frames: {}", frames);
    }
    println!(
        "Transfer function: {} ({})",
        transfer_label, transfer_function
    );
    let info = VideoInfo {
        width,
        height,
        total_frames: frame_count,
        transfer_function,
    };

    Ok((info, input_context))
}

/// Set up hardware-accelerated decoder based on the specified acceleration type.
///
/// # Arguments
/// * `decoder_context` - The decoder context to configure
/// * `hwaccel` - Hardware acceleration type ("cuda", "vaapi", "videotoolbox")
///
/// # Returns
/// `Result<codec::decoder::Video>` - Configured hardware decoder
pub fn setup_hardware_decoder(
    decoder_context: codec::context::Context,
    hwaccel: &str,
) -> Result<codec::decoder::Video> {
    match hwaccel {
        "cuda" => {
            // Try to find CUDA-specific decoder
            if let Some(cuda_decoder) = codec::decoder::find_by_name("hevc_cuvid") {
                let mut context = codec::context::Context::new_with_codec(cuda_decoder);
                // Copy parameters from the original context
                unsafe {
                    (*context.as_mut_ptr()).width = (*decoder_context.as_ptr()).width;
                    (*context.as_mut_ptr()).height = (*decoder_context.as_ptr()).height;
                    (*context.as_mut_ptr()).pix_fmt = (*decoder_context.as_ptr()).pix_fmt;
                }
                context
                    .decoder()
                    .video()
                    .context("Failed to create CUDA hardware decoder")
            } else {
                println!("CUDA decoder not available, falling back to software decoder");
                decoder_context
                    .decoder()
                    .video()
                    .context("Failed to create fallback software decoder")
            }
        }
        "vaapi" | "videotoolbox" => {
            // For VAAPI and VideoToolbox, we'll use software decoding for now
            // as hardware acceleration setup is more complex and requires device contexts
            println!(
                "Hardware acceleration {} requested, using software decoder for now",
                hwaccel
            );
            decoder_context
                .decoder()
                .video()
                .context("Failed to create software decoder")
        }
        _ => {
            println!(
                "Unknown hardware acceleration type '{}', using software decoder",
                hwaccel
            );
            decoder_context
                .decoder()
                .video()
                .context("Failed to create software decoder")
        }
    }
}
