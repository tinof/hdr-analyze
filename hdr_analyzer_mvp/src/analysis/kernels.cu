// Single-launch HDR frame analysis kernel.
//
// counts layout (u32 words):
//   [0, 256)          v5 luminance histogram (binned via luminance_bin_lut)
//   [256, 287)        hue histogram (31 bins over the full hue circle)
//   [287, 287+4096)   4096-bin PQ histogram in the selected peak domain
//   [4383]            max luma PQ observed (f32 bit pattern)
//   [4384]            max max-RGB PQ observed (f32 bit pattern)
// sums layout (u64 words):
//   [0] sum of luma PQ, fixed point * 2^32
//   [1] sum of max-RGB PQ, fixed point * 2^32
//   [2] analyzed pixel count
#define LUM_BINS 256
#define HUE_BINS 31
#define PQ_BINS 4096
#define PQ_HIST_BASE (LUM_BINS + HUE_BINS)
#define MAX_LUMA_WORD (PQ_HIST_BASE + PQ_BINS)
#define MAX_RGB_WORD (MAX_LUMA_WORD + 1)
#define FIXED_POINT_SCALE 4294967296.0
#define TWO_PI 6.28318530717958647692f

extern "C" __global__ void analyze_frame(
    const unsigned char* y_plane,
    const unsigned char* u_plane,
    const unsigned char* v_plane,
    const float* transfer_lut,
    const unsigned short* luminance_bin_lut,
    unsigned int* counts,
    unsigned long long* sums,
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
    int rgb_peak_is_luma,
    int peak_is_max_rgb
) {
    __shared__ unsigned int s_lum[LUM_BINS];
    __shared__ unsigned int s_hue[HUE_BINS];
    __shared__ unsigned int s_pq[PQ_BINS];
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
    if (threadIdx.x == 0) {
        s_sum_luma = 0ull;
        s_sum_rgb = 0ull;
        s_count = 0u;
        s_max_luma = 0u;
        s_max_rgb = 0u;
    }
    __syncthreads();

    const int sample_index = blockIdx.x * blockDim.x + threadIdx.x;
    if (sample_index < sample_count) {
        const int sample_width = (crop_width + sample_stride - 1) / sample_stride;
        const int sx = sample_index % sample_width;
        const int sy = sample_index / sample_width;
        const int x = crop_x + sx * sample_stride;
        const int y = crop_y + sy * sample_stride;

        if (x < crop_x + crop_width && y < crop_y + crop_height && x < width && y < height) {
            const int y_offset = y * y_stride + x * 2;
            const unsigned short raw_y =
                (unsigned short)y_plane[y_offset] |
                ((unsigned short)y_plane[y_offset + 1] << 8);
            const unsigned int code = layout == 1 ? (raw_y >> 6) & 1023u : raw_y & 1023u;
            const float luma_pq = transfer_lut[code];
            atomicAdd(&s_lum[luminance_bin_lut[code]], 1u);
            atomicMax(&s_max_luma, __float_as_uint(luma_pq));

            // Co-sited 4:2:0 chroma sample for this pixel's 2x2 quad.
            const int cx = x >> 1;
            const int cy = y >> 1;
            unsigned int cb_code;
            unsigned int cr_code;
            if (layout == 1) {
                const int uv_offset = cy * u_stride + cx * 4;
                const unsigned short raw_u =
                    (unsigned short)u_plane[uv_offset] |
                    ((unsigned short)u_plane[uv_offset + 1] << 8);
                const unsigned short raw_v =
                    (unsigned short)u_plane[uv_offset + 2] |
                    ((unsigned short)u_plane[uv_offset + 3] << 8);
                cb_code = (raw_u >> 6) & 1023u;
                cr_code = (raw_v >> 6) & 1023u;
            } else {
                const int u_offset = cy * u_stride + cx * 2;
                const int v_offset = cy * v_stride + cx * 2;
                cb_code = ((unsigned int)u_plane[u_offset] |
                    ((unsigned int)u_plane[u_offset + 1] << 8)) & 1023u;
                cr_code = ((unsigned int)v_plane[v_offset] |
                    ((unsigned int)v_plane[v_offset + 1] << 8)) & 1023u;
            }

            float rgb_peak_pq;
            if (rgb_peak_is_luma) {
                rgb_peak_pq = luma_pq;
            } else {
                // Same non-constant-luminance approximation as the CPU path:
                // mix the PQ-encoded signal directly in Y'CbCr space.
                const float y_signal = ((float)((int)code - 64)) / 876.0f;
                const float cb = ((float)cb_code - 512.0f) / 896.0f;
                const float cr = ((float)cr_code - 512.0f) / 896.0f;
                const float red = y_signal + 1.4746f * cr;
                const float blue = y_signal + 1.8814f * cb;
                const float green = (y_signal - 0.2627f * red - 0.0593f * blue) / 0.6780f;
                const float peak = fmaxf(red, fmaxf(green, blue));
                rgb_peak_pq = fminf(fmaxf(peak, 0.0f), 1.0f);
            }
            atomicMax(&s_max_rgb, __float_as_uint(rgb_peak_pq));

            const float peak_pq = peak_is_max_rgb ? rgb_peak_pq : luma_pq;
            int pq_bin = (int)(peak_pq * (float)(PQ_BINS - 1) + 0.5f);
            if (pq_bin < 0) {
                pq_bin = 0;
            }
            if (pq_bin > PQ_BINS - 1) {
                pq_bin = PQ_BINS - 1;
            }
            atomicAdd(&s_pq[pq_bin], 1u);

            atomicAdd(&s_sum_luma, (unsigned long long)((double)luma_pq * FIXED_POINT_SCALE));
            atomicAdd(&s_sum_rgb, (unsigned long long)((double)rgb_peak_pq * FIXED_POINT_SCALE));
            atomicAdd(&s_count, 1u);

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
    __syncthreads();

    for (int bin = threadIdx.x; bin < LUM_BINS; bin += blockDim.x) {
        if (s_lum[bin] != 0) {
            atomicAdd(&counts[bin], s_lum[bin]);
        }
    }
    for (int bin = threadIdx.x; bin < HUE_BINS; bin += blockDim.x) {
        if (s_hue[bin] != 0) {
            atomicAdd(&counts[LUM_BINS + bin], s_hue[bin]);
        }
    }
    for (int bin = threadIdx.x; bin < PQ_BINS; bin += blockDim.x) {
        if (s_pq[bin] != 0) {
            atomicAdd(&counts[PQ_HIST_BASE + bin], s_pq[bin]);
        }
    }
    if (threadIdx.x == 0) {
        if (s_count != 0) {
            atomicAdd(&sums[0], s_sum_luma);
            atomicAdd(&sums[1], s_sum_rgb);
            atomicAdd(&sums[2], (unsigned long long)s_count);
        }
        atomicMax(&counts[MAX_LUMA_WORD], s_max_luma);
        atomicMax(&counts[MAX_RGB_WORD], s_max_rgb);
    }
}
