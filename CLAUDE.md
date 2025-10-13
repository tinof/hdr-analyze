# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

HDR-Analyze is a Rust workspace containing tools for analyzing HDR10 video files to generate dynamic metadata for Dolby Vision conversion. The project implements advanced algorithms to analyze video on a per-frame and per-scene basis, creating measurement files for use with tools like `dovi_tool`.

## Workspace Structure

This is a Rust workspace with two main components:
- **`hdr_analyzer_mvp/`**: Main HDR analysis tool that processes video files and generates madVR-compatible measurement files
- **`verifier/`**: Utility tool for reading, validating, and inspecting madVR measurement files

## Development Commands

### Building
```bash
# Build entire workspace in release mode
cargo build --release --workspace

# Build only the analyzer
cargo build --release -p hdr_analyzer_mvp

# Build only the verifier
cargo build --release -p verifier

# Debug builds
cargo build
```

### Testing
```bash
# Run all tests
cargo test --workspace --verbose

# Run specific test
cargo test test_name

# Run with output
cargo test -- --nocapture
```

### Running Tools
```bash
# Analyzer (basic usage with auto-generated output filename, positional input)
./target/release/hdr_analyzer_mvp "video.mkv"

# Analyzer with flag-based input and custom output
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "measurements.bin"

# Analyzer with optimizer disabled (enabled by default)
./target/release/hdr_analyzer_mvp "video.mkv" -o "out.bin" --disable-optimizer

# Different madVR versions
./target/release/hdr_analyzer_mvp "video.mkv" -o "out.bin" --madvr-version 6 --target-peak-nits 1000

# Hardware acceleration (CUDA)
./target/release/hdr_analyzer_mvp "video.mkv" --hwaccel cuda -o "out.bin"

# Verifier
./target/release/verifier "measurements.bin"

# Using cargo run
cargo run -p hdr_analyzer_mvp --release -- "video.mkv" -o "out.bin"
cargo run -p verifier -- "measurements.bin"
```

### Code Quality
```bash
# Format code
cargo fmt

# Linting (must pass with no warnings)
cargo clippy --release -- -D warnings

# Combined pre-commit check
cargo fmt && cargo clippy --release -- -D warnings && cargo test --workspace
```

## Architecture Overview

### Main Analysis Pipeline
The analyzer uses a native Rust pipeline via `ffmpeg-next` for direct video frame access:

1. **Video Initialization**: Uses ffmpeg-next to open input and detect best video stream
2. **Frame Decoding**: Software decoding (with optional CUDA acceleration via `hevc_cuvid`)
3. **Format Conversion**: Scales to YUV420P10LE for consistent 10-bit luminance analysis
4. **Active Area Detection**: Black bar crop detection on Y-plane (once per video)
5. **Per-Frame Analysis**: 
   - 10-bit Y-plane analysis within detected crop area
   - v5-compatible 256-bin luminance histogram (64 SDR + 192 HDR bins)
   - Peak brightness (MaxCLL) and Average Picture Level (APL) computation
6. **Scene Detection**: Histogram-distance based scene cut detection
7. **Optimization** (optional): Dynamic target nits generation with rolling averages
8. **Output**: madVR-compatible `.bin` files via `madvr_parse` library

### Key Components
- **`hdr_analyzer_mvp/src/main.rs`**: Thin application entry point.
- **`hdr_analyzer_mvp/src/pipeline.rs`**: Main orchestration logic for the analysis pipeline.
- **`hdr_analyzer_mvp/src/analysis/`**: Module containing all core analysis logic:
  - `frame.rs`: Per-frame analysis.
  - `scene.rs`: Scene detection.
  - `histogram.rs`: Histogram and PQ/nits conversion logic.
- **`hdr_analyzer_mvp/src/optimizer.rs`**: Dynamic target nits generation.
- **`hdr_analyzer_mvp/src/ffmpeg_io.rs`**: FFmpeg initialization and I/O.
- **`hdr_analyzer_mvp/src/writer.rs`**: madVR measurement file writing.
- **`hdr_analyzer_mvp/src/cli.rs`**: CLI definition and parsing.
- **`hdr_analyzer_mvp/src/crop.rs`**: Active-video area detection.
- **`verifier/src/main.rs`**: Measurement file validation and inspection.

### Critical Implementation Details
- **Limited-range normalization**: HDR10 Y' codes (64-940) normalized to [0,1] before PQ domain binning
- **v5 histogram semantics**: Mid-bin weighting for average PQ computation, black-bar heuristic for bin 0
- **Scene detection**: Chi-squared-like histogram distance with configurable threshold (default 0.3)
- **Hardware acceleration**: CUDA attempted on NVIDIA GPUs, graceful fallback to software decoding
- **HLG conversion**: ARIB STD-B67 streams converted in-memory to PQ histograms using configurable peak nits (default 1000)

## Dependencies and Prerequisites

### System Requirements
- **Rust toolchain**: 1.70 or later (install from https://rustup.rs/)
- **FFmpeg development libraries**: Required for `ffmpeg-next` compilation
  - macOS: `brew install ffmpeg pkg-config`
  - Ubuntu/Debian: `sudo apt install libavformat-dev libavcodec-dev libavutil-dev libavfilter-dev libavdevice-dev libswscale-dev pkg-config`
  - Windows: Install FFmpeg dev libraries or use vcpkg
- **Build tools**: C compiler and clang (Xcode CLT on macOS, build-essential on Linux, MSVC on Windows)
- **Additional for Ubuntu/ARM64**: `sudo apt install build-essential clang libclang-dev`

### FFmpeg Version Compatibility
- **Current**: Latest ARM64 optimized FFmpeg build (N-120864-g9a34ddc345-20250901)
- **Supported**: FFmpeg versions 3.4 through 8.0+ via ffmpeg-next 8.0.0
- **Legacy Issues**: FFmpeg 8.0+ header issues resolved with ffmpeg-next 8.0.0
- **ARM64 Build Fix**: If encountering header errors, set: `BINDGEN_EXTRA_CLANG_ARGS="-I/usr/include/$(gcc -dumpmachine)"`

### Key Rust Dependencies
- **`ffmpeg-next`**: 8.0.0 - Native video processing with automatic version detection (Windows uses `build` feature)
- **`madvr_parse`**: Reading/writing madVR measurement files
- **`clap`**: 4.5+ - Command-line interface with derive macros
- **`anyhow`**: Error handling
- **`rayon`**: 1.11+ - Parallel processing capabilities
- **`colored`**: 3.0+ - Enhanced verifier output formatting

## Current CLI Flags

### Analyzer (`hdr_analyzer_mvp`)
- `<INPUT>`: Input HDR video file (positional argument)
- `-o, --output <PATH>`: Output `.bin` measurement file (optional - auto-generates from input filename if not provided)
- `--disable-optimizer`: Disable dynamic target nits generation (enabled by default)
- `--hwaccel <TYPE>`: Hardware acceleration (`cuda`, `vaapi`, `videotoolbox`)
- `--madvr-version <5|6>`: Output file version (default: 5)
- `--scene-threshold <float>`: Scene cut threshold (default: 0.3)
- `--target-peak-nits <nits>`: Override target_peak_nits for v6 files
- `--target-smoother <off|ema>`: Target nits smoother (default `ema`)
- `--smoother-bidirectional`: Use forward+backward EMA (default on)
- `--smoother-alpha <float>`: EMA alpha coefficient (default 0.2)
- `--hlg-peak-nits <float>`: Peak luminance used for HLG analysis (default 1000.0 nits)

### Verifier
- Single positional argument: path to `.bin` file to verify

## Refactoring (Milestone R) - ✓ COMPLETE

The `main.rs` file has been successfully refactored into a modular structure, improving maintainability and testability. The application logic is now separated into distinct modules, each with a single responsibility.

## Development Workflow

1. **Format checking**: Always run `cargo fmt` before commits
2. **Linting**: Code must pass `cargo clippy --release -- -D warnings`
3. **Testing**: Run full test suite with `cargo test --workspace --verbose`
4. **Beta validation workflow**: Build → Analyze sample → Verify → Test with dovi_tool
5. **Performance considerations**: HDR analysis is memory and CPU intensive, test with 4K+ content

## Oracle Cloud ARM Compatibility

Fully compatible with ARM64 (Ampere) processors:
- Uses software decoding (no CUDA on ARM)
- Recommended packages for Ubuntu arm64: `build-essential pkg-config clang lld libavformat-dev libavcodec-dev libavutil-dev libavfilter-dev libavdevice-dev libswscale-dev`
  - `clang` + `lld` enable significantly faster linking with LTO
- Performance is CPU-bound; planned improvements include parallel histogram processing

## Version Compatibility Notes

- **v5 format**: Default, widely compatible
- **v6 format**: Newer format with additional fields
  - `target_peak_nits` written to header
  - Per-gamut peaks currently duplicated from BT.2020 (temporary until proper gamut computation)
- Both formats validated with `verifier` tool and compatible with downstream `dovi_tool`
