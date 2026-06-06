# CLI Reference

Complete command-line reference for the three HDR-Analyze binaries. For a quick start, see the
[README](../README.md). For HDR10+ peak mapping and Dolby Vision metadata details, see
[DOLBY_VISION.md](DOLBY_VISION.md).

All defaults below are taken directly from `--help`; run `<binary> --help` to confirm for your build.

---

## `hdr_analyzer_mvp`

Analyzes an HDR10/HLG video and writes a madVR-compatible `.bin` measurement file.

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
| `--no-crop` | off | Disable active-area crop detection (analyze full frame). See [Known Limitations](../README.md#known-limitations) |

### Scene detection

| Flag | Default | Description |
|------|---------|-------------|
| `--scene-threshold <float>` | `0.3` | Scene-cut distance threshold |
| `--scene-metric <hist\|hybrid>` | `hist` | `hist` = histogram distance; `hybrid` = prototype histogram + flow fusion |
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
| `--peak-source <max\|histogram99\|histogram999>` | `histogram99` (balanced/aggressive), `max` (conservative) | Per-frame peak brightness source |
| `--header-peak-source <max\|histogram99\|histogram999>` | — | MaxCLL source for the header only; per-frame peaks still use `--peak-source` |
| `--hist-bin-ema-beta <0.0–1.0>` | `0.1` | EMA smoothing for histogram bins (lower = more smoothing, 0 = disabled) |
| `--hist-temporal-median <N>` | `0` | Temporal median filter window in frames (3 = good for aggressive smoothing) |
| `--pre-denoise <nlmeans\|median3\|off>` | `off` | Pre-analysis Y-plane denoising (`median3` good for grain; `nlmeans` reserved) |

- `max`: direct max from the Y-plane (most responsive to noise).
- `histogram99`: 99th percentile (recommended, reduces noise impact).
- `histogram999`: 99.9th percentile (most conservative).

### HLG

| Flag | Default | Description |
|------|---------|-------------|
| `--hlg-peak-nits <nits>` | `1000` | Peak luminance used when analyzing auto-detected HLG (ARIB STD-B67) content |

### Performance & diagnostics

| Flag | Default | Description |
|------|---------|-------------|
| `--analysis-threads <N>` | logical cores | Override Rayon worker count for histogram analysis |
| `--profile-performance` | off | Print per-stage throughput (decode vs. analysis) when finished |

### Notes for v6 output

- Per-gamut peaks (`peak_pq_dcip3`, `peak_pq_709`) are currently **approximated** from BT.2020
  (`peak_pq_2020`) using 99% and 95% factors. More exact gamut-aware computation remains planned.

### Examples

```bash
# v6 file with explicit target peak
hdr_analyzer_mvp -i "video.mkv" -o "out_v6.bin" --madvr-version 6 --target-peak-nits 1000

# Aggressive smoothing for very noisy / grainy content
hdr_analyzer_mvp -i "grainy.mkv" -o "out.bin" \
  --hist-bin-ema-beta 0.05 --hist-temporal-median 3 --pre-denoise median3

# Disable histogram smoothing for clean content
hdr_analyzer_mvp -i "clean.mkv" -o "out.bin" --hist-bin-ema-beta 0

# Conservative profile with direct max (most responsive)
hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --optimizer-profile conservative --peak-source max

# Native HLG, override assumed peak
hdr_analyzer_mvp -i "hlg.mkv" -o "out.bin" --hlg-peak-nits 1200

# Disable crop detection (full-frame diagnostics)
hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --no-crop

# Via cargo
cargo run -p hdr_analyzer_mvp --release -- -i "video.mkv" -o "out.bin" --downscale 2
```

---

## `mkvdolby`

Orchestrates the full HDR10/HDR10+/HLG → Dolby Vision Profile 8.1 (CM v4.0) conversion. Internally
calls `dovi_tool`, `mkvmerge`, and (for HDR10+) `hdr10plus_tool`; these must be installed separately
(see [README Prerequisites](../README.md#prerequisites)).

```bash
mkvdolby                 # process all .mkv files recursively from the current directory
mkvdolby "input.mkv"     # process a specific file
```

### General

| Flag | Default | Description |
|------|---------|-------------|
| `[INPUT]...` | cwd `*.mkv` | One or more input files; recurses cwd if omitted |
| `--keep-source` | off | Keep the source file (by default it is **deleted** after success) |
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

### Examples

```bash
mkvdolby "input.mkv" --keep-source             # keep source for A/B testing
mkvdolby "input.mkv" --keep-source --verify    # recommended first run
mkvdolby "input.mkv" --content-type sport      # high-motion content
mkvdolby "input.mkv" --cm-version v29          # legacy CM v2.9
mkvdolby "input.mkv" --source-primaries 0      # force P3-D65
mkvdolby "input.mkv" --encoder videotoolbox    # fast HLG→PQ on Apple Silicon
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

### Converter (encoding via mkvdolby)

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
