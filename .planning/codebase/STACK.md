# Technology Stack

**Analysis Date:** 2026-08-12

## Languages

**Primary:**
- Rust 1.80+ (stable channel) - Core implementation for all three binaries (`hdr_analyzer_mvp`, `mkvdovi`, `verifier`)
  - Edition: 2021
  - Pinned via `rust-toolchain.toml`

**Build/Tooling:**
- Bash - Shell scripts in `scripts/` and `.pre-commit-config.yaml`
- YAML - CI/CD workflows and configuration

## Runtime

**Environment:**
- Rust stable toolchain via `dtolnay/rust-toolchain@stable` (CI)
- Local development: `rustup` with `rust-toolchain.toml` pinning

**Package Manager:**
- Cargo (bundled with Rust)
- Lockfile: `Cargo.lock` (committed, locked dependencies)

**Toolchain Components** (`rust-toolchain.toml`):
- `clippy` - Linting
- `rustfmt` - Code formatting
- Cross-compilation targets:
  - `aarch64-unknown-linux-gnu` (ARM64 Linux)
  - `x86_64-unknown-linux-gnu` (x86-64 Linux)
  - `x86_64-apple-darwin` (macOS Intel)
  - `aarch64-apple-darwin` (macOS Apple Silicon)
  - `x86_64-pc-windows-msvc` (Windows)

## Frameworks

**Core:**
- FFmpeg 8.0 (via `ffmpeg-next` crate) - Video decoding, frame access, codec enumeration
  - Direct FFmpeg C bindings for 10-bit pixel data access
  - Hardware decode support: NVIDIA NVDEC (when CUDA feature enabled)
  - Software fallback for all platforms

**CLI:**
- Clap 4.5 - Command-line argument parsing with derive macros
  - Location: `hdr_analyzer_mvp/src/cli.rs`, `mkvdovi/src/cli.rs`, `verifier/src/cli.rs`

**Serialization:**
- Serde 1.0 - Structured data serialization
- Serde JSON 1.0 - JSON output for measurements and sidecar files

**Testing:**
- assert_cmd 2.0 - CLI testing
- predicates 3.1 - CLI output assertions
- tempfile 3.14 - Temporary test files

**Build/Dev:**
- cc 1.2 - C compiler wrapper (for bindgen/FFmpeg build)
- bindgen 0.72 - C header binding generation (for FFmpeg and CUDA)
- pkg-config 0.3 - Library discovery (FFmpeg, system libraries)

## Key Dependencies

**Critical:**
- `ffmpeg-next` 8.0 - Native FFmpeg bindings for video I/O
  - Requires FFmpeg dev libs (libavformat, libavcodec, libavutil, libavfilter)
  - Requires clang/libclang for bindgen at build time

- `dolby_vision` 3.3.2 - Dolby Vision RPU metadata parsing and generation
  - Used by `mkvdovi` for metadata operations

- `rayon` 1.11 - Data parallelism
  - Used in `hdr_analyzer_mvp` for frame batch processing
  - Parallel histogram computation

- `indicatif` 0.18.3 (analyzer), 0.17.11 (mkvdovi) - Progress bars and spinners
  - TTY detection and auto-disable in CI

- `madvr_parse` 1.0.3 - MadVR measurement file format parsing
  - Used by `hdr_analyzer_mvp` for writing `.bin` files
  - Used by `verifier` for inspection

**Infrastructure:**
- `anyhow` 1.0 - Flexible error handling at application level
- `thiserror` 2.0 - Custom error type derivation (library-level errors)
- `colored` 3.0 - Terminal color output
- `walkdir` 2.5 - Recursive directory traversal
- `regex` 1.10 - Pattern matching for file discovery and metadata parsing
- `ctrlc` 3.4 - Graceful shutdown on SIGINT (mkvdovi)

**GPU Acceleration (Optional):**
- `cudarc` 0.19.8 - CUDA runtime and NVRTC bindings (optional `cuda` feature)
  - Only required when `--features cuda` is built
  - Provides NVIDIA GPU detection, kernel compilation, and execution
  - Falls back to CPU if unavailable
- `libloading` 0.9 - Dynamic library loading for CUDA

**Parsing & Data:**
- `nom` 7.x - Parser combinator library (transitive, for binary format parsing)
- `bitvec` 1.1 - Bit-level data manipulation (transitive, for bitfield parsing)
- `serde_json` 1.0 - JSON serialization for L1 sidecars

## Configuration

**Environment:**
- `.env` files: Not used in this codebase
- Environment variables for testing:
  - `HDR_ANALYZE_REAL_SAMPLE` - Path to real content test media
  - `HDR_ANALYZE_REFERENCE_CSV` - Reference measurement data for validation
  - `HDR_ANALYZE_SHOTLIST` - Shot list for consistency tests

**Build:**
- `rust-toolchain.toml` - Pins stable Rust channel and cross-compilation targets
- `Cargo.toml` (root) - Workspace configuration with three members
  - Release profile: `lto = "fat"`, `codegen-units = 1`, `strip = true`, `panic = "abort"`
- `.cargo/config.toml` - Build optimization flags:
  - `rustflags = ["-C", "target-cpu=native"]` - CPU-native instruction set
  - ARM64 Linux specific: `linker = "clang"`, `-fuse-ld=lld` for fast LTO linking
- `clippy.toml` - Lint configuration with raised complexity thresholds
- `deny.toml` - Dependency security/license checks:
  - Platforms: x86_64/aarch64 Linux, macOS, Windows
  - Notable exceptions: `RUSTSEC-2025-0119` (indicatif transitive), `WTFPL` (ffmpeg-next)

**CI/CD:**
- `.github/workflows/ci.yml` - Linux/macOS/Windows builds
  - FFmpeg dev lib installation per platform
  - BINDGEN_EXTRA_CLANG_ARGS configuration for header discovery
- `.github/workflows/release.yml` - Tag-triggered binary releases
- `.pre-commit-config.yaml` - Local pre-commit hooks (fmt check, clippy deny-warnings, optional test)

## Platform Requirements

**Development:**
- Rust 1.80+ (stable)
- FFmpeg dev libraries:
  - Ubuntu: `libavformat-dev libavcodec-dev libavutil-dev libavfilter-dev libavdevice-dev libswscale-dev`
  - macOS: `ffmpeg` via Homebrew + LLVM 16+ via Homebrew
  - Windows: FFmpeg via vcpkg or system PATH
- C compiler + clang (for bindgen):
  - Ubuntu: `build-essential pkg-config clang lld libclang-dev`
  - macOS: Xcode Command Line Tools (automatic via Homebrew)
  - Windows: MSVC toolchain (via `rustup`)
- Optional: NVIDIA CUDA Toolkit 12.0+ for `--features cuda`

**Production:**
- FFmpeg (shared libraries or static) - must be in system PATH or discoverable
- Target platforms: Linux x86-64/ARM64, macOS Intel/Apple Silicon, Windows x64
- Release binaries published as: `.tar.gz` (Unix), `.zip` (Windows)

---

*Stack analysis: 2026-08-12*
