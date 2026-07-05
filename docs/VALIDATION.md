# Validation Report — Base-Layer Measurement Accuracy

**Analyzer version:** hdr_analyzer_mvp 0.3.0 · **Date:** 2026-07-05 · **Rule:** accuracy is
*measured, never asserted*. This report exists so that no accuracy claim in this project ever
outruns its evidence. Reproduction commands are at the bottom.

## Method

Three independent ground truths, in increasing order of authority:

1. **Synthetic (mathematical truth).** Lossless FFV1 PQ clips whose peak is known by
   construction (solid 10-bit codes for 100 / 1000 / 4000 nits). Automated as
   `hdr_analyzer_mvp/tests/synthetic_accuracy.rs`; runs in CI wherever ffmpeg exists.
2. **Embedded retail-style L1.** The community FEL test asset
   (`FEL TEST ST DL P7 CMV4.0 4000nits`, 2908 frames / 34 shots, mastered 4000 nits) carries
   Dolby-ecosystem-authored CM v4.0 L1. Extracted with `dovi_tool extract-rpu` + `export`
   (no Dolby software involved).
3. **cm_analyze (licensed Dolby Vision Professional Tools v5.6.4).** Run on the *identical
   demuxed base layer* our analyzer sees (16-bit TIFF, `u16 i444 rgb tight computer pq bt2020`,
   mastering display ID 8, the reference shot list). Used strictly to score output accuracy —
   see the validation boundary in `docs/PROVENANCE.md`.

Comparison harness: `tools/l1_diff` (per-frame deltas in 12-bit PQ codes and nits).

## Results

### 1. Synthetic truth — peak is exact

| Constructed peak | Measured error |
|---|---|
| 100 nits | < 0.25 of one 12-bit PQ code |
| 1000 nits | < 0.25 of one 12-bit PQ code |
| 4000 nits | < 0.25 of one 12-bit PQ code |

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

**Consequence:** our current Y-based peak is exact *as a luma measurement* but is not the same
quantity as DV L1 max. On saturated content the difference is large. Max-RGB peak measurement
is therefore promoted from "planned" to **required** (roadmap WS1); until it ships, peak
comparisons against DV L1 must expect underread on saturated highlights.

### 3. cm_analyze on the identical base layer (full 2908 frames, 34 shots)

*In progress — the full run is being redone on x86 hardware (cm_analyze is x86-64 only; the
initial run on this project's ARM host required qemu emulation). Results land here next.*

### 4. Max-RGB preview (planned WS1 measurement vs cm_analyze)

Preview data is measured (per-frame max-RGB of the full BL: constant ≈2091–2103 codes, vs
embedded L1 max of 2081 on BL-limited shots — median agreement 12.8 codes — and 4095 on the
10,000-nit shots whose peaks live in the FEL enhancement layer). The scored table vs
cm_analyze follows with §3.

### 5. Known limitations surfaced by this study

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
  --peak-source max --header-peak-source max --disable-optimizer --no-crop

# comparison
cargo run --release --manifest-path tools/l1_diff/Cargo.toml -- \
  --ours BL_measurements.bin --reference l1_ref.csv --scenes scenes.txt

# synthetic truth
cargo test -p hdr_analyzer_mvp --test synthetic_accuracy
```
