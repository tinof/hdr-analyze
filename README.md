# HDR-Analyze

[![CI](https://github.com/tinof/hdr-analyze/actions/workflows/ci.yml/badge.svg)](https://github.com/tinof/hdr-analyze/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-stable-blue.svg)](https://github.com/rust-lang/rust)

Convert any HDR10, HLG, or HDR10+ video to dynamic metadata — entirely free and open-source.

HDR-Analyze reads raw 10-bit pixel data frame-by-frame, computes precise per-frame luminance
measurements, and generates dynamic metadata (`.bin`) compatible with existing open-source tools
like `dovi_tool`. The companion `mkvdovi` tool then packages the result into a final MKV with
dynamic tone-mapping metadata.

**Workflow:** `HDR10/HLG MKV → hdr_analyzer_mvp → measurements.bin → mkvdovi → Dynamic HDR MKV`

> ⚡ **CUDA-accelerated** — as far as we know, this is the only open-source HDR10 → Dolby Vision
> measurement pipeline with end-to-end GPU acceleration: NVDEC hardware decode plus a custom CUDA
> analysis kernel. **Zero-config**: `mkvdovi` auto-detects your NVIDIA GPU at startup and enables
> the CUDA path automatically. On an RTX 4070 the analysis pass runs ~12× faster than the CPU path
> (a 43-minute 4K episode measures in ~6 minutes), with bit-identical L1 output. See
> [GPU acceleration](#gpu-acceleration-cuda).

> **Renamed in v0.3.0:** the converter formerly called `mkvdolby` is now **`mkvdovi`**
> (see [docs/PROVENANCE.md](docs/PROVENANCE.md) for why). For one release, archives ship a
> transitional `mkvdolby` copy and leftover `mkvdolby_temp_*` directories still resume.

For HDR10+ inputs, `mkvdovi` extracts the source HDR10+ metadata directly and passes it to
`dovi_tool`; it does not run `hdr_analyzer_mvp` unless HDR10+ metadata extraction fails and the
workflow falls back to HDR10 analysis.

## Documentation

- **[docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md)** — complete flag reference for all three tools.
- **[docs/DOLBY_VISION.md](docs/DOLBY_VISION.md)** — current conversion paths, HDR10+ mapping, CM v4.0 metadata, and verification.
- **[docs/CM_ANALYZE_PARITY.md](docs/CM_ANALYZE_PARITY.md)** — analyzer accuracy gaps and validation design.
- **[docs/TECHNICAL_REFERENCE.md](docs/TECHNICAL_REFERENCE.md)** — analysis internals and research.
- **[docs/PROVENANCE.md](docs/PROVENANCE.md)** — clean-room statement: the public standards this is built from.
- **[ROADMAP.md](ROADMAP.md)** — canonical status and active work.
- **[CHANGELOG.md](CHANGELOG.md)** · **[CONTRIBUTING.md](CONTRIBUTING.md)**

## Workspace Members

This is a Rust workspace with three shipped binaries:

- **`hdr_analyzer_mvp`** — HDR analysis engine; processes video and writes madVR-compatible `.bin`
  measurement files plus explicit `.l1.json` measurement sidecars.
- **`mkvdovi`** — native HDR10/HDR10+/HLG → Dolby Vision Profile 8.1 (CM v4.0) conversion orchestrator.
- **`verifier`** — utility for reading, validating, and inspecting `.bin` measurement files.

## Key Features

- **Native video processing** via `ffmpeg-next` for direct, zero-copy access to high-bit-depth pixel
  data — precise per-pixel 10-bit luminance analysis instead of parsing external tool logs.
- **Accurate per-frame analysis**: MaxCLL and APL from 10-bit YUV420P10LE frames, with multi-position
  active-video crop probing to ignore black bars.
- **True L1 statistics plus v5/v6 histograms**: full-precision per-pixel Y/max-RGB means, a
  noise-rejected active-area minimum, and the compatible 256-bin SDR/HDR histogram (64 + 192).
- **Native scene detection**: histogram-distance-based cut detection with a configurable threshold.
- **Dynamic metadata optimizer**: per-frame `target_nits` from a rolling average, 99th-percentile knee
  detection, scene-aware blending/resets, and bidirectional EMA smoothing (on by default).
- **Noise robustness**: opt-in max-RGB percentile and synthetic-calibrated grain-robust peak
  estimators, per-bin EMA smoothing, optional temporal median filtering and pre-analysis denoising.
  Direct `max` remains the estimator default pending a successful real-content parity gate.
- **Native HLG workflow**: auto-detects ARIB STD-B67 and converts to PQ histograms in-memory
  (`--hlg-peak-nits`, default 1000).
- **CUDA-accelerated analysis** (optional `cuda` build feature): NVDEC hardware decode through a
  proper FFmpeg `AVHWDeviceContext` plus a single-launch NVRTC-compiled analysis kernel computing
  the v5 histogram, hue histogram, 4096-bin peak-domain PQ histogram, max-RGB peaks, and exact
  per-pixel means on the GPU. Validated bit-identical to the CPU path; automatic CPU fallback at
  every stage.
- **Dolby Vision CM v4.0** output by default (L1/L2/L6/L9/L11/L254) via `mkvdovi`.
- **Profile 7 FEL preservation**: composites BL+EL polynomial/MMR reshaping and NLQ residuals,
  then emits a Profile 8.1-compatible base layer; local and Modal encoding backends are supported.
- **Dolby Vision metadata repair**: `mkvdovi inspect` audits RPU L1 patterns, while `--mdfix`
  rebuilds Profile 7 MEL/Profile 8 metadata from fresh base-layer measurements without re-encoding
  the picture. Dolby Vision and repair inputs keep their source by default.
- **Cross-platform**: software decoding everywhere; optional CUDA attempt on NVIDIA with graceful
  fallback. ARM64-tuned (NEON, `--sample-rate`/`--downscale` give 3–4× throughput on CPU-limited systems).

See [docs/TECHNICAL_REFERENCE.md](docs/TECHNICAL_REFERENCE.md) for implementation details (PQ-domain
histogram, scene detection, crop detection) and [docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md) for
hardware-acceleration and throughput options.

## Project Status

This is a personal research project, shared as-is under the MIT license. Issues and pull requests are
welcome, but there is no support SLA. Please do not expect production-level maintenance.

## Prerequisites

- **Rust toolchain**: install from <https://rustup.rs/> (the repo pins the stable channel via
  `rust-toolchain.toml`).
- **FFmpeg development libraries** (for compiling `ffmpeg-next`):
  - macOS: `brew install ffmpeg pkg-config`
  - Ubuntu/Debian: `sudo apt install libavformat-dev libavcodec-dev libavutil-dev libavfilter-dev libavdevice-dev libswscale-dev pkg-config`
  - Windows: install FFmpeg dev libraries or use vcpkg
- **Build tools**: C compiler / build tools (Xcode CLT on macOS, `build-essential` on Linux, MSVC on Windows).
- **External tools (NOT included)** — install and place in your `PATH`:
  - [`dovi_tool`](https://github.com/quietvoid/dovi_tool/releases): required for RPU generation/injection.
    **2.3.2+ recommended** (fixes duplicated end-padding in `inject-rpu`).
  - [`hdr10plus_tool`](https://github.com/quietvoid/hdr10plus_tool/releases): required for HDR10+ inputs.
  - `mkvmerge` (from [MKVToolNix](https://mkvtoolnix.download/)): required by `mkvdovi` for final MKV packaging.

## Installation & Setup

Clone and build the workspace (compiles all three binaries):

```bash
git clone https://github.com/tinof/hdr-analyze.git
cd hdr-analyze
cargo build --release --workspace
```

On a machine with an NVIDIA GPU, build the analyzer with the CUDA backend
(the other binaries are unaffected):

```bash
cargo build --release -p hdr_analyzer_mvp --features cuda
cargo build --release -p mkvdovi -p verifier
```

Binaries land in `target/release/`:

- Analyzer: `./target/release/hdr_analyzer_mvp`
- Converter: `./target/release/mkvdovi`
- Verifier: `./target/release/verifier`

After a `git pull`, **always rebuild** so the binaries match the source:

```bash
git pull
cargo build --release --workspace   # CRITICAL
```

Optional local install from a source checkout:

```bash
install -Dm755 target/release/hdr_analyzer_mvp "$HOME/.local/bin/hdr_analyzer_mvp"
install -Dm755 target/release/mkvdovi        "$HOME/.local/bin/mkvdovi"
install -Dm755 target/release/verifier        "$HOME/.local/bin/verifier"
install -Dm755 scripts/mkvdovi_hifi_workflow.sh "$HOME/.local/bin/mkvdovi_hifi_workflow.sh"
```

`mkvdovi_hifi_workflow.sh` is a specialist comparison helper for regenerating files that already
contain Dolby Vision metadata. Use `mkvdovi` directly for HDR10+ sources.

Prebuilt binaries for **Windows**, **macOS** (Intel & Apple Silicon), and **Linux** are published on
the [Releases page](https://github.com/tinof/hdr-analyze/releases).

## Usage

The examples below cover the common paths. For every flag and default, see
**[docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md)**.

### Analyzer

```bash
# Standard analysis (optimizer on by default)
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "measurements.bin"

# v6 output with explicit target peak
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "out_v6.bin" --madvr-version 6 --target-peak-nits 1000

# Tune scene sensitivity / speed up analysis
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --scene-threshold 0.25 --downscale 2

# Full-frame analysis (disable crop detection)
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --no-crop

# GPU-accelerated analysis (requires the `cuda` build feature and an NVIDIA GPU)
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --hwaccel cuda

# Opt into grain-robust max-RGB peaks and save per-frame diagnostics
./target/release/hdr_analyzer_mvp -i "grainy.mkv" -o "out.bin" \
  --peak-estimator robust --dump-frame-stats "frame_stats.csv"
```

→ Noise-robustness, optimizer, and HLG flags: [docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md#hdr_analyzer_mvp).

### mkvdovi (conversion tool)

Converts HDR10/HDR10+/HLG/Profile 7 input to a Profile 8.1 MKV with CM v4.0 metadata.

> For non-DV conversions, `mkvdovi` deletes the source file after success unless `--keep-source` is
> passed. Dolby Vision inputs and all `--mdfix` runs keep the source as a metadata-safety default.
>
> An interrupted run (e.g. a dropped SSH session) keeps its `mkvdovi_temp_*` directory and prints a
> resume hint — just re-run the same command to **resume** from the last completed step (`--no-resume`
> forces a clean run). For long conversions, run under `tmux`/`nohup` so a disconnect can't kill them.

```bash
mkvdovi                                  # convert all .mkv files in the current directory
mkvdovi "input.mkv"                      # convert a specific file
mkvdovi "input.mkv" --keep-source --verify   # recommended first run (A/B safe, validated)
mkvdovi "input.mkv" --hwaccel none           # force the CPU pipeline (auto-detection is the default)
mkvdovi "input.mkv" --analysis-quality accurate   # auto (default) | fast | balanced | accurate
mkvdovi "input.mkv" --encoder videotoolbox        # ~10× faster HLG→PQ on Apple Silicon
mkvdovi "input.mkv" --no-resume                   # ignore a leftover temp dir, start clean
mkvdovi inspect "input.mkv"                       # full RPU metadata inspection
mkvdovi "input.DV.mkv" --mdfix                    # rebuild DV metadata; writes *.mdfix.DV.mkv
mkvdovi "profile7-fel.mkv" --fel-crf 16 --fel-preset slow
```

Profile 7 MEL takes a fast metadata-only path by default. Profile 7 FEL is composited and re-encoded
before new Profile 8.1 metadata is generated. `--mdfix` strips the old RPU from MEL/Profile 8 video,
analyzes the clean base layer, and remuxes a fresh RPU while preserving sampled L5 active-area
offsets when available. See [the FEL preservation design](docs/profile7_fel_to_profile81_preservation.md)
and [developer handoff](docs/profile7_fel_developer_handoff.md).

→ HDR10+ peak mapping, CM v4.0 metadata, and verification details:
[docs/DOLBY_VISION.md](docs/DOLBY_VISION.md). Full flag list:
[docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md#mkvdovi).

### GPU acceleration (CUDA)

**`mkvdovi` is zero-config**: on startup it probes for an NVIDIA GPU (`nvidia-smi`) and, when
found, automatically enables CUDA — NVDEC + GPU analysis in the analyzer it spawns, and NVENC for
FEL/HLG re-encodes (guarded by an ffmpeg `hevc_nvenc` capability check, with automatic libx265
fallback). When the spawned `hdr_analyzer_mvp` was built with `--features cuda` (its `--version`
reports `+cuda`), auto mode also upgrades analysis quality to `accurate` — full-resolution,
every-frame measurement that is still ~4× faster than the old CPU default. Opt out with
`--hwaccel none` or pin quality with `--analysis-quality balanced`.

For direct analyzer use, build `hdr_analyzer_mvp` with `--features cuda` and pass `--hwaccel cuda`:

- **Decode**: HEVC 4K10 frames are decoded by NVDEC via an FFmpeg CUDA `AVHWDeviceContext`
  (with `hevc_cuvid` and software fallbacks).
- **Analysis**: one CUDA kernel launch per frame computes the luminance/hue/PQ histograms,
  max-RGB peaks, and exact per-pixel means directly on full-resolution frames using a sampling
  stride — swscale downscaling is bypassed entirely, and only a few KB of results leave the GPU
  per frame.
- **Parity**: validated bit-identical (12-bit precision) L1 measurements, scene cuts, and MaxCLL
  against the CPU path. Measured on an RTX 4070: analysis throughput 17 → 213 fps (~12×).
- **Fallbacks**: no `cuda` build feature, no NVIDIA device, `--pre-denoise median3`, or
  `--peak-estimator robust` (which needs the CPU grain statistics) all fall back to the CPU path
  automatically — mid-run kernel failures do too.

The `cuda` feature needs only the NVIDIA driver and NVRTC at *runtime* (the kernel is compiled
on startup); no `nvcc` or CUDA toolchain is required at build time.

### Verifier

```bash
./target/release/verifier "measurements.bin"
```

Reports version/flags, scene & frame stats, peak brightness and avg PQ, histogram integrity, and
`target_nits` stats (if the optimizer was enabled).

## Known Limitations

- **Variable-aspect-ratio analysis uses one conservative crop.** Seven seek-based probes reject
  black/low-signal frames and commit a stable active area before analysis. When multiple aspect-ratio
  modes are observed, their union preserves all picture; scene cuts report crop changes but do not
  apply a new crop per scene. Use `--crop-probes 0` for in-stream fallback detection or `--no-crop`
  for full-frame diagnostics. L5 active-area metadata is not emitted yet.
- **HLG/VAAPI/VideoToolbox decode** currently fall back to software decoding; proper device contexts
  are planned (see [Roadmap](#roadmap)).
- **v6 per-gamut peaks** (`peak_pq_dcip3`, `peak_pq_709`) are approximated from BT.2020. These are a
  **madVR measurement-file** feature only and are **not used by the Dolby Vision conversion** (which
  uses the v5 file plus the BT.2020 peak and histogram), so the approximation does not affect DV output;
  it matters only for a standalone v6 `.bin` consumed by madVR. PQ max-RGB peak measurement is now
  implemented; accurate target-gamut transforms remain a follow-up (see [Roadmap](#roadmap)).

## Quick Start Validation

```bash
cargo build --release --workspace
./target/release/hdr_analyzer_mvp -i sample_hdr10.mkv -o measurements_v5.bin
./target/release/verifier measurements_v5.bin
```

Expected: version 5/6 as selected; flags 2 (no optimizer) or 3 (optimizer on); 256-bin histograms
summing ≈ 100; PQ values in `[0,1]`; scenes valid and within frame range.

## Roadmap

See **[ROADMAP.md](ROADMAP.md)**. Near-term work includes source-honest Dolby Vision generation,
robust L1 min/average measurements, L5 emission, numerical CI regression gates, hybrid scene
detection, and proper VAAPI/VideoToolbox device contexts.

## Contributing & Quality Gates

Contributions welcome — see **[CONTRIBUTING.md](CONTRIBUTING.md)**. Before committing, run the local
gates (also enforced in CI):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Optional pre-commit hooks (config in `.pre-commit-config.yaml`):

```bash
pipx install pre-commit            # or: pip install --user pre-commit
pre-commit install                 # fmt + clippy on commit
pre-commit install --hook-type pre-push   # quick tests on push
```

## Acknowledgements

- **quietvoid** — for `dovi_tool`, `hdr10plus_tool`, and the MIT-licensed `madvr_parse` library.
- **The Doom9 and MakeMKV forum communities** — for the collective research and documentation of
  HDR formats and Dolby Vision packaging that made an open implementation possible.
- `ffmpeg-next`, `clap`, `anyhow` and the wider Rust ecosystem.

## License

MIT License.

## What This Is NOT

- Does not include, redistribute, or reverse-engineer any Dolby Laboratories proprietary code,
  lookup tables, CM v4.0 trims, or binary blobs.
- Does not bypass, circumvent, or interfere with any DRM or content-protection mechanism.
- Not an official Dolby or HDR10+ Technologies product; no trademarks claimed.
- The analyzer outputs generic per-frame luminance data. Final packaging into a playback-compatible
  stream is done by `dovi_tool` and `mkvmerge`, which the user installs independently.

## Legal & Trademarks

This software is a research project for video analysis and is not an official product of Dolby
Laboratories.

- **Dolby Vision** is a trademark of Dolby Laboratories.
- **HDR10+** is a trademark of HDR10+ Technologies, LLC.

This project is not affiliated with, endorsed by, or sponsored by Dolby Laboratories or HDR10+
Technologies, LLC. Reference to these standards is strictly for compatibility and interoperability
purposes.
