# HDR / HDR10+ / HLG → Dolby Vision: Conversion Quality

Findings and the improvement plan for perfecting `mkvdovi`'s conversion quality. Companion to
[CM_ANALYZE_PARITY.md](CM_ANALYZE_PARITY.md) (the analyzer-accuracy deep dive) and
[cmv40_upgrade_implementation_plan.md](cmv40_upgrade_implementation_plan.md).

> **Chosen direction:** the **default is source-honest / reference-accurate** — produce an accurate L1
> from the content and let the Dolby Vision display do its own mapping; do **not** bake the optimizer's
> per-frame target nits into the RPU by default. A display-targeted mode (the madMeasureHDR "set my
> display peak" workflow, e.g. 680 nits) is added as an **opt-in** `--target-nits`.

---

## 1. How conversion works today

Three paths in `mkvdovi/src/pipeline.rs`:

- **HDR10** and **HDR10+**: the base layer is **copied untouched** (`ffmpeg -c:v copy`). The picture is
  never re-encoded, so conversion quality is **100% the RPU metadata + the display's mapping**. There is
  **no way to "filter" the image** without re-encoding (out of scope). The lever is metadata accuracy.
- **HLG**: re-encoded HLG→PQ via `zscale ... rangein=tv:range=tv:npl=<hlg-peak>` + x265. (This is the only
  path that touches pixels.)

L1 (min/avg/max) is computed by `dovi_tool generate` from either the madVR measurements file
(HDR10/HLG) or the extracted HDR10+ metadata. We emit neutral L2 trims (all `2048`), L6, L9, L11; L254 is
added by `dovi_tool`.

---

## 2. Findings (answers to the review questions)

### 2.1 Are bare-`mkvdovi` defaults quality-optimal?

Defaults: `cm v40`, `content-type movies`, `reference-mode false`, `peak-source histogram`,
`optimizer-profile conservative`, `analysis-quality balanced`, neutral L2, source deleted on success.
They are **balanced/safe, not max-quality**:

- `analysis-quality balanced` analyzes at **half resolution** (`analysis_quality_args` → downscale 2).
  `accurate` (full-res) yields more precise L1/peak.
- The **optimizer is on and its per-frame `target_nits` are baked into the RPU** (see 2.2) — an opinionated
  transform applied by default.

### 2.2 The optimizer and the missing "680 nits" control

- The optimizer (`hdr_analyzer_mvp/src/optimizer.rs`: 240-frame rolling average + knee + per-profile
  clamps) is a madMeasureHDR-style dynamic optimizer and is **on by default**.
- `mkvdovi` runs the analyzer with it on and then calls
  `dovi_tool generate --madvr-file --use-custom-targets`, so the per-frame `target_nits` **flow into the
  RPU**. (Exact effect of `--use-custom-targets` on L1/L2/L8 is to be confirmed empirically — see P0.)
- **There is no per-display target control in `mkvdovi`.** `--target-peak-nits` exists only in the
  analyzer and only sets a v6 header field; it does **not** drive the optimizer clamps. So the
  "type 680 and it optimizes for my display" workflow is **not wired up**. → added as opt-in `--target-nits`
  (P4).

### 2.3 `hdr10plus_tool` (installed: 1.7.1)

Used correctly: `extract` to JSON → `dovi_tool generate --hdr10plus-json --hdr10plus-peak-source`
(`hdr10plus_tool` has no RPU conversion of its own). Only gap: no `--skip-reorder` fallback for
mis-authored files (P5).

### 2.4 `dovi_tool` (installed: 2.3.2 — latest)

We are **current** (2.3.1 even shipped "Corrected RPU generation from madVR files"). Underused features:
**`--canvas-width/--canvas-height`** to generate **L5 active area** (we leave L5 at defaults), and
`--long-play-mode`.

### 2.5 The v5/v6 madVR format & gamut peaks are orthogonal to DV

A common misconception is that the "v6 per-gamut peak approximation" lowers DV conversion quality. It
does not, for two independent reasons:

1. **`mkvdovi` writes v5.** `run_hdr_analyzer` never passes `--madvr-version`, so the analyzer defaults
   to v5 (`hdr_analyzer_mvp/src/cli.rs`). The v6 approximation in `hdr_analyzer_mvp/src/writer.rs`
   (guarded by `madvr_version >= 6`) is **dead code on the DV path**.
2. **DV L1 is a single luminance triplet** (min/avg/max in PQ) with no per-gamut concept. `dovi_tool
   generate --madvr-file` builds L1 from `peak_pq_2020` + the histogram; the DCI-P3/709 peaks are a
   **madVR playback** feature only.

The real DV lever is the accuracy of `peak_pq_2020` itself. PQ input now defaults to a BT.2020
max-RGB direct peak (`--peak-domain luma` retains the legacy measurement), while histogram-derived
peaks remain Y-based. True v6 target-gamut transforms are a separate follow-up. So "full v6" is not
a DV goal; accurate BT.2020 peak is.

---

## 3. Quality issues → root causes → fix

For HDR10/HDR10+ the base layer is untouched, so every fix below is **metadata accuracy** or an
**optional source-honest mode** — not a picture filter.

| Symptom | Likely root cause | Fix |
|---------|-------------------|-----|
| **Raised blacks** | No robust L1 **min** — `dovi_tool` infers min from a histogram that includes letterbox black + grain | Robust low-percentile min over the active area (P2 / WS1) |
| **Raised blacks** | **No L5 active area** — black bars pollute L1 stats | Emit L5 from detected crop (P3 / WS3) |
| **Raised blacks / unnatural look** | Optimizer `target_nits` **baked into RPU** by default | Source-honest default (P1) |
| **Strange colors** | **L9 source primaries mis-detected** (P3 vs BT.2020) | Harden L9 detection; `--source-primaries` override (P5) |
| **Strange colors (HLG only)** | HLG re-encode tags `master-display` with **hardcoded P3 primaries** though HLG is BT.2020 | Use BT.2020 master-display primaries (P5) |

---

## 4. Decisions

- **Default = source-honest.** Accurate, content-derived L1; neutral trims; no optimizer baking; display
  maps to its own peak.
- **`--target-nits` is opt-in** for the display-targeted workflow.
- **Delete-by-default retained** (use `--keep-source` to keep). No change this round.

---

## 5. Prioritized improvement plan

Each item names goal · change locus · validation. P2/P3 are the conversion-side framing of parity
workstreams [WS1/WS3](CM_ANALYZE_PARITY.md#5-dependency-ordered-workstreams).

- **P0 — Verify empirically** *(gate)*: run `mkvdovi` on a sample, then `dovi_tool info` on the RPU to
  confirm exactly what we emit and what `--use-custom-targets` does to L1/L2/L8. (= parity WS0.)
- **P1 — Source-honest default**: stop baking optimizer targets by default (drop `--use-custom-targets`
  and/or run the analyzer with `--disable-optimizer` in the default path — confirm mechanism in P0);
  consider `analysis-quality accurate` as the quality-first default. `mkvdovi/src/pipeline.rs`
  (`run_hdr_analyzer`, `generate_rpu`).
- **P2 — Robust L1 min + true-mean avg**: low-percentile min over the active area (noise-rejected),
  full-precision mean PQ. `hdr_analyzer_mvp/src/analysis/frame.rs`. Directly targets raised blacks.
- **P3 — Emit L5 active area** from the detected crop (`Level5` in `extra.json` via
  `mkvdovi/src/metadata.rs`, or `dovi_tool --canvas-*`). Fixes letterbox-polluted stats/levels.
- **P4 — `--target-nits` opt-in**: wire a user display peak into the optimizer (the 680-nits workflow);
  active only in the non-default display-targeted path. `mkvdovi/src/cli.rs`, `optimizer.rs`.
- **P5 — Robustness**: `hdr10plus_tool extract --skip-reorder` fallback on failure; HLG `master-display`
  primaries → BT.2020 (`convert_hlg_to_pq`); harden L9 detection (`detect_source_primaries`).
- **P6 — Problematic-source detection** (cryptochrome-inspired): warn on missing/inconsistent source
  metadata (partly done for L6/L9) and default to the safe/source-honest path.

---

## 6. Validation

- A/B real titles on the target display; diff RPUs with `dovi_tool info` before/after each change.
- Use the reference corpus from [WS0](CM_ANALYZE_PARITY.md#5-dependency-ordered-workstreams) to measure
  L1 min/avg/max error, not just eyeball it.
- No change ships as a new default without an A/B that shows it is at least as good as the current output.
