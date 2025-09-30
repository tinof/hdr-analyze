use anyhow::{Context, Result};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{codec, format, media};

/// Native video information extraction using ffmpeg-next.
///
/// This function replaces the external ffprobe process with native FFmpeg library calls
/// to extract essential video metadata needed for analysis.
///
/// # Arguments
/// * `input_path` - Path to the input video file
///
/// # Returns
/// `Result<(u32, u32, Option<u32>, format::context::Input)>` - (width, height, optional_frame_count, input_context)
pub fn get_native_video_info(
    input_path: &str,
) -> Result<(u32, u32, Option<u32>, format::context::Input)> {
    // Initialize FFmpeg
    ffmpeg::init().context("Failed to initialize FFmpeg")?;

    // Open input file
    let input_context = format::input(input_path).context("Failed to open input video file")?;

    // Find the best video stream
    let video_stream = input_context
        .streams()
        .best(media::Type::Video)
        .context("No video stream found in input file")?;

    let video_params = video_stream.parameters();

    // Extract width and height from video parameters
    let (width, height) = match video_params.medium() {
        media::Type::Video => {
            // Get decoder context to access width/height
            let decoder_context = codec::context::Context::from_parameters(video_params)
                .context("Failed to create decoder context")?;
            let decoder = decoder_context
                .decoder()
                .video()
                .context("Failed to create video decoder")?;
            (decoder.width(), decoder.height())
        }
        _ => return Err(anyhow::anyhow!("Stream is not a video stream")),
    };

    // Try to estimate frame count from duration and frame rate
    let frame_count = if video_stream.duration() != ffmpeg::ffi::AV_NOPTS_VALUE {
        let duration = video_stream.duration();
        let time_base = video_stream.time_base();
        let avg_frame_rate = video_stream.avg_frame_rate();

        if avg_frame_rate.numerator() > 0 && avg_frame_rate.denominator() > 0 {
            let duration_seconds = (duration as f64) * f64::from(time_base);
            let fps = avg_frame_rate.numerator() as f64 / avg_frame_rate.denominator() as f64;
            Some((duration_seconds * fps) as u32)
        } else {
            None
        }
    } else {
        None
    };

    println!("Native video info: {}x{}", width, height);
    if let Some(frames) = frame_count {
        println!("Estimated frames: {}", frames);
    }

    Ok((width, height, frame_count, input_context))
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
