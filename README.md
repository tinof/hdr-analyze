# HDR-Analyze: Dynamic HDR Metadata Generator

A powerful, open-source workspace containing tools for analyzing HDR10 video files to generate dynamic metadata for Dolby Vision conversion.

This workspace implements advanced, research-backed algorithms to analyze video on a per-frame and per-scene basis, creating measurement files that can be used by tools like `dovi_tool` to produce high-quality Dolby Vision Profile 8.1 content from a standard HDR10 source.

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
│       └── main.rs
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
- Professional output: Writes madVR-compatible `.bin` measurement files through the `madvr_parse` library.
- Cross-platform: CPU decoding on all platforms with optional CUDA attempt on NVIDIA (graceful fallback to software decoding everywhere else).

## Native Pipeline Architecture

The analyzer uses a fully native Rust pipeline via `ffmpeg-next`, providing direct access to decoded video frames in memory (no external process invocation).

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

- CUDA: Attempts the `hevc_cuvid` decoder when `--hwaccel cuda` is specified (automatic fallback to software decoding if unavailable).
- VAAPI / VideoToolbox: Currently log and use software decoding paths; proper device contexts are planned. The pipeline remains fully functional via software decoding.

### Throughput controls and ARM optimizations

- Downscale analysis: Use `--downscale 2` (half) or `--downscale 4` (quarter) to speed up analysis with minimal impact on histogram/scene detection quality.
- Skips unnecessary scaling: If the decoder outputs `YUV420P10LE` and `--downscale 1`, the scaler is bypassed to avoid extra copies.
- Faster scaling: When scaling is needed, uses `FAST_BILINEAR` for analysis (sufficient for statistics).
- Decoder threading: Enables FFmpeg multi-threading (auto thread count) for better CPU utilization.
- Rust build tuning: Workspace includes `.cargo/config.toml` with `-C target-cpu=native` to enable host-specific optimizations (NEON on ARM).

## Prerequisites

- Rust toolchain: Install from https://rustup.rs/
- FFmpeg development libraries: Required for compiling `ffmpeg-next`
  - macOS: `brew install ffmpeg pkg-config`
  - Ubuntu/Debian: `sudo apt install libavformat-dev libavcodec-dev libavutil-dev libavfilter-dev libavdevice-dev libswscale-dev pkg-config`
  - Windows: Install FFmpeg dev libraries or use vcpkg
- Build tools: C compiler and build tools (Xcode CLT on macOS, build-essential on Linux, MSVC on Windows)

## Installation

Clone the repository and build all workspace members:

```bash
git clone https://github.com/tinof/hdr-analyze.git
cd hdr-analyze
cargo build --release --workspace
```

Executables:
- Analyzer: `./target/release/hdr_analyzer_mvp`
- Verifier: `./target/release/verifier`

### Building individual tools

```bash
# Build only the main analyzer
cargo build --release -p hdr_analyzer_mvp

# Build only the verifier tool
cargo build --release -p verifier
```

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

Using cargo:
```bash
cargo run -p hdr_analyzer_mvp --release -- -i "video.mkv" -o "measurements.bin" --madvr-version 6 --target-peak-nits 1000 --scene-threshold 0.3 --downscale 2
```

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

- `-i, --input <PATH>`: Input HDR video file
- `-o, --output <PATH>`: Output `.bin` measurement file
- `--disable-optimizer`: Disable dynamic target nits generation (enabled by default)
- `--hwaccel <TYPE>`: Hardware acceleration hint (`cuda`, `vaapi`, `videotoolbox`)
- `--madvr-version <5|6>`: Output file version (default: 5)
- `--scene-threshold <float>`: Scene cut threshold (default: 0.3)
- `--min-scene-length <frames>`: Drop cuts closer than N frames (default: 24)
- `--scene-smoothing <frames>`: Rolling window over scene-change metric (default: 5)
- `--target-peak-nits <nits>`: Override header.target_peak_nits for v6 (default: computed MaxCLL)
- `--downscale <1|2|4>`: Downscale internal analysis resolution for speed (default: 1)
- `--no-crop`: Disable active-area crop detection (analyze full frame)
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

## The Native Pipeline Algorithm (Overview)

1. Initialize video with `ffmpeg-next`, open input, detect best video stream.
2. Decode frames (software by default; CUDA attempted if requested) and scale to YUV420P10LE.
3. Active-video crop detection on Y-plane to ignore black bars.
4. For each frame:
   - Read 10-bit Y-plane samples
   - Normalize (HDR10 limited-range) to [0,1]
   - Bin into v5 histogram layout (SDR+HDR), compute avg PQ via mid-bin weighting
   - Track peak PQ
   - Optionally compute optimizer target nits using rolling averages and highlight knee
5. Scene detection using histogram distance with configurable threshold.
6. Serialize measurements via `madvr_parse` as v5 or v6.

## Roadmap

- Proper VAAPI/VideoToolbox device contexts and hardware frame transfer
- Parallel frame processing (rayon) for multi-core performance
- SIMD optimizations for histogram calculations
- Additional HDR formats (HDR10+, Dolby Vision ancillary data)
- Configurable heuristics for optimizer and scene detection
- Automated tests and CI validation

## Acknowledgements

- `ffmpeg-next`: Rust bindings for FFmpeg
- `madvr_parse`: Library for reading/writing madVR measurement files
- `clap`, `anyhow`: CLI and error handling

## License

MIT License
