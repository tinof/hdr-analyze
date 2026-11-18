//! The CPU analysis frame of a P010 (NVDEC) frame at downscale 1 holds the decoder's codes
//! (ROADMAP E12.2): `scale_to_yuv420p10` repacks it instead of converting it with swscale.

use ffmpeg_next::format::Pixel;
use ffmpeg_next::{frame, software};

use super::analyze::scale_to_yuv420p10;

/// Luma code of sample (x, y): varies per sample over the whole 10-bit range.
fn luma_code(x: usize, y: usize) -> u16 {
    ((x * 37 + y * 101 + (x * y) % 13) % 1024) as u16
}

/// (Cb, Cr) codes of chroma sample (cx, cy), different from each other and from their
/// neighbours.
fn chroma_codes(cx: usize, cy: usize) -> (u16, u16) {
    (
        ((cx * 53 + cy * 29 + 7) % 1024) as u16,
        ((cx * 11 + cy * 83 + 500) % 1024) as u16,
    )
}

/// Low bits P010 leaves unused, set on some samples: the repack must drop them.
fn low_bits(x: usize, y: usize) -> u16 {
    match (x + 2 * y) % 4 {
        0 => 0x3F,
        1 => 0x15,
        _ => 0,
    }
}

fn put(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn get(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

/// A `width` x `height` P010LE frame of the codes above. With `allocated_width` above `width`,
/// both planes carry padding past the last sample (filled with ones), as a decoder's aligned
/// surface does.
fn p010_frame(width: usize, height: usize, allocated_width: usize) -> frame::Video {
    let mut frame = frame::Video::new(Pixel::P010LE, allocated_width as u32, height as u32);
    for plane in 0..2 {
        frame.data_mut(plane).fill(0xFF);
    }
    frame.set_width(width as u32);
    let stride = frame.stride(0);
    let data = frame.data_mut(0);
    for y in 0..height {
        for x in 0..width {
            put(
                data,
                y * stride + x * 2,
                (luma_code(x, y) << 6) | low_bits(x, y),
            );
        }
    }
    let stride = frame.stride(1);
    let data = frame.data_mut(1);
    for cy in 0..height.div_ceil(2) {
        for cx in 0..width.div_ceil(2) {
            let (cb, cr) = chroma_codes(cx, cy);
            put(data, cy * stride + cx * 4, (cb << 6) | low_bits(cx, cy));
            put(data, cy * stride + cx * 4 + 2, (cr << 6) | low_bits(cy, cx));
        }
    }
    frame
}

/// Asserts that `frame` is the YUV420P10LE frame of the codes above at `width` x `height`.
fn assert_formula_codes(frame: &frame::Video, width: usize, height: usize, label: &str) {
    assert_eq!(frame.format(), Pixel::YUV420P10LE, "{label}: format");
    assert_eq!(
        (frame.width() as usize, frame.height() as usize),
        (width, height),
        "{label}: size"
    );
    let mut wrong = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let code = get(frame.data(0), y * frame.stride(0) + x * 2);
            if code != luma_code(x, y) {
                wrong.push(format!("Y({x},{y}) {code} != {}", luma_code(x, y)));
            }
        }
    }
    for cy in 0..height.div_ceil(2) {
        for cx in 0..width.div_ceil(2) {
            let (cb, cr) = chroma_codes(cx, cy);
            let got_cb = get(frame.data(1), cy * frame.stride(1) + cx * 2);
            let got_cr = get(frame.data(2), cy * frame.stride(2) + cx * 2);
            if (got_cb, got_cr) != (cb, cr) {
                wrong.push(format!(
                    "CbCr({cx},{cy}) ({got_cb},{got_cr}) != ({cb},{cr})"
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{label}: {} samples differ from the decoder's codes, first: {}",
        wrong.len(),
        wrong.iter().take(4).cloned().collect::<Vec<_>>().join("; ")
    );
}

#[test]
fn a_p010_frame_at_downscale_1_keeps_the_decoder_codes() {
    let input = p010_frame(64, 48, 64);
    let mut scaler = None;
    let mut output = frame::Video::empty();
    let repacked = scale_to_yuv420p10(&input, &mut scaler, &mut output, 1).unwrap();
    // The samples first, so swscale's conversion fails here and not at the flags below.
    assert_formula_codes(&output, 64, 48, "64x48");
    assert!(repacked, "the frame must be reported as repacked");
    assert!(scaler.is_none(), "the repack must not create a scaler");
}

#[test]
fn an_odd_sized_p010_frame_with_padded_strides_is_repacked_at_its_size() {
    let input = p010_frame(63, 47, 96);
    assert!(input.stride(0) > 63 * 2 && input.stride(1) > 32 * 4);
    let mut scaler = None;
    let mut output = frame::Video::empty();
    let repacked = scale_to_yuv420p10(&input, &mut scaler, &mut output, 1).unwrap();
    assert_formula_codes(&output, 63, 47, "63x47");
    assert!(repacked);
    assert!(scaler.is_none());
}

/// swscale's YUV420P10LE output for `input` at half size, from a scaler of its own.
fn swscale_half(input: &frame::Video) -> frame::Video {
    let mut scaler = software::scaling::Context::get(
        input.format(),
        input.width(),
        input.height(),
        Pixel::YUV420P10LE,
        input.width() / 2,
        input.height() / 2,
        software::scaling::Flags::FAST_BILINEAR,
    )
    .unwrap();
    let mut output = frame::Video::empty();
    scaler.run(input, &mut output).unwrap();
    output
}

fn planes_equal(a: &frame::Video, b: &frame::Video) -> bool {
    (a.format(), a.width(), a.height()) == (b.format(), b.width(), b.height())
        && (0..3).all(|plane| {
            let row_bytes = a.plane_width(plane) as usize * 2;
            (0..a.plane_height(plane) as usize).all(|row| {
                a.data(plane)[row * a.stride(plane)..][..row_bytes]
                    == b.data(plane)[row * b.stride(plane)..][..row_bytes]
            })
        })
}

#[test]
fn downscale_2_keeps_swscale_around_a_repack_on_the_same_buffers() {
    let input = p010_frame(64, 48, 64);
    let reference = swscale_half(&input);
    let mut scaler = None;
    let mut output = frame::Video::empty();

    assert!(!scale_to_yuv420p10(&input, &mut scaler, &mut output, 2).unwrap());
    assert!(scaler.is_some(), "downscale 2 converts with swscale");
    assert!(
        planes_equal(&output, &reference),
        "first downscale 2 differs"
    );

    assert!(scale_to_yuv420p10(&input, &mut scaler, &mut output, 1).unwrap());
    assert!(scaler.is_none());
    assert_formula_codes(&output, 64, 48, "repack after downscale 2");

    assert!(!scale_to_yuv420p10(&input, &mut scaler, &mut output, 2).unwrap());
    assert!(scaler.is_some());
    assert!(
        planes_equal(&output, &reference),
        "downscale 2 after the repack differs"
    );
}

#[test]
fn an_8_bit_frame_at_downscale_1_is_still_converted_by_swscale() {
    let mut input = frame::Video::new(Pixel::YUV420P, 64, 48);
    for plane in 0..3 {
        input.data_mut(plane).fill(128);
    }
    let mut scaler = None;
    let mut output = frame::Video::empty();
    assert!(!scale_to_yuv420p10(&input, &mut scaler, &mut output, 1).unwrap());
    assert!(scaler.is_some(), "an 8-bit frame needs swscale");
    assert_eq!(
        (output.format(), output.width(), output.height()),
        (Pixel::YUV420P10LE, 64, 48)
    );
}

#[test]
fn a_malformed_p010_frame_is_an_error() {
    // Format and size set, but no planes allocated.
    let mut unallocated = frame::Video::empty();
    unallocated.set_format(Pixel::P010LE);
    unallocated.set_width(64);
    unallocated.set_height(48);
    // Wider than its planes: the rows do not fit the stride.
    let mut too_wide = p010_frame(64, 48, 64);
    too_wide.set_width(128);

    for (label, input) in [("unallocated", unallocated), ("too wide", too_wide)] {
        let mut scaler = None;
        let mut output = frame::Video::empty();
        let error = scale_to_yuv420p10(&input, &mut scaler, &mut output, 1)
            .expect_err(label)
            .to_string();
        assert!(error.contains("Malformed P010 frame"), "{label}: {error}");
        assert!(scaler.is_none(), "{label}");
    }
}
