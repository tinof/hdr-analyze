// Single-launch HDR frame analysis kernel.
//
// One result buffer, cleared by one memset and downloaded by one copy.
// u32 words:
//   [0, 256)          v5 luminance histogram (binned via luminance_bin_lut)
//   [256, 287)        hue histogram (31 bins over the full hue circle)
//   [287, 287+4096)   4096-bin PQ histogram in the selected peak domain
//   [4383]            max luma PQ observed (f32 bit pattern)
//   [4384]            max max-RGB PQ observed (f32 bit pattern)
//   [4385]            padding, so the u64 sums are 8-byte aligned
// u64 words from u32 word SUMS_WORD (4386):
//   [0] sum of luma PQ, fixed point * 2^32
//   [1] sum of max-RGB PQ, fixed point * 2^32
//   [2] analyzed pixel count
// u32 words from DIFF_WORD (SUMS_WORD + 6), filled only when grain_stats is set:
//   [band * DIFF_BINS + diff]  cross-quad difference histogram of the grain-robust peak
//                              estimator, DIFF_BANDS x DIFF_BINS counts (see frame.rs)
//
// Only integer counts, u64 fixed-point sums and non-negative f32 bit-pattern maxima are
// combined across threads, and all of them are order-independent, so the launch shape
// (grid-stride loop, warp reductions) never changes the result.
// dovi_params layout (f32 words), built by gpu.rs `build_dovi84_params` from
// analysis::hlg::Dovi84Decoder; used only when dovi84_rgb is set (HLG input):
//   [0, 1024)       luma term per 10-bit code, coef0 * (Y' - offset0), unclamped
//   DOVI_MMR_CB     Cb MMR: constant, then coef[order][term] (3 orders x 7 terms)
//   DOVI_MMR_CR     Cr MMR, same layout
//   DOVI_YCC        ycc_to_rgb chroma columns: R<-Cb, R<-Cr, G<-Cb, G<-Cr, B<-Cb, B<-Cr
//   DOVI_CHROMA_OFFSET  Cb, Cr offsets
//   DOVI_CHROMA_CLAMP   reshaped chroma lo, hi
//   DOVI_SOURCE_RANGE   max-RGB lo, hi (source_min_pq, source_max_pq) / 4095
#define DOVI_MMR_TERMS 7
#define DOVI_MMR_ORDERS 3
#define DOVI_MMR_WORDS (1 + DOVI_MMR_ORDERS * DOVI_MMR_TERMS)
#define DOVI_MMR_CB 1024
#define DOVI_MMR_CR (DOVI_MMR_CB + DOVI_MMR_WORDS)
#define DOVI_YCC (DOVI_MMR_CR + DOVI_MMR_WORDS)
#define DOVI_CHROMA_OFFSET (DOVI_YCC + 6)
#define DOVI_CHROMA_CLAMP (DOVI_CHROMA_OFFSET + 2)
#define DOVI_SOURCE_RANGE (DOVI_CHROMA_CLAMP + 2)
#define LUM_BINS 256
#define HUE_BINS 31
#define PQ_BINS 4096
#define PQ_HIST_BASE (LUM_BINS + HUE_BINS)
#define MAX_LUMA_WORD (PQ_HIST_BASE + PQ_BINS)
#define MAX_RGB_WORD (MAX_LUMA_WORD + 1)
#define SUMS_WORD (MAX_RGB_WORD + 2)
#define DIFF_BANDS 16
#define DIFF_BINS 64
#define DIFF_WORDS (DIFF_BANDS * DIFF_BINS)
#define DIFF_WORD (SUMS_WORD + 6)
// 2^32 as f32. x * 2^32 is an exact exponent shift for finite x in [0, 1], so truncating
// it with __float2ull_rz equals the former (unsigned long long)((double)x * 2^32).
#define FIXED_POINT_SCALE 4294967296.0f
#define TWO_PI 6.28318530717958647692f
#define FULL_WARP 0xffffffffu

__device__ __forceinline__ unsigned long long fixed_point(float x) {
    return __float2ull_rz(__fmul_rn(x, FIXED_POINT_SCALE));
}

__device__ __forceinline__ unsigned long long warp_sum_u64(unsigned long long value) {
    for (int offset = 16; offset > 0; offset >>= 1) {
        value += __shfl_xor_sync(FULL_WARP, value, offset);
    }
    return value;
}

__device__ __forceinline__ unsigned int warp_sum_u32(unsigned int value) {
    for (int offset = 16; offset > 0; offset >>= 1) {
        value += __shfl_xor_sync(FULL_WARP, value, offset);
    }
    return value;
}

__device__ __forceinline__ unsigned int warp_max_u32(unsigned int value) {
    for (int offset = 16; offset > 0; offset >>= 1) {
        value = max(value, __shfl_xor_sync(FULL_WARP, value, offset));
    }
    return value;
}

// One chroma component through its single-piece MMR curve. Mirrors
// Dovi84Decoder::reshape_chroma operation for operation; the _rn intrinsics keep
// NVRTC from contracting multiply-adds into FMAs, so results match the CPU bit for bit.
__device__ float dovi84_reshape_chroma(
    const float* mmr,
    const float* chroma_clamp,
    float y,
    float u,
    float v
) {
    const float yu = __fmul_rn(y, u);
    const float yv = __fmul_rn(y, v);
    const float uv = __fmul_rn(u, v);
    const float yuv = __fmul_rn(yu, v);
    const float x[DOVI_MMR_TERMS] = {y, u, v, yu, yv, uv, yuv};
    float s = mmr[0];
    for (int order = 0; order < DOVI_MMR_ORDERS; order++) {
        for (int term = 0; term < DOVI_MMR_TERMS; term++) {
            float p = x[term];
            if (order >= 1) {
                p = __fmul_rn(x[term], x[term]);
            }
            if (order == 2) {
                p = __fmul_rn(p, x[term]);
            }
            s = __fadd_rn(s, __fmul_rn(mmr[1 + order * DOVI_MMR_TERMS + term], p));
        }
    }
    return fminf(fmaxf(s, chroma_clamp[0]), chroma_clamp[1]);
}

// max(R', G', B') in PQ of the Dolby Vision Profile 8.4 decode; mirrors
// Dovi84Decoder::max_rgb_pq.
__device__ float dovi84_max_rgb_pq(
    const float* params,
    unsigned int y_code,
    unsigned int cb_code,
    unsigned int cr_code
) {
    const float y = __fdiv_rn((float)y_code, 1023.0f);
    const float u = __fdiv_rn((float)cb_code, 1023.0f);
    const float v = __fdiv_rn((float)cr_code, 1023.0f);
    const float* clamp = params + DOVI_CHROMA_CLAMP;
    const float cb = __fsub_rn(
        dovi84_reshape_chroma(params + DOVI_MMR_CB, clamp, y, u, v), params[DOVI_CHROMA_OFFSET]);
    const float cr = __fsub_rn(
        dovi84_reshape_chroma(params + DOVI_MMR_CR, clamp, y, u, v),
        params[DOVI_CHROMA_OFFSET + 1]);
    const float luma = params[y_code];
    const float* m = params + DOVI_YCC;
    const float red = __fadd_rn(__fadd_rn(luma, __fmul_rn(m[0], cb)), __fmul_rn(m[1], cr));
    const float green = __fadd_rn(__fadd_rn(luma, __fmul_rn(m[2], cb)), __fmul_rn(m[3], cr));
    const float blue = __fadd_rn(__fadd_rn(luma, __fmul_rn(m[4], cb)), __fmul_rn(m[5], cr));
    const float peak = fmaxf(fmaxf(red, green), blue);
    return fminf(fmaxf(peak, params[DOVI_SOURCE_RANGE]), params[DOVI_SOURCE_RANGE + 1]);
}

// Everything the kernel derives from one luma pixel and its co-sited chroma sample.
struct Sample {
    unsigned int code;
    unsigned int cb_code;
    unsigned int cr_code;
    float luma_pq;
    float rgb_peak_pq;
    int pq_bin;
};

__device__ __forceinline__ Sample load_sample(
    const unsigned char* y_plane,
    const unsigned char* u_plane,
    const unsigned char* v_plane,
    const float* transfer_lut,
    const float* dovi_params,
    int x,
    int y,
    int y_stride,
    int u_stride,
    int v_stride,
    int layout,
    int dovi84_rgb,
    int peak_is_max_rgb
) {
    Sample sample;
    const int y_offset = y * y_stride + x * 2;
    const unsigned short raw_y =
        (unsigned short)y_plane[y_offset] |
        ((unsigned short)y_plane[y_offset + 1] << 8);
    sample.code = layout == 1 ? (raw_y >> 6) & 1023u : raw_y & 1023u;
    sample.luma_pq = transfer_lut[sample.code];

    // Co-sited 4:2:0 chroma sample for this pixel's 2x2 quad.
    const int cx = x >> 1;
    const int cy = y >> 1;
    if (layout == 1) {
        const int uv_offset = cy * u_stride + cx * 4;
        const unsigned short raw_u =
            (unsigned short)u_plane[uv_offset] |
            ((unsigned short)u_plane[uv_offset + 1] << 8);
        const unsigned short raw_v =
            (unsigned short)u_plane[uv_offset + 2] |
            ((unsigned short)u_plane[uv_offset + 3] << 8);
        sample.cb_code = (raw_u >> 6) & 1023u;
        sample.cr_code = (raw_v >> 6) & 1023u;
    } else {
        const int u_offset = cy * u_stride + cx * 2;
        const int v_offset = cy * v_stride + cx * 2;
        sample.cb_code = ((unsigned int)u_plane[u_offset] |
            ((unsigned int)u_plane[u_offset + 1] << 8)) & 1023u;
        sample.cr_code = ((unsigned int)v_plane[v_offset] |
            ((unsigned int)v_plane[v_offset + 1] << 8)) & 1023u;
    }

    if (dovi84_rgb) {
        sample.rgb_peak_pq =
            dovi84_max_rgb_pq(dovi_params, sample.code, sample.cb_code, sample.cr_code);
    } else {
        // Same non-constant-luminance approximation as the CPU path (frame.rs): mix the
        // PQ-encoded signal directly in Y'CbCr space. The _rn intrinsics keep NVRTC from
        // contracting multiply-adds into FMAs, so the result matches the CPU bit for bit.
        const float y_signal = __fdiv_rn((float)((int)sample.code - 64), 876.0f);
        const float cb = __fdiv_rn(__fsub_rn((float)sample.cb_code, 512.0f), 896.0f);
        const float cr = __fdiv_rn(__fsub_rn((float)sample.cr_code, 512.0f), 896.0f);
        const float red = __fadd_rn(y_signal, __fmul_rn(1.4746f, cr));
        const float blue = __fadd_rn(y_signal, __fmul_rn(1.8814f, cb));
        const float green = __fdiv_rn(
            __fsub_rn(__fsub_rn(y_signal, __fmul_rn(0.2627f, red)), __fmul_rn(0.0593f, blue)),
            0.6780f);
        const float peak = fmaxf(red, fmaxf(green, blue));
        sample.rgb_peak_pq = fminf(fmaxf(peak, 0.0f), 1.0f);
    }

    const float peak_pq = peak_is_max_rgb ? sample.rgb_peak_pq : sample.luma_pq;
    int pq_bin = (int)__fadd_rn(__fmul_rn(peak_pq, (float)(PQ_BINS - 1)), 0.5f);
    if (pq_bin < 0) {
        pq_bin = 0;
    }
    if (pq_bin > PQ_BINS - 1) {
        pq_bin = PQ_BINS - 1;
    }
    sample.pq_bin = pq_bin;
    return sample;
}

extern "C" __global__ void analyze_frame(
    const unsigned char* y_plane,
    const unsigned char* u_plane,
    const unsigned char* v_plane,
    const float* transfer_lut,
    const unsigned short* luminance_bin_lut,
    const float* dovi_params,
    unsigned int* results,
    int width,
    int height,
    int y_stride,
    int u_stride,
    int v_stride,
    int crop_x,
    int crop_y,
    int crop_width,
    int crop_height,
    int sample_stride,
    int layout,
    int sample_count,
    int dovi84_rgb,
    int peak_is_max_rgb,
    int grain_stats
) {
    __shared__ unsigned int s_lum[LUM_BINS];
    __shared__ unsigned int s_hue[HUE_BINS];
    __shared__ unsigned int s_pq[PQ_BINS];
    __shared__ unsigned int s_diff[DIFF_WORDS];
    __shared__ unsigned long long s_sum_luma;
    __shared__ unsigned long long s_sum_rgb;
    __shared__ unsigned int s_count;
    __shared__ unsigned int s_max_luma;
    __shared__ unsigned int s_max_rgb;

    for (int bin = threadIdx.x; bin < LUM_BINS; bin += blockDim.x) {
        s_lum[bin] = 0;
    }
    for (int bin = threadIdx.x; bin < HUE_BINS; bin += blockDim.x) {
        s_hue[bin] = 0;
    }
    for (int bin = threadIdx.x; bin < PQ_BINS; bin += blockDim.x) {
        s_pq[bin] = 0;
    }
    for (int bin = threadIdx.x; bin < DIFF_WORDS; bin += blockDim.x) {
        s_diff[bin] = 0;
    }
    if (threadIdx.x == 0) {
        s_sum_luma = 0ull;
        s_sum_rgb = 0ull;
        s_count = 0u;
        s_max_luma = 0u;
        s_max_rgb = 0u;
    }
    __syncthreads();

    // Per-thread partials, reduced per warp and then per block after the loop. A grid of
    // a few blocks per SM strides over the samples, so the shared-histogram clear and
    // flush run a few hundred times per frame instead of once per 256 samples.
    unsigned long long t_sum_luma = 0ull;
    unsigned long long t_sum_rgb = 0ull;
    unsigned int t_count = 0u;
    unsigned int t_max_luma = 0u;
    unsigned int t_max_rgb = 0u;

    const long long grid_step = (long long)gridDim.x * blockDim.x;
    for (long long index = (long long)blockIdx.x * blockDim.x + threadIdx.x;
         index < sample_count;
         index += grid_step) {
        const int sample_index = (int)index;
        const int sample_width = (crop_width + sample_stride - 1) / sample_stride;
        const int sx = sample_index % sample_width;
        const int sy = sample_index / sample_width;
        const int x = crop_x + sx * sample_stride;
        const int y = crop_y + sy * sample_stride;

        if (x < crop_x + crop_width && y < crop_y + crop_height && x < width && y < height) {
            const Sample sample = load_sample(
                y_plane, u_plane, v_plane, transfer_lut, dovi_params, x, y,
                y_stride, u_stride, v_stride, layout, dovi84_rgb, peak_is_max_rgb);
            const unsigned int code = sample.code;
            const unsigned int cb_code = sample.cb_code;
            const unsigned int cr_code = sample.cr_code;
            const float luma_pq = sample.luma_pq;
            const float rgb_peak_pq = sample.rgb_peak_pq;
            atomicAdd(&s_lum[luminance_bin_lut[code]], 1u);
            t_max_luma = max(t_max_luma, __float_as_uint(luma_pq));
            t_max_rgb = max(t_max_rgb, __float_as_uint(rgb_peak_pq));
            atomicAdd(&s_pq[sample.pq_bin], 1u);

            // Grain statistics, as frame.rs `record_cross_quad_diff`: on every luma row the
            // first pixel of each chroma quad inside the crop is compared with the one of
            // the quad to its left. With a sampling stride the same rule runs on the grid
            // of sampled pixels; the compared pixels are then 2 * stride apart and picture
            // detail inflates sigma, so the estimator is only meaningful at stride 1. The
            // neighbour is read by position, so the count does not depend on which thread
            // handles which sample.
            if (grain_stats) {
                const int column = sample_stride == 1 ? x : sx;
                const int first_column = sample_stride == 1 ? crop_x : 0;
                const int quad = column >> 1;
                const int quad_column = max(quad * 2, first_column);
                if (column == quad_column && quad > (first_column >> 1)) {
                    const int previous_column = max((quad - 1) * 2, first_column);
                    const int previous_x = sample_stride == 1
                        ? previous_column
                        : crop_x + previous_column * sample_stride;
                    const Sample previous = load_sample(
                        y_plane, u_plane, v_plane, transfer_lut, dovi_params, previous_x, y,
                        y_stride, u_stride, v_stride, layout, dovi84_rgb, peak_is_max_rgb);
                    const int brighter = max(sample.pq_bin, previous.pq_bin);
                    const int band = min(brighter >> 8, DIFF_BANDS - 1);
                    const int difference = min(abs(sample.pq_bin - previous.pq_bin), DIFF_BINS - 1);
                    atomicAdd(&s_diff[band * DIFF_BINS + difference], 1u);
                }
            }

            t_sum_luma += fixed_point(luma_pq);
            t_sum_rgb += fixed_point(rgb_peak_pq);
            t_count += 1u;

            // One hue sample per chroma sample (only the even-even pixel of each quad).
            if ((x & 1) == 0 && (y & 1) == 0) {
                const int u_centered = (int)cb_code - 512;
                const int v_centered = (int)cr_code - 512;
                if (u_centered * u_centered + v_centered * v_centered >= 100) {
                    float hue = atan2f((float)v_centered, (float)u_centered);
                    if (hue < 0.0f) {
                        hue += TWO_PI;
                    }
                    int hue_bin = (int)(hue * ((float)HUE_BINS / TWO_PI));
                    if (hue_bin > HUE_BINS - 1) {
                        hue_bin = HUE_BINS - 1;
                    }
                    atomicAdd(&s_hue[hue_bin], 1u);
                }
            }
        }
    }

    // blockDim.x is a multiple of 32 and every thread reaches this point, so all lanes of
    // each warp take part in the shuffles; lanes without samples contribute zeros.
    t_sum_luma = warp_sum_u64(t_sum_luma);
    t_sum_rgb = warp_sum_u64(t_sum_rgb);
    t_count = warp_sum_u32(t_count);
    t_max_luma = warp_max_u32(t_max_luma);
    t_max_rgb = warp_max_u32(t_max_rgb);
    if ((threadIdx.x & 31) == 0 && t_count != 0) {
        atomicAdd(&s_sum_luma, t_sum_luma);
        atomicAdd(&s_sum_rgb, t_sum_rgb);
        atomicAdd(&s_count, t_count);
        atomicMax(&s_max_luma, t_max_luma);
        atomicMax(&s_max_rgb, t_max_rgb);
    }
    __syncthreads();

    unsigned long long* sums = (unsigned long long*)(results + SUMS_WORD);
    for (int bin = threadIdx.x; bin < LUM_BINS; bin += blockDim.x) {
        if (s_lum[bin] != 0) {
            atomicAdd(&results[bin], s_lum[bin]);
        }
    }
    for (int bin = threadIdx.x; bin < HUE_BINS; bin += blockDim.x) {
        if (s_hue[bin] != 0) {
            atomicAdd(&results[LUM_BINS + bin], s_hue[bin]);
        }
    }
    for (int bin = threadIdx.x; bin < PQ_BINS; bin += blockDim.x) {
        if (s_pq[bin] != 0) {
            atomicAdd(&results[PQ_HIST_BASE + bin], s_pq[bin]);
        }
    }
    if (grain_stats) {
        for (int bin = threadIdx.x; bin < DIFF_WORDS; bin += blockDim.x) {
            if (s_diff[bin] != 0) {
                atomicAdd(&results[DIFF_WORD + bin], s_diff[bin]);
            }
        }
    }
    if (threadIdx.x == 0) {
        if (s_count != 0) {
            atomicAdd(&sums[0], s_sum_luma);
            atomicAdd(&sums[1], s_sum_rgb);
            atomicAdd(&sums[2], (unsigned long long)s_count);
        }
        atomicMax(&results[MAX_LUMA_WORD], s_max_luma);
        atomicMax(&results[MAX_RGB_WORD], s_max_rgb);
    }
}
