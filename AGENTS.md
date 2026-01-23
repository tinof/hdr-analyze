# AGENTS.md - AI Agent Guidelines for HDR-Analyze

> Guidelines for AI coding agents operating in this Rust-based HDR video analysis workspace.

## Project Overview

A Rust monorepo (Cargo workspace) for HDR video analysis and Dolby Vision metadata generation.

**Workspace members:**
- `hdr_analyzer_mvp/` - Core HDR10 analysis engine (generates PQ-based histograms and DV metadata)
- `mkvdolby/` - MKV container handling and Dolby Vision metadata injection
- `verifier/` - MadVR measurement file validation

**Language:** Rust 1.82.0 (see `rust-toolchain.toml`)

---

## Build, Test & Lint Commands

### Build
```bash
cargo build                              # Debug build
cargo build --release --workspace        # Release build (all crates)
cargo build --release -p hdr_analyzer_mvp  # Single crate
```

### Test
```bash
cargo test --workspace                   # Run all tests
cargo test --workspace -- --nocapture    # With stdout output
cargo test test_name                     # Run single test by name
cargo test -p hdr_analyzer_mvp           # Run tests for single crate
cargo test -p verifier -- test_name      # Single test in specific crate
```

### Lint & Format
```bash
cargo fmt --all                          # Format all code
cargo fmt --all -- --check               # Check formatting (CI)
cargo clippy --workspace --all-targets -- -D warnings  # Lint (must pass)
```

### Audit & Verify
```bash
cargo audit                              # Security vulnerability check
cargo deny check                         # License/crate ban enforcement
```

---

## Code Style Guidelines

### Imports
- Use **absolute imports** with `crate::` for internal modules
- **Group imports** from the same crate with curly braces
- **Order:** std library → third-party crates → internal `crate::` modules

```rust
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use crate::analysis::FrameData;
use crate::cli::Cli;
```

### Naming Conventions
| Element          | Convention          | Example                    |
|------------------|---------------------|----------------------------|
| Files/Dirs       | `snake_case`        | `ffmpeg_io.rs`, `analysis/` |
| Functions/Vars   | `snake_case`        | `run_analysis`, `frame_count` |
| Structs/Enums    | `PascalCase`        | `MadVRFrame`, `TransferFunction` |
| Constants        | `SCREAMING_SNAKE`   | `MAX_FRAME_SIZE`           |
| Traits           | `PascalCase`        | `FrameProcessor`           |

### Type Definitions
- Define types in modules corresponding to their purpose
- Use `derive` macros for standard traits: `#[derive(Debug, Clone, Parser)]`
- Prefer structs with named fields over tuples for clarity

### Error Handling
- Use `anyhow::Result<T>` for application-level functions
- Propagate errors with `?` operator
- Add context with `.context()` or `.with_context()`
- Bail early with `anyhow::bail!()` for explicit errors

```rust
fn load_config(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .context("Failed to read config file")?;
    serde_json::from_str(&content)
        .context("Failed to parse config as JSON")
}
```

### Function Style
- Use `fn` declarations (not arrow functions)
- Closures for inline functional operations (`map`, `filter`)
- Large orchestrator functions delegate to smaller helpers
- Visibility: explicit `pub` or `pub(crate)` modifiers

### Documentation
- Use `///` for public API documentation
- Use `//` for inline implementation notes
- Use `// SAFETY:` to justify `unsafe` blocks
- Section separators: `// --- Section Name ---`

```rust
/// Analyzes HDR10 frames and generates luminance histograms.
///
/// # Arguments
/// * `input` - Path to the source video file
/// * `config` - Analysis configuration options
///
/// # Returns
/// Vector of frame measurements on success
pub fn analyze_frames(input: &Path, config: &Config) -> Result<Vec<FrameData>> {
    // Implementation
}
```

---

## Project Structure

```
hdr-analyze/
├── hdr_analyzer_mvp/       # Main analysis tool
│   ├── src/
│   │   ├── main.rs         # Entry point
│   │   ├── cli.rs          # CLI argument parsing
│   │   ├── pipeline.rs     # Orchestration logic
│   │   └── analysis/       # Core analysis modules
│   └── tests/              # Integration tests
├── mkvdolby/               # MKV/DV metadata tool (CM v4.0)
│   ├── src/
│   │   ├── cli.rs          # CLI with --cm-version, --content-type
│   │   ├── metadata.rs     # CmV40Config, L2/L9/L11 generation
│   │   └── pipeline.rs     # Conversion orchestration
├── verifier/               # Measurement validator
├── coordination/           # Orchestration/memory bank
├── tools/                  # Auxiliary scripts
└── Cargo.toml              # Workspace root
```

---

## Key Dependencies

| Crate         | Purpose                              |
|---------------|--------------------------------------|
| `clap`        | CLI argument parsing (v4, derive)    |
| `anyhow`      | Application error handling           |
| `thiserror`   | Library-level error types            |
| `ffmpeg-next` | FFmpeg bindings for video I/O        |
| `rayon`       | Parallel processing                  |
| `indicatif`   | Terminal progress bars               |
| `serde`       | Serialization/deserialization        |

---

## Testing Guidelines

- **Unit tests**: Inside source modules with `#[cfg(test)]`
- **Integration tests**: In `tests/` directory of each crate
- Test code may use `unwrap()`, `expect()`, and `print!()` (configured in `clippy.toml`)
- Use `assert_cmd` and `predicates` for CLI integration tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pq_conversion() {
        let result = convert_pq(0.5);
        assert!((result - expected).abs() < 0.001);
    }
}
```

---

## Pre-commit Checks

Before committing, ensure:
```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The project uses `.pre-commit-config.yaml` with hooks for:
- `cargo-fmt` (on commit)
- `cargo-clippy` (on commit)
- `cargo-test` (on push)

---

## Running the Tools

```bash
# HDR Analyzer
cargo run -p hdr_analyzer_mvp --release -- "video.mkv" -o "measurements.bin"

# With HLG content
cargo run -p hdr_analyzer_mvp --release -- "video.mkv" -o "out.bin" --hlg-peak-nits 1000

# Verifier
target/release/verifier "path/to/measurements.bin"
```

---

## Debugging

- Set `RUST_LOG=debug` for verbose logging
- Use `cargo run --release` for performance testing
- Profile with `perf` or `cargo flamegraph`

---

## mkvdolby CM v4.0 Options

mkvdolby generates Dolby Vision Content Mapping v4.0 metadata by default.

### CLI Arguments
| Argument | Default | Description |
|----------|---------|-------------|
| `--cm-version` | `v40` | Content Mapping version (v29 or v40) |
| `--content-type` | `cinema` | L11 content type (film, live, animation, cinema, gaming, graphics) |
| `--reference-mode` | `true` | L11 reference mode flag |
| `--source-primaries` | auto | L9 source primaries (0=BT.2020, 1=P3, 2=709) |
| `-v, --verbose` | `false` | Show raw command output (useful for debugging) |
| `-q, --quiet` | `false` | Minimal output (only errors and final result) |

### Generated Metadata Levels
- **L1**: Per-frame luminance from HDR10+ or hdr_analyzer
- **L2**: Trim parameters for 100/600/1000 nit target displays
- **L6**: Static mastering display metadata (MaxCLL, MaxFALL)
- **L9**: Source color primaries (auto-detected from MediaInfo)
- **L11**: Content type and reference mode hints

### Progress Indicators
mkvdolby uses `indicatif` for user-friendly progress feedback:
- **Spinners** with elapsed time for long-running operations (dovi_tool, mkvmerge, hdr10plus_tool)
- **Success/failure indicators** (✓/✗) with timing information
- **TTY detection**: Automatically disables spinners for non-interactive/CI environments
- Use `--verbose` to see raw tool output for debugging
