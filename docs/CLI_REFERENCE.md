# CLI Reference

Complete command-line reference for the three HDR-Analyze binaries. For a quick start, see the
[README](../README.md). For HDR10+ peak mapping and Dolby Vision metadata details, see
[FORMAT_COMPATIBILITY.md](FORMAT_COMPATIBILITY.md).

All defaults below are taken directly from `--help`; run `<binary> --help` to confirm for your build.

---

## `hdr_analyzer_mvp`

Analyzes an HDR10/HLG video and writes a madVR-compatible `.bin` measurement file plus an
analyzer-owned `<output>.l1.json` sidecar containing explicit full-precision-derived L1 statistics and
provenance (sidecar version 5: analyzer version, input identity, sampling settings, full-resolution
crop, `analysis.luminance_mapping`, and the stream frame count with the leading pictures no decoder
outputs, `source.stream_frames` / `source.leading_skipped_frames`). Inputs tagged with a non-HDR
transfer are refused.

```bash
hdr_analyzer_mvp -i "video.mkv" -o "measurements.bin"
# positional input also works; output auto-generated from input name if -o omitted:
hdr_analyzer_mvp "video.mkv"
```

### Core options

| Flag | Default | Description |
|------|---------|-------------|
| `-i, --input <PATH>` | none | Input video file (flag-based alternative to the positional arg) |
| `-o, --output <PATH>` | auto | Output `.bin` path; auto-generated from input name if omitted |
| `--madvr-version <5\|6>` | `5` | madVR measurement file version to write |
| `--hwaccel <TYPE>` | none | GPU hint: `cuda`, `vaapi`, `videotoolbox` (see [Hardware acceleration](#hardware-acceleration)) |
| `--downscale <1\|2\|4>` | `1` | Downscale internal analysis resolution for speed (1=full, 2=half, 4=quarter) |
| `--sample-rate <N>` | `1` | Analyze every Nth frame. Skipped frames inherit the previous frame's measurements. High performance impact |
| `--crop-probes <N>` | `7` | Seek-based active-area probes across the middle 70% of the input; `0` uses hardened in-stream fallback detection |
| `--no-crop` | off | Disable crop probing/detection and analyze the full frame |

### Scene detection

| Flag | Default | Description |
|------|---------|-------------|
| `--scene-threshold <float>` | `3.0` | Smallest histogram distance (0–200) that can be a scene cut. A cut must also stand out from the local frame-to-frame level, so grainy sources do not need a higher value |
| `--scene-metric <hist\|hybrid>` | `hist` | `hist` = histogram distance; `hybrid` is a prototype that currently falls back to the same histogram metric |
| `--min-scene-length <frames>` | `12` | Minimum scene length. Of two cuts closer than N frames the stronger one is kept |
| `--scene-smoothing <frames>` | – | Ignored (hidden). Accepted so older command lines keep working |

### Optimizer

| Flag | Default | Description |
|------|---------|-------------|
| `--disable-optimizer` | off | Disable dynamic `target_nits` generation (enabled by default) |
| `--optimizer-profile <conservative\|balanced\|aggressive>` | `balanced` | Optimizer behavior preset |
| `--target-peak-nits <nits>` | computed MaxCLL | Override `header.target_peak_nits` (v6 only) |
| `--target-smoother <off\|ema>` | `ema` | `target_nits` smoother type |
| `--smoother-bidirectional` | on (always) | Forward+backward EMA smoothing when `--target-smoother ema`. The flag defaults to true and has no off form, so it currently cannot be disabled |
| `--smoother-alpha <0.0-1.0>` | `0.2` | EMA alpha for `target_nits` smoothing (lower = more smoothing) |

### Noise robustness

| Flag | Default | Description |
|------|---------|-------------|
| `--peak-domain <max-rgb\|luma>` | `max-rgb` | Domain used for direct peak measurement. For HLG, `max-rgb` uses the full Dolby Vision 8.4 decode and `luma` the 8.4 luma curve alone |
| `--peak-source <max\|histogram99\|histogram999>` | `max` in max-RGB domain; in luma, `histogram99` (balanced/aggressive) or `max` (conservative) | Per-frame peak brightness source |
| `--peak-estimator <max\|percentile\|robust>` | `max` | Estimator applied in the direct peak domain: raw maximum, fine-histogram percentile, or the raw maximum less the part of the upper tail that the measured grain explains |
| `--peak-percentile <0-100>` | `99.99` | Fine 4096-bin percentile used by `--peak-estimator percentile` |
| `--header-peak-source <max\|histogram99\|histogram999>` | none | MaxCLL source for the header only; per-frame peaks still use `--peak-source` |
| `--hist-bin-ema-beta <0.0-1.0>` | `0.1` | EMA smoothing for histogram bins and the `.bin` frame average (lower = more smoothing, 0 = disabled). Does not affect the L1 sidecar averages. |
| `--hist-temporal-median <N>` | `0` | Temporal median filter window in frames (3 = good for aggressive smoothing) |
| `--pre-denoise <nlmeans\|median3\|off>` | `off` | Pre-analysis Y-plane denoising (`median3` good for grain; `nlmeans` reserved) |
| `--min-percentile <0-100>` | `0.1` | Lower percentile used for the noise-rejected active-area minimum, in percent; `0` selects the absolute minimum |

- `max`: direct max from `--peak-domain` (most responsive to noise). For PQ, `max-rgb` decodes
  limited-range BT.2020 NCL and takes the maximum R′/G′/B′ PQ signal; `luma` retains the legacy Y′ peak.
  For HLG, `max-rgb` takes the maximum R′/G′/B′ of the 8.4 decode (see [HLG](#hlg)).
- `histogram99`: 99th percentile (recommended, reduces noise impact).
- `histogram999`: 99.9th percentile (most conservative).

`--peak-source` selects the existing madVR/Y-histogram path; `--peak-estimator` controls how the
direct max-RGB or luma peak itself is measured. Robust mode measures PQ-domain grain (sigma) from
cross-chroma-quad differences. It then describes the pixels at the top of the frame's PQ histogram
by a centre and a width, and takes the grain variance out of that width: a top as narrow as the
grain reads its centre, and a top much wider than the grain is nearly kept. The correction is
limited to 5.2 sigma. A flat top (several pixels on the brightest value) is kept. A small group
of pixels above a clear gap is kept exactly when it has a single value, and reads its mean when
it is no wider than sigma ([TECHNICAL_REFERENCE.md §2.4](TECHNICAL_REFERENCE.md)).

Robust mode stays opt-in, and its limits are measured, not solved:

- Highlights. Flat highlights of 2x2 pixels and more and one- to three-frame flashes far above
  the picture are kept. A flat highlight of fewer than about 5 pixels (about 17 on a 10-bit code
  grid at sigma 19) is kept only when a gap separates it from the grain; one to three pixels,
  or a few pixels that carry grain, within a few sigma of the highest grain pixel are corrected
  as grain and read up to 5.2 sigma low.
- Grain. On three clean/grain pairs the per-frame bias against the clean twin falls by 77%,
  59% and 39% (+102.7 → +23.4, +41.0 → +17.0, +83.3 → +50.5 codes), and the mean absolute
  error by 44%, 55% and 17%; up to 27% of the frames then read more than 10 codes below the
  clean twin.
- Clean content is lowered by 3 to 21 codes per frame on average (single frames by up to 332),
  because the sigma measurement cannot tell sensor noise and fine texture from grain.
- Frame-to-frame variation inside shots is about 10% higher than with `max`.
- Use it with `--downscale 1`; with 2 or 4 the analyzer prints a warning.

Histogram percentiles and APL remain Y-based in both domains, preserving madVR histogram semantics.
An explicit histogram peak source therefore opts out of max-RGB peak selection.

The analyzer computes the average directly from active-area pixels rather than reconstructing it
from 256 histogram bins. The JSON sidecar records per-frame robust minimum, Y-luma mean, and
max-RGB mean as 12-bit PQ codes, plus scene aggregates and the crop/denoise settings. Neither
average domain nor the spatially noise-rejected minimum is temporally smoothed in the sidecar; the
configured EMA/temporal smoothing applies to the histograms and the `.bin` frame average only. `mkvdovi` passes each scene's minimum from
the sidecar to `dovi_tool generate` as the L1 `min_pq`; the generator writes at most 12 codes, so a
higher measured minimum is stored as 12. The per-frame values stay in the sidecar for
validation.

### HLG

HLG (ARIB STD-B67) input is detected from the stream and needs no flag. It is mapped to PQ through
the Dolby Vision Profile 8.4 decode of the composer selected with `--hlg-composer`, clamped to the
8.4 source range (PQ codes 62–3079, about 0–1000 nits), on both the CPU and CUDA paths
(bit-identical). The composer must be the one the RPU carries; `mkvdovi` passes the matching value.
Luma statistics use the 8.4 luma reshaping curve. Max-RGB (the default peak domain, and the max-RGB
mean) reconstructs each pixel through the luma curve, the two chroma MMR curves and the RPU's
YCbCr-to-RGB matrix, then takes max(R′, G′, B′). With the `preset` composer, neutral
content reads about 2% higher in max-RGB than in luma, because the preset's chroma curves tint
neutrals slightly blue. The `bt2100` composer keeps neutrals neutral, so luma and max-RGB agree on
grey. `--peak-domain luma` restores the luma-only peak. The sidecar records the composer as
`analysis.luminance_mapping`: `"dovi84-v2"` (preset) or `"dovi84-bt2100-v1"` (bt2100). `mkvdovi`
re-analyzes a sidecar of the other composer and older luma-only `"dovi84-v1"` sidecars. The former
`--hlg-peak-nits` flag was removed: the 8.4 RPU fixes the mapping.

Samples are assumed to be limited range with BT.2020 non-constant-luminance coefficients. A stream
tagged full range or with another matrix (BT.2020 constant luminance included) is analyzed with a
warning; `mkvdovi` refuses such HLG input (see [HLG input](#hlg-input)).

| Flag | Default | Description |
|------|---------|-------------|
| `--transfer <auto\|pq\|hlg>` | `auto` | Transfer to analyze with. `auto` uses the stream tag, or the first decoded frame's tag when that is PQ/HLG (catches HLG signalled via the alternative-transfer SEI). `hlg`/`pq` force it; `mkvdovi` passes `--transfer hlg` for inputs it classified as HLG, because some FFmpeg versions drop an HLG tag held only in the MKV colour element |
| `--hlg-composer <bt2100\|preset>` | `bt2100` | Profile 8.4 composer HLG is measured through. `preset`: the `dolby_vision` crate's `Profile84` preset, which `dovi_tool generate` writes. `bt2100` (default): fitted to the BT.2100 / BT.2408 1000-nit HLG-to-PQ conversion, neutrals kept neutral ([HLG_COMPOSER.md](HLG_COMPOSER.md)). Accepted and ignored for PQ input |

### Performance & diagnostics

| Flag | Default | Description |
|------|---------|-------------|
| `--analysis-threads <N>` | logical cores | Override Rayon worker count for histogram analysis |
| `--profile-performance` | off | Print per-stage throughput (decode vs. analysis) when finished |
| `--dump-frame-stats <PATH>` | none | Write sample-rate-aligned CSV with selected/raw/percentile/robust peaks, sigma, correction, effective-tail count, and the scene-detection series (`scene_diff`, `scene_score`, `scene_baseline`, `scene_start`) |

### Notes for v6 output

- v6 adds per-gamut peaks (`peak_pq_dcip3`, `peak_pq_709`) and a `target_peak_nits` header on top of v5.
- The per-gamut peaks are currently **approximated** from BT.2020 (`peak_pq_2020`) using 99% and 95%
  factors. They are a **madVR measurement-file** feature and are **not consumed by the Dolby Vision
  conversion**: `mkvdovi` writes v5, and `dovi_tool` builds L1 from the BT.2020 peak + histogram. The
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

# HLG source: max-RGB of the Dolby Vision 8.4 decode (no extra flag)
hdr_analyzer_mvp -i "hlg.mkv" -o "out.bin"

# Disable seek-based probing and use the first usable in-stream crop
hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --crop-probes 0

# Disable crop detection entirely (full-frame diagnostics)
hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --no-crop

# Via cargo
cargo run -p hdr_analyzer_mvp --release -- -i "video.mkv" -o "out.bin" --downscale 2
```

---

## `mkvdovi`

Orchestrates the full HDR10/HDR10+/Profile 7 MEL → Dolby Vision Profile 8.1 and HLG → Profile 8.4 (CM v4.0) conversion. The video
stream is always copied; `mkvdovi` never encodes video. Profile 7 FEL input is refused (see [FEL_PLAN.md](FEL_PLAN.md)). Internally
calls `ffmpeg`, `mkvmerge`, `dovi_tool`, `mediainfo` or `ffprobe`, and (for HDR10+) `hdr10plus_tool`; these must be installed separately
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
| `--mdfix` | off | Rebuild Profile 7 MEL/Profile 8.1 RPU metadata from fresh base-layer measurements; writes `*.mdfix.DV.mkv`. Profile 8.4 (HLG base layer) and Profile 7 FEL inputs are refused |
| `--no-resume` | off | Discard a leftover temp directory and re-run from scratch (by default an interrupted run **resumes**, reusing completed steps, when the temp dir was created for the same input, mkvdovi version, and settings; a temp dir left by an older mkvdovi, with no fingerprint, resumes with a warning, or is discarded when a non-preset `--hlg-composer` is selected) |
| `--stall-timeout <SECS>` | `300` | Warn if the current step's output file stops growing for this long (`0` disables). This tells a stalled tool apart from merely slow storage |
| `--verify` | off | After muxing, validate the result: RPU structure, and RPU frame count against the muxed video track and the L1 sidecar. For HLG output it fails when the measurements are missing or the sidecar does not load, and checks that every RPU frame carries the composer the sidecar names (see [FORMAT_COMPATIBILITY.md](FORMAT_COMPATIBILITY.md#post-mux-verification)) |
| `-v, --verbose` | off | Show raw command output (debugging) |
| `-q, --quiet` | off | Minimal output (errors and final result only) |
| `--drop-chapters` | off | Drop chapters in the output (kept by default) |
| `--drop-tags` | off | Drop global tags in the output (kept by default) |

### Analysis & quality

| Flag | Default | Description |
|------|---------|-------------|
| `--analysis-quality <auto\|fast\|balanced\|accurate>` | `auto` | Analyzer sampling: `auto` = `accurate` when GPU analysis is available, else `balanced`; fast = half-res/every 3rd frame, balanced = half-res/every frame, accurate = full-res/every frame |
| `--optimizer-profile <conservative\|balanced\|aggressive>` | `conservative` | Optimizer profile passed to the `hdr_analyzer_mvp` pass (affects the madVR `.bin`, not the RPU's L1 unless `--legacy-madvr-l1` is set) |
| `--legacy-madvr-l1` | off | Compatibility escape: build L1 from the madVR `.bin` with `dovi_tool --use-custom-targets` (optimizer targets as L1 max, placeholder avg) instead of the measured sidecar. Existing measurements are then reused without sidecar validation. Not available for HLG input (the file is refused) |
| `--hlg-composer <bt2100\|preset>` | `bt2100` | Profile 8.4 composer written into the RPU of HLG inputs, and measured through. `bt2100` (default) is fitted to the BT.2100 / BT.2408 1000-nit conversion: mkvdovi replaces the composer on every RPU frame. `preset` keeps the RPU exactly as `dovi_tool generate` writes it; use it if a display renders bt2100 output wrongly. Whether playback devices apply a composer other than the preset is not yet confirmed by a playback test. When analysis runs, mkvdovi passes the composer to the analyzer; an analyzer whose `--help` does not list `--hlg-composer` is accepted only with `preset`. Ignored for non-HLG input. See [HLG_COMPOSER.md](HLG_COMPOSER.md) |
| `--hwaccel <auto\|none\|cuda>` | `auto` | Hardware acceleration: `auto` detects an NVIDIA GPU at startup (CUDA when found, CPU otherwise); it selects GPU decode and analysis in the spawned analyzer (HDR10, HLG and `--mdfix`) and nothing else |
| `--dovi-input <auto\|raw\|mkv>` | `auto` | Feed mode to `dovi_tool` for remove/convert/demux: `auto` passes the MKV directly when `dovi_tool` is 2.3.4+ (skipping a full-size HEVC extraction), falling back to extraction on failure; `raw` forces extraction; `mkv` forces direct MKV input |

### HDR10+ peak mapping

| Flag | Default | Description |
|------|---------|-------------|
| `--peak-source <histogram\|histogram99\|max-scl\|max-scl-luminance>` | `histogram` | Maps to `dovi_tool generate --hdr10plus-peak-source` |
| `-b, --boost` | off | Brighter preset; switches another selected `--peak-source` to `histogram99` |
| `--boost-experimental` | off | Asks `hdr_analyzer_mvp` to use a more aggressive optimizer profile |

See [FORMAT_COMPATIBILITY.md](FORMAT_COMPATIBILITY.md#hdr10-peak-mapping) for guidance on each source.

### Dolby Vision metadata (CM v4.0)

| Flag | Default | Description |
|------|---------|-------------|
| `--cm-version <v29\|v40>` | `v40` | Content Mapping version |
| `--content-type <default\|movies\|game\|sport\|user-generated-content>` | `movies` | L11 content type (`cinema`/`film` alias `movies`, `gaming` aliases `game`) |
| `--reference-mode <true\|false>` | `false` | L11 reference mode (critical/studio viewing) |
| `--source-primaries <0\|1\|2>` | auto | L9 source primaries: `0=P3-D65, 1=BT.709, 2=BT.2020` (auto-detected from MediaInfo if unset) |
| `--trim-targets <csv>` | `100,600,1000` | Nits values for the DV L2 trim pass (neutral compatibility trims, not a panel calibration) |

### HLG input

HLG becomes Profile 8.4 with the base layer copied unchanged, and the 8.4 RPU describes only
limited-range BT.2020 non-constant-luminance video with BT.2020 primaries. An HLG file tagged full
range, with another matrix (BT.709, BT.2020 constant luminance) or with other primaries is refused
before any temporary work, whatever the composer. The check reads MediaInfo (`colour_range`,
`matrix_coefficients`, `colour_primaries` and their `_Original` variants) and ffprobe
(`color_range`, `color_space`, `color_primaries`); any source with a non-conforming value refuses
the file, and the error lists every source and value. Untagged fields are accepted as limited range,
BT.2020 NCL and BT.2020 primaries, with a warning. The source is kept and a multi-file run continues
with the next file.

### Profile 7 FEL input

A Profile 7 FEL file is refused, with or without `--mdfix`. The file fails before any temporary
work with this error, the source is kept, and a multi-file run continues with the next file:

```text
Profile 7 FEL input is not supported: the BL+EL compositor was removed because it did not match
the Dolby Vision reconstruction specification. A no-re-encode FEL design is planned (docs/FEL_PLAN.md).
```

The `composite-pipe` subcommand and the flags `--fel-crf`, `--fel-preset`, `--fel-encoder`,
`--fel-nvenc-preset` and `--encoder` no longer exist. `mkvdovi inspect` still reports whether a
Profile 7 file is MEL or FEL. The plan for FEL input is in [FEL_PLAN.md](FEL_PLAN.md).

### Subcommands

| Command | Description |
|---------|-------------|
| `mkvdovi inspect <INPUT>` | Extract the complete RPU and report suspicious/static/clipped L1 patterns |

### Examples

```bash
mkvdovi "input.mkv" --keep-source             # keep source for A/B testing
mkvdovi "input.mkv" --keep-source --verify    # recommended first run
mkvdovi "input.mkv" --content-type sport      # high-motion content
mkvdovi "input.mkv" --cm-version v29          # legacy CM v2.9
mkvdovi "input.mkv" --source-primaries 0      # force P3-D65
mkvdovi inspect "input.DV.mkv"                 # inspect source RPU metadata
mkvdovi "input.DV.mkv" --mdfix                 # write input.mdfix.DV.mkv
```

### Resilience for long conversions

A 4K remux conversion moves tens of gigabytes through several passes (extract base layer →
inject RPU → mux), so on slow storage it can legitimately run for many minutes per step. To
keep long runs safe and observable:

- **Run under `tmux`/`screen`/`nohup`** so a dropped SSH or terminal session cannot kill it
  mid-conversion (`SIGHUP`). On interrupt, `mkvdovi` preserves its `mkvdovi_temp_*` directory.
- **Resume is automatic.** Re-running over the same input reuses every completed step (analysis,
  RPU, extracted base layer, …) from the leftover temp dir, so it does not redo hours of work.
  A temp dir created for a different input or settings is discarded; one left by an older
  mkvdovi (no fingerprint) resumes with a warning. Pass `--no-resume` to force a clean re-run.
- **Progress is live.** Extract/inject/mux show bytes written, throughput, and ETA, and
  warn (after `--stall-timeout` seconds, default 300) if the output file stops growing, so a
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

### mkvdovi auto-detection (default)

`--hwaccel auto` (the default) resolves once at startup:

- NVIDIA GPU present (`nvidia-smi -L` succeeds, including the WSL2 location
  `/usr/lib/wsl/lib/nvidia-smi`) → behaves as `--hwaccel cuda`.
- Otherwise → behaves as `--hwaccel none` (today's CPU pipeline).
- `--analysis-quality auto` (the default) additionally resolves to `accurate` only when CUDA is
  active **and** the spawned `hdr_analyzer_mvp` advertises `+cuda` in `--version`; otherwise
  `balanced`. This avoids accidentally running full-res CPU analysis with a non-CUDA analyzer build.

The resolved choice is printed at startup. Explicit `--hwaccel none|cuda` values skip detection
entirely.

### Analyzer (decoding + analysis)

- `cuda`: with a `--features cuda` build, enables the full GPU path: NVDEC decode through an
  FFmpeg CUDA `AVHWDeviceContext` (falling back to `hevc_cuvid`, then software) plus an
  NVRTC-compiled CUDA kernel that computes the histograms, max-RGB peaks, and per-pixel means on
  full-resolution frames with a sampling stride (`--downscale` maps to the stride). P010 frames from
  the FFmpeg CUDA device decoder are analyzed in GPU memory without a host round trip; `hevc_cuvid`
  frames, 8-bit and 12-bit surfaces are downloaded to host memory first. Bit-identical L1 output vs.
  the CPU path. On an RTX 4070, 4K sources analyze at about 325 fps (HDR10) and 490 fps (HLG) end to end;
  see [PERFORMANCE.md](PERFORMANCE.md) and [CUDA_PIPELINE.md](CUDA_PIPELINE.md).
  `--pre-denoise median3` is CPU-only and disables the GPU kernel. `--peak-estimator robust` runs
  on the GPU with the same output as on the CPU at `--downscale 1`. Use it only at
  `--downscale 1`: with a sampling stride the compared pixels are 4 or 8 pixels apart, picture
  detail counts as grain, and the correction grows (median sigma 15, 26 and 43 codes at
  stride 1, 2 and 4 on one grainy cut).
  A failed CUDA call while decoding on NVDEC stops the run with an error, because the decoder
  shares the analyzer's CUDA context (rerun with `--hwaccel none`). Frames rejected before any
  CUDA work, and GPU failures with software-decoded frames, fall back to host-memory or CPU
  analysis mid-run. Setting `HDR_ANALYZER_CUDA_HOST_FRAMES` (to any
  value) forces the download path, for parity checks. Without the `cuda` build feature,
  `--hwaccel cuda` still attempts hardware decode and otherwise behaves as before.
  Release archives are built without the `cuda` feature, so their analyzer uses the CPU path;
  GPU analysis needs a source build (see [INSTALLATION.md](INSTALLATION.md#cuda-analysis-build)).
- `vaapi` / `videotoolbox`: currently log and fall back to software decoding (proper device
  contexts are planned). The pipeline remains fully functional via software decoding everywhere.

### Converter (mkvdovi)

`mkvdovi` does not encode video, so it has no encoder acceleration. `--hwaccel` only selects GPU
decode and analysis in the analyzer it spawns.

---

## Throughput controls & ARM optimizations

- **Frame sampling** (`--sample-rate N`): scaling and analysis are skipped for non-selected frames.
- **Downscale** (`--downscale 2|4`): speeds up analysis with minimal histogram/scene-detection impact.
- **Smart skipping**: the pipeline skips scaling/cropping for frames not selected for analysis.
- **Faster scaling**: uses `FAST_BILINEAR` when scaling is required.
- **Decoder threading**: FFmpeg multi-threading (auto thread count).
- **Build tuning**: `.cargo/config.toml` sets `-C target-cpu=native` (NEON on ARM); on Linux ARM64
  it links with `clang` and `lld`.

### Oracle Cloud ARM (Ampere) notes

- Fully functional with software decoding (no CUDA on Ampere).
- Rayon-backed histogram analysis saturates available cores; pin with `--analysis-threads`.
- Use `--profile-performance` to capture decode vs. analysis throughput when validating instances.
- For build dependencies, including `clang` and `lld` for the ARM64 linker, see
  [INSTALLATION.md](INSTALLATION.md#linux-arm64).
