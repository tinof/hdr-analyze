# Profile 7 FEL: plan for a no-re-encode design

## 1. Status

- 2026-10-03: the BL+EL compositor and its re-encode were removed from `mkvdovi`. A review showed
  that the compositor did not follow the reconstruction specification and produced wrong pixels on
  real discs (section 3).
- `mkvdovi` refuses Profile 7 FEL inputs, with or without `--mdfix`. `inspect` still reports FEL.
  Profile 7 MEL, Profile 8, HDR10, HDR10+ and HLG are not affected.
- Nothing in this document is implemented. It is the starting point for a later session.
- The earlier research notes (`docs/experimental/`) were removed. Their "80-90% of artistic intent"
  preservation estimate was retracted before removal; no measurement ever supported it.

## 2. Scope decided by the owner

In scope: FEL features that can be delivered **without re-encoding the video**. The base layer (BL)
is copied bit-exactly, the output is a single-layer Profile 8.1 stream, and whatever FEL information
survives is carried in the RPU, the way the MEL conversion already works.

Out of scope:

- Any pixel re-encode of the picture (x265, NVENC or other), and with it dither and encoder tuning.
- 12-bit output. A Profile 8 base layer is 10-bit (the `dolby_vision` crate validates
  `bl_bit_depth_minus8 == 2`).
- Profile 7 passthrough as a product feature.
- Profile 5 output (needs a colour transform and a re-encode; see R2 in the roadmap).

A general lossless conversion does not exist: FEL carries residual video samples and Profile 8.1 has
no enhancement-layer stream. Every candidate below is an approximation, and its cost must be
measured per title.

## 3. What the removed implementation got wrong

Checklist for any future reconstruction code, including analysis-only code. Clauses refer to ETSI
GS CCM 001 V1.1.1. "Measured" means measured in the review against a specification-literal integer
model; a replica of the removed code matched the release binary bit for bit.

| # | Defect in the removed code | Specification |
|---|----------------------------|---------------|
| 1 | Polynomial: only the linear term was scaled correctly. `c0` was added in 10-bit code units (about 1024 times too small); the second-order term lacked the division by 2^BL_bit_depth. | 5.4.2.3.2: terms are `coef[i] * (s^i << (20 - i*BL_bit_depth))`, result `>> (4 + coeff_log2_denom)` |
| 2 | MMR: constant about 1024 times too small; cross terms normalised by `coeff_log2_denom` where the BL bit depth is required, so they were effectively zero; order-2 and order-3 terms were always zero at denominator 23, and their term set was wrong (the specification uses the squares and cubes of the seven order-1 terms). | 5.4.2.3.3 |
| 3 | Pivots were used as read. They are delta-coded, the `dolby_vision` crate (3.4.0) returns the coded values, and they must be accumulated. With real luma pivots every sample of 128 or more selected the last piece. | 5.3 (`pivot_value[cmp][i] = pivot_value[cmp][i-1] + pred_pivot_value[cmp][i]`) |
| 4 | Polynomial curves on chroma were skipped (passthrough). | 5.4.2 |
| 5 | No clamp of the inputs to `[pivot_value[0], pivot_value[last]]`. | 5.4.2 |
| 6 | Chroma pieces were selected with the luma sample; each component must use its own sample. | 5.4.2 |
| 7 | The prediction was cut to 10 bits before the residual was added, and the output was 10-bit. The specification keeps a 16-bit mapped BL, adds the residual, and reconstructs at `hdr_bit_depth` (12). | 5.4.2, 5.4.3.3 |
| 8 | MMR luma input used a 2x2 box mean; the specification filters `[1 2 1]` horizontally and averages two rows. | 5.4.2.3.3 |
| 9 | EL upsampling used a centred lanczos filter and floored the result; informative Annex B defines an integer filter whose even output samples are co-sited horizontally. | Annex B.3 (informative) |
| 10 | The unit tests encoded the wrong convention (coefficients pre-scaled to match the code), so they passed. | n/a |

Measured size:

- Seven of eight Profile 7 FEL test cuts use order-3 MMR chroma on every frame. On real BL pixels
  the removed code missed the specification prediction by a mean of **69 to 534 10-bit codes** in
  chroma, mostly clipped to 0 or 1023: a gross colour cast.
- Luma on the same cuts: the `c0` defect moved the frame mean by 0 to 11.4 codes; the pivot defect
  alone caused up to 25.6 codes and touched 351 to 1458 frames per cut.
- The eighth cut has the identity mapping on all three components and was unaffected in prediction.
  It is the control clip for the NLQ and upsampling path.
- Small next to the above, in 12-bit codes: item 7 mean 0.5 to 1.0; item 8 mean 0.02 to 0.20; item
  9 mean 0.5 to 4.7 with a bias of -0.4 to -0.5 from the floor.

What was right and can be reused as knowledge: the `NLQ_LINEAR_DZ` dequantisation matched 5.4.3.2
line by line, and the final rounding matched 5.4.3.3 apart from the output depth.

Rules that follow: test vectors come from the specification, never from the implementation; the
ETSI integer form is the reference (it agreed with a float evaluation within 0.3 codes); an
unsupported mapping state is an error, never a silent identity.

## 4. Measured facts about real FEL streams

From the eight Profile 7 FEL test cuts and one MEL title, all frames unless stated.

- Header: BL and EL 10-bit, reconstructed signal 12-bit, `coeff_log2_denom` 23,
  `el_spatial_resampling_filter_flag` set, residual enabled.
- Luma mapping: 8 linear pieces, cumulative pivots 0, 128, ..., 1023. No second-order term on any
  of 81,368 pieces. Many pieces have a non-zero `c0`.
- Chroma mapping: one piece, order-3 MMR, constant near -0.145 (normalised).
- NLQ: linear dead zone, offset 512, slope 2^-12, threshold 0, `vdr_in_max` 0.125. One EL step is
  about 0.25 10-bit codes; the residual range is about ±128 10-bit codes.
- Share of frames whose luma mapping is the identity: 0% to 53% on the seven cuts with a real
  mapping, 100% on the control cut. So on many frames the BL and the reconstruction differ in level
  before any residual is added.
- The specification prediction of chroma stays within a mean of 0.2 to 2.8 10-bit codes of the BL
  chroma (28 plane-frames).
- One cut signals `chroma_location = topleft` in both layers. Annex B's vertical chroma filter
  assumes centred chroma. Which siting the EL encoder used is not determined.
- Authored metadata: CM v2.9 on all nine RPUs, no L3, no L8. L2 trims on the FEL cuts are typically
  100 to 400 codes from neutral (2048); one cut has no L2. L4 is on every frame. L1 is constant
  inside a shot. L1 minima above 12 codes occur (up to 251). L5 is missing on three cuts.

## 5. Candidate designs

All four keep the BL bit-exact. None is chosen yet.

### a. Authored RPU converted to 8.1, mapping removed (FEL handled like MEL)

`dovi_tool` mode 2 on a FEL RPU sets the mapping to identity, disables the residual and keeps all
display-management metadata. Verified mechanically in the review with `dovi_tool` 2.3.4.

- Keeps: authored L1, L2, L4, L5, L6, scene flags; BL pixels.
- Loses: the whole reconstruction (mapping and residual).
- Risk carried over from the former roadmap item F2 (now the FEL placeholder): metadata authored
  for the reconstructed picture can give wrong brightness on some titles when the EL is dropped.
  Section 4 adds that the luma mapping is not the identity on many frames.
- Measure first: per title and per shot, the distance between BL L1 and reconstruction L1 (max and
  avg), and the BL-to-reconstruction picture distance.
- Device risk: low. It is the stream type the MEL path already produces.

### b. Authored prediction mapping kept in the 8.1 RPU, residual dropped

Conversion in the style of `dovi_tool` mode 5 (8.1 with the mapping preserved): polynomial and MMR
stay, NLQ is cleared.

- Keeps: the global part of the reconstruction that the disc encoder fitted, plus the authored
  metadata as in (a).
- Loses: the residual.
- Open questions: does a Profile 8.1 device apply a non-identity mapping; does the stream meet the
  profile constraints; what does the output bit depth become; does the HDR10 fallback (which sees
  only the BL) stay acceptable.
- Measure first: distance of (b) to the reference reconstruction against the distance of (a).
- Device risk: high until tested on the target player. `dovi_tool` removes the mapping for FEL in
  mode 2 on purpose.

### c. A new global mapping fitted per frame or scene (former roadmap item R1)

Fit the best mapping the 8.1 RPU can express (8-piece polynomial on luma, order-3 MMR on chroma)
from the BL to the reference reconstruction.

- Principled limit: the 8.1 mapping is a function of pixel value and the residual is spatial. Two
  pixels with the same BL value that need different outputs cannot be separated (the "collision"
  test of the old notes). The disc encoder already fitted the global part, so the possible gain
  over (b) is only what a refit can still capture.
- Measure first: explained variance of the refit over (b), before and after coefficient
  quantisation.
- Device risk: as (b), plus fitted coefficients outside what authored streams use.

### d. Decision support per title

Report residual statistics so the user can see what dropping the layer costs: share of samples with
a non-zero residual, RMS in 12-bit codes, split between low and high spatial frequency, and the
L1 distance from (a). This changes no output and is useful with any of a to c.

## 6. What must exist first

An analysis-only, specification-literal reference reconstruction:

- Integer arithmetic, ETSI GS CCM 001 5.4.2 to 5.4.3, Annex B upsampling, 12-bit output.
- Probably a tool crate outside the workspace (like `tools/l1_diff`), not part of `mkvdovi`.
- It measures candidates a to d. It never produces delivered pixels.

Validation, in this order:

1. Hand-calculated basis vectors from the specification, one per term of section 3. Example at
   denominator 23 and `s = 512`: polynomial (0.01, 0.98, 0.01) gives 32931 at 16 bits; MMR constant
   0.01 gives 655; a `s0*s1` coefficient of 0.5 gives 8192.
2. The oracle found in the review: on real frames the chroma prediction stays within about 0.2 to
   2.8 10-bit codes of the BL chroma. It needs no EL decode.
3. The control cut (identity mapping, NLQ active) to test NLQ and upsampling apart from prediction.
4. Cross-checks against libplacebo's FEL composition and vs-nlq, with versions pinned. libplacebo
   works in float at 4:4:4, so a tolerance must be set first; whether 4:2:0 against 4:4:4 differs
   by more than about 1 LSB on real content is unverified.

Open points: Annex B is informative, so a hardware decoder may use another filter; the chroma
siting question of section 4; frame alignment between BL, EL and RPU must be checked, not assumed.
Nothing published validates a FEL reconstruction bit-exactly against Dolby hardware.

## 7. Decision gates

Thresholds are fixed before each scored run.

1. **Reference validated** (section 6). Stop: no agreement with the specification vectors and the
   oracle. Without a trusted reference no later number means anything.
2. **Cost of (a)** on all FEL test cuts, with the statistics of (d). Stop for b and c: (a) is
   within the fixed tolerance on every cut; then FEL is handled like MEL and (d) is the feature.
3. **Device feasibility of a non-identity mapping in Profile 8.1** on the target player, with a
   synthetic stream whose mapping has a known visible effect. Stop for b and c: the device ignores
   or misrenders the mapping, or the stream fails profile validation.
4. **(b) against (a)**. Stop for b and c: (b) is not closer to the reference than (a), or it is
   worse in any important scene.
5. **(c) over (b)**. Stop for c: the refit explains little of the remaining error, or the gain
   disappears after quantisation.
6. **Real playback** of the chosen candidate against dual-layer playback of the disc, with the
   chain recorded. Stop: instability (hue or luma pumping) or a result worse than (a).

A candidate becomes a default only after gate 6. Until then FEL inputs stay refused, or (a) is
offered as an explicit opt-in with the measured cost shown.

## 8. Metadata questions for any candidate

- **Keep authored L1 together with authored trims and L4.** Measured on the BL: the analyzer's
  averages are 24 to 169 codes below authored on one cut and up to 538 to 987 codes below on
  others, and the shot structure differs (7 analyzer scenes against 32 authored shots on one cut).
  Trims were authored against the authored L1 and shot boundaries; new L1 with old trims applies a
  correction to a curve it was not made for. Use analyzer L1 only where no authored metadata exists.
- **Generator clamps do not apply to a carried RPU.** `dovi_tool generate` clamps L1 (min at most
  12, max at least 2081, avg floor), writes no L4, keeps only one sampled L5 for the whole file,
  and takes L6 from the container. A converted RPU keeps the authored values, including L1 minima
  above 12.
- **CM version.** The discs are CM v2.9. Decide whether to keep v2.9 as authored or add the tool's
  CM v4.0 levels (L9, L11) without touching authored levels.
- **L5.** Missing on some discs. Decide whether to add a measured L5 to an otherwise authored RPU.
- **What `--mdfix` means for FEL.** It exists for unreliable authored metadata, so copying the
  authored L1 back defeats it; regenerating from the BL discards trims and L4 and describes the BL,
  not the reconstruction. Unresolved. Targeted repair of single levels is the likely direction.
- **`--verify`** must check the frame count of a carried RPU against the video track, as it does
  for generated RPUs.

## 9. Prior art and references

- ETSI GS CCM 001 V1.1.1, composer and metadata syntax:
  <https://www.etsi.org/deliver/etsi_gs/CCM/001_099/001/01.01.01_60/gs_ccm001v010101p.pdf>
- ETSI TS 103 572, the public text of the ST 2094-10 metadata syntax (absent L2 fields are
  inferred as 2048):
  <https://www.etsi.org/deliver/etsi_ts/103500_103599/103572/01.02.01_60/ts_103572v010201p.pdf>
- libplacebo FEL composition (float, 4:4:4; states EL siting found empirically as co-sited
  horizontally and centred vertically, which agrees with Annex B):
  <https://github.com/haasn/libplacebo/blob/c42968d8616a1d1c8ad5f4f1a8d6f5a9cb396e56/src/shaders/colorspace.c>.
  libplacebo's tone mapping changed on 2026-09-30, so any comparison must pin the version.
- vs-nlq, integer NLQ step on an already mapped 16-bit BL: <https://github.com/quietvoid/vs-nlq>
- `dovi_tool` conversion modes (mode 2 removes the mapping only for FEL and keeps it for MEL; mode
  5 converts to 8.1 with the mapping preserved; read from the crate source, mode 5 not run in the
  review): <https://github.com/quietvoid/dovi_tool>
- Open-source player work that composes FEL on the GPU and sends a Profile 8.1 RPU that keeps the
  authored L1/L2: <https://github.com/djnice/CoreELEC/releases/tag/dv-fel-s5-20260930>. A port of
  the libplacebo composition to FFmpeg tone-map filters:
  <https://github.com/jellyfin/jellyfin-ffmpeg/pull/774>. Both re-create the picture at playback;
  they are prior art for the reconstruction and for carrying authored metadata, not for this scope.
- Dolby Vision UHD Blu-ray authoring workflow guide (public; MEL has zero residual, FEL non-zero):
  <https://professional.dolby.com/siteassets/pdfs/dolby_vision_uhd_bluray_authoring_workflow.pdf>

[`PROVENANCE.md`](PROVENANCE.md) applies: public standards and open-source code only; no Dolby tool
output is used to derive or tune anything here.
