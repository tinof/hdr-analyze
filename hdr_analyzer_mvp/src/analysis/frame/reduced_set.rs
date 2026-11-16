//! The SMPTE ST 2094-10 reduced pixel set (§6.1.2), PQ path (`--peak-estimator st2094-10`).
//!
//! The picture inside the crop is cut into non-overlapping 2x2 blocks anchored at the crop's
//! top-left; a block at an odd right or bottom edge holds only the pixels inside. Each pixel's
//! R'G'B' is the analyzer's PQ-path mix (its own 4:2:0 chroma sample, no upsampling), each
//! component is clamped to [0, 1], converted to linear light through one ST 2084 EOTF table and
//! averaged per block. The frame value is the highest block max-RGB (the L1 max candidate) and
//! the lowest block min-RGB (the §6.1.3 minimum, a diagnostic). Both stay in linear light until
//! [`linear_to_pq`] converts them once per frame on the host.
//!
//! `reduced_set_extrema` in `kernels.cu` runs the same f32 operations in the same order, so the
//! CPU and CUDA paths agree bit for bit; the frame max and min are order-independent.

use std::sync::LazyLock;

use rayon::prelude::*;

use crate::analysis::histogram::{nits_to_pq, pq_to_nits};
use crate::crop::CropRect;

/// Intervals of [`PQ_EOTF_LUT`]. 4096 keeps the PQ round trip within 0.25 12-bit code of the
/// f64 EOTF everywhere and within 0.001 code above PQ 0.5 (measured 2026-11-09).
pub(crate) const EOTF_LUT_INTERVALS: usize = 4096;

/// ST 2084 EOTF, normalized PQ signal to linear light / 10000 nits, at `i / EOTF_LUT_INTERVALS`.
/// Uploaded unchanged to the CUDA kernel.
pub(crate) static PQ_EOTF_LUT: LazyLock<Vec<f32>> = LazyLock::new(|| {
    (0..=EOTF_LUT_INTERVALS)
        .map(|i| (pq_to_nits(i as f64 / EOTF_LUT_INTERVALS as f64) / 10_000.0) as f32)
        .collect()
});

/// Linear light of a PQ signal in [0, 1] by linear interpolation in [`PQ_EOTF_LUT`]; as
/// `pq_eotf` in `kernels.cu`. Rust never contracts these operations into an FMA.
fn eotf(lut: &[f32], signal: f32) -> f32 {
    let position = signal * EOTF_LUT_INTERVALS as f32;
    let index = (position as usize).min(EOTF_LUT_INTERVALS - 1);
    let fraction = position - index as f32;
    lut[index] + fraction * (lut[index + 1] - lut[index])
}

/// Normalized PQ of a linear-light value (/ 10000 nits), in f64 on the host for both backends.
pub(crate) fn linear_to_pq(linear: f32) -> f64 {
    nits_to_pq(f64::from(linear) * 10_000.0)
}

/// The PQ-path R'G'B' of one pixel from its 10-bit codes, operation for operation as the
/// `None =>` branch of `analyze_native_frame_cropped` and `pq_rgb` in `kernels.cu`.
pub(crate) fn pq_rgb(y_code: u16, cb_code: u16, cr_code: u16) -> [f32; 3] {
    let y_signal = (f32::from(y_code) - 64.0) / 876.0;
    let cb = (f32::from(cb_code) - 512.0) / 896.0;
    let cr = (f32::from(cr_code) - 512.0) / 896.0;
    let red = y_signal + 1.4746 * cr;
    let blue = y_signal + 1.8814 * cb;
    let green = (y_signal - 0.2627 * red - 0.0593 * blue) / 0.6780;
    [red, green, blue]
}

/// The planes of a 10-bit 4:2:0 frame (little-endian, low 10 bits), as the CPU path reads them.
pub(crate) struct Planes<'a> {
    pub y: &'a [u8],
    pub y_stride: usize,
    pub u: &'a [u8],
    pub u_stride: usize,
    pub v: &'a [u8],
    pub v_stride: usize,
    pub width: usize,
    pub height: usize,
}

impl Planes<'_> {
    fn code(data: &[u8], offset: usize) -> Option<u16> {
        let bytes = data.get(offset..offset + 2)?;
        Some(u16::from_le_bytes([bytes[0], bytes[1]]) & 0x03FF)
    }

    /// The pixel's linear-light R, G and B, each clamped to [0, 1] before the EOTF.
    fn linear_rgb(&self, lut: &[f32], x: usize, y: usize) -> Option<[f32; 3]> {
        let y_code = Self::code(self.y, y * self.y_stride + x * 2)?;
        let cb_code = Self::code(self.u, (y / 2) * self.u_stride + (x / 2) * 2)?;
        let cr_code = Self::code(self.v, (y / 2) * self.v_stride + (x / 2) * 2)?;
        Some(pq_rgb(y_code, cb_code, cr_code).map(|component| eotf(lut, component.clamp(0.0, 1.0))))
    }
}

/// Highest block max-RGB and lowest block min-RGB of the crop's reduced pixel set, in linear
/// light, or `None` when the crop holds no pixel.
pub(crate) fn reduced_set_extrema(planes: &Planes<'_>, crop: &CropRect) -> Option<(f32, f32)> {
    let lut = PQ_EOTF_LUT.as_slice();
    let x_start = crop.x as usize;
    let y_start = crop.y as usize;
    let x_end = (x_start + crop.width as usize).min(planes.width);
    let y_end = (y_start + crop.height as usize).min(planes.height);
    if x_end <= x_start || y_end <= y_start {
        return None;
    }
    let block_rows = (y_end - y_start).div_ceil(2);
    let block_columns = (x_end - x_start).div_ceil(2);
    (0..block_rows)
        .into_par_iter()
        .filter_map(|block_row| {
            let y0 = y_start + 2 * block_row;
            let mut extrema: Option<(f32, f32)> = None;
            for block_column in 0..block_columns {
                let x0 = x_start + 2 * block_column;
                let mut sums = [0.0f32; 3];
                let mut count = 0u32;
                for y in y0..(y0 + 2).min(y_end) {
                    for x in x0..(x0 + 2).min(x_end) {
                        let Some(rgb) = planes.linear_rgb(lut, x, y) else {
                            continue;
                        };
                        for (sum, component) in sums.iter_mut().zip(rgb) {
                            *sum = *sum + component;
                        }
                        count += 1;
                    }
                }
                if count == 0 {
                    continue;
                }
                let [red, green, blue] = sums.map(|sum| sum / count as f32);
                let high = red.max(green.max(blue));
                let low = red.min(green.min(blue));
                extrema = Some(extrema.map_or((high, low), |(h, l)| (h.max(high), l.min(low))));
            }
            extrema
        })
        .reduce_with(|(h1, l1), (h2, l2)| (h1.max(h2), l1.min(l2)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent f64 reference: the standard's statistic from the R'G'B' components of each
    /// pixel, with the exact ST 2084 EOTF instead of the table.
    fn reference(pixels: &[Vec<[f64; 3]>], crop: (usize, usize, usize, usize)) -> (f64, f64) {
        let (cx, cy, cw, ch) = crop;
        let eotf = |e: f64| pq_to_nits(e.clamp(0.0, 1.0)) / 10_000.0;
        let (mut high, mut low) = (f64::MIN, f64::MAX);
        for by in (cy..cy + ch).step_by(2) {
            for bx in (cx..cx + cw).step_by(2) {
                let mut sums = [0.0; 3];
                let mut count = 0.0;
                for row in pixels.iter().take((by + 2).min(cy + ch)).skip(by) {
                    for rgb in row.iter().take((bx + 2).min(cx + cw)).skip(bx) {
                        for k in 0..3 {
                            sums[k] += eotf(rgb[k]);
                        }
                        count += 1.0;
                    }
                }
                let mean = sums.map(|s| s / count);
                high = high.max(mean[0].max(mean[1].max(mean[2])));
                low = low.min(mean[0].min(mean[1].min(mean[2])));
            }
        }
        (
            nits_to_pq(high * 10_000.0) * 4095.0,
            nits_to_pq(low * 10_000.0) * 4095.0,
        )
    }

    struct Frame {
        width: usize,
        height: usize,
        y: Vec<u8>,
        u: Vec<u8>,
        v: Vec<u8>,
    }

    impl Frame {
        fn new(
            width: usize,
            height: usize,
            y: impl Fn(usize, usize) -> u16,
            cbcr: impl Fn(usize, usize) -> (u16, u16),
        ) -> Self {
            let mut frame = Self {
                width,
                height,
                y: vec![0; width * height * 2],
                u: vec![0; width.div_ceil(2) * height.div_ceil(2) * 2],
                v: vec![0; width.div_ceil(2) * height.div_ceil(2) * 2],
            };
            for row in 0..height {
                for column in 0..width {
                    let offset = (row * width + column) * 2;
                    frame.y[offset..offset + 2].copy_from_slice(&y(column, row).to_le_bytes());
                }
            }
            for row in 0..height.div_ceil(2) {
                for column in 0..width.div_ceil(2) {
                    let (cb, cr) = cbcr(column, row);
                    let offset = (row * width.div_ceil(2) + column) * 2;
                    frame.u[offset..offset + 2].copy_from_slice(&cb.to_le_bytes());
                    frame.v[offset..offset + 2].copy_from_slice(&cr.to_le_bytes());
                }
            }
            frame
        }

        fn planes(&self) -> Planes<'_> {
            Planes {
                y: &self.y,
                y_stride: self.width * 2,
                u: &self.u,
                u_stride: self.width.div_ceil(2) * 2,
                v: &self.v,
                v_stride: self.width.div_ceil(2) * 2,
                width: self.width,
                height: self.height,
            }
        }

        fn code(data: &[u8], offset: usize) -> u16 {
            u16::from_le_bytes([data[offset], data[offset + 1]])
        }

        /// The components of every pixel in f64, from the BT.2020 NCL inverse of the codes
        /// (written out here, not taken from `pq_rgb`).
        fn components(&self) -> Vec<Vec<[f64; 3]>> {
            (0..self.height)
                .map(|row| {
                    (0..self.width)
                        .map(|column| {
                            let luma =
                                (f64::from(Self::code(&self.y, (row * self.width + column) * 2))
                                    - 64.0)
                                    / 876.0;
                            let chroma = ((row / 2) * self.width.div_ceil(2) + column / 2) * 2;
                            let cb = (f64::from(Self::code(&self.u, chroma)) - 512.0) / 896.0;
                            let cr = (f64::from(Self::code(&self.v, chroma)) - 512.0) / 896.0;
                            let red = luma + 1.4746 * cr;
                            let blue = luma + 1.8814 * cb;
                            [red, (luma - 0.2627 * red - 0.0593 * blue) / 0.6780, blue]
                        })
                        .collect()
                })
                .collect()
        }

        fn measure(&self, crop: (u32, u32, u32, u32)) -> (f64, f64) {
            let rect = CropRect {
                x: crop.0,
                y: crop.1,
                width: crop.2,
                height: crop.3,
            };
            let (high, low) = reduced_set_extrema(&self.planes(), &rect).expect("pixels in crop");
            (linear_to_pq(high) * 4095.0, linear_to_pq(low) * 4095.0)
        }

        fn expected(&self, crop: (u32, u32, u32, u32)) -> (f64, f64) {
            let crop = (
                crop.0 as usize,
                crop.1 as usize,
                crop.2 as usize,
                crop.3 as usize,
            );
            reference(&self.components(), crop)
        }
    }

    fn assert_close(label: &str, measured: (f64, f64), expected: (f64, f64)) {
        assert!(
            (measured.0 - expected.0).abs() < 0.25 && (measured.1 - expected.1).abs() < 0.25,
            "{label}: measured max/min {measured:?}, standard {expected:?}"
        );
    }

    /// Luma code of the test speculars: 3534 12-bit codes, the spike's synthetic highlight.
    const SPECULAR_Y: u16 = 820;

    /// Neutral 2x2 specular (Y' SPECULAR_Y) on a dark background (Y' 200).
    fn specular(x0: usize, y0: usize) -> Frame {
        Frame::new(
            16,
            12,
            |x, y| {
                if (x0..x0 + 2).contains(&x) && (y0..y0 + 2).contains(&y) {
                    SPECULAR_Y
                } else {
                    200
                }
            },
            |_, _| (512, 512),
        )
    }

    #[test]
    fn eotf_table_tracks_the_exact_curve() {
        let lut = PQ_EOTF_LUT.as_slice();
        assert_eq!(lut.len(), EOTF_LUT_INTERVALS + 1);
        assert_eq!(lut[0], 0.0);
        assert_eq!(lut[EOTF_LUT_INTERVALS], 1.0);
        assert!(lut.windows(2).all(|pair| pair[1] > pair[0]));
        for step in 0..=10_000 {
            let signal = step as f32 / 10_000.0;
            let round_trip = linear_to_pq(eotf(lut, signal)) * 4095.0;
            assert!(
                (round_trip - f64::from(signal) * 4095.0).abs() < 0.25,
                "signal {signal}"
            );
        }
    }

    #[test]
    fn aligned_specular_keeps_its_pixel_value() {
        let frame = specular(4, 4);
        let (high, _) = frame.measure((0, 0, 16, 12));
        let pixel = (f64::from(SPECULAR_Y) - 64.0) / 876.0 * 4095.0;
        assert!((high - pixel).abs() < 0.25, "{high} vs {pixel}");
        assert_close(
            "aligned",
            frame.measure((0, 0, 16, 12)),
            frame.expected((0, 0, 16, 12)),
        );
    }

    #[test]
    fn off_grid_specular_reads_the_linear_quarter_mean() {
        let pixel = (f64::from(SPECULAR_Y) - 64.0) / 876.0 * 4095.0;
        let peak = pq_to_nits((f64::from(SPECULAR_Y) - 64.0) / 876.0);
        let background = pq_to_nits((200.0 - 64.0) / 876.0);
        // One pixel off the grid in x: two blocks hold half of the specular each.
        let (high, _) = specular(5, 4).measure((0, 0, 16, 12));
        let half = nits_to_pq((peak + background) / 2.0) * 4095.0;
        assert!((high - half).abs() < 0.25, "{high} vs {half}");
        // The spike measured about 300 codes below the 3534-code pixel value (2026-11-07).
        assert!(
            (pixel - 330.0..pixel - 280.0).contains(&high),
            "about 300 codes low: {high}"
        );
        // Off the grid in x and y: four blocks hold a quarter each.
        let (high, _) = specular(5, 5).measure((0, 0, 16, 12));
        let quarter = nits_to_pq((peak + 3.0 * background) / 4.0) * 4095.0;
        assert!((high - quarter).abs() < 0.25, "{high} vs {quarter}");
    }

    #[test]
    fn odd_crop_anchor_moves_the_block_grid() {
        // Off the grid from the frame's top-left, on the grid of a crop anchored at (5, 5).
        let frame = specular(5, 5);
        let pixel = (f64::from(SPECULAR_Y) - 64.0) / 876.0 * 4095.0;
        let (high, _) = frame.measure((5, 5, 11, 7));
        assert!((high - pixel).abs() < 0.25, "{high} vs {pixel}");
        assert_close(
            "odd anchor",
            frame.measure((5, 5, 11, 7)),
            frame.expected((5, 5, 11, 7)),
        );
        assert_close(
            "odd x",
            frame.measure((3, 0, 13, 12)),
            frame.expected((3, 0, 13, 12)),
        );
    }

    #[test]
    fn partial_edge_blocks_average_only_the_pixels_inside() {
        // A specular in the last column and row of an odd-sized crop: its edge blocks hold one
        // or two pixels, so a one-pixel corner keeps its full value.
        let frame = Frame::new(
            9,
            7,
            |x, y| if x == 8 && y == 6 { SPECULAR_Y } else { 200 },
            |_, _| (512, 512),
        );
        let pixel = (f64::from(SPECULAR_Y) - 64.0) / 876.0 * 4095.0;
        let (high, _) = frame.measure((0, 0, 9, 7));
        assert!((high - pixel).abs() < 0.25, "{high} vs {pixel}");
        assert_close(
            "partial",
            frame.measure((0, 0, 9, 7)),
            frame.expected((0, 0, 9, 7)),
        );
        // Two pixels of a 2-wide edge block: the mean of the specular and the background.
        let column = Frame::new(
            9,
            8,
            |x, y| if x == 8 && y == 6 { SPECULAR_Y } else { 200 },
            |_, _| (512, 512),
        );
        assert_close(
            "edge column",
            column.measure((0, 0, 9, 8)),
            column.expected((0, 0, 9, 8)),
        );
    }

    #[test]
    fn components_average_before_the_max_and_clamp_before_the_eotf() {
        // An odd-anchor block over four chroma quads of different hues: red wins in two pixels,
        // blue in the others, so max(mean(R), mean(G), mean(B)) is below mean(max-RGB). Strong
        // chroma pushes components outside [0, 1], which must clamp before the EOTF.
        let frame = Frame::new(
            8,
            8,
            |x, y| 300 + 40 * ((x + y) % 3) as u16,
            |cx, cy| match (cx + cy) % 3 {
                0 => (512, 960),
                1 => (960, 300),
                _ => (64, 512),
            },
        );
        let components = frame.components();
        assert!(components
            .iter()
            .flatten()
            .flatten()
            .any(|&c| c < 0.0 || c > 1.0));
        for crop in [(0, 0, 8, 8), (1, 1, 7, 7), (1, 0, 6, 8), (3, 3, 5, 5)] {
            assert_close(
                &format!("chroma {crop:?}"),
                frame.measure(crop),
                frame.expected(crop),
            );
        }
        // The mean of each block's per-pixel max-RGB would read higher on this picture.
        let eotf = |e: f64| pq_to_nits(e.clamp(0.0, 1.0)) / 10_000.0;
        let mean_of_max = (0..4)
            .map(|block| {
                let (bx, by) = (1 + 2 * (block % 2), 1 + 2 * (block / 2));
                let sum: f64 = (by..by + 2)
                    .flat_map(|y| (bx..bx + 2).map(move |x| (x, y)))
                    .map(|(x, y)| components[y][x].map(eotf).into_iter().fold(0.0, f64::max))
                    .sum();
                nits_to_pq(sum / 4.0 * 10_000.0) * 4095.0
            })
            .fold(0.0, f64::max);
        let (high, _) = frame.measure((1, 1, 4, 4));
        assert!(
            mean_of_max > high + 1.0,
            "mean of max {mean_of_max} vs reduced {high}"
        );
    }

    #[test]
    fn empty_crop_has_no_reduced_set() {
        let frame = specular(0, 0);
        let rect = CropRect {
            x: 16,
            y: 0,
            width: 4,
            height: 4,
        };
        assert!(reduced_set_extrema(&frame.planes(), &rect).is_none());
    }
}
