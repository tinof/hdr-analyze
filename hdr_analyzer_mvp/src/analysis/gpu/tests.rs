use super::*;
use crate::analysis::hlg::{downsample_luma, upsample_chroma, ChromaSiting};

#[test]
fn pq_lut_matches_limited_range_contract() {
    let lut = build_transfer_lut(TransferFunction::Pq, Composer::Preset);
    assert_eq!(lut.len(), 1024);
    assert_eq!(lut[0], 0.0);
    assert_eq!(lut[64], 0.0);
    assert!((f64::from(lut[502]) - 0.5).abs() < 1.0e-6);
    assert_eq!(lut[940], 1.0);
    assert_eq!(lut[1023], 1.0);
}

#[test]
fn hlg_lut_is_monotonic_and_peak_limited() {
    let lut = build_transfer_lut(TransferFunction::Hlg, Composer::Preset);
    // The DV 8.4 curve has a ~3e-7 PQ seam at the 910/911 piece boundary.
    assert!(lut
        .windows(2)
        .all(|pair| f64::from(pair[1]) >= f64::from(pair[0]) - 1.0e-6));
    let source_max = (3079.0_f64 / 4095.0) as f32;
    assert_eq!(lut[940], source_max);
    assert_eq!(lut[1023], source_max);
    assert!(lut.iter().all(|&pq| pq <= source_max));
}

#[test]
fn hlg_transfer_lut_equals_dovi84_lut_bit_exactly() {
    for composer in Composer::ALL {
        let lut = build_transfer_lut(TransferFunction::Hlg, composer);
        let reference = dovi84_pq_lut(composer);
        assert_eq!(lut.len(), reference.len());
        for (code, (&gpu, &cpu)) in lut.iter().zip(reference.iter()).enumerate() {
            assert_eq!(gpu.to_bits(), cpu.to_bits(), "{composer:?} code {code}");
        }
    }
}

/// Line-by-line transcription of `dovi84_reshape_chroma` as `compose_chroma` in
/// kernels.cu calls it, reading the flat parameter buffer by the kernel's offsets.
fn kernel_composed(params: &[f32], mmr_luma: u16, cb_code: u16, cr_code: u16) -> [f32; 2] {
    let reshape = |mmr: &[f32], y: f32, u: f32, v: f32| {
        let (yu, yv, uv) = (y * u, y * v, u * v);
        let x = [y, u, v, yu, yv, uv, yu * v];
        let mut s = mmr[0];
        for order in 0..MMR_MAX_ORDER {
            for (term, &value) in x.iter().enumerate() {
                let mut p = value;
                if order >= 1 {
                    p = value * value;
                }
                if order == 2 {
                    p *= value;
                }
                s += mmr[1 + order * MMR_TERMS + term] * p;
            }
        }
        s.max(params[DOVI_CHROMA_CLAMP])
            .min(params[DOVI_CHROMA_CLAMP + 1])
    };
    let (y, u, v) = (
        f32::from(mmr_luma) / 1023.0,
        f32::from(cb_code) / 1023.0,
        f32::from(cr_code) / 1023.0,
    );
    [
        reshape(&params[DOVI_MMR_CB..], y, u, v),
        reshape(&params[DOVI_MMR_CR..], y, u, v),
    ]
}

/// Transcription of `dovi84_max_rgb_composed` in kernels.cu.
fn kernel_max_rgb_composed(params: &[f32], y_code: u16, composed: [f32; 2]) -> f32 {
    let cb = composed[0] - params[DOVI_CHROMA_OFFSET];
    let cr = composed[1] - params[DOVI_CHROMA_OFFSET + 1];
    let luma = params[usize::from(y_code)];
    let m = &params[DOVI_YCC..];
    let red = luma + m[0] * cb + m[1] * cr;
    let green = luma + m[2] * cb + m[3] * cr;
    let blue = luma + m[4] * cb + m[5] * cr;
    red.max(green)
        .max(blue)
        .max(params[DOVI_SOURCE_RANGE])
        .min(params[DOVI_SOURCE_RANGE + 1])
}

/// Transcription of `downsample_luma` in kernels.cu (signed arithmetic, as there).
fn kernel_downsample(luma: &[u16], width: i32, height: i32, cx: i32, cy: i32) -> u16 {
    let code = |x: i32, y: i32| u32::from(luma[(y * width + x) as usize]);
    let mut x = 2 * cx;
    let left = (x - 1).max(0);
    let right = (x + 1).min(width - 1);
    x = x.min(width - 1);
    let rows: [u32; 2] = std::array::from_fn(|k| {
        let y = (2 * cy + k as i32).min(height - 1);
        (code(left, y) + 2 * code(x, y) + code(right, y) + 2) >> 2
    });
    ((rows[0] + rows[1] + 1) >> 1) as u16
}

/// Transcription of `chroma_taps` in kernels.cu.
fn kernel_taps(pos: i32, len: i32, centred: bool) -> (i32, f32, i32, f32) {
    let j = (pos / 2).min(len - 1);
    let last = len - 1;
    match (centred, pos & 1) {
        (false, 0) => (j, 1.0, j, 0.0),
        (false, _) => (j, 0.5, (j + 1).min(last), 0.5),
        (true, 0) => (j, 0.75, (j - 1).max(0), 0.25),
        (true, _) => (j, 0.75, (j + 1).min(last), 0.25),
    }
}

/// Transcription of `upsample_chroma` in kernels.cu, on the interleaved composed buffer.
#[allow(clippy::too_many_arguments)]
fn kernel_upsample(
    composed: &[f32],
    component: i32,
    (cx0, cy0, rect_width): (i32, i32, i32),
    chroma_width: i32,
    chroma_height: i32,
    centred: bool,
    x: i32,
    y: i32,
) -> f32 {
    let (c0, cw0, c1, cw1) = kernel_taps(x, chroma_width, false);
    let (r0, rw0, r1, rw1) = kernel_taps(y, chroma_height, centred);
    let at = |cx: i32, cy: i32| {
        composed[(2 * ((cy - cy0) * rect_width + (cx - cx0)) + component) as usize]
    };
    let row0 = cw0 * at(c0, r0) + cw1 * at(c1, r0);
    let row1 = cw0 * at(c0, r1) + cw1 * at(c1, r1);
    rw0 * row0 + rw1 * row1
}

#[test]
fn dovi84_kernel_params_reproduce_the_cpu_decoder_bit_exactly() {
    for composer in Composer::ALL {
        let decoder = dovi84_decoder(composer);
        let params = build_dovi84_params(decoder);
        assert_eq!(params.len(), DOVI_PARAM_WORDS);
        for mmr_luma in (0..1024_u16).step_by(7) {
            for cb in (0..1024_u16).step_by(31) {
                for cr in (0..1024_u16).step_by(29) {
                    let cpu = decoder.composed_chroma(mmr_luma, &decoder.chroma(cb, cr));
                    let kernel = kernel_composed(&params, mmr_luma, cb, cr);
                    assert_eq!(
                        cpu.map(f32::to_bits),
                        kernel.map(f32::to_bits),
                        "{composer:?} codes ({mmr_luma}, {cb}, {cr})"
                    );
                    // The pixel's own luma differs from the MMR's in general.
                    let y = 1023 - mmr_luma;
                    assert_eq!(
                        decoder.max_rgb_pq_composed(y, cpu).to_bits(),
                        kernel_max_rgb_composed(&params, y, kernel).to_bits(),
                        "{composer:?} pixel {y} with composed chroma of ({mmr_luma}, {cb}, {cr})"
                    );
                }
            }
        }
    }
}

/// The kernels' spatial decode (pre-pass rectangle, down-sampling, taps, upsampling) equals
/// the CPU's building blocks pixel for pixel, on even and odd frames, both chroma
/// locations and crops at and away from the frame edge.
#[test]
fn dovi84_kernel_spatial_decode_matches_the_cpu() {
    for composer in Composer::ALL {
        let decoder = dovi84_decoder(composer);
        let params = build_dovi84_params(decoder);
        for (width, height) in [(10_i32, 6_i32), (9, 5)] {
            let (chroma_width, chroma_height) = ((width + 1) / 2, (height + 1) / 2);
            let mut state = 0x2545_f491_u32;
            let mut code = |range: u32| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state % range) as u16
            };
            let luma: Vec<u16> = (0..width * height).map(|_| 64 + code(877)).collect();
            let cb: Vec<u16> = (0..chroma_width * chroma_height)
                .map(|_| 64 + code(897))
                .collect();
            let cr: Vec<u16> = (0..chroma_width * chroma_height)
                .map(|_| 64 + code(897))
                .collect();
            for siting in [ChromaSiting::Left, ChromaSiting::TopLeft] {
                let centred = siting == ChromaSiting::Left;
                for (crop_x, crop_y, crop_width, crop_height) in
                    [(0, 0, width, height), (2, 2, width - 4, height - 3)]
                {
                    // As launch_and_collect.
                    let cx0 = crop_x / 2;
                    let cy0 = (crop_y / 2 - 1).max(0);
                    let cx1 = ((crop_x + crop_width - 1) / 2 + 1).min(chroma_width - 1);
                    let cy1 = ((crop_y + crop_height - 1) / 2 + 1).min(chroma_height - 1);
                    let rect_width = cx1 + 1 - cx0;
                    let mut composed = Vec::new();
                    for cy in cy0..=cy1 {
                        for cx in cx0..=cx1 {
                            let index = (cy * chroma_width + cx) as usize;
                            let mmr_luma = kernel_downsample(&luma, width, height, cx, cy);
                            composed.extend(kernel_composed(
                                &params, mmr_luma, cb[index], cr[index],
                            ));
                        }
                    }
                    let cpu_composed = |cx: usize, cy: usize| {
                        let mmr_luma = downsample_luma(
                            |x, y| luma[y * width as usize + x],
                            width as usize,
                            height as usize,
                            cx,
                            cy,
                        );
                        let index = cy * chroma_width as usize + cx;
                        decoder.composed_chroma(mmr_luma, &decoder.chroma(cb[index], cr[index]))
                    };
                    for y in crop_y..crop_y + crop_height {
                        for x in crop_x..crop_x + crop_width {
                            let pixel = luma[(y * width + x) as usize];
                            let cpu_chroma: [f32; 2] = std::array::from_fn(|k| {
                                upsample_chroma(
                                    |cx, cy| cpu_composed(cx, cy)[k],
                                    chroma_width as usize,
                                    chroma_height as usize,
                                    siting,
                                    x as usize,
                                    y as usize,
                                )
                            });
                            let kernel_chroma: [f32; 2] = std::array::from_fn(|k| {
                                kernel_upsample(
                                    &composed,
                                    k as i32,
                                    (cx0, cy0, rect_width),
                                    chroma_width,
                                    chroma_height,
                                    centred,
                                    x,
                                    y,
                                )
                            });
                            assert_eq!(
                                decoder.max_rgb_pq_composed(pixel, cpu_chroma).to_bits(),
                                kernel_max_rgb_composed(&params, pixel, kernel_chroma).to_bits(),
                                "{composer:?} {width}x{height} {siting:?} crop ({crop_x}, {crop_y}) pixel ({x}, {y})"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn result_diff_hist_reads_the_words_after_the_sums() {
    let mut results = vec![0u32; RESULT_WORDS];
    // kernels.cu: results[DIFF_WORD + band * DIFF_BINS + difference]
    results[DIFF_WORD] = 7;
    results[DIFF_WORD + 3 * DIFF_BINS + 5] = 11;
    results[RESULT_WORDS - 1] = 13;
    let diff_hist = result_diff_hist(&results);
    assert_eq!(diff_hist[0][0], 7);
    assert_eq!(diff_hist[3][5], 11);
    assert_eq!(diff_hist[DIFF_VALUE_BANDS - 1][DIFF_BINS - 1], 13);
    let total: u32 = diff_hist.iter().flatten().sum();
    assert_eq!(total, 31);
    assert_eq!(DIFF_WORD, 4392);
    assert_eq!(RESULT_WORDS, 5416);
}

#[test]
fn result_sums_read_the_kernels_u64_words() {
    let mut results = vec![0u32; RESULT_WORDS];
    let expected = [u64::MAX - 5, 1u64 << 32, 8_294_400];
    for (index, value) in expected.iter().enumerate() {
        let bytes = value.to_ne_bytes();
        let offset = SUMS_WORD + 2 * index;
        results[offset] = u32::from_ne_bytes(bytes[..4].try_into().unwrap());
        results[offset + 1] = u32::from_ne_bytes(bytes[4..].try_into().unwrap());
    }
    assert_eq!(result_sums(&results), expected);
}

#[test]
fn f32_fixed_point_matches_the_former_f64_conversion() {
    // The kernel now computes truncate(x * 2^32) in f32; the scale is a power of two,
    // so the product is exact and equals the old f64 path for every x in [0, 1].
    let check = |x: f32| {
        let old = (f64::from(x) * 4_294_967_296.0) as u64;
        let new = (x * 4_294_967_296.0_f32) as u64;
        assert_eq!(old, new, "x = {x:e}");
    };
    for x in [0.0, f32::MIN_POSITIVE, 1.0e-30, 0.5, 0.999_999_94, 1.0] {
        check(x);
    }
    for lut in [
        build_transfer_lut(TransferFunction::Pq, Composer::Preset),
        build_transfer_lut(TransferFunction::Hlg, Composer::Preset),
        build_transfer_lut(TransferFunction::Hlg, Composer::Bt2100V1),
    ] {
        lut.into_iter().for_each(check);
    }
}

#[test]
fn luminance_bin_lut_uses_exact_cpu_boundaries() {
    let bins = build_luminance_bin_lut(TransferFunction::Pq, Composer::Preset);
    let sdr_peak_pq = nits_to_pq(100.0);
    let sdr_step = sdr_peak_pq / 64.0;
    for (code, &bin) in bins.iter().enumerate() {
        let pq = pq_for_code(code as i32, TransferFunction::Pq, Composer::Preset);
        if pq < sdr_peak_pq {
            assert_eq!(usize::from(bin), (pq / sdr_step).floor() as usize);
        }
    }
}
