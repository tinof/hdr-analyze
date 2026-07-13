# CLI Reference

Complete command-line reference for the three HDR-Analyze binaries. For a quick start, see the
[README](../README.md). For HDR10+ peak mapping and Dolby Vision metadata details, see
[DOLBY_VISION.md](DOLBY_VISION.md).

All defaults below are taken directly from `--help`; run `<binary> --help` to confirm for your build.

---

## `hdr_analyzer_mvp`

Analyzes an HDR10/HLG video and writes a madVR-compatible `.bin` measurement file plus an
analyzer-owned `<output>.l1.json` sidecar containing explicit full-precision-derived L1 statistics.

```bash
hdr_analyzer_mvp -i "video.mkv" -o "measurements.bin"
# positional input also works; output auto-generated from input name if -o omitted:
hdr_analyzer_mvp "video.mkv"
```

### Core options

| Flag | Default | Description |
|------|---------|-------------|
| `-i, --input <PATH>` | — | Input video file (flag-based alternative to the positional arg) |
| `-o, --output <PATH>` | auto | Output `.bin` path; auto-generated from input name if omitted |
| `--madvr-version <5\|6>` | `5` | madVR measurement file version to write |
| `--hwaccel <TYPE>` | — | GPU hint: `cuda`, `vaapi`, `videotoolbox` (see [Hardware acceleration](#hardware-acceleration)) |
| `--downscale <1\|2\|4>` | `1` | Downscale internal analysis resolution for speed (1=full, 2=half, 4=quarter) |
| `--sample-rate <N>` | `1` | Analyze every Nth frame. Skipped frames inherit the previous frame's measurements. High performance impact |
| `--crop-probes <N>` | `7` | Seek-based active-area probes across the middle 70% of the input; `0` uses hardened in-stream fallback detection |
| `--no-crop` | off | Disable crop probing/detection and analyze the full frame |

### Scene detection

| Flag | Default | Description |
|------|---------|-------------|
| `--scene-threshold <float>` | `0.3` | Scene-cut distance threshold |
| `--scene-metric <hist\|hybrid>` | `hist` | `hist` = histogram distance; `hybrid` is a prototype that currently falls back to the same histogram metric |
| `--min-scene-length <frames>` | `24` | Drop cuts closer than N frames |
| `--scene-smoothing <frames>` | `5` | Rolling window over the scene-change metric (0 disables) |

### Optimizer

| Flag | Default | Description |
|------|---------|-------------|
| `--disable-optimizer` | off | Disable dynamic `target_nits` generation (enabled by default) |
| `--optimizer-profile <conservative\|balanced\|aggressive>` | `balanced` | Optimizer behavior preset |
| `--target-peak-nits <nits>` | computed MaxCLL | Override `header.target_peak_nits` (v6 only) |
| `--target-smoother <off\|ema>` | `ema` | `target_nits` smoother type |
| `--smoother-bidirectional` | off | Forward+backward EMA smoothing when `--target-smoother ema` |
| `--smoother-alpha <0.0–1.0>` | `0.2` | EMA alpha for `target_nits` smoothing (lower = more smoothing) |

### Noise robustness

| Flag | Default | Description |
|------|---------|-------------|
| `--peak-domain <max-rgb\|luma>` | `max-rgb` (PQ), `luma` (HLG) | Domain used for direct peak measurement. HLG forces luma because per-channel scene-to-display conversion is not implemented |
| `--peak-source <max\|histogram99\|histogram999>` | `max` in max-RGB domain; in luma, `histogram99` (balanced/aggressive) or `max` (conservative) | Per-frame peak brightness source |
| `--peak-estimator <max\|percentile\|robust>` | `max` | Estimator applied in the direct peak domain: raw maximum, fine-histogram percentile, or synthetic-calibrated grain correction |
| `--peak-percentile <0–100>` | `99.99` | Fine 4096-bin percentile used by `--peak-estimator percentile` |
| `--header-peak-source <max\|histogram99\|histogram999>` | — | MaxCLL source for the header only; per-frame peaks still use `--peak-source` |
| `--hist-bin-ema-beta <0.0–1.0>` | `0.1` | EMA smoothing for histogram bins (lower = more smoothing, 0 = disabled) |
| `--hist-temporal-median <N>` | `0` | Temporal median filter window in frames (3 = good for aggressive smoothing) |
| `--pre-denoise <nlmeans\|median3\|off>` | `off` | Pre-analysis Y-plane denoising (`median3` good for grain; `nlmeans` reserved) |
| `--min-percentile <0–100>` | `0.1` | Lower percentile used for the noise-rejected active-area minimum, in percent; `0` selects the absolute minimum |

- `max`: direct max from `--peak-domain` (most responsive to noise). For PQ, `max-rgb` decodes
  limited-range BT.2020 NCL and takes the maximum R′/G′/B′ PQ signal; `luma` retains the legacy Y′ peak.
- `histogram99`: 99th percentile (recommended, reduces noise impact).
- `histogram999`: 99.9th percentile (most conservative).

`--peak-source` selects the existing madVR/Y-histogram path; `--peak-estimator` controls how the
direct max-RGB or luma peak itself is measured. Robust mode estimates PQ-domain grain from
cross-chroma-quad differences and corrects Gaussian extremes while retaining isolated highlights.
It passes deterministic synthetic truth but remains opt-in because its first two-title real-content
gate did not justify changing the default.

Histogram percentiles and APL remain Y-based in both domains, preserving madVR histogram semantics.
An explicit histogram peak source therefore opts out of max-RGB peak selection.

The analyzer computes the average directly from active-area pixels rather than reconstructing it
from 256 histogram bins. The JSON sidecar records per-frame robust minimum, Y-luma mean, and
max-RGB mean as 12-bit PQ codes, plus scene aggregates and the crop/denoise settings. Both average
domains receive the same configured EMA/temporal smoothing with per-scene resets; the spatially
noise-rejected minimum is not temporally smoothed. The sidecar is measurement and validation output;
its minimum is not currently inserted into generated RPUs.

### HLG

| Flag | Default | Description |
|------|---------|-------------|
| `--hlg-peak-nits <nits>` | `1000` | Peak luminance used when analyzing auto-detected HLG (ARIB STD-B67) content |

### Performance & diagnostics

| Flag | Default | Description |
|------|---------|-------------|
| `--analysis-threads <N>` | logical cores | Override Rayon worker count for histogram analysis |
| `--profile-performance` | off | Print per-stage throughput (decode vs. analysis) when finished |
| `--dump-frame-stats <PATH>` | — | Write sample-rate-aligned CSV with selected/raw/percentile/robust peaks, sigma, correction, and effective-tail count |

### Notes for v6 output

- v6 adds per-gamut peaks (`peak_pq_dcip3`, `peak_pq_709`) and a `target_peak_nits` header on top of v5.
- The per-gamut peaks are currently **approximated** from BT.2020 (`peak_pq_2020`) using 99% and 95%
  factors. They are a **madVR measurement-file** feature and are **not consumed by the Dolby Vision
  conversion** — `mkvdovi` writes v5, and `dovi_tool` builds L1 from the BT.2020 peak + histogram. The
  approximation therefore affects only a standalone v6 `.bin` used directly by madVR, not DV output.
- Accurate per-gamut peaks are still a follow-up. The max-RGB decode machinery now exists, but v6
  output continues to use the approximations above until target-gamut transforms are implemented.

### Examples

```bash
# v6 file with explicit target peak
hdr_analyzer_mvp -i "video.mkv" -o "out_v6.bin" --madvr-version 6 --target-peak-nits 1000

# Aggressive smoothing for very noisy / grainy content
hdr_analyzer_mvp -i "grainy.mkv" -o "out.bin" \
  --hist-bin-ema-beta 0.05 --hist-temporal-median 3 --pre-denoise median3

# Opt into the max-RGB grain estimator and capture its per-frame decisions
hdr_analyzer_mvp -i "grainy.mkv" -o "robust.bin" \
  --peak-source max --peak-estimator robust --dump-frame-stats frame_stats.csv

# Select a fine-histogram direct peak instead
hdr_analyzer_mvp -i "grainy.mkv" -o "p9999.bin" \
  --peak-source max --peak-estimator percentile --peak-percentile 99.99

# Disable histogram smoothing for clean content
hdr_analyzer_mvp -i "clean.mkv" -o "out.bin" --hist-bin-ema-beta 0

# Use the absolute active-area minimum instead of the default noise-rejected P0.1
hdr_analyzer_mvp -i "clean.mkv" -o "out.bin" --min-percentile 0

# Conservative profile with direct max (most responsive)
hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --optimizer-profile conservative --peak-source max

# Retain the legacy direct Y-luma peak for PQ input
hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --peak-source max --peak-domain luma

# Native HLG, override assumed peak
hdr_analyzer_mvp -i "hlg.mkv" -o "out.bin" --hlg-peak-nits 1200

# Disable seek-based probing and use the first usable in-stream crop
hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --crop-probes 0

# Disable crop detection entirely (full-frame diagnostics)
hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --no-crop

# Via cargo
cargo run -p hdr_analyzer_mvp --release -- -i "video.mkv" -o "out.bin" --downscale 2
```

---

## `mkvdovi`

Orchestrates the full HDR10/HDR10+/HLG/Profile 7 → Dolby Vision Profile 8.1 (CM v4.0) conversion. Internally
calls `dovi_tool`, `mkvmerge`, and (for HDR10+) `hdr10plus_tool`; these must be installed separately
(see [README Prerequisites](../README.md#prerequisites)).

```bash
mkvdovi                 # process all .mkv files recursively from the current directory
mkvdovi "input.mkv"     # process a specific file
```

### General

| Flag | Default | Description |
|------|---------|-------------|
| `[INPUT]...` | cwd `*.mkv` | One or more input files; recurses cwd if omitted |
| `--keep-source` | off | Keep a non-DV source (DV inputs and `--mdfix` runs are always kept by default) |
| `--mdfix` | off | Rebuild Profile 7 MEL/Profile 8 RPU metadata from fresh base-layer measurements; writes `*.mdfix.DV.mkv` |
| `--no-resume` | off | Discard a leftover temp directory and re-run from scratch (by default an interrupted run **resumes**, reusing completed steps) |
| `--stall-timeout <SECS>` | `300` | Warn if the current step's output file stops growing for this long (`0` disables) — tells a stalled tool apart from merely slow storage |
| `--verify` | off | After muxing, validate the result (see [DOLBY_VISION.md](DOLBY_VISION.md#post-mux-verification)) |
| `-v, --verbose` | off | Show raw command output (debugging) |
| `-q, --quiet` | off | Minimal output (errors and final result only) |
| `--drop-chapters` | off | Drop chapters in the output (kept by default) |
| `--drop-tags` | off | Drop global tags in the output (kept by default) |

### Analysis & quality

| Flag | Default | Description |
|------|---------|-------------|
| `--analysis-quality <fast\|balanced\|accurate>` | `balanced` | HDR10/HLG analysis preset: `fast` = half-res/every 3rd frame; `balanced` = half-res/every frame; `accurate` = full-res/every frame |
| `--optimizer-profile <conservative\|balanced\|aggressive>` | `conservative` | Optimizer profile passed to the `hdr_analyzer_mvp` pass |
| `--hwaccel <none\|cuda>` | `none` | Hardware acceleration hint for analysis and encoding |
| `--encoder <libx265\|videotoolbox>` | `libx265` | Encoder for HLG→PQ conversion (`videotoolbox` ≈ 10× faster on Apple Silicon) |

### HDR10+ peak mapping

| Flag | Default | Description |
|------|---------|-------------|
| `--peak-source <histogram\|histogram99\|max-scl\|max-scl-luminance>` | `histogram` | Maps to `dovi_tool generate --hdr10plus-peak-source` |
| `-b, --boost` | off | Brighter preset; switches another selected `--peak-source` to `histogram99` |
| `--boost-experimental` | off | Asks `hdr_analyzer_mvp` to use a more aggressive optimizer profile |

See [DOLBY_VISION.md](DOLBY_VISION.md#hdr10-peak-mapping) for guidance on each source.

### Dolby Vision metadata (CM v4.0)

| Flag | Default | Description |
|------|---------|-------------|
| `--cm-version <v29\|v40>` | `v40` | Content Mapping version |
| `--content-type <default\|movies\|game\|sport\|user-generated-content>` | `movies` | L11 content type (`cinema`/`film` alias `movies`, `gaming` aliases `game`) |
| `--reference-mode <true\|false>` | `false` | L11 reference mode (critical/studio viewing) |
| `--source-primaries <0\|1\|2>` | auto | L9 source primaries: `0=P3-D65, 1=BT.709, 2=BT.2020` (auto-detected from MediaInfo if unset) |
| `--trim-targets <csv>` | `100,600,1000` | Nits values for the DV L2 trim pass (neutral compatibility trims — not a panel calibration) |

### HLG encode tuning

| Flag | Default | Description |
|------|---------|-------------|
| `--hlg-crf <N>` | `17` | CRF for HLG→PQ conversion |
| `--hlg-preset <preset>` | `medium` | x265 preset for HLG→PQ |
| `--hlg-peak-nits <nits>` | `1000` | Nominal HLG peak luminance (cd/m²) |

### Profile 7 FEL encode tuning

| Flag | Default | Description |
|------|---------|-------------|
| `--fel-crf <N>` | `18` | Local x265 CRF or Modal/NVENC quality parameter |
| `--fel-preset <preset>` | `medium` | Local x265 preset |
| `--fel-encoder <local\|modal>` | `local` | Encode the composited BL+EL result locally or offload it to Modal |

### Subcommands

| Command | Description |
|---------|-------------|
| `mkvdovi inspect <INPUT>` | Extract the complete RPU and report suspicious/static/clipped L1 patterns |
| `mkvdovi composite-pipe --bl <HEVC> --el <HEVC> --rpu <BIN> -w <PX> -H <PX>` | Write raw NLQ-composited frames to stdout for an encoder pipe; dispatched before global dependency checks |

### Examples

```bash
mkvdovi "input.mkv" --keep-source             # keep source for A/B testing
mkvdovi "input.mkv" --keep-source --verify    # recommended first run
mkvdovi "input.mkv" --content-type sport      # high-motion content
mkvdovi "input.mkv" --cm-version v29          # legacy CM v2.9
mkvdovi "input.mkv" --source-primaries 0      # force P3-D65
mkvdovi "input.mkv" --encoder videotoolbox    # fast HLG→PQ on Apple Silicon
mkvdovi inspect "input.DV.mkv"                 # inspect source RPU metadata
mkvdovi "input.DV.mkv" --mdfix                 # write input.mdfix.DV.mkv
mkvdovi "profile7-fel.mkv" --fel-crf 16        # composite FEL locally
```

### Resilience for long conversions

A 4K remux conversion moves tens of gigabytes through several passes (extract base layer →
inject RPU → mux), so on slow storage it can legitimately run for many minutes per step. To
keep long runs safe and observable:

- **Run under `tmux`/`screen`/`nohup`** so a dropped SSH or terminal session cannot kill it
  mid-conversion (`SIGHUP`). On interrupt, `mkvdovi` preserves its `mkvdovi_temp_*` directory.
- **Resume is automatic.** Re-running over the same input reuses every completed step (analysis,
  RPU, extracted base layer, …) from the leftover temp dir — it does not redo hours of work.
  Pass `--no-resume` to force a clean re-run.
- **Progress is live.** Extract/inject/mux/encode show bytes written, throughput, and ETA, and
  warn (after `--stall-timeout` seconds, default 300) if the output file stops growing — so a
  genuinely stalled tool is distinguishable from slow-but-moving I/O.

```bash
tmux new -s dv "mkvdovi 'input.mkv' --keep-source --verify"   # survive disconnects
mkvdovi "input.mkv" --no-resume        # ignore a leftover temp dir, start clean
mkvdovi "input.mkv" --stall-timeout 0  # disable the stall warning
```

---

## `verifier`

Validates a madVR `.bin` measurement file.

```bash
verifier "measurements.bin"
# or
cargo run -p verifier -- "measurements.bin"
```

Reports file format (version, flags), scene/frame stats, peak brightness and avg PQ, histogram
integrity, `target_nits` stats (if the optimizer was enabled), and FALL-header / flag coherence.

---

## Hardware acceleration

### Analyzer (decoding)

- `cuda`: attempts the `hevc_cuvid` decoder; automatic fallback to software decoding if unavailable.
- `vaapi` / `videotoolbox`: currently log and fall back to software decoding (proper device
  contexts are planned). The pipeline remains fully functional via software decoding everywhere.

### Converter (encoding via mkvdovi)

- **macOS Apple Silicon**: `--encoder videotoolbox` enables `hevc_videotoolbox` for accelerated
  HLG→PQ conversion.
- **Other platforms**: default `libx265` (software) for maximum compatibility and quality.

---

## Throughput controls & ARM optimizations

- **Frame sampling** (`--sample-rate N`): scaling and analysis are skipped for non-selected frames.
- **Downscale** (`--downscale 2|4`): speeds up analysis with minimal histogram/scene-detection impact.
- **Smart skipping**: the pipeline skips scaling/cropping for frames not selected for analysis.
- **Faster scaling**: uses `FAST_BILINEAR` when scaling is required.
- **Decoder threading**: FFmpeg multi-threading (auto thread count).
- **Build tuning**: `.cargo/config.toml` sets `-C target-cpu=native` (NEON on ARM); on Linux ARM64
  it uses the LLD linker when available.

### Oracle Cloud ARM (Ampere) notes

- Fully functional with software decoding (no CUDA on Ampere).
- Rayon-backed histogram analysis saturates available cores; pin with `--analysis-threads`.
- Use `--profile-performance` to capture decode vs. analysis throughput when validating instances.
- Recommended packages (Ubuntu 22.04/24.04 arm64):

  ```bash
  sudo apt update
  sudo apt install -y \
    build-essential pkg-config clang lld \
    libavformat-dev libavcodec-dev libavutil-dev \
    libavfilter-dev libavdevice-dev libswscale-dev
  ```

  `clang` + `lld` provide much faster linking, especially with LTO.
