extern "C" __global__ void analyze_frame(
    const unsigned char* y_plane,
    const unsigned char* u_plane,
    const unsigned char* v_plane,
    const float* transfer_lut,
    const unsigned short* luminance_bin_lut,
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
    int sample_count
) {
    __shared__ unsigned int lum_histogram[256];
    __shared__ unsigned int hue_histogram[31];

    for (int bin = threadIdx.x; bin < 256; bin += blockDim.x) {
        lum_histogram[bin] = 0;
    }
    for (int bin = threadIdx.x; bin < 31; bin += blockDim.x) {
        hue_histogram[bin] = 0;
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
            const unsigned int code = layout == 1 ? (raw_y >> 6) & 1023 : raw_y & 1023;
            const float pq = transfer_lut[code];
            atomicAdd(&lum_histogram[luminance_bin_lut[code]], 1u);
            atomicMax(&results[287], __float_as_uint(pq));

            if ((x & 1) == 0 && (y & 1) == 0) {
                const int cx = x / 2;
                const int cy = y / 2;
                unsigned short raw_u;
                unsigned short raw_v;
                if (layout == 1) {
                    const int uv_offset = cy * u_stride + cx * 4;
                    raw_u = (unsigned short)u_plane[uv_offset] |
                        ((unsigned short)u_plane[uv_offset + 1] << 8);
                    raw_v = (unsigned short)u_plane[uv_offset + 2] |
                        ((unsigned short)u_plane[uv_offset + 3] << 8);
                    raw_u >>= 6;
                    raw_v >>= 6;
                } else {
                    const int u_offset = cy * u_stride + cx * 2;
                    const int v_offset = cy * v_stride + cx * 2;
                    raw_u = ((unsigned short)u_plane[u_offset] |
                        ((unsigned short)u_plane[u_offset + 1] << 8)) & 1023;
                    raw_v = ((unsigned short)v_plane[v_offset] |
                        ((unsigned short)v_plane[v_offset + 1] << 8)) & 1023;
                }

                const int u_centered = (int)raw_u - 512;
                const int v_centered = (int)raw_v - 512;
                if (u_centered * u_centered + v_centered * v_centered >= 100) {
                    float hue = atan2f((float)v_centered, (float)u_centered);
                    if (hue < 0.0f) {
                        hue += 6.2831853071795864769f;
                    }
                    int hue_bin = (int)(hue * (31.0f / 6.2831853071795864769f));
                    if (hue_bin > 30) {
                        hue_bin = 30;
                    }
                    atomicAdd(&hue_histogram[hue_bin], 1u);
                }
            }
        }
    }
    __syncthreads();

    for (int bin = threadIdx.x; bin < 256; bin += blockDim.x) {
        if (lum_histogram[bin] != 0) {
            atomicAdd(&results[bin], lum_histogram[bin]);
        }
    }
    for (int bin = threadIdx.x; bin < 31; bin += blockDim.x) {
        if (hue_histogram[bin] != 0) {
            atomicAdd(&results[256 + bin], hue_histogram[bin]);
        }
    }
}
