# Coding Conventions

**Analysis Date:** 2026-08-12

## Naming Patterns

**Files:**
- Lowercase with underscores: `main.rs`, `pipeline.rs`, `external.rs`, `ffmpeg_io.rs`
- Binary entrypoint: `main.rs` in each crate's `src/` directory
- Module files match their declared module name exactly

**Functions:**
- All functions use `snake_case`: `peak_estimator_name()`, `write_frame_stats_csv()`, `collect_default_inputs()`, `natural_segment_cmp()`, `run_command_with_spinner()`
- Abbreviations kept lowercase in snake_case: `pq_to_nits()`, `nits_to_pq()`

**Structs & Types:**
- All structs use `PascalCase`: `CropStabilityMonitor`, `FrameAnalysisOptions`, `VideoInfo`, `Cli`, `FramePeakStats`
- Type aliases: `PascalCase`

**Enums:**
- Enum names: `PascalCase` — `PeakEstimator`, `PeakDomain`, `SubCmd`
- Enum variants: `PascalCase` — `Max`, `Percentile`, `Robust`, `MaxRgb`, `Luma`

**Constants:**
- All uppercase with underscores: `ST2084_Y_MAX`, `ST2084_M1`, `CROP_EDGE_TOLERANCE`, `W`, `H`, `FRAMES`
- Typically declared at module top level or inside functions that use them

**Variables:**
- Local variables: `snake_case` — `video_info`, `input_path`, `frame_index`, `peak_nits`
- Struct fields: `snake_case` — `peak_pq_2020`, `target_nits`, `checked_scenes`

## Code Style

**Formatting:**
- Tool: `cargo fmt` (rustfmt)
- Max line width: 100 characters
- Tab spaces: 4 (soft tabs, not hard tabs)
- Import reordering: enabled (automatic)
- Newline style: Auto (platform-native)
- Enforced on commit via pre-commit hook

**Linting:**
- Tool: `cargo clippy --workspace --all-targets -- -D warnings`
- Workspace lints configured in root `Cargo.toml`: clippy `correctness` denied, `dbg_macro` denied, `all` allowed with priority -2
- `unsafe_code` allowed, `unsafe_op_in_unsafe_fn` denied
- Complexity thresholds tuned for video processing (higher than defaults):
  - Cognitive complexity: 30 (default 25)
  - Type complexity: 400 (default 250)
  - Max function arguments: 10 (default 7)
  - Max lines per function: 150 (default 100)
  - Enum variant size: 500 bytes (default 200)
  - Large error threshold: 256 bytes
- Enforced on commit via pre-commit hook
- `unwrap`/`expect`/`panic`/`print`/`dbg` allowed in tests only (via `clippy.toml`)

## Import Organization

**Order:**
1. Standard library imports (`use std::...`)
2. External crate imports (alphabetical)
3. Crate-local imports (`use crate::...`)

**Example from `pipeline.rs`:**
```rust
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use madvr_parse::{MadVRFrame, MadVRScene};

use crate::analysis::frame::analyze_native_frame_cropped;
use crate::cli::{Cli, PeakDomain};
use crate::crop::detect_crop;
```

**Path Aliases:**
- Absolute `crate::` paths used internally (no relative paths like `super::`)
- Imports grouped with braces when importing multiple items from the same module

## Error Handling

**Pattern:**
- Application-level return type: `anyhow::Result<T>` (not custom error types)
- Library-level (mkvdovi): uses `thiserror` for structured error types in `src/` modules
- Error creation: `anyhow::anyhow!("message")` macro
- Context addition: `.context()` or `.with_context(|| "message")`  combinator
- Error propagation: `?` operator

**Examples:**
```rust
// In hdr_analyzer_mvp/src/main.rs
if threads == 0 {
    return Err(anyhow::anyhow!("--analysis-threads must be at least 1"));
}

// In pipeline.rs
let file = File::create(path)
    .with_context(|| format!("Failed to create frame-stats CSV {}", path.display()))?;
```

**Guidelines:**
- Use `?` operator for early return on errors
- Add context when crossing subsystem boundaries (e.g., file I/O, FFmpeg, external tools)
- Avoid silent failures; surface errors to stdout/stderr via `println!()` or exit code

## Logging & Output

**Strategy:**
- Minimal logging in library code (avoid dependencies on logging crates)
- User-facing output via `println!()` for informational messages
- Diagnostic output via `eprintln!()` for skipped tests or warnings
- No debug-level logging (verbose mode is CLI-controlled)
- Progress indicators via `indicatif` for long-running operations

**Examples:**
```rust
// Informational output (hdr_analyzer_mvp/src/main.rs)
println!("Video resolution: {}x{}", video_info.width, video_info.height);

// Test skip diagnostic (mkvdovi/tests/integration.rs)
eprintln!("Skipping: dovi_tool not found in PATH");
```

## Comments

**Documentation Comments:**
- `///` for documenting public functions, structs, and methods
- `//!` for module-level documentation (at top of file)
- Markdown formatting supported in doc comments

**Inline Comments:**
- `//` for explaining complex logic, algorithm choices, or non-obvious code
- Used sparingly; code should be self-documenting via naming

**Examples:**
```rust
/// Create a copy of a MadVRFrame (MadVRFrame doesn't implement Clone)
fn copy_frame(frame: &MadVRFrame) -> MadVRFrame { ... }

//! Synthetic ground-truth accuracy test.
//!
//! Builds a lossless (FFV1) PQ clip whose peak luminance is known by construction...

// Convert (and optionally downscale) a decoded frame to YUV420P10LE, creating the
// scaler lazily from the actual input frame format...
```

## Function Design

**Size:**
- Prefer functions under 150 lines (clippy threshold is 150)
- Cognitive complexity target: 30 or less
- Extract helper functions when logic becomes complex

**Parameters:**
- Maximum 10 parameters (clippy threshold)
- Use `&str`, `&Path` for borrowed string/path arguments
- Use options/enums for boolean flags when multiple related options exist
- Use struct for related configuration parameters

**Return Values:**
- Return `anyhow::Result<T>` at application boundaries
- Return `Option<T>` for optional values
- Return owned types unless lifetime justifies borrowing
- Avoid returning raw pointers (use safer types like `Box<T>`)

**Examples:**
```rust
pub fn write_frame_stats_csv(path: &Path, stats: &[FramePeakStats]) -> Result<()> { ... }

pub fn find_tool(tool_name: &str) -> Option<PathBuf> { ... }

fn peak_estimator_name(estimator: PeakEstimator) -> &'static str { ... }
```

## Module Design

**Exports:**
- Public items marked with `pub`
- Private items are default (no `pub` keyword)
- Module declarations at top of `main.rs` as `mod <name>;`
- Re-exports: use `pub use` for public API items

**Module Structure:**
```
hdr_analyzer_mvp/src/
├── main.rs          # CLI parse, validation, orchestration
├── cli.rs           # clap::Parser-based CLI definition
├── pipeline.rs      # Main processing orchestration
├── analysis/        # Analysis submodules
│   ├── mod.rs
│   ├── frame.rs     # Per-frame analysis
│   ├── gpu.rs       # CUDA GPU analysis
│   └── scene.rs     # Scene detection
├── ffmpeg_io.rs     # FFmpeg integration
└── writer.rs        # Output file writing
```

**Impl Blocks:**
- One `impl` block per type (unless trait implementations)
- Group related methods together within the block
- Prefer methods over standalone functions when operating on a type

**Example:**
```rust
impl CropStabilityMonitor {
    fn new() -> Self { ... }
    fn record(&mut self, ...) { ... }
    fn report(&self) { ... }
}
```

## Dependency Management

**Workspace Dependencies:**
- Defined in root `Cargo.toml` under `[workspace.dependencies]`
- Individual crates use `{ workspace = true }` to reference them
- Allows coordinated version bumping across crates

**Key Workspace Dependencies:**
- `anyhow` 1.0 — error handling (app level)
- `clap` 4.5 with derive feature — CLI parsing
- `assert_cmd` 2.0 — CLI testing (dev-dependency)
- `predicates` 3.1 — assertion matchers (dev-dependency)
- `tempfile` 3.14 — temporary test files (dev-dependency)

**Crate-Specific Dependencies:**
- `mkvdovi`: `thiserror` for structured errors, `colored` for terminal output, `indicatif` for progress
- `hdr_analyzer_mvp`: `ffmpeg-next`, `rayon` for parallelism, `indicatif` for progress, optional `cudarc` for CUDA
- `verifier`: minimal, just `madvr_parse` for measurement file parsing

---

*Convention analysis: 2026-08-12*
