# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

HDR-Analyze is a Rust workspace containing tools for analyzing HDR10 video files to generate dynamic metadata for Dolby Vision conversion. The project implements advanced algorithms to analyze video on a per-frame and per-scene basis, creating measurement files for use with tools like `dovi_tool`.

## Workspace Structure

This is a Rust workspace with three main components:
- **`hdr_analyzer_mvp/`**: Main HDR analysis tool that processes video files and generates madVR-compatible measurement files
- **`mkvdolby/`**: Dolby Vision conversion tool — converts HDR10/HDR10+/HLG/Profile 7 FEL files to Profile 8.1 DV
- **`verifier/`**: Utility tool for reading, validating, and inspecting madVR measurement files

Non-workspace crates (not built by default):
- **`tools/compare_baseline/`**: Baseline comparison utility
- **`ffpb-rs-main/`**: FFmpeg progress bar helper

## Development Commands

### Building
```bash
# Build entire workspace in release mode
cargo build --release --workspace

# Build only the analyzer
cargo build --release -p hdr_analyzer_mvp

# Build only mkvdolby
cargo build --release -p mkvdolby

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

### Code Quality
```bash
# Format code
cargo fmt

# Linting (must pass with no warnings)
cargo clippy --release -- -D warnings

# Combined pre-commit check
cargo fmt && cargo clippy --release -- -D warnings && cargo test --workspace
```

### Running Tools
```bash
# Analyzer (positional input, auto-generated output filename)
./target/release/hdr_analyzer_mvp "video.mkv"

# Analyzer with flag-based input and custom output
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "measurements.bin"

# Analyzer with optimizer disabled (enabled by default)
./target/release/hdr_analyzer_mvp "video.mkv" -o "out.bin" --disable-optimizer

# Verifier
./target/release/verifier "measurements.bin"

# mkvdolby (converts all MKV files in current directory if no input given)
./target/release/mkvdolby "input.mkv"

# Using cargo run
cargo run -p hdr_analyzer_mvp --release -- "video.mkv" -o "out.bin"
cargo run -p verifier -- "measurements.bin"
```

## Workspace Lint Configuration

The workspace `Cargo.toml` defines a permissive clippy baseline:
- `clippy::all = allow` (priority -2) — suppresses most clippy lints by default
- `clippy::correctness = deny` (priority 1) — only correctness lints are enforced
- `clippy::dbg_macro = deny` — no `dbg!()` in committed code
- `unsafe_code = allow` but `unsafe_op_in_unsafe_fn = deny`

All workspace members inherit these via `[lints] workspace = true`. The CI and pre-commit hooks run `cargo clippy --workspace --all-targets -- -D warnings`, which catches any remaining warnings.

## Toolchain and CI

- **`rust-toolchain.toml`** pins `stable` channel with clippy + rustfmt components and cross-compilation targets (aarch64-linux, x86_64-linux, both macOS, Windows MSVC)
- **`.cargo/config.toml`**: ARM64 Linux uses `clang` linker with LLD and `-C target-cpu=native`; x86_64 target has empty `rustflags` (for cross-compilation)
- **Pre-commit hooks** (`.pre-commit-config.yaml`): `cargo fmt --check` and `cargo clippy -D warnings` on commit; `cargo test --workspace -q` on push
- **CI** (`.github/workflows/ci.yml`): Runs on `main` and `fix-*` branches. Stages: lint → test → cross-platform build (Ubuntu/macOS/Windows) + security audit + cargo-deny

## Architecture Overview

### Main Analysis Pipeline (hdr_analyzer_mvp)
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

### Key Components (hdr_analyzer_mvp)
- **`main.rs`**: Thin application entry point
- **`pipeline.rs`**: Main orchestration logic — decoding loop, progress bar, frame routing
- **`analysis/`**: Core analysis module:
  - `frame.rs`: Per-frame luminance analysis
  - `scene.rs`: Scene detection (chi-squared histogram distance)
  - `histogram.rs`: Histogram binning, PQ/nits conversion, EMA smoothing
  - `hlg.rs`: HLG (ARIB STD-B67) to PQ conversion
- **`optimizer.rs`**: Dynamic target nits generation with rolling averages and EMA smoothing
- **`ffmpeg_io.rs`**: FFmpeg initialization, hardware decoder setup, video info extraction
- **`writer.rs`**: madVR measurement file writing (v5/v6 formats)
- **`cli.rs`**: CLI definition and parsing (clap derive)
- **`crop.rs`**: Active-video area detection (black bar cropping)

### Key Components (mkvdolby)
- **`main.rs`**: Entry point — dispatches `composite-pipe` subcommand *before* dependency checks (so the subcommand works without external tools installed)
- **`cli.rs`**: CLI definition — `Args` struct, `SubCmd` enum, `CompositePipeArgs`, value enums for encoders/profiles
- **`pipeline.rs`**: Main conversion orchestration — HDR format detection, routing through HDR10/HDR10+/HLG/FEL paths, temp directory management, cleanup
- **`fel_composite.rs`**: Profile 7 FEL compositing engine — NLQ LinearDeadzone algorithm, polynomial (luma) and MMR (chroma) reshaping, Modal.com encoding integration
- **`metadata.rs`**: HDR format detection and static metadata extraction (mastering display, MaxCLL/MaxFALL)
- **`external.rs`**: External tool discovery (`find_tool`) and command execution helpers (`run_command_with_spinner`, `run_command_live`)
- **`progress.rs`**: Spinner and progress display with TTY detection, verbose/quiet mode
- **`verify.rs`**: Post-conversion verification (runs verifier + dovi_tool + mediainfo checks)

### Critical Implementation Details
- **Limited-range normalization**: HDR10 Y' codes (64-940) normalized to [0,1] before PQ domain binning
- **v5 histogram semantics**: Mid-bin weighting for average PQ computation, black-bar heuristic for bin 0
- **Scene detection**: Chi-squared-like histogram distance with configurable threshold (default 0.3)
- **Hardware acceleration**: CUDA attempted on NVIDIA GPUs, graceful fallback to software decoding
- **HLG conversion**: ARIB STD-B67 streams converted in-memory to PQ histograms using configurable peak nits (default 1000)
- **composite-pipe dispatch**: The subcommand is dispatched before `check_dependencies()` runs, so it works in environments (like Modal) that lack external tools like dovi_tool

## External Tool Dependencies

### hdr_analyzer_mvp
- **FFmpeg development libraries**: Required at compile time for `ffmpeg-next` (libavformat, libavcodec, libavutil, libavfilter, libavdevice, libswscale)

### mkvdolby (runtime)
mkvdolby orchestrates several external CLI tools that must be in PATH (or current directory):
- **`dovi_tool`**: RPU extraction, generation, injection — required for all DV conversions
- **`hdr10plus_tool`**: HDR10+ dynamic metadata extraction (only for HDR10+ sources)
- **`mkvmerge`** (MKVToolNix): Final MKV muxing
- **`mediainfo`**: HDR format detection and metadata extraction
- **`ffmpeg`**: Encoding (HLG→PQ conversion, FEL re-encoding)
- **`hdr_analyzer_mvp`**: Called as a subprocess for HDR10 analysis

### System Build Requirements
- **Rust toolchain**: stable (pinned via rust-toolchain.toml)
- **Ubuntu/Debian**: `sudo apt install build-essential pkg-config clang lld libclang-dev libavformat-dev libavcodec-dev libavutil-dev libavfilter-dev libavdevice-dev libswscale-dev`
- **macOS**: `brew install llvm ffmpeg pkg-config`
- **ARM64 Build Fix**: If encountering header errors, set: `BINDGEN_EXTRA_CLANG_ARGS="-I/usr/include/$(gcc -dumpmachine)"`

### Key Rust Dependencies
- **`ffmpeg-next`**: 8.0.0 — Native video processing (Windows uses `build` feature)
- **`madvr_parse`**: 1.0.2 — Reading/writing madVR measurement files
- **`dolby_vision`**: 3.3 — RPU parsing for Profile 7 FEL compositing (mkvdolby only)
- **`clap`**: 4.5+ — CLI with derive macros
- **`anyhow`/`thiserror`**: Error handling

## Current CLI Flags

### Analyzer (`hdr_analyzer_mvp`)
- `<INPUT>`: Input HDR video file (positional argument)
- `-o, --output <PATH>`: Output `.bin` measurement file (optional — auto-generates from input filename)
- `--disable-optimizer`: Disable dynamic target nits generation (enabled by default)
- `--hwaccel <TYPE>`: Hardware acceleration (`cuda`, `vaapi`, `videotoolbox`)
- `--madvr-version <5|6>`: Output file version (default: 5)
- `--scene-threshold <float>`: Scene cut threshold (default: 0.3)
- `--scene-metric <hist|hybrid>`: Scene detection metric (default: hist)
- `--target-peak-nits <nits>`: Override target_peak_nits for v6 files
- `--header-peak-source <max|histogram99|histogram999>`: How to select header MaxCLL (default: max)
- `--target-smoother <off|ema>`: Target nits smoother (default `ema`)
- `--smoother-bidirectional`: Use forward+backward EMA (default on)
- `--smoother-alpha <float>`: EMA alpha coefficient (default 0.2)
- `--hlg-peak-nits <float>`: Peak luminance for HLG analysis (default 1000.0 nits)
- `--sample-rate <N>`: Analyze every Nth frame (default: 1)

### mkvdolby
- `--verify`: Run verifier + dovi_tool + mediainfo post-mux checks
- `--keep-source`: Disable auto-cleanup (by default, source file and intermediates are deleted on success)
- `--hwaccel <cuda|none>`: Hardware acceleration hint (default: none)
- `--fel-encoder <local|modal>`: FEL re-encoding backend (default: local)
- `--fel-crf <N>`: Quality parameter for FEL re-encoding (default: 18)
- `--fel-preset <PRESET>`: x265 preset for local FEL encoding (default: medium)
- `--fel-nvenc-preset <PRESET>`: NVENC preset for Modal/CUDA encoding (default: p5)
- `--encoder <libx265|videotoolbox>`: Encoder for HLG→PQ conversion (default: libx265)
- `--cm-version <v29|v40>`: Content Mapping version (default: v40)
- `--content-type <cinema|film|live|animation|gaming|graphics|unknown>`: L11 content type (default: cinema)
- `--peak-source <max-scl-luminance|histogram|histogram99>`: HDR10+ peak source for dovi_tool (default: histogram99)
- `--boost`: Switch peak-source to histogram99 for brighter mapping
- `--optimizer-profile <conservative|balanced|aggressive>`: hdr_analyzer_mvp optimizer profile (default: conservative)
- `--trim-targets <nits>`: Comma-separated nits for DV trim pass (default: "100,600,1000")
- **Subcommand**: `composite-pipe` — Output raw NLQ-composited frames to stdout for piping to an encoder

### Profile 7 FEL → Profile 8.1 Conversion
mkvdolby detects Profile 7 FEL sources and converts them to Profile 8.1:
1. Extract HEVC → demux BL + EL + RPU via `dovi_tool`
2. Composite BL+EL using NLQ LinearDeadzone algorithm (polynomial + MMR reshaping)
3. Re-encode composited output (local x265/NVENC or Modal.com GPU)
4. Generate new Profile 8.1 RPU via `dovi_tool generate`
5. Inject RPU and mux final MKV

**Modal.com offload** (`--fel-encoder modal`): Uploads BL+EL+RPU to Modal where a cross-compiled x86_64 `mkvdolby composite-pipe` binary runs compositing, piping raw YUV directly to ffmpeg hevc_nvenc on an L4 GPU.

## Development Workflow

1. **Format checking**: Always run `cargo fmt` before commits
2. **Linting**: Code must pass `cargo clippy --release -- -D warnings`
3. **Testing**: Run full test suite with `cargo test --workspace --verbose`
4. **Pre-commit**: If installed (`pre-commit install`), fmt and clippy run automatically on commit
5. **Beta validation workflow**: Build → Analyze sample → Verify → Test with dovi_tool

## Oracle Cloud ARM Compatibility

- Uses software decoding (no CUDA on ARM)
- Recommended packages: `build-essential pkg-config clang lld libavformat-dev libavcodec-dev libavutil-dev libavfilter-dev libavdevice-dev libswscale-dev`
- Performance optimizations: `--sample-rate 3`, `--downscale 2`, smart skipping of scaling/cropping for non-analyzed frames
- Expected: ~10-12 fps for 4K HEVC on 4-core Ampere (~30-40 fps effective with sampling)

### Cross-Compilation (ARM64 → x86_64)
For Modal.com GPU offload, a cross-compiled x86_64 binary of `mkvdolby` is needed:
```bash
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-gnu
cargo zigbuild --release -p mkvdolby --target x86_64-unknown-linux-gnu
```

## Version Compatibility Notes

- **v5 format**: Default, widely compatible
- **v6 format**: Newer format — `target_peak_nits` written to header; per-gamut peaks currently duplicated from BT.2020 (temporary)
- Both formats validated with `verifier` tool and compatible with downstream `dovi_tool`
