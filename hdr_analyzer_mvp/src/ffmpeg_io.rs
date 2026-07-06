use std::fmt;

use anyhow::{anyhow, Context, Result};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{
    codec, format, frame, media, software,
    util::{color, mathematics::rescale},
    Rescale,
};

use crate::crop::{
    detect_crop, is_frame_usable_for_crop, vote_crop_candidates, CropVote, CROP_EDGE_TOLERANCE,
};

const MAX_DECODED_FRAMES_PER_PROBE: usize = 120;

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

fn spread_probe_timestamps(start: i64, duration: i64, count: u32) -> Vec<i64> {
    if count == 0 || duration <= 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![start.saturating_add(duration / 2)];
    }

    let intervals = i128::from(count - 1);
    (0..count)
        .map(|index| {
            let numerator = 15 * intervals + 70 * i128::from(index);
            let offset = i128::from(duration) * numerator / (100 * intervals);
            start.saturating_add(offset as i64)
        })
        .collect()
}

/// Probe crop candidates in a separate seekable input context.
///
/// The returned rectangle uses the same YUV420P10LE/downscaled geometry as the main analysis pass,
/// so it can be committed before that pass contributes any measurements.
pub fn probe_crop(input_path: &str, probe_count: u32, downscale: u32) -> Result<CropVote> {
    let mut input_context =
        format::input(input_path).context("failed to open an independent crop probe input")?;
    let video_stream = input_context
        .streams()
        .best(media::Type::Video)
        .context("no video stream found for crop probing")?;

    let stream_index = video_stream.index();
    let time_base = video_stream.time_base();
    let stream_start = match video_stream.start_time() {
        ffmpeg::ffi::AV_NOPTS_VALUE => 0,
        start => start,
    };
    let stream_duration = video_stream.duration();
    let decoder_context = codec::context::Context::from_parameters(video_stream.parameters())
        .context("failed to create crop probe decoder context")?;

    let duration = if stream_duration != ffmpeg::ffi::AV_NOPTS_VALUE && stream_duration > 0 {
        stream_duration
    } else {
        let container_duration = input_context.duration();
        if container_duration <= 0 {
            return Err(anyhow!("input duration is unavailable"));
        }
        container_duration.rescale(rescale::TIME_BASE, time_base)
    };

    let targets = spread_probe_timestamps(stream_start, duration, probe_count);
    if targets.is_empty() {
        return Err(anyhow!("no crop probe timestamps could be generated"));
    }

    let mut decoder = decoder_context
        .decoder()
        .video()
        .context("failed to open crop probe decoder")?;
    let target_w = if downscale > 1 {
        (decoder.width() / downscale).max(2) & !1
    } else {
        decoder.width()
    };
    let target_h = if downscale > 1 {
        (decoder.height() / downscale).max(2) & !1
    } else {
        decoder.height()
    };
    let need_scaler = decoder.format() != format::Pixel::YUV420P10LE || downscale > 1;
    let mut scaler = if need_scaler {
        Some(
            software::scaling::Context::get(
                decoder.format(),
                decoder.width(),
                decoder.height(),
                format::Pixel::YUV420P10LE,
                target_w,
                target_h,
                software::scaling::Flags::FAST_BILINEAR,
            )
            .context("failed to create crop probe scaling context")?,
        )
    } else {
        None
    };

    let mut candidates = Vec::with_capacity(targets.len());
    let mut seek_failures = 0usize;

    for target in targets {
        let seek_timestamp = target.rescale(time_base, rescale::TIME_BASE);
        if input_context
            .seek(seek_timestamp, ..seek_timestamp)
            .is_err()
        {
            seek_failures += 1;
            continue;
        }
        decoder.flush();

        let mut decoded_frame = frame::Video::empty();
        let mut scaled_frame = frame::Video::empty();
        let mut decoded_after_target = 0usize;
        let mut candidate = None;

        'packets: for (stream, packet) in input_context.packets() {
            if stream.index() != stream_index {
                continue;
            }

            decoder
                .send_packet(&packet)
                .context("failed to send crop probe packet to decoder")?;

            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                if decoded_frame
                    .timestamp()
                    .is_some_and(|timestamp| timestamp < target)
                {
                    continue;
                }

                decoded_after_target += 1;
                let analysis_frame = if let Some(ref mut scaler) = scaler {
                    scaler
                        .run(&decoded_frame, &mut scaled_frame)
                        .context("failed to scale crop probe frame")?;
                    &scaled_frame
                } else {
                    &decoded_frame
                };

                if is_frame_usable_for_crop(analysis_frame) {
                    candidate = Some(detect_crop(analysis_frame));
                    break 'packets;
                }
                if decoded_after_target >= MAX_DECODED_FRAMES_PER_PROBE {
                    break 'packets;
                }
            }
        }

        if let Some(candidate) = candidate {
            candidates.push(candidate);
        }
    }

    vote_crop_candidates(&candidates, CROP_EDGE_TOLERANCE).ok_or_else(|| {
        anyhow!(
            "crop probing found no usable frames ({} seek failures)",
            seek_failures
        )
    })
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
    // SAFETY: decoder_context is valid and as_ptr() returns a non-null pointer.
    // We only read the color_trc field which is a simple integer value.
    // The pointer dereference is safe because Context guarantees the underlying
    // AVCodecContext is valid for the lifetime of the Context object.
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

#[cfg(test)]
mod tests {
    use super::spread_probe_timestamps;

    #[test]
    fn probe_timestamps_span_the_middle_seventy_percent() {
        assert_eq!(
            spread_probe_timestamps(1_000, 10_000, 7),
            vec![2_500, 3_666, 4_833, 6_000, 7_166, 8_333, 9_500]
        );
    }

    #[test]
    fn one_probe_targets_the_midpoint() {
        assert_eq!(spread_probe_timestamps(200, 1_000, 1), vec![700]);
    }
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
                // SAFETY: Both context pointers are valid - context is newly created and
                // decoder_context is passed by value (moved). We copy simple POD fields
                // (width, height, pix_fmt) which are safe integer/enum values.
                // The mutable pointer is valid because we own `context`.
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
