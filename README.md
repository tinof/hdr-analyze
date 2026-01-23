# HDR-Analyze: Dynamic HDR Metadata Generator

[![CI Build](https://github.com/tinof/hdr-analyze/actions/workflows/rust.yml/badge.svg)](https://github.com/tinof/hdr-analyze/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust Version](https://img.shields.io/badge/rust-1.70%2B-blue.svg)](https://github.com/rust-lang/rust)

A powerful, open-source workspace containing tools for analyzing HDR10 and HLG video files to generate dynamic metadata for Dolby Vision conversion.

This workspace implements advanced, research-backed algorithms to analyze video on a per-frame and per-scene basis, creating measurement files that can be used by tools like `dovi_tool` to produce high-quality Dolby Vision Profile 8.1 content from standard HDR10 or HLG sources.

## Project Structure

This is a Rust workspace containing two main components:

```
hdr_project/
├── Cargo.toml              # Workspace configuration
├── README.md               # This file
├── LICENSE                 # MIT License
├── CHANGELOG.md            # Version history
├── CONTRIBUTING.md         # Contribution guidelines
├── hdr_analyzer_mvp/       # Main HDR analysis tool
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs         # Application entry point
│       ├── pipeline.rs     # Main orchestration logic
│       ├── analysis/       # Core analysis modules
│       │   ├── mod.rs
│       │   ├── frame.rs
│       │   ├── scene.rs
│       │   └── histogram.rs
│       ├── optimizer.rs    # Dynamic target nits generation
│       ├── ffmpeg_io.rs    # FFmpeg initialization and I/O
│       ├── writer.rs       # .bin file writing
│       ├── cli.rs          # CLI definition
│       └── crop.rs         # Active area detection
└── verifier/               # Measurement file verification tool
    ├── Cargo.toml
    └── src/
        └── main.rs
```

### Workspace Members

- `hdr_analyzer_mvp`: Main HDR analysis application that processes video files and generates madVR-compatible measurement files
- `verifier`: Utility tool for reading, validating, and inspecting madVR measurement files

## Key Features

- Native video processing: Built with `ffmpeg-next` for direct access to high-bit-depth video data, enabling precise 10-bit luminance analysis.
- Accurate per-frame analysis: Peak Brightness (MaxCLL) and Average Picture Level (APL) computed from 10-bit YUV420P10LE frames, with active-video crop detection to ignore black bars.
- v5-compatible luminance histogram: 256-bin luminance histogram with SDR/HDR split (64 + 192) and mid-bin averaging, consistent with madVR v5 layout.
- Native scene detection: Real-time histogram-distance based scene cut detection with configurable threshold.
- Dynamic metadata optimizer (optional): Per-frame target nits generation using a 240-frame rolling average, 99th percentile highlight knee detection, and scene-aware heuristics.
  - Scene-aware APL blending and per-scene smoothing resets to avoid cross-scene lag.
  - Per-frame delta limiting for temporal stability of `target_nits`.
  - Bidirectional EMA smoothing enabled by default (configurable via `--target-smoother` / `--smoother-*`).
- Noise robustness: Advanced histogram smoothing and robust peak detection for grainy content.
  - Histogram-based peak selection (99th/99.9th percentile) instead of direct max to reduce noise impact.
  - Per-bin EMA smoothing with scene-aware resets to stabilize APL measurements.
  - Optional temporal median filtering and pre-analysis denoising for extremely noisy content.
- Native HLG workflow: Automatically detects ARIB STD-B67 transfers and converts to PQ histograms in-memory using the configurable `--hlg-peak-nits` (default 1000 nits).
- Professional output: Writes madVR-compatible `.bin` measurement files through the `madvr_parse` library.
- Cross-platform: CPU decoding on all platforms with optional CUDA attempt on NVIDIA (graceful fallback to software decoding everywhere else).
- Performance Tuned: Optimized for software decoding on ARM64. Features smart frame sampling (`--sample-rate`) and analysis downscaling (`--downscale`) to boost throughput by 3-4x on CPU-limited systems.
- Visual Progress Tracking: Real-time progress bar with ETA, frame count, and processing speed.

Unlike wrapper tools that rely on parsing text logs from external binaries, HDR-Analyze inspects raw 10-bit pixel data directly in application memory. This zero-copy approach eliminates inter-process overhead and enables precise, per-pixel luminance operations that would be impossible with text-based analysis.

### 10-bit luminance and PQ domain

- Frames are converted/scaled to YUV420P10LE for consistent 10-bit luminance (Y-plane) access.
- Histogram binning follows madVR v5 layout:
  - SDR portion (bins 0–63) and HDR portion (bins 64–255)
  - Mid-bin center values used for weighted average (APL) estimation
  - Heuristic black-bar filtering on bin 0
- Implementation detail:
  - The active per-frame analysis path normalizes HDR10 limited-range codes (nominal 64–940) to [0,1] before mapping into the v5 histogram bins (this aligns well with practical HDR10 limited-range content).
  - The average PQ is computed using the same mid-bin approach as consumers of v5 measurements. This ensures consistent values for downstream tooling.

### Scene detection

- Histogram distance metric (chi-squared-like, symmetric form) with a small epsilon for stability.
- Default threshold: 0.3 (configurable via `--scene-threshold`).
- Cut detection is performed during frame analysis and converted to scene ranges after processing.

### Hardware acceleration support

### Analyzer (Decoding)


- CUDA: Attempts the `hevc_cuvid` decoder when `--hwaccel cuda` is specified (automatic fallback to software decoding if unavailable).
- VAAPI / VideoToolbox: Currently log and use software decoding paths; proper device contexts are planned. The pipeline remains fully functional via software decoding.

### Converter (Encoding via mkvdolby)
- **macOS Apple Silicon**: Supports `hevc_videotoolbox` for accelerated HLG-to-PQ conversion. Use `--encoder videotoolbox` to enable.
- **Other Platforms**: Defaults to `libx265` (software) for maximum compatibility and quality.


### Throughput controls and ARM optimizations

- **Frame Sampling**: Use `--sample-rate 3` (analyze 1 in 3 frames) to significantly reduce CPU load. Scaling and complex analysis are skipped for ignored frames.
- **Downscale analysis**: Use `--downscale 2` (half) or `--downscale 4` (quarter) to speed up analysis with minimal impact on histogram/scene detection quality.
- **Smart Skipping**: The pipeline intelligently skips scaling and cropping operations for frames that aren't selected for analysis.
- **Faster scaling**: When scaling is needed, uses `FAST_BILINEAR` for analysis.
- **Visual Feedback**: Progress bar and ETA calculation help track long-running jobs.
- **Decoder threading**: Enables FFmpeg multi-threading (auto thread count) for better CPU utilization.
- **Rust build tuning**: Workspace includes `.cargo/config.toml` with `-C target-cpu=native` to enable host-specific optimizations (NEON on ARM).

## Prerequisites

- Rust toolchain: Install from https://rustup.rs/
- FFmpeg development libraries: Required for compiling `ffmpeg-next`
  - macOS: `brew install ffmpeg pkg-config`
  - Ubuntu/Debian: `sudo apt install libavformat-dev libavcodec-dev libavutil-dev libavfilter-dev libavdevice-dev libswscale-dev pkg-config`
  - Windows: Install FFmpeg dev libraries or use vcpkg
- Build tools: C compiler and build tools (Xcode CLT on macOS, build-essential on Linux, MSVC on Windows)
- External Tools (NOT included):
  - `dovi_tool`: Required for final RPU generation. Download from [quietvoid/dovi_tool](https://github.com/quietvoid/dovi_tool/releases) and place in your PATH.
  - `hdr10plus_tool`: Required for HDR10+ analysis. Download from [quietvoid/hdr10plus_tool](https://github.com/quietvoid/hdr10plus_tool/releases) and place in your PATH.

## Installation & Setup

### 1. Build the Rust Tools
Clone the repository and build the workspace. This compiles `hdr_analyzer_mvp` (the core analysis engine) and `verifier`.

```bash
git clone https://github.com/tinof/hdr-analyze.git
cd hdr-analyze
cargo build --release --workspace
```

The compiled binaries will be located in `target/release/`.

### 2. mkvdolby (Full Conversion Tool)
`mkvdolby` is now a native Rust binary included in the workspace. It orchestrates the entire conversion process without requiring Python or pipx.

To use it, simply build the workspace (as above). The binary will be at:
`./target/release/mkvdolby`

You can add it to your PATH or run it directly.

### 3. Updating (After `git pull`)
When you pull new changes from the repository, you **must** rebuild the Rust binaries to ensure they match the updated source code.

```bash
# 1. Pull changes
git pull

# 2. Rebuild Rust binaries (CRITICAL)
cargo build --release --workspace
```

### Executables Locations
- Analyzer: `./target/release/hdr_analyzer_mvp`
- Verifier: `./target/release/verifier`
- Converter: `./target/release/mkvdolby`

## Usage

### Analyzer

Standard analysis (optimized by default):
```bash
./target/release/hdr_analyzer_mvp -i "path/to/video.mkv" -o "measurements.bin"
```

Disable optimizer:
```bash
./target/release/hdr_analyzer_mvp -i "path/to/video.mkv" -o "measurements_noopt.bin" --disable-optimizer
```

Version selection (v5 default; v6 for broader compatibility):
```bash
# Write v6 file, set target_peak_nits to 1000 (if omitted, defaults to computed MaxCLL)
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "measurements_v6.bin" --madvr-version 6 --target-peak-nits 1000
```

Scene detection sensitivity and controls:
```bash
# Increase/decrease sensitivity (default 0.3)
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --scene-threshold 0.25

# Enforce minimum scene length (default 24 frames) and optional smoothing (rolling mean over the diff signal)
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --min-scene-length 24 --scene-smoothing 5

# Disable crop detection to analyze full frame (diagnostics/validation)
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --no-crop
```

Hardware acceleration (attempts CUDA; others fall back to software):
```bash
# NVIDIA GPUs on Windows/Linux (attempts hevc_cuvid)
./target/release/hdr_analyzer_mvp --hwaccel cuda -i "video.mkv" -o "out.bin"

# Linux VAAPI or macOS VideoToolbox currently log and fall back to software
./target/release/hdr_analyzer_mvp --hwaccel vaapi -i "video.mkv" -o "out.bin"
./target/release/hdr_analyzer_mvp --hwaccel videotoolbox -i "video.mkv" -o "out.bin"
```

Noise robustness for grainy content:
```bash
# Default behavior (histogram99 peak with EMA smoothing enabled)
./target/release/hdr_analyzer_mvp -i "grainy_video.mkv" -o "out.bin"

# Aggressive smoothing for very noisy content
./target/release/hdr_analyzer_mvp -i "grainy_video.mkv" -o "out.bin" \
  --hist-bin-ema-beta 0.05 --hist-temporal-median 3 --pre-denoise median3

# Disable histogram smoothing for clean content
./target/release/hdr_analyzer_mvp -i "clean_video.mkv" -o "out.bin" --hist-bin-ema-beta 0

# Conservative profile with direct max (most responsive)
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "out.bin" \
  --optimizer-profile conservative --peak-source max

# Tweak target_nits smoothing (enabled by default)
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --target-smoother off
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --smoother-alpha 0.1 --smoother-bidirectional false

# Native HLG content handling (auto-detected, override peak if desired)
./target/release/hdr_analyzer_mvp -i "hlg_video.mkv" -o "out.bin" --hlg-peak-nits 1200
```

Using cargo:
```bash
cargo run -p hdr_analyzer_mvp --release -- -i "video.mkv" -o "measurements.bin" --madvr-version 6 --target-peak-nits 1000 --scene-threshold 0.3 --downscale 2
```

### mkvdolby (Conversion Tool)

The `mkvdolby` tool orchestrates the entire conversion process from HDR10/HDR10+/HLG to Dolby Vision Profile 8.1 with Content Mapping v4.0.

```bash
# Basic usage (converts all MKV files in current directory)
mkvdolby

# Convert specific file
mkvdolby "input.mkv"

# Automatic Cleanup (Default Behavior)
# By default, mkvdolby deletes the source file and intermediate artifacts (.measurements, Details.txt)
# after a successful conversion to save space.

# To keep the source file and all intermediate files:
mkvdolby "input.mkv" --keep-source

# Additional flags
mkvdolby --help

# Hardware Acceleration
mkvdolby "input.mkv" --hwaccel cuda

# Hardware Encoding on macOS (Apple Silicon)
# Speed up HLG -> PQ conversion significantly (~10x) using VideoToolbox
mkvdolby "input.mkv" --encoder videotoolbox

# Verbose mode: show raw command output (useful for debugging)
mkvdolby "input.mkv" --verbose

# Quiet mode: minimal output (only errors and final result)
mkvdolby "input.mkv" --quiet
```

#### Progress Indicators

mkvdolby provides visual feedback for all operations:
- **Spinners** with elapsed time for long-running operations (dovi_tool, mkvmerge, hdr10plus_tool)
- **Success/failure indicators** (✓/✗) with timing information
- **TTY detection**: Automatically disables spinners for non-interactive/CI environments

#### CM v4.0 Metadata Options (New)

mkvdolby now generates Dolby Vision Content Mapping v4.0 metadata by default, which includes enhanced tone mapping (L8/L9/L11) for better picture quality on modern displays.

```bash
# Default: CM v4.0 with auto-detected settings
mkvdolby "input.mkv"

# Specify content type (affects display tone mapping)
mkvdolby "input.mkv" --content-type film      # For cinema/24fps content
mkvdolby "input.mkv" --content-type animation # For animated content

# Use legacy CM v2.9 if needed
mkvdolby "input.mkv" --cm-version v29

# Override source primaries detection
mkvdolby "input.mkv" --source-primaries 1  # 0=BT.2020, 1=P3-D65, 2=BT.709
```

**CM v4.0 metadata levels generated:**
- **L1**: Per-frame min/mid/max luminance (from HDR10+ or hdr_analyzer)
- **L2**: Trim parameters for 100/600/1000 nit displays
- **L6**: Static mastering display metadata
- **L9**: Source color primaries (auto-detected)
- **L11**: Content type and reference mode

### Verifier

```bash
./target/release/verifier "measurements.bin"
# or
cargo run -p verifier -- "measurements.bin"
```

Verifier reports:
- File format (version, flags)
- Scene/frame stats
- Peak brightness and avg PQ
- Histogram integrity checks
- Target nits stats (if optimizer enabled)
- Additional checks: FALL header coherence and flags vs. `target_nits` presence

## Command line arguments

### Core Options
- `-i, --input <PATH>`: Input HDR video file
- `-o, --output <PATH>`: Output `.bin` measurement file
- `--madvr-version <5|6>`: Output file version (default: 5)
- `--hwaccel <TYPE>`: Hardware acceleration hint (`cuda`, `vaapi`, `videotoolbox`)
- `--downscale <1|2|4>`: Downscale internal analysis resolution for speed (default: 1)
- `--sample-rate <N>`: Analyze every Nth frame (1=all, 2=every 2nd, etc.). Skipped frames inherit previous measurements. High impact on performance.
- `--no-crop`: Disable active-area crop detection (analyze full frame)

### Scene Detection
- `--scene-threshold <float>`: Scene cut threshold (default: 0.3)
- `--min-scene-length <frames>`: Drop cuts closer than N frames (default: 24)
- `--scene-smoothing <frames>`: Rolling window over scene-change metric (default: 5)

### Optimizer
- `--disable-optimizer`: Disable dynamic target nits generation (enabled by default)
- `--optimizer-profile <conservative|balanced|aggressive>`: Optimizer behavior preset (default: balanced)
- `--target-peak-nits <nits>`: Override header.target_peak_nits for v6 (default: computed MaxCLL)

### Noise Robustness (New in v1.4)
- `--peak-source <max|histogram99|histogram999>`: Peak brightness source (default: histogram99 for balanced/aggressive, max for conservative)
  - `max`: Direct max from Y-plane (most responsive to noise)
  - `histogram99`: 99th percentile from histogram (recommended, reduces noise impact)
  - `histogram999`: 99.9th percentile from histogram (most conservative)
- `--hist-bin-ema-beta <float>`: EMA smoothing for histogram bins, 0.0-1.0 (default: 0.1, lower = more smoothing, 0 = disabled)
- `--hist-temporal-median <N>`: Temporal median filter window in frames (default: 0/off, 3 = recommended for aggressive smoothing)
- `--pre-denoise <nlmeans|median3|off>`: Pre-analysis Y-plane denoising (default: off)
  - `median3`: 3x3 median filter (good for grainy content)
  - `nlmeans`: Non-local means denoising (reserved for future)

### Performance & Diagnostics
- `--analysis-threads <N>`: Override Rayon worker count for histogram analysis (default: logical cores)
- `--profile-performance`: Print per-stage throughput metrics (decode vs. analysis) when finished

Notes for v6 output:
- Per-gamut peaks (`peak_pq_dcip3`, `peak_pq_709`) are currently duplicated from BT.2020 (`peak_pq_2020`) as a compatibility placeholder. Proper per-gamut computation is planned.

## Minimal Beta Validation

1) Build:
```bash
cargo build --release --workspace
```

2) Analyze (v5 and v6):
```bash
./target/release/hdr_analyzer_mvp -i sample_hdr10.mkv -o measurements_v5.bin
./target/release/hdr_analyzer_mvp -i sample_hdr10.mkv -o measurements_v6.bin --madvr-version 6 --target-peak-nits 1000
```

3) Verify:
```bash
./target/release/verifier measurements_v5.bin
./target/release/verifier measurements_v6.bin
```

Expected:
- Version: 5 or 6 (matches selection)
- Flags: 2 (no optimizer) or 3 (optimizer enabled)
- Histograms: 256 bins, sums ≈ 100
- PQ values in [0,1]
- Scenes valid and within frame range

## Oracle Cloud ARM (Ampere) Readiness

- Fully functional with software decoding (no CUDA on Ampere).
- Rayon-backed histogram analysis saturates available cores; tune with `--analysis-threads` if you need to pin execution to available vCPUs.
- Use `--profile-performance` to capture decode vs. analysis throughput when validating new instances.
- Recommended packages (Ubuntu 22.04/24.04 arm64):
  ```bash
  sudo apt update
  sudo apt install -y \
    build-essential pkg-config clang lld \
    libavformat-dev libavcodec-dev libavutil-dev \
    libavfilter-dev libavdevice-dev libswscale-dev
  ```
  - `clang` + `lld` provide much faster linking, especially with LTO.
- Build and run using the steps above. Performance is CPU-bound; for higher throughput, planned improvements include parallelizing histogram accumulation across rows/tiles.
- Enabled optimizations: auto-threaded decode, skip-scaler when possible, fast scaling, host CPU tuning (`target-cpu=native`). Use `--downscale` for additional speedups. On Linux ARM64, the build uses the LLD linker when available.

## Local Quality Gates (Recommended)

- Pre-commit hooks (fmt, clippy):
  ```bash
  pipx install pre-commit  # or: pip install --user pre-commit
  pre-commit install        # installs pre-commit hook (fmt, clippy)
  pre-commit install --hook-type pre-push  # optional: quick tests on push
  ```
  Configuration lives in `.pre-commit-config.yaml`. Hooks run `cargo fmt --check` and `cargo clippy -D warnings` before commit.

- Pinned toolchain: rustc, clippy, and rustfmt are pinned via `rust-toolchain.toml` for reproducible CI/dev builds.



## Roadmap

- Proper VAAPI/VideoToolbox device contexts and hardware frame transfer
- Parallel frame processing (rayon) for multi-core performance
- SIMD optimizations for histogram calculations
- Configurable heuristics for optimizer and scene detection
- Automated tests and CI validation

## Acknowledgements

- `ffmpeg-next`: Rust bindings for FFmpeg
- `madvr_parse`: Library for reading/writing madVR measurement files
- `clap`, `anyhow`: CLI and error handling

## License

MIT License

## Legal & Trademarks

This software is a research project for video analysis and is not an official product of Dolby Laboratories.

- **Dolby Vision** is a trademark of Dolby Laboratories.
- **HDR10+** is a trademark of HDR10+ Technologies, LLC.

This project is not affiliated with, endorsed by, or sponsored by Dolby Laboratories or HDR10+ Technologies, LLC. Reference to these standards is strictly for compatibility and interoperability purposes.
