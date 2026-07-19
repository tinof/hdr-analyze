use ffmpeg_next::{format, frame};

pub const CROP_EDGE_TOLERANCE: u32 = 2;
const FRAME_SAMPLE_STEP: usize = 10;
const MIN_USABLE_NON_BLACK_FRACTION: f64 = 0.05;

/// Detected active-video rectangle (cropping out black bars)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CropRect {
    pub fn full(width: u32, height: u32) -> Self {
        CropRect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    fn right(self) -> u32 {
        self.x.saturating_add(self.width)
    }

    fn bottom(self) -> u32 {
        self.y.saturating_add(self.height)
    }

    fn area(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    pub fn approximately_matches(self, other: Self, tolerance: u32) -> bool {
        self.x.abs_diff(other.x) <= tolerance
            && self.y.abs_diff(other.y) <= tolerance
            && self.right().abs_diff(other.right()) <= tolerance
            && self.bottom().abs_diff(other.bottom()) <= tolerance
    }

    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());

        Self {
            x,
            y,
            width: right.saturating_sub(x),
            height: bottom.saturating_sub(y),
        }
    }
}

/// Result of clustering and voting across crop probe candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropVote {
    pub rect: CropRect,
    pub candidate_count: usize,
    pub modal_count: usize,
    pub cluster_count: usize,
    pub variable_ar: bool,
}

/// Cluster candidates by edge proximity and choose a conservative committed rectangle.
///
/// A single crop mode commits the union of its modal cluster to absorb small detector jitter.
/// Multiple modes indicate variable active area, so the union spans every observed mode and
/// cannot cut real picture from a fuller-frame scene.
pub fn vote_crop_candidates(candidates: &[CropRect], tolerance: u32) -> Option<CropVote> {
    if candidates.is_empty() {
        return None;
    }

    let mut visited = vec![false; candidates.len()];
    let mut clusters: Vec<Vec<CropRect>> = Vec::new();

    for start in 0..candidates.len() {
        if visited[start] {
            continue;
        }

        visited[start] = true;
        let mut pending = vec![start];
        let mut cluster = Vec::new();

        while let Some(index) = pending.pop() {
            let candidate = candidates[index];
            cluster.push(candidate);

            for next in 0..candidates.len() {
                if !visited[next] && candidates[next].approximately_matches(candidate, tolerance) {
                    visited[next] = true;
                    pending.push(next);
                }
            }
        }

        clusters.push(cluster);
    }

    let cluster_union = |cluster: &[CropRect]| {
        cluster
            .iter()
            .copied()
            .reduce(CropRect::union)
            .expect("crop clusters are never empty")
    };

    let mut modal_index = 0;
    for index in 1..clusters.len() {
        let modal_union = cluster_union(&clusters[modal_index]);
        let candidate_union = cluster_union(&clusters[index]);
        if clusters[index].len() > clusters[modal_index].len()
            || (clusters[index].len() == clusters[modal_index].len()
                && candidate_union.area() > modal_union.area())
        {
            modal_index = index;
        }
    }

    let variable_ar = clusters.len() > 1;
    let rect = if variable_ar {
        candidates
            .iter()
            .copied()
            .reduce(CropRect::union)
            .expect("candidates are not empty")
    } else {
        cluster_union(&clusters[modal_index])
    };

    Some(CropVote {
        rect,
        candidate_count: candidates.len(),
        modal_count: clusters[modal_index].len(),
        cluster_count: clusters.len(),
        variable_ar,
    })
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

pub fn is_frame_usable_for_crop(frame: &frame::Video) -> bool {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    if width == 0 || height == 0 {
        return false;
    }

    let y_data = frame.data(0);
    let stride = frame.stride(0);
    let p010 = frame.format() == format::Pixel::P010LE;
    let mut sampled = 0usize;
    let mut non_black = 0usize;

    for y in (0..height).step_by(FRAME_SAMPLE_STEP) {
        for x in (0..width).step_by(FRAME_SAMPLE_STEP) {
            sampled += 1;
            if is_non_black_10bit_limited(read_luma10(y_data, stride, x, y, p010)) {
                non_black += 1;
            }
        }
    }

    sampled > 0 && (non_black as f64 / sampled as f64) >= MIN_USABLE_NON_BLACK_FRACTION
}

fn read_luma10(y_data: &[u8], stride: usize, x: usize, y: usize, p010: bool) -> u16 {
    // YUV420P10LE stores the 10-bit code in the low bits; P010LE stores it in the high bits.
    let offset = y.saturating_mul(stride) + x.saturating_mul(2);
    if offset + 1 >= y_data.len() {
        return 0;
    }
    let lo = y_data[offset];
    let hi = y_data[offset + 1];
    let raw = u16::from_le_bytes([lo, hi]);
    if p010 {
        (raw >> 6) & 0x03FF
    } else {
        raw
    }
}

/// Detect active video area by scanning for rows/columns with sufficient non-black pixels.
/// - Works on the 10-bit Y plane of a scaled frame (e.g., YUV420P10LE)
/// - Samples every 10 pixels for speed
/// - Requires at least 10% non-black pixels to consider a row/column as active
/// - Rounds the result to even coordinates/dimensions (safer for chroma-subsampled formats)
pub fn detect_crop(frame: &frame::Video) -> CropRect {
    let width = frame.width();
    let height = frame.height();
    if width == 0 || height == 0 {
        return CropRect::full(width, height);
    }

    let y_data = frame.data(0);
    let stride = frame.stride(0);
    let p010 = frame.format() == format::Pixel::P010LE;

    let sample_step = FRAME_SAMPLE_STEP;
    let min_row_samples = (width as usize).div_ceil(sample_step).max(1);
    let min_col_samples = (height as usize).div_ceil(sample_step).max(1);
    let required_non_black_row = ((min_row_samples as f64) * 0.10).ceil() as usize;
    let required_non_black_col = ((min_col_samples as f64) * 0.10).ceil() as usize;

    // Scan top
    let mut top = 0u32;
    for y in 0..height as usize {
        let mut non_black = 0usize;
        let mut x = 0usize;
        while x < width as usize {
            let l = read_luma10(y_data, stride, x, y, p010);
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
            let l = read_luma10(y_data, stride, x, y, p010);
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
            let l = read_luma10(y_data, stride, x, y, p010);
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
            let l = read_luma10(y_data, stride, x, y, p010);
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
        if width >= w {
            x0 = width - w;
        } else {
            x0 = 0;
            w = width & !1;
        }
    }
    if y0 + h > height {
        if height >= h {
            y0 = height - h;
        } else {
            y0 = 0;
            h = height & !1;
        }
    }

    CropRect {
        x: x0,
        y: y0,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use ffmpeg_next::{format, frame};

    use super::{
        detect_crop, is_frame_usable_for_crop, vote_crop_candidates, CropRect, CROP_EDGE_TOLERANCE,
    };

    fn rect(x: u32, y: u32, width: u32, height: u32) -> CropRect {
        CropRect {
            x,
            y,
            width,
            height,
        }
    }

    fn synthetic_frame(width: u32, height: u32, active: impl Fn(u32, u32) -> bool) -> frame::Video {
        let mut frame = frame::Video::new(format::Pixel::YUV420P10LE, width, height);
        let stride = frame.stride(0);
        let data = frame.data_mut(0);

        for y in 0..height {
            for x in 0..width {
                let code = if active(x, y) { 256u16 } else { 64u16 };
                let offset = y as usize * stride + x as usize * 2;
                data[offset..offset + 2].copy_from_slice(&code.to_le_bytes());
            }
        }

        frame
    }

    #[test]
    fn stable_candidates_vote_for_modal_union() {
        let vote = vote_crop_candidates(
            &[
                rect(0, 100, 1920, 880),
                rect(0, 102, 1920, 876),
                rect(0, 100, 1920, 878),
            ],
            CROP_EDGE_TOLERANCE,
        )
        .expect("vote");

        assert_eq!(vote.rect, rect(0, 100, 1920, 880));
        assert_eq!(vote.cluster_count, 1);
        assert_eq!(vote.modal_count, 3);
        assert!(!vote.variable_ar);
    }

    #[test]
    fn tolerance_connects_adjacent_edge_jitter() {
        let vote = vote_crop_candidates(
            &[
                rect(0, 100, 1920, 880),
                rect(0, 102, 1920, 876),
                rect(0, 104, 1920, 872),
            ],
            CROP_EDGE_TOLERANCE,
        )
        .expect("vote");

        assert_eq!(vote.cluster_count, 1);
        assert_eq!(vote.modal_count, 3);
    }

    #[test]
    fn variable_active_area_commits_union_of_modes() {
        let vote = vote_crop_candidates(
            &[
                rect(0, 100, 1920, 880),
                rect(0, 102, 1920, 876),
                rect(0, 0, 1920, 1080),
            ],
            CROP_EDGE_TOLERANCE,
        )
        .expect("vote");

        assert!(vote.variable_ar);
        assert_eq!(vote.cluster_count, 2);
        assert_eq!(vote.modal_count, 2);
        assert_eq!(vote.rect, CropRect::full(1920, 1080));
    }

    #[test]
    fn black_and_sparse_noise_frames_are_not_usable() {
        let black = synthetic_frame(100, 100, |_, _| false);
        let sparse_noise = synthetic_frame(100, 100, |x, y| x == 0 && y == 0);

        assert!(!is_frame_usable_for_crop(&black));
        assert!(!is_frame_usable_for_crop(&sparse_noise));
    }

    #[test]
    fn letterboxed_picture_is_usable_and_detectable() {
        let letterboxed = synthetic_frame(100, 100, |_, y| (20..80).contains(&y));

        assert!(is_frame_usable_for_crop(&letterboxed));
        assert_eq!(detect_crop(&letterboxed), rect(0, 20, 100, 60));
    }
}
