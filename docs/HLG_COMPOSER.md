# HLG → Profile 8.4: a composer fitted to BT.2100

## 1. Status

- 2026-10-03: design note (ROADMAP priority 1, P8). Codex reviewed the first draft before any code
  was written (`docs/HLG_COMPOSER_REVIEW.md`, verdict CONCERNS, nine required changes). This
  revision includes them; section 13 lists how each was handled.
- 2026-10-03: implemented on branch `feat/hlg-neutral-composer` as described below; results in
  section 10. An implementation review (three independent reviewers) found only minor issues, all
  handled; the deviations from the first version of this note are marked "as built".
- First built opt-in. 2026-10-03, by owner decision: `bt2100` is the default for both binaries,
  ahead of the WS6 playback test, which the owner runs with it. Whether playback devices apply a
  composer that is not the preset is still unverified; `--hlg-composer preset` is the fallback.
- 2026-10-06 (P8): the analyzer measures both composers through the spec's 4:2:0 structure in
  `code / 1023` f32 (`spec-float`, section 9), with new sidecar names `dovi84-v3` (preset) and
  `dovi84-bt2100-v1-spec420` (bt2100). Section 9's table and the analyzer row above it describe the
  pre-P8 decode.

## 2. The problem, measured

`mkvdovi` writes the `dolby_vision` crate's `Profile84` preset into every HLG output (through
`dovi_tool generate`, profile `8.4`). The preset is a luma curve (8 pieces of order-2 polynomials)
and two chroma MMR curves (one piece, order 3), taken from phone-recorded RPUs. Its `ycc_to_rgb`
and `rgb_to_lms` matrices are plain BT.2020, so nothing later in the chain compensates its chroma.

Neutral HLG input (Cb = Cr = 512) decodes to R′G′B′ that is not neutral. The regression test
`profile84_preset_tints_neutrals_blue` (`hdr_analyzer_mvp/src/analysis/hlg.rs`) pins these values.
They come from the analyzer's f32 decoder, which matches libplacebo's DV render (`docs/VALIDATION.md` §8).

| HLG luma code | Decoded R′/G′/B′ (12-bit PQ codes) | Luminance | ΔE_ITP |
|---|---|---|---|
| 200 | 879 / 868 / 793 | 2.9 nits | 10.26 |
| 502 | 1819 / 1813 / 1851 | 51.8 nits | 4.48 |
| 721 (75%) | 2384 / 2387 / 2439 | 208.6 nits | 6.75 |
| 940 (100%) | 3134 / 3143 / 3155 | 1150.5 nits | 2.72 |

Method: ΔE_ITP per ITU-R BT.2124, between the decoded colour and R = G = B at the same BT.2020
luminance (`0.2627 R + 0.6780 G + 0.0593 B` in linear light). Nominal peak white decodes to about
1150 nits. L1 clamps it to `source_max_pq` (3079, 1000 nits). BT.2408 puts 75% HLG at 203 nits;
the preset gives 208.6.

## 3. Fitting target

The reference is the BT.2100 HLG-to-PQ conversion through display light, as BT.2408 describes it:
an HLG reference display with nominal peak `L_W = 1000` cd/m², black `L_B = 0`, so `γ = 1.2` and
`α = 1000`. Scene light is normalized to [0, 1] (not BT.2100's [0, 12] convention, which would need
a `/12^γ` in the OOTF).

For one HLG sample with 10-bit codes Y, Cb, Cr:

1. Limited-range BT.2020 NCL to HLG R′G′B′: `Y′ = (Y − 64) / 876`, `C′b = (Cb − 512) / 896`,
   `C′r = (Cr − 512) / 896`, then `R′ = Y′ + 1.4746 C′r`, `B′ = Y′ + 1.8814 C′b`,
   `G′ = (Y′ − 0.2627 R′ − 0.0593 B′) / 0.6780`. Negative values are clipped to 0 for the
   display-light target; the base layer itself is never changed.
2. Inverse OETF, per channel, normalized: `E = E′² / 3` for `E′ ≤ 1/2`, else
   `E = (exp((E′ − c) / a) + b) / 12` with `a = 0.17883277`, `b = 1 − 4a`, `c = 0.5 − a·ln(4a)`.
   Narrow-range superwhite reaches `E′ = (1019 − 64) / 876 ≈ 1.090`.
3. OOTF: `Y_S = 0.2627 R_S + 0.6780 G_S + 0.0593 B_S`, `F_D = 1000 · Y_S^0.2 · E` per channel.
4. **Peak policy (this project's choice, not prescribed):** display light is clipped per channel to
   [0, 1000] cd/m². With every `E′ ≤ 1`, `F_D ≤ 1000` already, so only superwhite is clipped.
   BT.2100's reference display clips at `E′ = 1` before the OOTF instead, and BT.2408 notes that
   narrow-range superwhite can extend the colour volume; for mixed-colour superwhite the three
   choices differ. Per-channel clipping in display light matches the declared `source_max_pq`
   (3079, 1000 nits) and the analyzer's existing clamp. Neutral superwhite holds at 1000 nits above
   code 940; the preset extrapolates towards 10,000 nits instead, so this is a behaviour change for
   content above nominal white.
5. ST 2084 inverse EOTF to target PQ R′G′B′ `T = (T_R, T_G, T_B)`.

What the composer can reach. The decoder computes
`L = (y_out − 1/16) · 9574/8192` from the luma curve's output `y_out`, and
`R′G′B′ = L + M_c · (s_cb − 0.5, s_cr − 0.5)` with the chroma columns `M_c` of the RPU's
`ycc_to_rgb` (unchanged, BT.2020 NCL limited range).

- **Luma target (neutrals only):** for a neutral code, `T_R = T_G = T_B = P`, so the luma curve's
  target is `y_out = 1/16 + P · 8192/9574` (limited-range reshaped luma, not PQ itself).
- **Colour:** the luma curve sees only input Y, and the PQ Y′ of a coloured pixel is not a function
  of HLG Y′ alone, so coloured R′G′B′ cannot be reproduced exactly. The chroma MMR is fitted
  conditional on the already-fitted luma curve: it minimizes `‖L + M_c·(s − 0.5) − T‖²` over the
  three channels. What remains is the irreducible luma error, reported separately.
  **As built:** each sample's residual vector is multiplied by the Jacobian of BT.2124's scaled
  ITP with respect to PQ R′G′B′ at the target before squaring (one Gauss-Newton step towards
  ΔE_ITP²). Plain PQ-domain least squares, as first written here, gave a held-out mean ΔE_ITP of
  16.4, worse than the preset's 15.0; the Jacobian weighting gives 9.9.

## 4. Composer shape and constraints

The new composer keeps the preset's syntax, so the decoder, the CUDA kernel and libplacebo need
no structural change:

- Luma: 8 pieces of order-2 polynomials, 9 delta-coded 10-bit pivots. The decoder clamps the output
  to `[first pivot, last pivot] / 1023`. With the first pivot at or below 63, black (output 1/16)
  stays inside the clamp; a test proves the clamp never changes an intended target.
- Chroma: one MMR piece of order 3 per component, pivots `[0, 1023]` (output clamp [0, 1]), terms
  `[y, u, v, yu, yv, uv, yuv]` with powers 1 to 3 and a constant: 22 coefficients each.
- Coefficients: fixed point, `coefficient_log2_denom = 23` (the Profile 8 header `dovi_tool`
  writes), integer part signed, fraction unsigned in [0, 2^23).
- DM block: unchanged (`ycc_to_rgb`, `rgb_to_lms`, offsets, `source_min_pq` 62, `source_max_pq`
  3079).

Constraints on the fit:

1. **Neutrals stay neutral.** MMR inputs are raw `code / 1023`, so the neutral axis is
   `u = v = n = 512/1023`, not 0.5; only the output offset is exactly 0.5. Substituting `u = v = n`
   turns each MMR curve into a cubic in `y`. With `a_t,o` the coefficient of term t at order o and
   `c` the serialized constant, its coefficients are:
   - `A0 = c + Σ_o [ n^o (a_u,o + a_v,o) + n^(2o) a_uv,o ]`, which must equal 0.5;
   - `A_o = a_y,o + n^o (a_yu,o + a_yv,o) + n^(2o) a_yuv,o`, which must equal 0 for o = 1, 2, 3.

   These four equalities per component are imposed in the f64 solve and **restored after
   quantization**: all other coefficients are quantized first, then `c` and `a_y,1..3` are
   recomputed from the equalities and quantized. The residual is at most a few 2^-23 units per
   equation and is reported.
2. **Luma** is fitted to neutrals only (section 3).
3. **Nominal white at 1000 nits:** code 940 maps to PQ 3078.7 (12-bit) ≤ 3079, so the analyzer's
   source clamp does not engage below nominal white.
4. **Monotonic luma** in 12-bit codes, continuous at the pivots. C0 is enforced in the f64 fit;
   the coefficients are then quantized independently, so seams of up to about 1.3e-7 PQ (under
   0.001 of a 12-bit code) remain, like the preset's 3e-7 at 910/911.
5. **Conditioning.** The decoder sums in f32. A small ridge term keeps coefficients moderate (the
   preset's integer parts reach ±19); large cancelling terms would cost precision.

## 5. Fitting procedure

A fitter crate, `tools/fit_hlg_composer` (outside the workspace, like the other tools), produces
the integer coefficients. It is deterministic and has no randomness. Its output is a committed Rust
constants file, so builds never fit at run time.

- **Luma:** the neutral target `y_out(code)` for codes 0 to 1023. Pivots are integer, searched by
  coordinate descent on the maximum error in 12-bit PQ codes over codes 64 to 1019. For fixed pivots,
  the coefficients come from weighted least squares with C0 equality constraints at the inner pivots,
  with Lawson reweighting towards the minimax error (as built: no separate black-biased weight;
  errors below `source_min_pq` count as zero on both sides). One piece holds 1000 nits above code
  940. As built, the first pivot is 64 (black): the decoder's output clamp then holds black and
  sub-black at 64/1023, 0.29 of a 12-bit code above PQ 0. A first build used pivot 0, which kept the
  clamp idle but let the curve fall to −23 codes at black and −299 codes for sub-black, a negative
  luma no display should receive; the synthetic checks and libplacebo did not show it. The pivots
  are 64, 149, 228, 333, 486, 636, 806, 940, 1023.
- **Chroma:** a training set of HLG R′G′B′ on a grid over [0, 1.09]³ (narrow-range headroom
  included), plus a denser low-end and near-neutral set. Each sample is encoded to **integer**
  Y′CbCr codes clamped to 4..1019, because the decoder sees integers, and its target `T` comes from
  that integer triplet. A joint equality-constrained least squares (KKT system) over both
  components' 44 coefficients minimizes the R′G′B′ error of section 3, with the eight neutral
  constraints and a small ridge term. Errors in PQ R′G′B′ approximate ΔE_ITP: T and P are differences
  of PQ-encoded LMS, so a PQ-domain error weighs about the same at every level.
- **Held-out set:** a second grid offset by half a step, never used in the fit. Criterion 5 uses
  it; the 52 validation patches are reported separately.
- **Quantization**, then the constraints are restored (section 4), then everything is re-evaluated
  on the quantized integers. The fitter reports maximum and mean errors, ΔE_ITP statistics, the
  coefficient magnitudes, and how often each clamp engages: luma output clamp, chroma output clamp
  [0, 1], source range [62, 3079].

## 6. Acceptance criteria

Measured through the analyzer's f32 `Dovi84Decoder` on raw R′G′B′ (`rgb_pq`, before the source
clamp), except where the source clamp itself is the subject:

1. Neutral codes 64 to 1019: `max(R′,G′,B′) − min(R′,G′,B′)` ≤ 0.05 of a 12-bit code.
2. Neutral codes 64 to 940: luma within 1.0 12-bit code of the reference `P`; ΔE_ITP ≤ 0.5 against
   the reference neutral. Both sides are floored at `source_min_pq` (62), below which the RPU's
   declared range makes them black; raw errors there (up to 23 codes at code 64, below 0.0001
   cd/m²) do not reach L1.
3. Neutral codes 941 to 1019: within 1.0 code of 3078.7.
4. Luma LUT monotonic in 12-bit codes; luma output clamp never active for codes 64 to 1019.
5. Colour, held-out set excluding superwhite: mean and 95th-percentile ΔE_ITP against the reference
   lower than the preset's on the same set. The chroma clamp activation rate is reported, not bounded.
6. Source-range clamp (`max_rgb_pq`): neutral black → 62, superwhite → 3079, as for the preset.
7. CPU/CUDA: the kernel-transcription test is bit-exact for both composers.

If 5 fails, the composer still ships opt-in only if 1 to 4 hold, with the colour result stated.

## 7. One composer definition, two binaries

A small workspace library crate, `dovi84_composer`, is the single source:

- `Composer::{Preset, Bt2100V1}`, with `rpu_data_mapping()` and `luminance_mapping()` (the sidecar
  name). `Preset` returns `Profile84::rpu_data_mapping()` unchanged; `Bt2100V1` builds the mapping
  from the committed constants. It also holds the RPU rewrite function, so mkvdovi and the
  validation tooling share one implementation.
- The analyzer builds its `Dovi84Decoder` and luma LUT from the selected composer's mapping. The
  CPU path and the CUDA parameter buffer both come from that one struct, as today.

**Sidecar.** `analysis.luminance_mapping` names the composer and the decode: `dovi84-v2` (preset,
unchanged) or `dovi84-bt2100-v1` as built on 2026-10-03; since P8 (2026-10-06) `dovi84-v3` and
`dovi84-bt2100-v1-spec420` for the spec 4:2:0 decode, with the old names kept as legacy
(`dovi84_composer::LEGACY_LUMINANCE_MAPPINGS`) and re-analyzed with a "pre-spec 4:2:0" message.
A refit gets a new name (`dovi84-bt2100-v2-spec420`), never a silent change. The sidecar layout does
not change, so the version stayed at 4 then and stays at 5 for the P8 names: each is a new value
of an existing, strictly validated field, as when `dovi84-v2` was added. An older mkvdovi rejects
the unknown value with a visible message; it never pairs it with an RPU of another composer.

**mkvdovi.**

- `--hlg-composer bt2100|preset` (default `bt2100` since the default change; first built with
  `preset` as default) is always passed to the analyzer for HLG. Since P8 mkvdovi first requires the
  analyzer's `--help` to contain the selected composer's `luminance_mapping()` name, for every
  composer including the preset, and refuses an analyzer without it before any analysis. (As built
  on 2026-10-03, an analyzer without `--hlg-composer` was accepted for `--hlg-composer preset`.)
- `check_luminance_mapping` and `dv_profile_for` take the expected composer explicitly and compare
  the name exactly (no `dovi84-*` prefix match). A preset sidecar is re-analyzed under `bt2100` and
  the other way round, with a visible message and no fallback.
- **RPU rewrite**, after `dovi_tool generate` and before the `RPU.bin` sentinel:
  1. Parse `RPU.bin` with the `dolby_vision` crate.
  2. On every frame assert `coefficient_data_type == 0`, `coefficient_log2_denom == 23`,
     `coefficient_log2_denom_length == 23`, `use_prev_vdr_rpu_flag == false`, and a mapping present.
  3. Runtime identity check: re-serialize every frame unchanged; the bytes must equal the input.
     This catches an external `dovi_tool` writing something the pinned crate does not preserve.
  4. Replace `rpu_data_mapping`, set `modified = true`, serialize in the exact `RPU.bin` format
     (`00 00 00 01` followed by `write_hevc_unspec62_nalu()[2..]`, as `GenerateConfig::write_rpus`),
     to a sibling temporary file.
  5. Re-parse the temporary file: same frame count, the new mapping on every frame, CRC valid
     (the parser checks it). Then rename it over `RPU.bin`.
  6. Only then `resume::mark_done` for `RPU.bin`. Today the sentinel is written right after the
     external call (`pipeline.rs` `generate_rpu`); it moves after the rewrite.

  For `preset` the step is skipped and the bytes stay exactly what `dovi_tool` wrote.
- **Resume.** The flag joins `resume_settings`. `extra.json` does not change with the composer, so
  the extra.json comparison would not catch a change; the fingerprint must. A temp dir without a
  fingerprint (older mkvdovi) is resumed today with a warning; under a non-default composer it is
  discarded instead.
- **`--verify`** for HLG output hard-fails when the sidecar cannot be loaded (today a load failure
  skips the sidecar checks), and compares the mapping of **every** RPU frame with the mapping of
  the composer the sidecar names.

**Alternatives considered.** (a) Carrying the coefficients in the sidecar: stronger against version
skew, but a schema change (version 5 on both sides) that also lets an arbitrary mapping into the RPU;
the named composer already refuses skew. (b) `dovi_tool editor`: it cannot replace composer curves.
(c) Generating the whole RPU in-process with the crate's `GenerateConfig` (suggested by the review):
it avoids the parse-and-rewrite step, but moves L1 to L11 generation out of `dovi_tool` for HLG,
which ROADMAP P10 treats as a separate decision. Rejected for now; the rewrite touches only the
mapping and is guarded by the identity check.

## 8. CPU/CUDA bit identity

The kernel takes all composer parameters from the `dovi_params` buffer. The new composer has the
same shape (luma LUT, order-3 single-piece MMR, the same matrix), so neither the kernel nor the
buffer layout changes. `GpuAnalyzer::new` and `build_transfer_lut` take the selected decoder in place
of the process-global one. The test `dovi84_kernel_params_reproduce_the_cpu_decoder_bit_exactly`
runs for both composers, and `scripts/cuda-parity.sh` gets a run with `--hlg-composer bt2100`,
because CI has no GPU.

## 9. Validation

- **Unit tests** for the criteria of section 6, in the analyzer (f32 decoder) and the composer crate
  (constants, constraint residuals, mapping round trip). The preset test stays as a regression record.
- **RPU rewrite tests:** identity round trip on a `dovi_tool`-generated RPU, rewrite then re-parse,
  then `dovi_tool inject-rpu` + `extract-rpu` + `info` on the result (skipped without `dovi_tool`).
- **Independent decode (libplacebo):** both scripts gain `--composer preset|bt2100`. They inject an
  RPU rewritten with the selected mapping (a `rewrite-rpu` subcommand of the fitter tool, calling the
  composer crate's rewrite function) and accept the matching sidecar name. libplacebo reads the
  composer from the RPU, so its render checks the analyzer's decode of the new mapping. The colour
  script also prints a BT.2100 reference column and ΔE_ITP of the libplacebo render against it, and
  exits non-zero when the libplacebo-vs-analyzer difference exceeds its tolerance, as today.
  Reported numbers name the ffmpeg/libplacebo build: its tone mapping changed on 2026-09-30.
- **End to end:** `mkvdovi/tests/hlg_profile84.rs` gets a `--hlg-composer bt2100` case: sidecar name,
  the fitted mapping on every frame of the muxed RPU, and `--verify` passing.
- **Not covered:** a Dolby display's behaviour. Device acceptance and visibility of the change stay
  open for the WS6 playback test.

**4:2:0 chroma: the analyzer is not the spec decode (both composers; measured 2026-10-05).** Three
decodes of the same 4:2:0 frame differ:

| Decode | MMR luma input | Where the MMR runs | Arithmetic |
|---|---|---|---|
| Analyzer (`frame.rs`) | the pixel's own luma | full resolution, each chroma sample replicated over its 2×2 quad | f32, `code / 1023` |
| Spec (ETSI GS CCM 001 v1.1.1 §5.4.2.3.3) | luma down-sampled to the chroma position: `[1 2 1]` around the co-sited column, mean of the two rows, edges replicated | chroma resolution; the display upsamples the composed chroma | integer: `code / 1024` (`s << 10` on a 2^20 scale), inputs clamped to the pivot ranges, 16-bit output |
| libplacebo (`pl_shader_decode_color` after the planes are merged) | the pixel's own luma | full resolution, chroma upsampled first | float, `code / 1023` |

`fit_hlg_composer chroma-siting` computes every variant per pixel, and
`scripts/validate_hlg_chroma_siting.sh` ties it to two independent decodes before using it. Its
analyzer variant reproduces the analyzer's per-frame max-RGB maximum exactly and the frame average
within 0.5 code (the sidecar's integer rounding) on every frame measured. Its renderer variant
matches libplacebo (`upscaler=bilinear`, v7.370.0, llvmpipe) within 0.32 codes per pixel through
`bt2100` and 3.84 through the preset, at every pixel around the 1×1 to 3×3 test highlights, for
chroma location left and top-left (the script refuses a real cut with any other location).
`spec-float` keeps the analyzer's arithmetic and changes only
the structure; `spec-fixed` is the spec. The two deviations are separate:

- **Structure** (`spec-float` − analyzer). Large per pixel at colour edges and on small saturated
  highlights: on the synthetic patterns the frame maximum of a field of 1×1 to 3×3 75% highlights
  on mid-grey drops by 548 codes (`bt2100`, left) and the average of 1-px colour lines by up to
  22. On the five HLG cuts it moves L1 max by up to 17.4 codes per scene (preset 14.4) and the
  average by less than 1.
- **Arithmetic** (`spec-fixed` − `spec-float`). `code / 1024` shifts every pixel: flat red −4.0
  codes (`bt2100`) and −4.5 (preset), flat 60% grey −0.6 and −3.6. On the cuts the spec's average
  is 0.6 to 3.3 codes per cut below the analyzer's (mean over scenes), and almost all of that is
  arithmetic: the structure alone moves it by 0.3 code at most. libplacebo uses `code / 1023` as well, so
  the colour script's agreement with libplacebo (0.49 / 0.59 codes) means both deviate from the
  spec in the same way.

Per-scene difference spec − analyzer on the five HLG development cuts, each at its own chroma
location with bilinear upsampling of the composed chroma, 12-bit codes, largest |difference| over
the scenes. `bt2100`: every frame (12,136); preset: every 4th frame, with the scene statistics over
the same frames on both sides:

| Cut (chroma location, scenes) | `bt2100` L1 max | `bt2100` L1 avg | preset L1 max | preset L1 avg |
|---|---|---|---|---|
| Wimbledon 2024 (top-left, 22) | 18.3 | 3.2 | 18.2 | 3.3 |
| Glastonbury 2025 (top-left, 20) | 0.0 | 4.9 | 0.0 | 3.5 |
| The Green Planet (left, 14) | 12.4 | 1.3 | 9.9 | 1.5 |
| Champions League (left, 9) | 0.0 | 2.3 | 0.0 | 2.6 |
| Bluelights (left, 32) | 14.1 | 2.3 | 10.1 | 1.8 |

Every scene of Glastonbury and the Champions League cut reaches the 3079 source clamp in all
decodes, so their L1 max cannot differ; the L1 max evidence comes from three cuts. The spec
leaves the upsampling of the composed chroma to the display: replicating it instead of bilinear
upsampling gives 6.5 codes on Wimbledon instead of 18.3. Up to 18 codes of L1 max is below the
smallest point of the WS7 display scale (+75 codes). Not measured: the `bt2100` neutral criteria
(section 6) under the spec's fixed-point decode, because the tool reports max-RGB only.

The step's 4-code limit (the libplacebo tolerance) is exceeded on both composers, so the analyzer
decode was changed (ROADMAP P8, below). The table above compares the pre-P8 analyzer.

**The analyzer since P8 (2026-10-06): `spec-float`.** For max-RGB the analyzer runs the chroma MMR
once per 4:2:0 chroma sample on luma down-sampled with `[1 2 1]` across two rows (the spec's
structure), in `code / 1023` f32 arithmetic, then upsamples the composed chroma bilinearly at the
stream's chroma location: left (also when unspecified) or top-left; other locations warn and use
left. CUDA does the chroma pass in a second kernel, `compose_chroma`
([CUDA_PIPELINE.md](CUDA_PIPELINE.md)). The luma path is unchanged. `spec-fixed` (the spec's
`code / 1024` fixed point) was not adopted: under it `bt2100` is not neutral (R′G′B′ spread 5.9
codes, luma error 3.57 codes; [ROADMAP_LOG.md](ROADMAP_LOG.md), 2026-10-06), and libplacebo
evaluates at `code / 1023` as well. Which convention devices use is a WS6 open question.
`scripts/validate_hlg_chroma_siting.sh --gate` checks the analyzer against the tool's `spec-float`
at the stream's own location: within 0.5 code per frame on the synthetic clips and the five cuts,
bit for bit per pixel on the synthetic clips, CPU equal to CUDA. Result (2026-10-06): 0.000 codes
worst |max| and |avg| on every clip and cut, both composers, both backends. Against the pre-P8
analyzer, scene L1 max moved by up to 19 codes (mostly down) and the max-RGB average by at most
1 code on the two cuts measured; luma averages did not move. Numbers:
[ROADMAP_LOG.md](ROADMAP_LOG.md), 2026-10-06.

## 10. Results

All numbers are from this branch on 2026-10-03. Fitter and analyzer numbers are f32-decoder
evaluations of the committed constants; libplacebo numbers are its render of an RPU rewritten
with the composer (ffmpeg N-125472-g97cbffe917-20260705, libplacebo v7.370.0, software Vulkan on
llvmpipe).

**Acceptance criteria (section 6), `bt2100`:**

| # | Criterion | Measured | Limit |
|---|---|---|---|
| 1 | Neutral R′G′B′ spread, codes 64–1019 | 0.005 code | 0.05 |
| 2 | Neutral luma error, codes 64–940 (floored) | 0.64 code | 1.0 |
| 2 | Neutral ΔE_ITP, codes 64–940 (floored) | 0.11 | 0.5 |
| 3 | Superwhite 941–1019, distance from 3078.73 | 0.005 code | 1.0 |
| 4 | Luma monotonic; output clamp active where the reference is above `source_min_pq` | yes; 0 codes | yes; 0 |
| 4b | Codes 4–64 (sub-black, black) decode to | +0.29 code | within ±1 of black |
| 5 | Held-out colour ΔE_ITP, mean / p95 (preset) | 9.91 / 19.6 (15.00 / 43.4) | below preset |
| 6 | Source clamp: black → 62, superwhite → 3079 | as specified | — |
| 7 | CPU/CUDA bit identity | identical (`scripts/cuda-parity.sh`, RTX 4070, 242 NVDEC frames analyzed in place) | identical |

The neutral constraint residuals after quantization are below 6e-8 per equation.

**Neutral ramp (12-bit PQ codes, all three channels):**

| HLG code | Preset R′/G′/B′ | `bt2100` | BT.2100 reference |
|---|---|---|---|
| 200 | 879 / 868 / 793 | 879.40 | 878.94 |
| 502 | 1819 / 1813 / 1851 | 1807.85 | 1808.35 |
| 721 | 2384 / 2387 / 2439 | 2378.60 | 2378.24 |
| 940 | 3134 / 3143 / 3155 | 3078.73 | 3078.73 |

**Colour (ΔE_ITP against the reference):** on the held-out grid without superwhite (4802 samples)
the preset reaches mean 15.00, p95 43.4, max 298; `bt2100` mean 9.91, p95 19.6, max 27.3. On the
superwhite part (1030 samples): preset mean 32.4, `bt2100` 11.8. The remaining error is mostly
structural: the luma curve sees only Y′, and a per-sample least-squares chroma with the same luma
curve reaches a mean of 8.1.

**Independent decode (libplacebo):** the analyzer matches libplacebo's render of the rewritten RPU
within 1.77 12-bit codes on the luma ramp and 0.49 on the 52 colour patches (preset: 3.16 and
0.59; tolerance 4). CPU and `--hwaccel cuda` give identical output. libplacebo's render against
the BT.2100 reference:

| Patches | Preset ΔE_ITP | `bt2100` ΔE_ITP |
|---|---|---|
| Grey, levels 0.25 / 0.5 / 0.75 / 1.0 | 5.20 / 4.80 / 7.12 / 11.36 | 0.03 / 0.10 / 0.07 / 0.01 |
| All 52, mean / max | 26.14 / 275.24 | 11.84 / 25.92 |

These grey ΔE values are measured against the BT.2100 reference neutral, so they include the
preset's luminance error; section 2's 6.75 at code 721 is measured against R = G = B at the
decoded luminance and isolates the tint. The preset's maximum is the 100% magenta patch at level
1.0, whose blue channel the preset drives to the top of the PQ range.

**Real content:** one 120 s HLG broadcast cut (3026 frames, 16 scenes) through `mkvdovi
--hlg-composer bt2100 --hwaccel cuda --verify`: the composer was installed on every frame,
`--verify` confirmed it on all 3026, and no temporary file was left (13.0 s wall). A default run on
the same copy rejected the `bt2100` sidecar with a visible message, re-analyzed, and verified the
preset on every frame (13.4 s). Per-scene L1, `bt2100` minus preset, 12-bit codes: maximum −82
to 0 (median −32),
average max-RGB +5 to +30 (median +20), minimum −31 to +13 (median −17). All maxima stay at or below
3079. The rewrite's byte-identical round-trip guard was exercised with `dovi_tool` 2.3.4 only; a
`dovi_tool` that serializes differently is refused by design.

**Not measured:** any Dolby display, device acceptance of a non-preset composer, and visibility on
a TV. They stay open for the WS6 playback test, which should include sub-black fades, coloured
superwhite and dark saturated colours, where the two composers differ most.

## 11. Input contract: refuse what the 8.4 RPU cannot describe

The 8.4 RPU's `ycc_to_rgb` is BT.2020 non-constant-luminance, limited range, and the BL is copied
bit-exactly. A full-range HLG stream, or one with another matrix (BT.709, BT.2020 constant
luminance) or other primaries, therefore decodes wrong on every DV display. Before this change
the analyzer only warned (and treated BT.2020 CL as fine), and mkvdovi did not check at all. In addition, the
`hevc_metadata` filter that adds a missing HLG transfer tag silently writes `matrix_coefficients=9`.

mkvdovi refuses HLG inputs tagged full range, a matrix other than BT.2020 NCL (BT.2020 CL included),
or primaries other than BT.2020:

- Sources: MediaInfo `colour_range`, `matrix_coefficients`, `colour_primaries` and their
  `_Original` variants; ffprobe stream `color_range`, `color_space`, `color_primaries`. Each field is
  resolved on its own, so a partial MediaInfo result does not stop ffprobe from filling the others.
- Any source naming a non-conforming value refuses, and the message lists every source and value,
  so conflicting tags are reported, not resolved silently.
- The check runs before any extraction, so before the filter that rewrites the matrix tag.
- As built, a value the classifier does not recognise refuses, with the value shown: a false
  refusal is preferred to a false accept.
- Untagged (unspecified) fields are accepted under the documented assumption (limited range,
  BT.2020 NCL) with a visible warning.

The analyzer keeps warnings (it is also used outside mkvdovi), and BT.2020 CL now warns as well.

## 12. Provenance

- The fitting target is public: ITU-R BT.2100 (HLG OETF and OOTF, PQ), BT.2408 (HLG-to-PQ conversion
  at 1000 cd/m²), BT.2124 (ΔE_ITP), and ETSI GS CCM 001 for the composer syntax. No Dolby tool output
  is used to fit or tune anything.
- The rewrite means the composer section of the RPU is written by the MIT `dolby_vision` crate
  (the library `dovi_tool` is built on) inside mkvdovi, not by `dovi_tool`. `docs/PROVENANCE.md`
  changes accordingly: a sources row for the fitted composer, and the "authored entirely by
  `dovi_tool`" sentence is corrected.

## 13. Review disposition

| # | Codex required change | Handling |
|---|---|---|
| 1 | Inverse-HLG equations; reshaped-luma target; no claim of exact colour | Section 3 |
| 2 | Superwhite as an explicit policy; fit coloured superwhite through the headroom | Section 3 step 4; training grid to 1.09 (section 5) |
| 3 | Reduced neutral-axis equations; restore after quantization | Section 4.1 |
| 4 | Clamp activation, raw RGB, source range, held-out set, numeric criteria | Sections 5 and 6 |
| 5 | Reject fingerprint-less resume under a non-default composer; sentinel after rewrite | Section 7 |
| 6 | Verify hard-fails on sidecar errors; mapping on every frame | Section 7 |
| 7 | Header asserts, exact format, atomic write, round-trip tests | Sections 7 and 9 |
| 8 | Refuse BT.2020 CL, warn on untagged, conflicting tags, analyzer CL warning | Section 11 |
| 9 | 4:2:0 chroma siting | Measured 2026-10-05 against the spec composer and libplacebo (section 9); the analyzer decode is to change to the spec's (ROADMAP P8) |
| — | Implementation review (three reviewers, all minor) | Rewrite refuses an RPU that does not carry the preset; analyzer support probed before use; full error chains printed; colour and neutral-constraint tests added; this note corrected where the build differs ("as built") |
| — | Generate in-process instead of rewriting | Considered and rejected for now (section 7, alternative c) |
