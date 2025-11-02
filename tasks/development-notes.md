# Development Notes — Current Status, Workflow, and Next Steps

This document captures the up‑to‑date development status, how to run and verify the end‑to‑end workflow, and the prioritized plan for upcoming work. Use this as a handoff to continue coding in the next session.

## Current Status Snapshot

- Analyzer (hdr_analyzer_mvp)
  - Native FFmpeg pipeline; v5/v6 writer; optimizer enabled by default.
  - Histogram smoothing (EMA + optional temporal median), pre‑denoise (median3), hue histogram (31 bins).
  - Active‑area crop detection; native HLG analysis path (HLG→nits→PQ→histogram) with `--hlg-peak-nits`.
  - Validation: `verifier` tool checks header fields, histograms, FALLs, hue histogram.
- mkvdolby
  - Uses native HLG analysis to generate measurements; re‑encodes HLG→PQ only for the DV base layer (required for Profile 8.1).
  - Generates DV RPU from madVR measurements (`dovi_tool generate`) and injects into a PQ base layer (HEVC) for muxing to MKV.
- Baseline/Tools
  - Baseline comparison tool present at `tools/compare_baseline`.
  - Docs for `dovi_tool` and `hdr10plus_tool` available under `tests/`.

## End‑to‑End Workflow

### Prerequisites

- Build Rust workspace
  - `cargo build --release --workspace`
- Ensure required external tools are installed and in PATH
  - `ffmpeg`, `mkvmerge`, `mediainfo`
  - `dovi_tool`, `hdr10plus_tool`
- Ensure analyzer is in PATH
  - Add `$(pwd)/target/release` to PATH

### Run mkvdolby on a file

- Using local checkout without install:
  - `PYTHONPATH="mkvdolby/src" PATH="$(pwd)/target/release:$PATH" python -m mkvdolby.cli "<input_video>"`
  - With verification: add `--verify` to run our `verifier` + `dovi_tool` checks post-mux and fail on inconsistencies.

Notes
- HDR10 inputs: mkvdolby will use an existing measurements file if present, otherwise runs the analyzer.
- HLG inputs: mkvdolby runs analyzer natively on the original HLG file and passes `--hlg-peak-nits` to the analyzer; it also performs a single HLG→PQ encode to produce the PQ base layer required for DV P8.1.

### Example (used in validation)

Input: `tests/hdr-media/LG Cymatic Jazz HDR10 4K Demo.mp4`

1) Generate measurements + DV output
- `PYTHONPATH="mkvdolby/src" PATH="$(pwd)/target/release:$PATH" python -m mkvdolby.cli "tests/hdr-media/LG Cymatic Jazz HDR10 4K Demo.mp4"`

2) Verify measurements (.bin)
- `target/release/verifier "tests/hdr-media/LG Cymatic Jazz HDR10 4K Demo_measurements.bin"`
  - Expect: Version 5, scene/frame counts printed, optimizer flag present, validation OK.

3) Verify DV output (Profile 8.1)
- Inspect container:
  - `mediainfo "tests/hdr-media/LG Cymatic Jazz HDR10 4K Demo.DV.mkv"`
  - Expect: Dolby Vision Profile 8.1, PQ transfer, BT.2020 primaries, HDR10 compatible.
- Inspect RPU:
  - `dovi_tool extract-rpu -i "tests/hdr-media/LG Cymatic Jazz HDR10 4K Demo.DV.mkv" -o RPU.bin`
  - `dovi_tool info -i RPU.bin --summary`
  - Expect: Frames equal to measurements, Profile 8, CM v4.0, scene/shot count aligns with measurements.

Alt: single-step verification using mkvdolby
- `PYTHONPATH="mkvdolby/src" PATH="$(pwd)/target/release:$PATH" python -m mkvdolby.cli "tests/hdr-media/LG Cymatic Jazz HDR10 4K Demo.mp4" --verify`
  - Runs verifier on measurements, extracts RPU + prints summary, and cross-checks frame counts. Exits non-zero on mismatch.

### Running the analyzer directly

- PQ/HDR10 content:
  - `cargo run -p hdr_analyzer_mvp --release -- "video.mkv" -o "video_measurements.bin"`
- HLG content (native HLG path):
  - `cargo run -p hdr_analyzer_mvp --release -- "video_hlg.mkv" -o "video_hlg_measurements.bin" --hlg-peak-nits 1000`

### Baseline Harness

- Build: `cargo build --release -p compare_baseline`
- Prepare directories:
  - Baseline dir: contains known‑good `.bin` files (e.g., frozen outputs).
  - Current dir: contains new `.bin` files from latest run.
- Run:
  - `target/release/compare_baseline --baseline ./baseline_bins --current ./current_bins`
  - Reports deltas for scene count, MaxCLL/MaxFALL, and target_nits 95th‑percentile delta.

## Prioritized Next Steps (Plan)

1) Scene Detection — Hybrid Metric
- Fuse flow magnitude (optical‑flow histogram) with histogram distance; add `--scene-metric hybrid`.
- Gate default switch on ≥3% F1 uplift with ≤15% runtime overhead at `--downscale 4`.

2) Optimizer Calibration + Robust Peaks
- Calibrate knee from P99/P99.9 with APL classes; enforce per-scene delta caps.
- Add `--header-peak-source {max|histogram99|histogram999}` to select header MaxCLL to reduce outlier spikes. ✓ Implemented.

3) Full RGB Gamut Peaks (v6)
- Implement BT.2020 YUV→RGB and 2020→P3/709 transforms and gamut clipping for per‑gamut peaks.

4) HLG Validation Parity
- Validate native HLG path vs legacy zscale re‑encode workflow on a small corpus; expose an “HLG validation mode” that dumps both paths for diff.

5) Baseline Harness + CI Integration
- Freeze a tiny corpus; run `tools/compare_baseline` in CI; publish deltas and verifier logs.
- Add a DV smoke (extract RPU + info) to CI for mux correctness.

6) Metadata Cohesion (mkvdolby)
- If static metadata is missing (MaxCLL/MaxFALL), fallback to values derived from measurements header.
- Add `--verify` option to run verifier + `dovi_tool info` and fail on inconsistencies. ✓ Implemented.

7) Unit/Integration Tests
- Add crop detection tests (synthetic letterbox) and HLG conversion sanity tests (known mappings).
- Add integration tests to run analyzer on short clips and assert scene counts, histogram sums, writer invariants.

8) Hardware Decode Contexts (Later)
- Add VAAPI/VideoToolbox device contexts for faster decode; keep default as software until stable.

## Acceptance & DoD Highlights

- Hybrid metric: F1 ↑ ≥3% vs histogram‑only on eval subset, ≤15% runtime overhead.
- Optimizer calibration: smoother per‑scene targets; reduced outlier‑driven spikes; documented profile tables.
- RGB peaks: per‑gamut peaks match proper color transforms; verifier extended accordingly.
- HLG parity: native vs legacy path within tolerance for histograms/peaks.
- CI: baseline diffs reported; DV smoke test passes.

## Known Caveats

- Dolby Vision Profile 8.1 requires a PQ (HDR10) base; we retain one HLG→PQ encode for the base layer even though measurements are generated natively from HLG.
- When static metadata is missing, mkvdolby currently defaults to 1000/400; planned improvement to derive from measurements.
- VAAPI/VideoToolbox are placeholders; CUDA path is attempted when available.

## References (Paths)

- Analyzer pipeline: `hdr_analyzer_mvp/src/pipeline.rs`
- HLG analysis: `hdr_analyzer_mvp/src/analysis/hlg.rs`
- Frame analysis (HLG branch): `hdr_analyzer_mvp/src/analysis/frame.rs`
- Writer: `hdr_analyzer_mvp/src/writer.rs`
- Verifier: `verifier/src/main.rs`
- mkvdolby conversion: `mkvdolby/src/mkvdolby/conversion.py`
- Baseline harness: `tools/compare_baseline`
