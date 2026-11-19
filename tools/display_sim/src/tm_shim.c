/*
 * Scalar wrapper around libplacebo 7.387.0's tone-mapping curves for tools/display_sim
 * (ROADMAP WS7.1). MIT, like the rest of this repository; it uses only libplacebo's public
 * headers. Rust calls these functions with scalar arguments, so it mirrors no libplacebo struct.
 *
 * tm_new sets the curve's parameters as the v7.387.0 renderer does for a frame
 * (src/shaders/colorspace.c, pl_shader_color_map_ex, before its dynamic target), restated here
 * from that code's behaviour, not copied:
 *   1. the source and display ranges as pl_color_space_nominal_luma_ex leaves them: values
 *      clamped to [PL_COLOR_HDR_BLACK nits, PQ 1], 0 read as absent, a black at or above the
 *      peak dropping both, an absent peak read as the PQ peak and an absent black as
 *      PL_COLOR_HDR_BLACK, the average clamped to [black, peak] and 0 when absent;
 *   2. pl_tone_map_params_infer;
 *   3. a peak or black within 1e-6 PQ of the other side's taken over;
 *   4. a black at or below PL_COLOR_HDR_BLACK set to 0 (the renderer's signal-black rule);
 *   5. input_max raised to output_max (no inverse tone mapping), so L1 max below the display
 *      peak never expands the picture;
 *   6. pl_tone_map_params_noop: the renderer then skips the curve and the display clips.
 * Every curve runs in PQ (PL_HDR_PQ in and out, as the renderer). BT.2390 uses the ITU knee
 * offset 0.5 instead of libplacebo's default 1.0.
 */

#include <math.h>
#include <stdlib.h>
#include <string.h>

#include <libplacebo/colorspace.h>
#include <libplacebo/config.h>
#include <libplacebo/tone_mapping.h>

#if PL_MAJOR_VER != 7 || PL_API_VER != 387
#error "tools/display_sim needs libplacebo 7.387.0: run scripts/build-libplacebo.sh"
#endif

struct tm_ctx {
    struct pl_tone_map_params params;
    /* The display's physical range (PQ), for a no-op: the display clips, no curve runs. */
    float panel_min;
    float panel_max;
    int noop;
};

static float clampf(float x, float lo, float hi)
{
    return fminf(fmaxf(x, lo), hi);
}

static float nits_to_pq(float nits)
{
    return pl_hdr_rescale(PL_HDR_NITS, PL_HDR_PQ, nits);
}

/* The renderer's signal black: a black at or below PL_COLOR_HDR_BLACK is true black. */
static float signal_black(float pq)
{
    return pq <= nits_to_pq(PL_COLOR_HDR_BLACK) ? 0.0f : pq;
}

float tm_nits_to_pq(float nits)
{
    return nits_to_pq(nits);
}

float tm_pq_to_nits(float pq)
{
    return pl_hdr_rescale(PL_HDR_PQ, PL_HDR_NITS, pq);
}

const char *tm_version(void)
{
    return pl_version();
}

int tm_fix_version(void)
{
    return pl_fix_ver();
}

/*
 * NULL for a curve libplacebo does not know. The caller validates the numbers (finite, blacks
 * below peaks, nits within PQ's range), so no libplacebo assertion can fire.
 */
struct tm_ctx *tm_new(const char *curve, float src_black_nits, float src_max_pq,
                      float src_avg_pq, float panel_black_nits, float panel_peak_nits)
{
    const struct pl_tone_map_function *function = pl_find_tone_map_function(curve);
    if (!function)
        return NULL;
    struct tm_ctx *ctx = calloc(1, sizeof(*ctx));
    if (!ctx)
        return NULL;

    const float hdr_min = nits_to_pq(PL_COLOR_HDR_BLACK);
    /* As pl_color_space_nominal_luma_ex: 0 means absent, a black at or above the peak (or at the
     * PQ maximum) drops both, then an absent peak is the PQ peak and an absent black hdr_min. */
    float in_min = src_black_nits > 0 ? clampf(nits_to_pq(src_black_nits), hdr_min, 1.0f) : 0.0f;
    float in_max = src_max_pq > 0 ? clampf(src_max_pq, hdr_min, 1.0f) : 0.0f;
    if ((in_max > 0 && in_min >= in_max) || in_min >= 1.0f)
        in_min = in_max = 0.0f;
    if (in_max <= 0)
        in_max = 1.0f;
    if (in_min <= 0)
        in_min = hdr_min;
    const float out_min = panel_black_nits > 0
                              ? clampf(nits_to_pq(panel_black_nits), hdr_min, 1.0f)
                              : hdr_min;
    const float out_max = clampf(nits_to_pq(panel_peak_nits), hdr_min, 1.0f);

    struct pl_tone_map_params *p = &ctx->params;
    *p = (struct pl_tone_map_params) {
        .function = function,
        .constants = { PL_TONE_MAP_CONSTANTS },
        .input_scaling = PL_HDR_PQ,
        .output_scaling = PL_HDR_PQ,
        .lut_size = 256,
        .input_min = in_min,
        .input_max = in_max,
        .input_avg = src_avg_pq > 0 ? clampf(src_avg_pq, in_min, in_max) : 0.0f,
        .output_min = out_min,
        .output_max = out_max,
    };
    if (function == &pl_tone_map_bt2390)
        p->constants.knee_offset = 0.5f;

    pl_tone_map_params_infer(p);
    if (fabsf(p->input_max - p->output_max) < 1e-6f)
        p->output_max = p->input_max;
    if (fabsf(p->input_min - p->output_min) < 1e-6f)
        p->output_min = p->input_min;
    p->input_min = signal_black(p->input_min);
    p->output_min = signal_black(p->output_min);
    p->input_max = fmaxf(p->input_max, p->output_max);

    ctx->panel_min = signal_black(out_min);
    ctx->panel_max = out_max;
    ctx->noop = pl_tone_map_params_noop(p);
    return ctx;
}

void tm_free(struct tm_ctx *ctx)
{
    free(ctx);
}

/* Displayed PQ of source PQ `x`. */
float tm_sample(const struct tm_ctx *ctx, float x)
{
    if (ctx->noop)
        return clampf(x, ctx->panel_min, ctx->panel_max);
    return pl_tone_map_sample(x, &ctx->params);
}

/*
 * The source PQ above which every value displays the same: the display peak on a no-op, else
 * the curve's input_max (pl_tone_map_sample clamps its input there).
 */
float tm_clip_ceiling(const struct tm_ctx *ctx)
{
    return ctx->noop ? ctx->panel_max : ctx->params.input_max;
}

int tm_is_noop(const struct tm_ctx *ctx)
{
    return ctx->noop;
}

/* input_min, input_max, input_avg, output_min, output_max (PQ) and the knee offset in use. */
void tm_params(const struct tm_ctx *ctx, float out[6])
{
    const struct pl_tone_map_params *p = &ctx->params;
    out[0] = p->input_min;
    out[1] = p->input_max;
    out[2] = p->input_avg;
    out[3] = p->output_min;
    out[4] = p->output_max;
    out[5] = p->constants.knee_offset;
}
