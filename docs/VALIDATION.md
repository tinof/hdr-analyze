# Validation Report — Base-Layer Measurement Accuracy

**Analyzer version:** hdr_analyzer_mvp 0.3.0 · **Date:** 2026-07-06 · **Rule:** accuracy is
*measured, never asserted*. This report exists so that no accuracy claim in this project ever
outruns its evidence. Reproduction commands are at the bottom.

## Method

Three independent ground truths, in increasing order of authority:

1. **Synthetic (mathematical truth).** Lossless FFV1 PQ clips whose peak is known by
   construction (solid 10-bit codes for 100 / 1000 / 4000 nits plus a saturated BT.2020
   color with independently constructed max-RGB and luma peaks). Automated as
   `hdr_analyzer_mvp/tests/synthetic_accuracy.rs`; runs in CI wherever ffmpeg exists.
2. **Embedded retail-style L1.** The community FEL test asset
   (`FEL TEST ST DL P7 CMV4.0 4000nits`, 2908 frames / 34 shots, mastered 4000 nits) carries
   Dolby-ecosystem-authored CM v4.0 L1. Extracted with `dovi_tool extract-rpu` + `export`
   (no Dolby software involved).
3. **cm_analyze (licensed Dolby Vision Professional Tools).** Run on the *identical
   demuxed base layer* our analyzer sees (16-bit TIFF, `u16 i444 rgb tight computer pq bt2020`,
   mastering display ID 8, the reference shot list). Trial run: v5.6.4 (ARM host, qemu);
   full run: v5.6.1 native x86-64 with CUDA on an RTX 4070 — the two versions produced
   bit-identical L1 on the 24-frame verification shot. Used strictly to score output
   accuracy — see the validation boundary in `docs/PROVENANCE.md`.

Comparison harness: `tools/l1_diff` (per-frame deltas in 12-bit PQ codes and nits).

## Results

### 1. Synthetic truth — peaks, mean, and robust minimum

| Constructed peak | Measured error |
|---|---|
| 100 nits | < 0.25 of one 12-bit PQ code |
| 1000 nits | < 0.25 of one 12-bit PQ code |
| 4000 nits | < 0.25 of one 12-bit PQ code |
| Saturated BT.2020 max-RGB | < 0.25 of one 12-bit PQ code |
| Same color, Y-luma domain | < 0.25 of one 12-bit PQ code |
| Solid-gray Y mean | exact after 12-bit sidecar quantization |
| Uniform 0.05-nit raised-black minimum | exact after 10-bit/fine-histogram quantization |
| Raised black + one dark speckle, P0.1 | raised floor preserved |
| Same pattern, P0 absolute minimum | dark speckle exposed as code 0 |

The analyzer reproduces mathematically known peaks to within the measurement format's own
quantization (observed error ≈ 0.03 code). Pixel reading, PQ math, and file writing are exact.

### 2. The definitional gap: Y-luma peak vs max-RGB (MaxSCL)

The FEL test asset turned out to be a max-RGB torture test: its base layer contains highly
saturated color whose max-RGB peak (~100 nits, PQ code 2081) is ~7× its luma peak
(~14 nits, code 1332). Three-way agreement on shot 0 confirms every measurement is faithful
and the gap is purely definitional:

| Measurement | Peak (12-bit PQ code) |
|---|---|
| Embedded L1 max (Dolby-authored) | 2081 |
| cm_analyze 5.6.4 on the same BL | 2081 (0.508078 × 4095) |
| Independent NumPy max-RGB of raw BL pixels | 2096 (chroma-upsampling tolerance) |
| **hdr_analyzer_mvp (Y-luma peak)** | **1332** |

**Consequence:** Y′ is exact *as a luma measurement* but is not the same quantity as DV L1 max.
PQ direct peaks therefore now default to max-RGB; `--peak-domain luma` retains Y′ for diagnostics
and compatibility. The implicit peak source is direct `max` in max-RGB domain, including under the
balanced/aggressive profiles; explicit histogram peak sources and APL remain Y-based. HLG forces
luma until per-channel scene-to-display conversion is implemented.

### 3. cm_analyze on the identical base layer (full 2908 frames, 34 shots)

Completed 2026-07-06 (cm_analyze 5.6.1, native x86-64, CUDA). Three results:

**cm_analyze agrees with the embedded L1 bit-exactly where the content fits the BL.**
cm_analyze reports `max_pq = 2081` on every one of the 2908 frames. On the 2668 frames whose
embedded L1 max is below 4000, the error against the Dolby-authored L1 is exactly **0.0
codes**. On the remaining 240 frames the embedded L1 says 4095 while the BL still measures
2081 — direct quantification of the FEL caveat in §5: those peaks exist only in the
enhancement layer, and *no* BL-only analyzer (Dolby's included) can see them.

**Pre-WS1 Y-luma baseline vs cm_analyze on identical pixels (per-frame, `tools/l1_diff`):**

| Metric (ours − cm_analyze, 12-bit PQ codes) | Peak (max_pq) | Average (avg_pq) |
|---|---|---|
| signed bias | −750.2 | +273.5 |
| \|error\| median / p95 / max | 748.7 / 753.4 / 753.4 | 273.6 / 279.6 / 279.6 |
| worst per-frame difference in nits | 86.5 | 30.9 |

The peak row is the definitional Y-vs-MaxSCL gap of §2, now scored against Dolby's own
analyzer over the full file instead of a single shot. The average row is *not* directly
actionable yet: Dolby's L1 mid is not a plain arithmetic mean, and the embedded L1 averages
disagree with cm_analyze-on-BL by even more (bias +211.7, max |error| 1487 codes) because the
authored metadata reflects L5 letterbox exclusion and EL composition. Scoring our average
waits for WS1 true-mean plus letterbox handling (§5).

Scene cuts: ours matched 1/34 against the reference shot list — the near-static-content
limitation already recorded in §5.

### 4. Implemented max-RGB peak vs cm_analyze

The production Rust max-RGB path was run over the full BL with `--peak-source max
--peak-domain max-rgb` and scored against cm_analyze on all 2908 frames:

| Metric (hdr_analyzer_mvp − cm_analyze, 12-bit PQ codes) | Value |
|---|---|
| signed bias | +12.8 |
| \|error\| median / p95 / max | 12.8 / 14.9 / 22.0 |
| worst per-frame difference | 5.5 nits |

The same result holds against embedded L1 on the 2668 BL-limited frames because cm_analyze and
embedded L1 are bit-identical there. The remaining 240 embedded-L1 frames describe EL-only peaks
and are intentionally excluded from a BL accuracy claim.

The small constant positive bias is chroma-upsampling tolerance (the implementation uses nearest-
neighbor 4:2:0→4:4:4; the cm_analyze input path used spline resampling). The production result
matches the independent NumPy preview and reduces the prior −750.2-code Y-luma bias to +12.8 codes.

A post-review default-path check used the first 24 BL frames with no `--peak-domain` or
`--peak-source` override, the default balanced optimizer profile, and default histogram smoothing.
The pipeline reported `Peak source: max` and scored +14.9 codes on every frame against cm_analyze.
The saturated synthetic integration test exercises this same default path in CI.

### 5. P2 sidecar comparison against embedded L1

The updated analyzer was run over all 2908 base-layer frames with the reproduction flags below, and
the extended `l1_diff` read `<output>.l1.json`. Against the asset's embedded, shot-authored FEL L1:

| Metric (ours − embedded L1, 12-bit PQ codes) | Bias | \|error\| median / p95 / max |
|---|---:|---:|
| Robust P0.1 minimum | +901.5 | 1057.0 / 1925.0 / 1925.0 |
| True Y-luma mean | −119.6 | 35.0 / 1818.0 / 1818.0 |

These numbers validate frame alignment and exercise all new scorer paths; they are not a parity
claim. The reference describes the composed BL+EL image and changing L5 active areas, while this run
measures the full uncropped base layer. In particular, its minimum and tail errors are expected to be
large. The Y-mean median error is 35 codes on this embedded reference, but Dolby L1 mid is not asserted
to be either arithmetic-mean domain. A fresh identical-pixel `cm_analyze` export is still needed for
a controlled before/after average-error delta. The max-RGB row from the initial P2 run is intentionally
omitted because it predated symmetric temporal smoothing of both average domains and has not been
rerun on the full corpus.

### 6. Known limitations surfaced by this study

- **Scene detection on near-static content:** the FEL asset's BL is nearly constant in luma
  (the signal lives in the enhancement layer); the histogram-distance metric found 1 of 34
  shots. Real-world HDR10 content behaves differently, but synthetic/test material is a known
  weak spot (see roadmap WS2 / hybrid metric).
- **P7 FEL base layers may not be HDR10-compatible.** This asset's BL is a reshaped ~14-nit
  signal. Measuring "the BL peak" of such files is well-defined but *not* comparable to the
  composed DV picture — relevant to any BL-vs-DV-peak inspection tooling built on top of this
  analyzer.
- **avg_pq comparisons need letterbox handling** (this asset carries varying L5 offsets up to
  320 rows); peak is unaffected by black bars, averages are not.

## Reproduction

```bash
# ground truth (no Dolby software)
ffmpeg -i FEL_TEST.mkv -c:v copy -bsf:v hevc_mp4toannexb -f hevc fel.hevc
dovi_tool extract-rpu fel.hevc -o RPU.bin
dovi_tool export -i RPU.bin -d all=rpu_full.json,scenes=scenes.txt   # L1 per frame from JSON
dovi_tool demux fel.hevc                                             # -> BL.hevc + EL.hevc

# our measurement (pure-measurement mode)
mkvmerge -o BL.mkv BL.hevc
hdr_analyzer_mvp BL.mkv -o BL_measurements.bin \
  --peak-source max --header-peak-source max --peak-domain max-rgb \
  --disable-optimizer --no-crop

# comparison
cargo run --release --manifest-path tools/l1_diff/Cargo.toml -- \
  --ours BL_measurements.bin --reference l1_ref.csv --scenes scenes.txt

# synthetic truth
cargo test -p hdr_analyzer_mvp --test synthetic_accuracy
```
