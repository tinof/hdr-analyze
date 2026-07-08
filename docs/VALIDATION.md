# Validation Report — Base-Layer Measurement Accuracy

**Analyzer version:** hdr_analyzer_mvp 0.3.0 · **Date:** 2026-07-06 (§1–§6), 2026-07-08 (§7) · **Rule:** accuracy is
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

> **Attribution superseded by §7.** The 2026-07-08 reconstruction-envelope ablation showed
> neighbor- and spline-prepped inputs produce *identical* cm_analyze peaks; the filter choice was
> never the cause. The residual offset comes from the YCbCr→RGB conversion/rounding path and is
> bounded at ~10 codes.

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

### 7. Real-content validation: native 4:2:0 ingest, two titles (2026-07-08)

Second cm_analyze round on real content, with the external resampling step removed entirely:
cm_analyze ingested the **untouched 4:2:0 base layer** as raw yuv (layout tags embedded in the
filename, e.g. `name_3840x2160_u10_420p_le_lsb16.yuv`, with `--source-format` carrying only
`"ycbcr_bt2020 video pq bt2020"` — cm_analyze rejects raw-layout tags inside `--source-format`
itself). Mastering display ID 21 (BT.2020/D65/ST.2084, 0.0001–1000 nits), CUDA, all frames.

Two titles, 20:00–22:00 cuts, scored per shot with `l1_diff --per-shot`:

- **Title A** — UHD Blu-ray remux, DV Profile 7 **MEL** (composed picture = BL, so the embedded
  Dolby-authored L1 is a full-coverage BL reference); heavy film grain; L5 letterbox 120/120;
  2855 frames / 15 shots.
- **Title B** — 2160p web-service encode with authored HDR10+ (used to validate mkvdovi's
  HDR10+→DV L1 derivation); 2825 frames / 11 shots.

**Finding 1 — cm_analyze's default (CM v4) L1 is not a raw measurement.** Its per-shot peak has
an exact floor at PQ(100 nits) = code 2081 (4 of 15 Title A shots and 4 of 11 Title B shots report
exactly 2081.0 while `--analysis-version 2` on the same pixels reads 1537–2541), and its "avg" is
an anchored near-constant (codes 1228/1286 across every Title A shot), not a mean. The Title A embedded
L1 was authored with the v2.9-style algorithm: embedded vs cm v2 agrees to **+16.5 codes** bias
(median 20) while embedded vs default v4 differs by +198.0 (max 480). Measurement-style scoring
must therefore use `--analysis-version 2`; v4 output is a mapping-oriented product.

**Finding 2 — per-shot peak scores** (12-bit PQ codes, ours − reference):

| Comparison | bias | median \|err\| | p95 | max |
|---|---:|---:|---:|---:|
| Title A ours max-RGB vs cm v2 (like-for-like) | **+92.6** | 67.8 | 205.6 | 205.6 |
| Title A ours max-RGB vs cm v4 (floor-affected) | +274.1 | 289.6 | 613.4 | 613.4 |
| Title A ours Y-luma vs cm v2 (two canceling errors) | +21.9 | 16.6 | 90.4 | 90.4 |
| Title A embedded (v2.9-authored) vs cm v2 | +16.5 | 20.0 | 134.0 | 134.0 |
| Title B ours max-RGB vs cm v2 | **+74.4** | 63.6 | 170.6 | 170.6 |
| **Title B mkvdovi HDR10+→L1 vs cm v4** | **+5.1** | **1.0** | 17.0 | 17.0 |

The Title B row is the round's cleanest result: **mkvdovi's HDR10+-derived L1 is essentially
identical to Dolby's own v4 analyzer per shot** (median 1 code, max 17), including the v4 floor
behavior.

**Finding 3 — the chroma-reconstruction envelope is ~10 codes and filter-independent.** On the
first four Title A shots (508 frames), cm_analyze was run three ways on the same pixels:

| shot (frames) | native 4:2:0 | neighbor-prep TIFF | spline-prep TIFF |
|---|---:|---:|---:|
| 0–28 | 2330 | 2321 | 2321 |
| 29–90 | 2798 | 2787 | 2787 |
| 91–146 | 2081 | 2081 | 2081 |
| 147–507 | 2426 | 2416 | 2416 |

Neighbor and spline are *identical* — the peak-determining pixels are not chroma-edge-sensitive —
and both sit 9–11 codes below native-420. The offset comes from the YCbCr→RGB conversion/rounding
path, not from upsampling-filter choice. Consequence: the nearest-neighbor chroma sharing in
`analysis/frame.rs` stays; spline/bilinear alternatives are closed as immaterial, and prep
artifacts cannot explain the grain gap below.

**Finding 4 — the grain watch item is confirmed and quantified.** Against the like-for-like cm v2
measurement on identical BL pixels, our direct max reads **+92.6 codes hot on heavy-grain Title A**
(up to +206/shot, 377 nits worst) and **+74.4 on the milder Title B** (up to +171) — an order of
magnitude above the ~10-code prep envelope. Dolby's peak — even in its v2 measurement form —
rejects isolated grain spikes that a raw maximum keeps. Closing this requires a robust peak
estimator in the max-RGB domain (percentile/small-area filtering); that is a deliberate design
decision tracked in the roadmap, not something to slip into a default silently.

**Supporting results.** Minimum: 0.0-code error per shot everywhere; running cm with
`--letterbox 0 0 120 120` moves the min comparison vs embedded from −2.8 to +0.1 codes — L5
exclusion fully explains the min story. Averages: our max-RGB true mean matches cm v2's per-shot
average within **+9.5 codes** (median 8.7, max 17.3) — direct evidence for the WS1 average-domain
decision — while all comparisons against cm v4's anchored "avg" are definitional, not errors.
Scene detection: on Title A, ours matched 13 of 14 authored cuts (±1 frame) while emitting 24 cuts
total — high recall with some over-segmentation on real content, unlike the near-static FEL
asset's 1/34 under-detection. Caveat for the record: the
Title B stream signals a non-default chroma siting; both cm runs used cm's default siting.
The env-gated `real_content_consistency` integration test passed against a 15-second Title A cut.

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

# real-content round (§7) — sample prep on the analysis host
mkvmerge -o sample.mkv --no-audio --no-subtitles --split parts:00:20:00-00:22:00 SOURCE.mkv
ffmpeg -i sample.mkv -c:v copy -bsf:v hevc_mp4toannexb sample.hevc
dovi_tool demux -i sample.hevc --bl-out BL.hevc --el-out EL.hevc   # P7 input
dovi_tool extract-rpu -i sample.hevc -o rpu.bin
dovi_tool export -i rpu.bin -d all=rpu.json -d scenes=scenes.txt   # embedded L1 + shotlist
hdr10plus_tool extract -i sample.hevc -o hdr10plus.json            # HDR10+ input

# real-content round (§7) — cm_analyze on the untouched 4:2:0 BL (x86-64 + CUDA host)
ffmpeg -i BL.hevc -f rawvideo -pix_fmt yuv420p10le bl_3840x2160_u10_420p_le_lsb16.yuv
./cm_analyze -s shotlist.txt -m 21 -r 24000/1001 --analysis-version 2 \
  --source-format "ycbcr_bt2020 video pq bt2020" \
  bl_3840x2160_u10_420p_le_lsb16.yuv out.xml

# per-shot scoring
cargo run --release --manifest-path tools/l1_diff/Cargo.toml -- \
  --ours BL_measurements.bin --reference cm_l1.csv --per-shot shotlist.txt
```
