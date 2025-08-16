use ffmpeg_next::frame;

/// Detected active-video rectangle (cropping out black bars)
#[derive(Clone, Copy, Debug)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CropRect {
    pub fn full(width: u32, height: u32) -> Self {
        CropRect { x: 0, y: 0, width, height }
    }
}

fn round_down_even(v: u32) -> u32 {
    v & !1
}

fn round_len_even(len: u32) -> u32 {
    if len <= 2 {
        2
    } else {
        len & !1
    }
}

/// Determine if a 10-bit limited-range luma code is "non-black".
/// Treat values near nominal black as black; default threshold ~1% above black.
/// - Limited-range (10-bit): nominal black ~64, white ~940. Range width 876.
/// - Normalize e' = clamp((code - 64) / 876, 0..1). Non-black if e' > 0.01.
fn is_non_black_10bit_limited(code10: u16) -> bool {
    let code = (code10 & 0x03FF) as i32; // 10-bit
    let norm = ((code - 64) as f64 / 876.0).clamp(0.0, 1.0);
    norm > 0.01
}

fn read_luma10(y_data: &[u8], stride: usize, x: usize, y: usize) -> u16 {
    // YUV420P10LE: 10-bit little-endian stored in 16-bit containers, 2 bytes per sample
    let offset = y.saturating_mul(stride) + x.saturating_mul(2);
    if offset + 1 >= y_data.len() {
        return 0;
    }
    let lo = y_data[offset];
    let hi = y_data[offset + 1];
    u16::from_le_bytes([lo, hi])
}

/// Detect active video area by scanning for rows/columns with sufficient non-black pixels.
/// - Works on the 10-bit Y plane of a scaled frame (e.g., YUV420P10LE)
/// - Samples every 10 pixels for speed
/// - Requires at least 10% non-black pixels to consider a row/column as active
/// - Rounds the result to even coordinates/dimensions (safer for chroma-subsampled formats)
pub fn detect_crop(frame: &frame::Video) -> CropRect {
    let width = frame.width() as u32;
    let height = frame.height() as u32;
    if width == 0 || height == 0 {
        return CropRect::full(width, height);
    }

    let y_data = frame.data(0);
    let stride = frame.stride(0) as usize;

    let sample_step = 10usize;
    let min_row_samples = ((width as usize + sample_step - 1) / sample_step).max(1);
    let min_col_samples = ((height as usize + sample_step - 1) / sample_step).max(1);
    let required_non_black_row = ((min_row_samples as f64) * 0.10).ceil() as usize;
    let required_non_black_col = ((min_col_samples as f64) * 0.10).ceil() as usize;

    // Scan top
    let mut top = 0u32;
    for y in 0..height as usize {
        let mut non_black = 0usize;
        let mut x = 0usize;
        while x < width as usize {
            let l = read_luma10(y_data, stride, x, y);
            if is_non_black_10bit_limited(l) {
                non_black += 1;
                if non_black >= required_non_black_row {
                    top = y as u32;
                    break;
                }
            }
            x += sample_step;
        }
        if non_black >= required_non_black_row {
            break;
        }
    }

    // Scan bottom
    let mut bottom = height.saturating_sub(1);
    for y in (0..height as usize).rev() {
        let mut non_black = 0usize;
        let mut x = 0usize;
        while x < width as usize {
            let l = read_luma10(y_data, stride, x, y);
            if is_non_black_10bit_limited(l) {
                non_black += 1;
                if non_black >= required_non_black_row {
                    bottom = y as u32;
                    break;
                }
            }
            x += sample_step;
        }
        if non_black >= required_non_black_row {
            break;
        }
    }

    // Scan left
    let mut left = 0u32;
    for x in 0..width as usize {
        let mut non_black = 0usize;
        let mut y = 0usize;
        while y < height as usize {
            let l = read_luma10(y_data, stride, x, y);
            if is_non_black_10bit_limited(l) {
                non_black += 1;
                if non_black >= required_non_black_col {
                    left = x as u32;
                    break;
                }
            }
            y += sample_step;
        }
        if non_black >= required_non_black_col {
            break;
        }
    }

    // Scan right
    let mut right = width.saturating_sub(1);
    for x in (0..width as usize).rev() {
        let mut non_black = 0usize;
        let mut y = 0usize;
        while y < height as usize {
            let l = read_luma10(y_data, stride, x, y);
            if is_non_black_10bit_limited(l) {
                non_black += 1;
                if non_black >= required_non_black_col {
                    right = x as u32;
                    break;
                }
            }
            y += sample_step;
        }
        if non_black >= required_non_black_col {
            break;
        }
    }

    // Validate
    if right <= left || bottom <= top {
        return CropRect::full(width, height);
    }

    // Round to even coordinates/dimensions and clamp
    let mut x0 = round_down_even(left);
    let mut y0 = round_down_even(top);
    let mut w = (right - x0 + 1).max(2);
    let mut h = (bottom - y0 + 1).max(2);
    w = round_len_even(w);
    h = round_len_even(h);

    // Ensure within bounds
    if x0 + w > width {
        if width >= w { x0 = width - w; } else { x0 = 0; w = width & !1; }
    }
    if y0 + h > height {
        if height >= h { y0 = height - h; } else { y0 = 0; h = height & !1; }
    }

    CropRect { x: x0, y: y0, width: w, height: h }
}
