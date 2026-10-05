//! `chroma-siting`: how the analyzer's 4:2:0 handling of the Profile 8.4 decode compares with
//! the spec composer and with a renderer that upsamples chroma first (docs/HLG_COMPOSER.md §9).
//!
//! Three decodes of the same 10-bit 4:2:0 HLG frame:
//! - **analyzer** (`hdr_analyzer_mvp` `analysis/frame.rs`): each chroma sample is shared by its
//!   2×2 quad and the MMR takes each pixel's own luma.
//! - **spec** (ETSI GS CCM 001 v1.1.1 §5.4.2.3.3): the MMR runs at chroma resolution on luma
//!   down-sampled to the chroma positions; the display upsamples the composed chroma. `spec-float`
//!   keeps the analyzer's arithmetic (`code / 1023`, f32) so only the structure differs;
//!   `spec-fixed` is the spec's integer arithmetic (`code / 1024`, 16-bit output, input clamps
//!   to the pivot ranges).
//! - **renderer** (libplacebo): chroma upsampled to full resolution first, then the MMR with each
//!   pixel's own luma.
//!
//! Every variant ends in max(R′,G′,B′) clamped to the RPU's source range, as the analyzer.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};

use anyhow::{bail, ensure, Context, Result};
use dolby_vision::rpu::profiles::{profile84::Profile84, DoviProfile};
use dolby_vision::rpu::rpu_data_mapping::{DoviReshapingCurve, RpuDataMapping};
use dovi84_composer::{Composer, COEFFICIENT_LOG2_DENOM};
use rayon::prelude::*;

use crate::model::{Decoder, LUMA_GAIN, LUMA_OFFSET};

/// Variant names, in output order. Index 0 is the analyzer, which the differences refer to.
pub const VARIANTS: [&str; 9] = [
    "analyzer",
    "spec-float-nearest",
    "spec-float-left",
    "spec-float-topleft",
    "spec-fixed-nearest",
    "spec-fixed-left",
    "spec-fixed-topleft",
    "renderer-left",
    "renderer-topleft",
];
const RENDERER_LEFT: usize = 7;
const RENDERER_TOPLEFT: usize = 8;

/// Histogram resolution of per-pixel differences: 0.01 code, up to 100 codes.
const DIFF_BINS: usize = 10_000;
const DIFF_STEP: f64 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upsample {
    /// The chroma sample replicated over its 2×2 quad (what the analyzer does).
    Nearest,
    /// Bilinear, chroma location type 0: co-sited horizontally, between the two rows vertically.
    Left,
    /// Bilinear, chroma location type 2: co-sited horizontally and vertically.
    TopLeft,
}

/// The two chroma samples and weights that interpolate luma position `pos` along one axis.
/// `centred` places chroma sample `j` between luma positions `2j` and `2j + 1`; otherwise it is
/// co-sited with `2j`. Edges replicate.
pub fn taps(pos: usize, len: usize, centred: bool) -> [(usize, f32); 2] {
    let j = pos / 2;
    let last = len - 1;
    match (centred, pos % 2) {
        (false, 0) => [(j, 1.0), (j, 0.0)],
        (false, _) => [(j, 0.5), ((j + 1).min(last), 0.5)],
        (true, 0) => [(j, 0.75), (j.saturating_sub(1), 0.25)],
        (true, _) => [(j, 0.75), ((j + 1).min(last), 0.25)],
    }
}

/// One plane at chroma resolution, `width_c` samples per row.
struct Plane<'a, T> {
    data: &'a [T],
    width_c: usize,
    height_c: usize,
}

impl<'a, T: Copy + Into<f32>> Plane<'a, T> {
    fn new(data: &'a [T], width_c: usize, height_c: usize) -> Self {
        Self {
            data,
            width_c,
            height_c,
        }
    }

    fn sample(&self, mode: Upsample, x: usize, y: usize) -> f32 {
        let at = |cx: usize, cy: usize| -> f32 { self.data[cy * self.width_c + cx].into() };
        if mode == Upsample::Nearest {
            return at(x / 2, y / 2);
        }
        let columns = taps(x, self.width_c, false);
        let rows = taps(y, self.height_c, mode == Upsample::Left);
        let row =
            |cy: usize| columns[0].1 * at(columns[0].0, cy) + columns[1].1 * at(columns[1].0, cy);
        rows[0].1 * row(rows[0].0) + rows[1].1 * row(rows[1].0)
    }
}

/// The spec's luma down-sampling for the MMR (§5.4.2.3.3): `[1 2 1]` horizontally around the
/// co-sited column, the mean of the two rows vertically, integer rounding as specified.
pub fn downsample_luma(luma: &[u16], width: usize, height: usize) -> Vec<u16> {
    let (width_c, height_c) = (width / 2, height / 2);
    let at = |x: isize, y: usize| -> u32 {
        let x = x.clamp(0, width as isize - 1) as usize;
        u32::from(luma[y.min(height - 1) * width + x])
    };
    (0..height_c)
        .into_par_iter()
        .flat_map_iter(|j| {
            (0..width_c).map(move |i| {
                let x = 2 * i as isize;
                let row = |y: usize| (at(x - 1, y) + 2 * at(x, y) + at(x + 1, y) + 2) >> 2;
                ((row(2 * j) + row(2 * j + 1) + 1) >> 1) as u16
            })
        })
        .collect()
}

fn cumulative_pivots(curve: &DoviReshapingCurve) -> Vec<i64> {
    let mut sum = 0_i64;
    curve
        .pivots
        .iter()
        .map(|&delta| {
            sum += i64::from(delta);
            sum
        })
        .collect()
}

fn fixed_point(int: i64, frac: u64) -> i128 {
    (i128::from(int) << COEFFICIENT_LOG2_DENOM) + i128::from(frac)
}

/// The spec composer in its integer arithmetic (§5.4.2.2–5.4.2.3), BL bit depth 10.
pub struct SpecComposer {
    luma_pivots: Vec<i64>,
    /// Per piece, the polynomial coefficients in fixed point.
    poly: Vec<Vec<i128>>,
    chroma_pivots: [Vec<i64>; 2],
    /// Per chroma component: MMR order, constant and `coef[order][term]` in fixed point.
    mmr: [(usize, i128, [[i128; 7]; 3]); 2],
}

const BL_BIT_DEPTH: u32 = 10;
const OUTPUT_SHIFT: u32 = 4 + COEFFICIENT_LOG2_DENOM;
/// 1.0 of the composer's 16-bit output.
pub const OUTPUT_ONE: f32 = 65536.0;

impl SpecComposer {
    pub fn new(mapping: &RpuDataMapping) -> Result<Self> {
        let luma = &mapping.curves[0];
        let poly_curve = luma.polynomial.as_ref().context("luma is not polynomial")?;
        let mut poly = Vec::new();
        for (piece, order_minus1) in poly_curve.poly_order_minus1.iter().enumerate() {
            let order = *order_minus1 as usize + 1;
            ensure!(
                order <= 2,
                "polynomial order {order} above 2 is not handled"
            );
            poly.push(
                (0..=order)
                    .map(|i| {
                        fixed_point(
                            poly_curve.poly_coef_int[piece][i],
                            poly_curve.poly_coef[piece][i],
                        )
                    })
                    .collect(),
            );
        }
        let mut mmr = [(0, 0, [[0; 7]; 3]); 2];
        for (slot, component) in mmr.iter_mut().zip([1, 2]) {
            let curve = mapping.curves[component]
                .mmr
                .as_ref()
                .context("chroma is not MMR")?;
            ensure!(
                curve.mmr_order_minus1.len() == 1,
                "Profile 8.4 chroma MMR has one piece"
            );
            let order = usize::from(curve.mmr_order_minus1[0]) + 1;
            let mut coef = [[0; 7]; 3];
            for (o, row) in coef.iter_mut().enumerate().take(order) {
                for (term, value) in row.iter_mut().enumerate() {
                    *value =
                        fixed_point(curve.mmr_coef_int[0][o][term], curve.mmr_coef[0][o][term]);
                }
            }
            *slot = (
                order,
                fixed_point(curve.mmr_constant_int[0], curve.mmr_constant[0]),
                coef,
            );
        }
        Ok(Self {
            luma_pivots: cumulative_pivots(luma),
            poly,
            chroma_pivots: [
                cumulative_pivots(&mapping.curves[1]),
                cumulative_pivots(&mapping.curves[2]),
            ],
            mmr,
        })
    }

    fn clamp_to(pivots: &[i64], s: i64) -> i64 {
        s.clamp(pivots[0], pivots[pivots.len() - 1])
    }

    fn finish(rr: i128) -> u16 {
        (rr.max(0) >> OUTPUT_SHIFT).min(0xffff) as u16
    }

    /// §5.4.2.3.2: the mapped luma of one BL code, 16-bit.
    pub fn luma(&self, code: u16) -> u16 {
        let s = Self::clamp_to(&self.luma_pivots, i64::from(code));
        // §5.4.2.2: the first piece whose upper pivot lies above s; the last one otherwise.
        let pieces = self.poly.len();
        let piece = (0..pieces)
            .find(|&k| s < self.luma_pivots[k + 1])
            .unwrap_or(pieces - 1);
        let mut vv = 0_i128;
        let mut ss = 1_i128;
        let mut shift = 2 * BL_BIT_DEPTH;
        for coef in &self.poly[piece] {
            vv += coef * (ss << shift);
            ss *= i128::from(s);
            shift = shift.saturating_sub(BL_BIT_DEPTH);
        }
        Self::finish(vv)
    }

    /// §5.4.2.3.3: the mapped Cb (`component` 0) or Cr (1) of one chroma sample, 16-bit, from
    /// the down-sampled luma `s0` and the BL chroma `s1`, `s2`.
    pub fn chroma(&self, component: usize, s0: u16, s1: u16, s2: u16) -> u16 {
        let s0 = i128::from(Self::clamp_to(&self.luma_pivots, i64::from(s0)));
        let s1 = i128::from(Self::clamp_to(&self.chroma_pivots[0], i64::from(s1)));
        let s2 = i128::from(Self::clamp_to(&self.chroma_pivots[1], i64::from(s2)));
        let up = 20 - BL_BIT_DEPTH;
        let mut tt = [0_i128; 22];
        tt[0] = 1 << 20;
        tt[1] = s0 << up;
        tt[2] = s1 << up;
        tt[3] = s2 << up;
        // 20 − 2·BL_bit_depth is 0 at 10 bits.
        tt[4] = s0 * s1;
        tt[5] = s0 * s2;
        tt[6] = s1 * s2;
        tt[7] = (tt[4] * tt[3]) >> 20;
        tt[8] = s0 * s0;
        tt[9] = s1 * s1;
        tt[10] = s2 * s2;
        tt[11] = (tt[4] * tt[4]) >> 20;
        tt[12] = (tt[5] * tt[5]) >> 20;
        tt[13] = (tt[6] * tt[6]) >> 20;
        tt[14] = (tt[7] * tt[7]) >> 20;
        tt[15] = (tt[1] * tt[8]) >> 20;
        tt[16] = (tt[2] * tt[9]) >> 20;
        tt[17] = (tt[3] * tt[10]) >> 20;
        tt[18] = (tt[4] * tt[11]) >> 20;
        tt[19] = (tt[5] * tt[12]) >> 20;
        tt[20] = (tt[6] * tt[13]) >> 20;
        tt[21] = (tt[7] * tt[14]) >> 20;
        let (order, constant, coef) = &self.mmr[component];
        let mut rr = constant * tt[0];
        for (o, row) in coef.iter().enumerate().take(*order) {
            for (term, c) in row.iter().enumerate() {
                rr += c * tt[1 + o * 7 + term];
            }
        }
        Self::finish(rr)
    }
}

/// Everything one frame needs, built once per composer.
pub struct Variants {
    decoder: Decoder,
    spec: SpecComposer,
    /// Spec-fixed luma term per BL code, as the analyzer's luma term but from the 16-bit output.
    fixed_luma_term: Vec<f32>,
    source_range: [f32; 2],
}

impl Variants {
    pub fn new(mapping: &RpuDataMapping) -> Result<Self> {
        let decoder = Decoder::new(mapping);
        let spec = SpecComposer::new(mapping)?;
        let fixed_luma_term = (0..1024_u16)
            .map(|code| {
                let y = f64::from(spec.luma(code)) / f64::from(OUTPUT_ONE);
                ((y - LUMA_OFFSET) * LUMA_GAIN) as f32
            })
            .collect();
        let dm = Profile84::dm_data();
        Ok(Self {
            decoder,
            spec,
            fixed_luma_term,
            source_range: [
                (f64::from(dm.source_min_pq) / 4095.0) as f32,
                (f64::from(dm.source_max_pq) / 4095.0) as f32,
            ],
        })
    }

    fn max_rgb(&self, rgb: [f32; 3]) -> f32 {
        rgb[0]
            .max(rgb[1])
            .max(rgb[2])
            .clamp(self.source_range[0], self.source_range[1])
    }

    /// max(R′,G′,B′) of every variant for every pixel of one frame, in normalized PQ.
    pub fn frame(&self, frame: &Frame<'_>) -> Vec<[f32; 9]> {
        let (width, height) = (frame.width, frame.height);
        let (width_c, height_c) = (width / 2, height / 2);
        let down = downsample_luma(frame.y, width, height);
        let composed: Vec<([f32; 2], [f32; 2])> = (0..width_c * height_c)
            .into_par_iter()
            .map(|k| {
                let (s0, s1, s2) = (down[k], frame.cb[k], frame.cr[k]);
                let (float, _) = self.decoder.reshape_chroma(
                    Decoder::normalize(s0),
                    Decoder::normalize(s1),
                    Decoder::normalize(s2),
                );
                let fixed = [0, 1].map(|c| f32::from(self.spec.chroma(c, s0, s1, s2)) / OUTPUT_ONE);
                (float, fixed)
            })
            .collect();
        let float_planes: [Vec<f32>; 2] =
            [0, 1].map(|c| composed.iter().map(|(float, _)| float[c]).collect());
        let fixed_planes: [Vec<f32>; 2] =
            [0, 1].map(|c| composed.iter().map(|(_, fixed)| fixed[c]).collect());
        let float = float_planes
            .each_ref()
            .map(|data| Plane::new(data, width_c, height_c));
        let fixed = fixed_planes
            .each_ref()
            .map(|data| Plane::new(data, width_c, height_c));
        let bl = [
            Plane {
                data: frame.cb,
                width_c,
                height_c,
            },
            Plane {
                data: frame.cr,
                width_c,
                height_c,
            },
        ];

        (0..height)
            .into_par_iter()
            .flat_map_iter(|y| {
                let (float, fixed, bl) = (&float, &fixed, &bl);
                (0..width).map(move |x| {
                    let y_code = frame.y[y * width + x];
                    let luma = self.decoder.luma_term_f32(y_code);
                    let quad = (y / 2) * width_c + x / 2;
                    let mut out = [0.0_f32; 9];
                    out[0] = self.max_rgb(
                        self.decoder
                            .rgb_pq(y_code, frame.cb[quad], frame.cr[quad])
                            .0,
                    );
                    for (k, mode) in [Upsample::Nearest, Upsample::Left, Upsample::TopLeft]
                        .into_iter()
                        .enumerate()
                    {
                        let chroma = [float[0].sample(mode, x, y), float[1].sample(mode, x, y)];
                        out[1 + k] = self.max_rgb(self.decoder.compose(luma, chroma));
                        let chroma = [fixed[0].sample(mode, x, y), fixed[1].sample(mode, x, y)];
                        out[4 + k] = self.max_rgb(
                            self.decoder
                                .compose(self.fixed_luma_term[usize::from(y_code)], chroma),
                        );
                    }
                    for (slot, mode) in [
                        (RENDERER_LEFT, Upsample::Left),
                        (RENDERER_TOPLEFT, Upsample::TopLeft),
                    ] {
                        let (chroma, _) = self.decoder.reshape_chroma(
                            Decoder::normalize(y_code),
                            bl[0].sample(mode, x, y) / 1023.0,
                            bl[1].sample(mode, x, y) / 1023.0,
                        );
                        out[slot] = self.max_rgb(self.decoder.compose(luma, chroma));
                    }
                    out
                })
            })
            .collect()
    }
}

/// One 10-bit 4:2:0 frame, codes masked to 10 bits.
pub struct Frame<'a> {
    pub width: usize,
    pub height: usize,
    pub y: &'a [u16],
    pub cb: &'a [u16],
    pub cr: &'a [u16],
}

/// Per-variant frame statistics in 12-bit codes.
pub struct FrameStats {
    pub max: [f64; 9],
    pub avg: [f64; 9],
    /// |variant − analyzer| per pixel: 99th percentile and maximum.
    pub diff_p99: [f64; 9],
    pub diff_max: [f64; 9],
    /// Maximum |variant − analyzer| over the region-of-interest pixels, when a mask is given.
    pub roi_diff_max: Option<[f64; 9]>,
}

fn code(pq: f32) -> f64 {
    f64::from(pq) * 4095.0
}

fn percentile(histogram: &[u64], fraction: f64) -> f64 {
    let total: u64 = histogram.iter().sum();
    let target = (total as f64 * fraction).ceil() as u64;
    let mut seen = 0;
    for (bin, count) in histogram.iter().enumerate() {
        seen += count;
        if seen >= target.max(1) {
            return bin as f64 * DIFF_STEP;
        }
    }
    (histogram.len() - 1) as f64 * DIFF_STEP
}

pub fn frame_stats(values: &[[f32; 9]], mask: Option<&[u8]>) -> FrameStats {
    let n = values.len() as f64;
    let mut max = [0.0_f64; 9];
    let mut sum = [0.0_f64; 9];
    let mut diff_max = [0.0_f64; 9];
    let mut roi = [0.0_f64; 9];
    let mut histogram = vec![[0_u64; DIFF_BINS]; 9];
    for (i, pixel) in values.iter().enumerate() {
        let in_roi = mask.is_some_and(|m| m[i] != 0);
        for v in 0..9 {
            let value = code(pixel[v]);
            max[v] = max[v].max(value);
            sum[v] += f64::from(pixel[v]);
            let diff = (value - code(pixel[0])).abs();
            diff_max[v] = diff_max[v].max(diff);
            histogram[v][((diff / DIFF_STEP) as usize).min(DIFF_BINS - 1)] += 1;
            if in_roi {
                roi[v] = roi[v].max(diff);
            }
        }
    }
    FrameStats {
        max,
        avg: sum.map(|s| s / n * 4095.0),
        diff_p99: std::array::from_fn(|v| percentile(&histogram[v], 0.99)),
        diff_max,
        roi_diff_max: mask.map(|_| roi),
    }
}

/// libplacebo's render (packed rgba64le) against the two renderer variants: per-pixel
/// |difference| p99 and maximum over the region of interest, and the frame-max difference.
pub fn render_anchor(
    values: &[[f32; 9]],
    render: &[u8],
    mask: Option<&[u8]>,
    source_range: [f32; 2],
) -> [[f64; 3]; 2] {
    let rendered: Vec<f64> = render
        .chunks_exact(8)
        .map(|px| {
            let channel = |k: usize| f64::from(u16::from_le_bytes([px[2 * k], px[2 * k + 1]]));
            let max = channel(0).max(channel(1)).max(channel(2)) / 65535.0;
            (max.clamp(f64::from(source_range[0]), f64::from(source_range[1]))) * 4095.0
        })
        .collect();
    let render_max = rendered.iter().copied().fold(0.0, f64::max);
    [RENDERER_LEFT, RENDERER_TOPLEFT].map(|slot| {
        let mut histogram = vec![0_u64; DIFF_BINS];
        let mut roi_max = 0.0_f64;
        let mut variant_max = 0.0_f64;
        for (i, (pixel, reference)) in values.iter().zip(&rendered).enumerate() {
            let value = code(pixel[slot]);
            variant_max = variant_max.max(value);
            let diff = (value - reference).abs();
            histogram[((diff / DIFF_STEP) as usize).min(DIFF_BINS - 1)] += 1;
            if mask.is_none_or(|m| m[i] != 0) {
                roi_max = roi_max.max(diff);
            }
        }
        [
            percentile(&histogram, 0.99),
            roi_max,
            (variant_max - render_max).abs(),
        ]
    })
}

struct Options {
    width: usize,
    height: usize,
    every: usize,
    render: Option<String>,
    mask: Option<String>,
    anchor_out: Option<String>,
}

fn parse(args: &[String]) -> Result<(String, Options)> {
    const USAGE: &str = "chroma-siting <preset|bt2100> <width> <height> [--every N] \
                         [--render <rgba64le>] [--mask <u8 per pixel>] [--anchor-out <csv>] < yuv420p10le";
    let composer = args.first().context(USAGE)?.clone();
    let width: usize = args.get(1).context(USAGE)?.parse()?;
    let height: usize = args.get(2).context(USAGE)?.parse()?;
    ensure!(
        width % 2 == 0 && height % 2 == 0,
        "width and height must be even"
    );
    let mut options = Options {
        width,
        height,
        every: 1,
        render: None,
        mask: None,
        anchor_out: None,
    };
    let mut rest = args[3..].iter();
    while let Some(arg) = rest.next() {
        let mut value = || {
            rest.next()
                .cloned()
                .with_context(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--every" => options.every = value()?.parse::<usize>()?.max(1),
            "--render" => options.render = Some(value()?),
            "--mask" => options.mask = Some(value()?),
            "--anchor-out" => options.anchor_out = Some(value()?),
            other => bail!("unknown argument {other}\n{USAGE}"),
        }
    }
    ensure!(
        options.render.is_none() || options.anchor_out.is_some(),
        "--render needs --anchor-out"
    );
    Ok((composer, options))
}

fn read_frame(reader: &mut impl Read, buffer: &mut [u8]) -> Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..])? {
            0 if filled == 0 => return Ok(false),
            0 => bail!("truncated frame: {filled} of {} bytes", buffer.len()),
            n => filled += n,
        }
    }
    Ok(true)
}

fn codes(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]) & 0x03FF)
        .collect()
}

pub fn run(args: &[String]) -> Result<()> {
    let (composer, options) = parse(args)?;
    let composer = match composer.as_str() {
        "preset" => Composer::Preset,
        "bt2100" => Composer::Bt2100V1,
        other => bail!("unknown composer {other} (preset or bt2100)"),
    };
    let variants = Variants::new(&composer.rpu_data_mapping())?;
    let (width, height) = (options.width, options.height);
    let luma_bytes = width * height * 2;
    let chroma_bytes = luma_bytes / 4;
    let mut buffer = vec![0_u8; luma_bytes + 2 * chroma_bytes];
    let mut input = BufReader::with_capacity(1 << 22, std::io::stdin().lock());
    let mut render = options
        .render
        .as_deref()
        .map(|path| {
            File::open(path)
                .map(BufReader::new)
                .with_context(|| format!("open {path}"))
        })
        .transpose()?;
    let mut mask = options
        .mask
        .as_deref()
        .map(|path| {
            File::open(path)
                .map(BufReader::new)
                .with_context(|| format!("open {path}"))
        })
        .transpose()?;
    let mut anchor = options
        .anchor_out
        .as_deref()
        .map(|path| {
            File::create(path)
                .map(BufWriter::new)
                .with_context(|| format!("create {path}"))
        })
        .transpose()?;
    if let Some(anchor) = anchor.as_mut() {
        writeln!(anchor, "frame,variant,p99,roi_max,frame_max_diff")?;
    }
    let mut render_buffer = vec![0_u8; width * height * 8];
    let mut mask_buffer = vec![0_u8; width * height];
    let mut out = BufWriter::new(std::io::stdout().lock());
    writeln!(out, "frame,variant,max,avg,diff_p99,diff_max,roi_diff_max")?;

    let mut index = 0_usize;
    while read_frame(&mut input, &mut buffer)? {
        let has_render = match render.as_mut() {
            Some(reader) => read_frame(reader, &mut render_buffer)?,
            None => false,
        };
        let has_mask = match mask.as_mut() {
            Some(reader) => read_frame(reader, &mut mask_buffer)?,
            None => false,
        };
        if index % options.every == 0 {
            let y = codes(&buffer[..luma_bytes]);
            let cb = codes(&buffer[luma_bytes..luma_bytes + chroma_bytes]);
            let cr = codes(&buffer[luma_bytes + chroma_bytes..]);
            let frame = Frame {
                width,
                height,
                y: &y,
                cb: &cb,
                cr: &cr,
            };
            let values = variants.frame(&frame);
            let mask = has_mask.then_some(mask_buffer.as_slice());
            let stats = frame_stats(&values, mask);
            for (v, name) in VARIANTS.iter().enumerate() {
                let roi = stats
                    .roi_diff_max
                    .map(|roi| format!("{:.3}", roi[v]))
                    .unwrap_or_default();
                writeln!(
                    out,
                    "{index},{name},{:.3},{:.3},{:.2},{:.3},{roi}",
                    stats.max[v], stats.avg[v], stats.diff_p99[v], stats.diff_max[v]
                )?;
            }
            if let (true, Some(anchor)) = (has_render, anchor.as_mut()) {
                let result = render_anchor(&values, &render_buffer, mask, variants.source_range);
                for (slot, name) in [(RENDERER_LEFT, 0), (RENDERER_TOPLEFT, 1)] {
                    let [p99, roi_max, frame_max] = result[name];
                    writeln!(
                        anchor,
                        "{index},{},{p99:.2},{roi_max:.3},{frame_max:.3}",
                        VARIANTS[slot]
                    )?;
                }
            }
        }
        index += 1;
    }
    out.flush()?;
    if let Some(mut anchor) = anchor {
        anchor.flush()?;
    }
    eprintln!(
        "chroma-siting: {index} frames read, every {} analyzed",
        options.every
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_follow_the_chroma_location() {
        // Horizontal (both sitings): co-sited with even columns, midway at odd ones.
        assert_eq!(taps(4, 8, false), [(2, 1.0), (2, 0.0)]);
        assert_eq!(taps(5, 8, false), [(2, 0.5), (3, 0.5)]);
        assert_eq!(taps(15, 8, false), [(7, 0.5), (7, 0.5)]);
        // Vertical, type 0: chroma row j sits between luma rows 2j and 2j+1.
        assert_eq!(taps(4, 8, true), [(2, 0.75), (1, 0.25)]);
        assert_eq!(taps(5, 8, true), [(2, 0.75), (3, 0.25)]);
        assert_eq!(taps(0, 8, true), [(0, 0.75), (0, 0.25)]);
        assert_eq!(taps(15, 8, true), [(7, 0.75), (7, 0.25)]);
    }

    #[test]
    fn spec_luma_downsampling_with_edge_replication() {
        #[rustfmt::skip]
        let luma: Vec<u16> = vec![
            100, 200, 300, 400,
            500, 600, 700, 800,
            10, 20, 30, 40,
            50, 60, 70, 80,
        ];
        let down = downsample_luma(&luma, 4, 4);
        // (i, j) = (0, 0): left edge replicates column 0.
        let row = |a: u32, b: u32, c: u32| (a + 2 * b + c + 2) >> 2;
        let expect = |r0: u32, r1: u32| ((r0 + r1 + 1) >> 1) as u16;
        assert_eq!(down[0], expect(row(100, 100, 200), row(500, 500, 600)));
        assert_eq!(down[1], expect(row(200, 300, 400), row(600, 700, 800)));
        assert_eq!(down[2], expect(row(10, 10, 20), row(50, 50, 60)));
        assert_eq!(down[3], expect(row(20, 30, 40), row(60, 70, 80)));
    }

    /// The fixed-point composer is the float composer evaluated at `code / 1024`, to within its
    /// 16-bit output and intermediate truncation.
    #[test]
    fn fixed_point_matches_float_at_code_over_1024() {
        for composer in Composer::ALL {
            let mapping = composer.rpu_data_mapping();
            let spec = SpecComposer::new(&mapping).unwrap();
            let decoder = Decoder::new(&mapping);
            for s0 in (64..=1019).step_by(37) {
                for s1 in (64..=960).step_by(53) {
                    for s2 in (64..=960).step_by(59) {
                        // The spec clamps MMR inputs to the pivot ranges; the float decoder
                        // does not (the preset's last luma pivot is 942).
                        let inside = |pivots: &[i64], s: u16| {
                            (pivots[0]..=pivots[pivots.len() - 1]).contains(&i64::from(s))
                        };
                        if !inside(&spec.luma_pivots, s0)
                            || !inside(&spec.chroma_pivots[0], s1)
                            || !inside(&spec.chroma_pivots[1], s2)
                        {
                            continue;
                        }
                        let (float, _) = decoder.reshape_chroma(
                            s0 as f32 / 1024.0,
                            s1 as f32 / 1024.0,
                            s2 as f32 / 1024.0,
                        );
                        for (c, expected) in float.iter().enumerate() {
                            let fixed = f32::from(spec.chroma(c, s0, s1, s2)) / OUTPUT_ONE;
                            assert!(
                                (fixed - expected).abs() < 2e-4,
                                "{composer:?} c{c} ({s0},{s1},{s2}): {fixed} vs {expected}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fixed_point_luma_matches_the_polynomial_at_code_over_1024() {
        for composer in Composer::ALL {
            let mapping = composer.rpu_data_mapping();
            let spec = SpecComposer::new(&mapping).unwrap();
            let luma = &mapping.curves[0];
            let poly = luma.polynomial.as_ref().unwrap();
            let pivots = cumulative_pivots(luma);
            for code in pivots[0]..=pivots[pivots.len() - 1] {
                let piece = (0..pivots.len() - 1)
                    .find(|&k| code < pivots[k + 1])
                    .unwrap_or(pivots.len() - 2);
                let s = code as f64 / 1024.0;
                let expected: f64 = (0..=poly.poly_order_minus1[piece] as usize + 1)
                    .map(|i| {
                        crate::model::fixed(poly.poly_coef_int[piece][i], poly.poly_coef[piece][i])
                            * s.powi(i as i32)
                    })
                    .sum();
                let fixed = f64::from(spec.luma(code as u16)) / f64::from(OUTPUT_ONE);
                assert!(
                    (fixed - expected.clamp(0.0, 65535.0 / 65536.0)).abs() < 3e-5,
                    "{composer:?} code {code}: {fixed} vs {expected}"
                );
            }
        }
    }

    #[test]
    fn flat_frames_agree_where_the_clamp_policies_agree() {
        let mapping = Composer::Bt2100V1.rpu_data_mapping();
        let variants = Variants::new(&mapping).unwrap();
        // In the pivot ranges, so the spec's input clamps are inactive.
        for (y, cb, cr) in [
            (600_u16, 600_u16, 450_u16),
            (900, 512, 512),
            (300, 700, 400),
        ] {
            let (w, h) = (8, 6);
            let luma = vec![y; w * h];
            let (cbs, crs) = (vec![cb; w * h / 4], vec![cr; w * h / 4]);
            let frame = Frame {
                width: w,
                height: h,
                y: &luma,
                cb: &cbs,
                cr: &crs,
            };
            for pixel in variants.frame(&frame) {
                for v in [1, 2, 3, 7, 8] {
                    assert!(
                        (pixel[v] - pixel[0]).abs() < 1e-6,
                        "{} at ({y},{cb},{cr}): {} vs {}",
                        VARIANTS[v],
                        pixel[v],
                        pixel[0]
                    );
                }
            }
        }
    }
}
