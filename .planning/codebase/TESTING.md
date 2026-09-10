# Testing Patterns

**Analysis Date:** 2026-08-12

## Test Framework

**Runner:**
- Built-in Rust test infrastructure (no external test framework)
- Invoked via `cargo test [--package <crate>] [-- <filter>]`

**Assertion Library:**
- `assert!()`, `assert_eq!()`, `assert_ne!()` — standard Rust macros
- `predicates` crate (3.1) — matcher library for CLI output assertions
- `assert_cmd` crate (2.0) — command execution and assertion helpers

**Run Commands:**
```bash
cargo test --workspace --verbose              # Run all tests with full output
cargo test -p hdr_analyzer_mvp                # Run tests for one crate
cargo test -p <crate> -- <test_name>         # Run single test by name
cargo test --workspace -q                    # Quick test run (quiet mode, push gate)
cargo fmt --all -- --check                   # Format check (pre-commit)
cargo clippy --workspace --all-targets -- -D warnings  # Lint check (pre-commit)
```

## Test File Organization

**Location:**
- `tests/` directory at crate root (alongside `src/`)
- Test files: `tests/*.rs` (parallel to source modules, not co-located)
- Structure per crate:
  - `hdr_analyzer_mvp/tests/` — CLI tests, synthetic accuracy, real-content consistency
  - `mkvdovi/tests/` — integration tests
  - `verifier/tests/` — CLI tests

**Naming:**
- Test functions: `test_*` pattern — `test_help_flag()`, `test_missing_input_shows_error()`, `test_invalid_peak_estimator()`
- Test files: descriptive names — `cli.rs` (CLI argument parsing), `integration.rs` (end-to-end), `synthetic_accuracy.rs` (generated content validation), `real_content_consistency.rs` (real-world validation)

## Test Structure

**File Template:**
```rust
//! Module doc comment explaining test scope.
//!
//! Details about what's being tested, preconditions, skipping behavior.

use [crates]...;

// Helper functions for setup/teardown
fn helper_function() -> Type { ... }

// Test functions
#[test]
fn test_some_condition() {
    // Arrange
    let input = helper_function();
    
    // Act & Assert
    assert_eq!(result, expected);
}
```

**Patterns:**

*CLI Testing (assert_cmd + predicates):*
```rust
use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn test_help_flag() {
    Command::cargo_bin("binary_name")
        .expect("find binary")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("expected text"));
}

#[test]
fn test_error_case() {
    Command::cargo_bin("binary_name")
        .expect("find binary")
        .arg("invalid_file.txt")
        .assert()
        .failure()
        .stderr(predicate::str::contains("error message"));
}
```

*Environment-Dependent Skip:*
```rust
fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok()
}

#[test]
fn test_with_ffmpeg() {
    if !have_ffmpeg() {
        eprintln!("Skipping: ffmpeg not found in PATH");
        return;
    }
    
    // Test implementation
}
```

*Process Spawning:*
```rust
use std::process::{Command, Stdio};

let status = Command::new("tool")
    .arg("--flag")
    .arg(input_path)
    .stdout(Stdio::null())
    .stderr(Stdio::inherit())
    .status()
    .expect("spawn tool");

assert!(status.success(), "tool failed on {}", input_path);
```

## Mocking

**Strategy:**
- Minimal mocking; prefer real implementations for integration tests
- Process spawning mocked by returning error when tool unavailable (graceful skip)
- File I/O tested with real temporary files (`tempfile` crate)
- No mock libraries in use (complex to maintain; tests remain close to real behavior)

**Tempfile Usage:**
```rust
use tempfile::TempDir;
use std::path::Path;

let dir = TempDir::new().expect("create temp dir");
let test_file = dir.path().join("output.bin");

// Use test_file in test...

// Auto-cleanup when dir goes out of scope
```

**What to Mock:**
- External tool availability (check with helper functions, skip if not found)
- File paths (use temporary directories for actual file I/O)

**What NOT to Mock:**
- FFmpeg I/O operations (use real ffmpeg if available, skip test otherwise)
- Measurement file parsing (test against real binary format)
- CLI argument parsing (test against actual clap parser)

## Fixtures and Factories

**Test Data:**
- Synthetic video generation: helper functions in test files encode test clips
  - `encode_yuv_plane_clip()` — arbitrary YUV planes as lossless FFV1
  - `encode_y_plane_clip()` — luma-only with constant chroma
  - `solid_color_clip()` — solid-color test pattern

**Location:**
- Test helpers defined in same test file (no separate fixtures directory)
- Test clips generated on-the-fly (not pre-committed)
- Real sample media: referenced via environment variables
  - `HDR_ANALYZE_REAL_SAMPLE` — path to real HDR10 video
  - `HDR_ANALYZE_REFERENCE_CSV` — reference measurements (optional)
  - `HDR_ANALYZE_SHOTLIST` — scene breaks list (optional)

**Example (from `real_content_consistency.rs`):**
```rust
fn real_sample() -> Option<PathBuf> {
    let Ok(value) = std::env::var("HDR_ANALYZE_REAL_SAMPLE") else {
        eprintln!("Skipping: HDR_ANALYZE_REAL_SAMPLE is not set");
        return None;
    };
    let path = PathBuf::from(value);
    if !path.exists() {
        eprintln!("Skipping: HDR_ANALYZE_REAL_SAMPLE does not exist: {}", path.display());
        return None;
    }
    Some(path)
}

#[test]
fn test_consistency() {
    let sample = match real_sample() {
        Some(p) => p,
        None => return,  // Skip gracefully
    };
    
    // Test implementation using sample
}
```

## Coverage

**Requirements:** Not enforced by CI (no coverage reporting tool configured)

**View Coverage:**
- Local coverage measurement requires separate tool: `cargo tarpaulin` or `cargo llvm-cov` (not in use)
- No automated coverage gating in CI workflow

**Coverage Gaps:**
- GPU analysis (CUDA feature): only tested on NVIDIA hosts; CI tests use CPU fallback
- External tool invocations: tests skip when tool unavailable (env-dependent)
- Real-world content: tests skip when sample media not provided

## Test Types

**Unit Tests:**
- Scope: Individual functions in isolation
- Approach: Direct function calls with assertion on results
- Example: `test_invalid_min_percentile()` validates CLI argument range checking in main.rs

**Integration Tests:**
- Scope: End-to-end binary execution with files
- Approach: `Command::cargo_bin()` + `assert_cmd` assertions
- Files in: `tests/*.rs` at crate root
- Example: `test_mkvdovi_execution_sample()` runs full mkvdovi conversion on test media

**Synthetic Accuracy Tests:**
- Scope: Validate analysis algorithms against known-good synthetic data
- Approach: Generate video with known peak luminance, run analyzer, compare results
- Example: `hdr_analyzer_mvp/tests/synthetic_accuracy.rs` — encodes lossless FFV1 clips with precise PQ codes, measures them, asserts peak matches to within ±1/4 of a 12-bit code

**Real-Content Consistency Tests:**
- Scope: Validate analyzer behavior on actual HDR10 video
- Approach: Run analyzer twice with different parameters, check cross-run invariants
- Precondition: Real sample via `HDR_ANALYZE_REAL_SAMPLE` env var (tests skip if not set)
- Example: `real_content_consistency.rs` — max-RGB peak must dominate luma peak, frame count identical across runs

## CI Test Gates

**Workflow Order (`.github/workflows/ci.yml`):**
1. **Lint** (fmt + clippy) — must pass, blocks later jobs
2. **Test** — runs `cargo test --workspace --verbose`, must pass
3. **Build** (cross-platform) — runs on Ubuntu/macOS/Windows after test passes

**Pre-commit Hooks (`.pre-commit-config.yaml`):**
- **Commit stage:**
  - `cargo fmt --all -- --check` — format validation
  - `cargo clippy --workspace --all-targets -- -D warnings` — lint validation
- **Push stage:**
  - `cargo test --workspace -q` — quick test run (fails if any test fails)

**Environment-Dependent Behavior:**
- CI machine has `ffmpeg`, `libclang`, no `dovi_tool` by default
- Tests gracefully skip when tools missing (via helper functions)
- Not all tests run in CI (real-content tests skip without env vars)

## Common Patterns

**Async Testing:**
- Not applicable (no async runtime in use; Rust tests are synchronous by default)

**Error Testing:**
- CLI error assertions use predicates on stderr/exit code:
  ```rust
  #[test]
  fn test_error_case() {
      Command::cargo_bin("analyzer")
          .arg("invalid.mkv")
          .assert()
          .failure()
          .stderr(predicate::str::contains("required"));
  }
  ```

**File I/O Testing:**
- Temporary directories via `tempfile::TempDir`
- Temporary file cleanup automatic (RAII)
- No manual cleanup needed

**Binary Output Testing:**
- Load binary file with `std::fs::read()`
- Parse with domain-specific parser: `madvr_parse::MadVRMeasurements::parse_measurements()`
- Assert on parsed structure fields

**Skipping Tests:**
- Early `return` with optional precondition:
  ```rust
  if !condition {
      eprintln!("Skipping: reason");
      return;
  }
  ```

## Test Allowances (Clippy)

**In Tests Only:**
- `unwrap()` — permitted (test panic is acceptable failure signal)
- `expect()` — permitted (with message)
- `panic!()` — permitted (test assertion failure)
- `println!()` / `eprintln!()` — permitted (diagnostic output)
- `dbg!()` — permitted (debug output, usually removed)

Configured in `clippy.toml` — these would be denied in production code.

---

*Testing analysis: 2026-08-12*
