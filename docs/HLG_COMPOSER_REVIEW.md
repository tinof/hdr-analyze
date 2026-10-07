# Codex review of docs/HLG_COMPOSER.md (2026-10-03)

Model as printed by the run header: gpt-5.6-sol, reasoning effort high, sandbox read-only. Reviewed the first draft of the design note, before any implementation. The note was revised afterwards; section 13 of the note records how each required change was handled.

The design is promising, but it is not implementation-ready. The main issues are ambiguity in the fitting domain, incomplete superwhite coverage, resume/verification holes, and insufficient spatial-chroma validation.

## Critical findings

1. **The luma fitting target is stated in the wrong/ambiguous domain.**

   Section 3 says the composer output luma is `Y′_PQ` and implies the matrix then reproduces RGB exactly ([HLG_COMPOSER.md:50](../docs/HLG_COMPOSER.md)). That is not what the decoder evaluates. The polynomial outputs limited-range reshaped Y′, after which the decoder computes:

   `PQ_luma = (reshape_y − 1/16) × (9574/8192)`

   as shown in [hlg.rs:91](../hdr_analyzer_mvp/src/analysis/hlg.rs) and [hlg.rs:93](../hdr_analyzer_mvp/src/analysis/hlg.rs). Therefore the neutral polynomial target must be:

   `reshape_y_target = 1/16 + PQ_target / (9574/8192)`

   The note’s later observation that PQ black requires reshaped output `0.0625` ([HLG_COMPOSER.md:64](../docs/HLG_COMPOSER.md)) shows the intended calculation is probably correct, but the fitter specification must state it explicitly.

   Likewise, colored RGB cannot generally be reproduced “exactly”: the luma curve sees only input Y′, and two chroma outputs cannot correct an independently wrong PQ Y′. The note acknowledges this at [HLG_COMPOSER.md:81](../docs/HLG_COMPOSER.md), contradicting [HLG_COMPOSER.md:50](../docs/HLG_COMPOSER.md). Define the chroma target as the best reconstruction conditional on the already-fitted luma, and report the irreducible luma error separately.

2. **The normalized HLG inverse-OETF convention is underspecified.**

   The proposed OOTF is correct only if inverse-OETF output has already been normalized from BT.2100’s scene range to `[0,1]`. In that convention:

   - low branch: `E = E′² / 3`
   - high branch: `E = (exp((E′−c)/a)+b) / 12`
   - `F_D = 1000 × Y_S^0.2 × E`

   If the fitter instead uses BT.2100’s `[0,12]` scene values, the OOTF scale must contain `/12^γ`. The note should write the inverse-OETF equations, not merely cite Table 5. The core choice of `L_W=1000`, `L_B=0`, and `γ=1.2` is correct. See the current [BT.2100 recommendation](https://www.itu.int/rec/R-REC-BT.2100).

3. **The superwhite rule is a project policy, not uniquely prescribed by BT.2100/BT.2408.**

   BT.2408’s 1000-nit conversion concept is correct, but BT.2408 also explains that narrow-range HLG superwhites can extend the HLG colour volume sufficiently to match 1000-nit PQ primaries without clipping. BT.2100’s reference-display formulation instead clips values above nominal `E′=1` before display. A third policy—decode extended HLG and then independently clamp display-linear RGB to 1000 nits—is reasonable for a 1000-nit PQ-volume target, but mixed-color superwhites do not necessarily match pre-OETF clipping.

   Therefore [HLG_COMPOSER.md:47](../docs/HLG_COMPOSER.md) and [HLG_COMPOSER.md:54](../docs/HLG_COMPOSER.md) must label per-channel display-light clipping as the selected gamut/peak-limiting policy and provide reference vectors for mixed superwhite colors. The current chroma training grid stops at RGB `[0,1]³` ([HLG_COMPOSER.md:102](../docs/HLG_COMPOSER.md)), so it does not fit the policy it describes. BT.2408 explicitly discusses legal narrow-range levels up to about `E′=1.09`; see [BT.2408-8](https://www.itu.int/dms_pub/itu-r/opb/rep/R-REP-BT.2408-8-2024-PDF-E.pdf).

   Clipping negative RGB to zero before inverse OETF is appropriate for the display-light target. It should be distinguished from preserving sub-black signal excursions in the untouched base layer.

## MMR and decoder semantics

The neutral-axis concept is correct: input neutral is `n = 512/1023`, while the output chroma offsets are exactly `0.5`, as confirmed by [hlg.rs:227](../hdr_analyzer_mvp/src/analysis/hlg.rs).

However, “the constant coefficient must equal 0.5” is safe only if it means the constant of the MMR after substituting `u=v=n`, not the serialized `mmr_constant` field. For each order `o=1..3`, the actual constraints are:

- `A0 = c + Σ[n^o(a_u,o+a_v,o) + n^(2o)a_uv,o] = 0.5`
- `Ao = a_y,o + n^o(a_yu,o+a_yv,o) + n^(2o)a_yuv,o = 0`

The implementation evaluates exactly those powered compound terms at [hlg.rs:257](../hdr_analyzer_mvp/src/analysis/hlg.rs). Put these equations in the note and enforce them again after fixed-point quantization; solving only in f64 and then independently rounding coefficients weakens the equality.

There are two important clamps:

- Luma polynomial output is clamped to cumulative first/last pivots at [hlg.rs:78](../hdr_analyzer_mvp/src/analysis/hlg.rs) and [hlg.rs:91](../hdr_analyzer_mvp/src/analysis/hlg.rs). The proposed first pivot `≤63` is sufficient for reshaped black `1/16`, but tests should prove the clamp never changes an intended target.
- Chroma MMR output is clamped at [hlg.rs:257](../hdr_analyzer_mvp/src/analysis/hlg.rs) and [hlg.rs:275](../hdr_analyzer_mvp/src/analysis/hlg.rs). Pivots `[0,1023]` give `[0,1]`, but the fitter must report clamp activation over its training and held-out domains. Large cancelling coefficients can otherwise appear accurate before the real decoder clips them.

Also distinguish raw reconstruction from analyzer policy. `rgb_pq()` is unclamped, while `max_rgb_pq()` clamps to `[62,3079]` at [hlg.rs:280](../hdr_analyzer_mvp/src/analysis/hlg.rs) and [hlg.rs:293](../hdr_analyzer_mvp/src/analysis/hlg.rs). Neutral/color-accuracy tests must use raw RGB; sidecar/L1 tests must separately test the source-range clamp.

## Analyzer/RPU synchronization

The named-composer model is sound, and sidecar version 4 does not need to become 5. `luminance_mapping` is already the mapping identity field ([l1_sidecar.rs:97](../hdr_analyzer_mvp/src/l1_sidecar.rs)); a new strictly validated value does not change the JSON schema. Keep names immutable and add a new suffix for any coefficient change.

There are nevertheless two control-flow holes:

- Fingerprint-less legacy directories are intentionally resumed at [pipeline/mod.rs](../mkvdovi/src/pipeline/mod.rs). Thus a `bt2100` run could reuse a preset `RPU.bin` from an older run despite adding the flag to `resume_settings()`. For non-default composers, a missing fingerprint must force a clean regeneration.
- Verification currently silently skips sidecar checks when loading fails at [verify.rs:177](../mkvdovi/src/verify.rs), and only inspects frame zero structurally at [verify.rs:119](../mkvdovi/src/verify.rs). Composer verification must hard-fail an unreadable/mismatched HLG sidecar and compare the mapping on every RPU frame, not only the first.

`dv_profile_for` and `check_luminance_mapping` must take the selected composer expectation explicitly. Do not broaden the current exact comparison at [metadata/sidecar.rs](../mkvdovi/src/metadata/sidecar.rs) into a mere `dovi84-*` prefix check.

## RPU rewriting

The crate correctly handles CRC regeneration when `modified=true`: it computes the CRC during writing and only compares against the parsed CRC when unmodified ([dovi_rpu.rs:293](https://docs.rs/crate/dolby_vision/3.4.0/source/src/rpu/dovi_rpu.rs)). That part is safe.

Required safeguards:

- Assert `coefficient_data_type == 0`, `coefficient_log2_denom == 23`, and `coefficient_log2_denom_length == 23` on every frame before installing coefficients. The default is 23 at [rpu_data_header.rs:246](https://docs.rs/crate/dolby_vision/3.4.0/source/src/rpu/rpu_data_header.rs), but the mapping writer interprets fractions using the parsed header at [rpu_data_mapping.rs:180](https://docs.rs/crate/dolby_vision/3.4.0/source/src/rpu/rpu_data_mapping.rs).
- Assert `use_prev_vdr_rpu_flag == false` and a mapping is present on every generated frame. Merely setting `rpu_data_mapping` does nothing when that header flag is true; writing is gated at [dovi_rpu.rs:271](https://docs.rs/crate/dolby_vision/3.4.0/source/src/rpu/dovi_rpu.rs).
- Match the binary RPU format exactly: `00 00 00 01` followed by `write_hevc_unspec62_nalu()[2..]`, not the full encoded NAL. That stripping is explicit in [generate.rs:205](https://docs.rs/crate/dolby_vision/3.4.0/source/src/rpu/generate.rs).
- Write to a sibling temporary file, reparse it, verify frame count/mappings/CRC, then rename it atomically.
- Move `resume::mark_done` until after rewriting and validation. Currently it is written immediately after external generation at [pipeline/dovi_steps.rs](../mkvdovi/src/pipeline/dovi_steps.rs), contrary to the design note’s ordering.
- Keep the runtime byte-identical unmodified round trip. It protects against a newer external `dovi_tool` producing metadata the pinned crate cannot preserve.

`dovi_tool editor` is not a better current alternative: its supported edits do not expose arbitrary replacement composer curves. A cleaner option is to deserialize the complete HLG `extra.json`, call the crate’s `GenerateConfig` in-process, replace mappings in the generated list, and serialize once. HLG already embeds complete length/shots in the configuration at [metadata/rpu_config.rs](../mkvdovi/src/metadata/rpu_config.rs). That avoids parse–rewrite risk and should be compared byte-for-byte against external `dovi_tool generate` for the preset path.

## Input contract and chroma siting

Refusing tagged full range, non-BT.2020 primaries, and non-BT.2020-NCL matrices is correct. BT.2020 constant-luminance must be refused: the RPU matrix and fitter both assume NCL.

Untagged fields are not proof of invalid input, so accepting them under an explicit limited/BT.2020-NCL assumption is reasonable, but it must emit a conspicuous warning. Field-by-field fallback is needed; do not let one partially populated MediaInfo result prevent ffprobe from filling another field. Conflicting container, bitstream, and `_Original` values should be reported rather than resolved silently.

The analyzer currently treats BT.2020 CL as an allowed no-warning case at [pipeline.rs:401](../hdr_analyzer_mvp/src/pipeline.rs), despite always applying NCL. That must change if the note promises analyzer warnings.

Chroma siting does not change the coefficient fit for spatially uniform code triplets, but it matters for actual playback and validation. MMR is nonlinear and depends on luma, so reshaping interpolated chroma is not equivalent to replicating a raw 4:2:0 sample and reshaping it four times. The analyzer currently does the latter at [frame.rs:614](../hdr_analyzer_mvp/src/analysis/frame.rs) and [frame.rs:642](../hdr_analyzer_mvp/src/analysis/frame.rs), while the validation scripts only use flat frames ([validate_hlg_dv84_color.sh:42](../scripts/validate_hlg_dv84_color.sh)). Add spatial color-edge/zone-plate tests with left and top-left siting against libplacebo. If errors affect L1 materially, the analyzer needs renderer-equivalent chroma reconstruction or an explicit approximation disclaimer.

VERDICT: CONCERNS

1. Specify the normalized inverse-HLG equations and correct limited-range reshaped-luma target; remove the claim that colored RGB is reproduced exactly.
2. Define per-channel 1000-nit clipping as an explicit superwhite policy, and fit/test colored superwhites through the legal narrow-range headroom.
3. Write the reduced neutral-axis MMR equations explicitly and restore the constraints after coefficient quantization.
4. Add clamp-activation, raw-RGB, source-range, held-out color, and numerical acceptance criteria; the present color validation has no pass threshold.
5. Make non-default composer runs reject fingerprint-less resume state, and move the RPU completion sentinel until after rewrite validation.
6. Make verification hard-fail sidecar errors and compare header plus composer mapping across every RPU frame.
7. Assert header denominator/data type and `use_prev_vdr_rpu_flag`, serialize the exact RPU file format atomically, and add CRC/inject/extract round-trip tests.
8. Refuse BT.2020 CL, warn conspicuously on assumed untagged colorimetry, test conflicting/numeric tag forms, and fix the analyzer’s current CL no-warning path.
9. Add non-flat 4:2:0 chroma-siting validation against libplacebo and either match its reconstruction or document and bound the analyzer approximation.
