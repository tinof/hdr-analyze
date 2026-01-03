# Contributing to HDR-Analyze

Thank you for your interest in contributing to HDR-Analyze! This document provides guidelines and information for contributors.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [How to Contribute](#how-to-contribute)
- [Development Setup](#development-setup)
- [Coding Standards](#coding-standards)
- [Testing](#testing)
- [Submitting Changes](#submitting-changes)
- [Reporting Issues](#reporting-issues)

## Code of Conduct

This project adheres to a code of conduct that we expect all contributors to follow. Please be respectful, inclusive, and constructive in all interactions.

## Getting Started

1. **Fork the repository** on GitHub
2. **Clone your fork** locally:
   ```bash
   git clone https://github.com/your-username/hdr-analyze.git
   cd hdr-analyze
   ```
3. **Set up the development environment** (see Development Setup below)
4. **Create a feature branch** for your changes:
   ```bash
   git checkout -b feature/your-feature-name
   ```

## How to Contribute

### Types of Contributions

We welcome several types of contributions:

- **Bug fixes**: Help us identify and fix issues
- **Feature enhancements**: Improve existing functionality
- **New features**: Add new capabilities (please discuss first via issues)
- **Documentation**: Improve README, code comments, or examples
- **Performance optimizations**: Make the tool faster or more efficient
- **Testing**: Add test cases or improve test coverage

### Before You Start

- **Check existing issues** to see if your idea is already being discussed
- **Open an issue** for new features or significant changes to discuss the approach
- **Keep changes focused** - one feature or fix per pull request
- **Follow the coding standards** outlined below

## Development Setup

### Prerequisites

- **Rust toolchain** (1.70 or later): Install from [rustup.rs](https://rustup.rs/)
- **FFmpeg**: Must be installed and available in your system PATH
- **Tools**:
  - `git` (Version Control)
  - `mkvmerge` (MKVToolNix)
  - `mediainfo`
  - `dovi_tool`
  - `hdr10plus_tool`

### Building the Project

```bash
# Clone the repository
git clone https://github.com/your-username/hdr-analyze.git
cd hdr-analyze

# Build in debug mode
cargo build

# Build in release mode (for performance testing)
cargo build --release

# Run the tool
./target/debug/hdr_analyzer_mvp --help
```

### Running the Tools manually

While `cargo run` works, the workspace includes scripts and specialized workflows.

#### 1. The `mkvdolby` Script
The `mkvdolby` script provides an end-to-end workflow for processing video files.

-   **Standard Run**:
    ```bash
    PYTHONPATH="mkvdolby/src" python -m mkvdolby.cli "<input_video>"
    ```
-   **Run with Verification**:
    ```bash
    PYTHONPATH="mkvdolby/src" python -m mkvdolby.cli "<input_video>" --verify
    ```

#### 2. Running the Analyzer Directly
-   **PQ/HDR10 Content**:
    ```bash
    cargo run -p hdr_analyzer_mvp --release -- "video.mkv" -o "video_measurements.bin"
    ```
-   **HLG Content (Native Path)**:
    ```bash
    cargo run -p hdr_analyzer_mvp --release -- "video_hlg.mkv" -o "video_hlg_measurements.bin" --hlg-peak-nits 1000
    ```

### Running Tests

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test test_name
```

## Coding Standards

### Rust Code Style

- **Use `cargo fmt`** before committing to ensure consistent formatting
- **Run `cargo clippy`** and fix all warnings before submitting
- **Follow Rust naming conventions**:
  - `snake_case` for variables and functions
  - `PascalCase` for types and structs
  - `SCREAMING_SNAKE_CASE` for constants

### Code Quality Requirements

- **All code must pass `cargo clippy --release -- -D warnings`**
- **All code must be formatted with `cargo fmt`**
- **Add comprehensive documentation** for public functions using `///` comments
- **Include examples** in documentation where helpful
- **Write descriptive commit messages**

### Documentation Standards

- Use **Rustdoc comments (`///`)** for all public functions
- Include **parameter descriptions** and **return value information**
- Add **examples** for complex functions
- Keep **README.md** up to date with new features

## Testing

### Test Requirements

- **Add tests** for new functionality
- **Ensure existing tests pass** before submitting
- **Test with various input formats** when possible
- **Include edge case testing** for robust code

### Test Categories

- **Unit tests**: Test individual functions and components
- **Integration tests**: Test complete workflows
- **Performance tests**: Verify optimization improvements
- **Compatibility tests**: Test with different video formats

## Verification & QA Workflows

### 1. Verifying Measurement Files (`.bin`)
Use the `verifier` tool to inspect and validate the generated measurement files.

```bash
target/release/verifier "path/to/measurements.bin"
```

### 2. Verifying Dolby Vision Output
To inspect the final Dolby Vision MKV file:

1.  **Check MediaInfo**:
    ```bash
    mediainfo "output.DV.mkv"
    ```
2.  **Inspect RPU with `dovi_tool`**:
    ```bash
    dovi_tool extract-rpu -i "output.DV.mkv" -o RPU.bin
    dovi_tool info -i RPU.bin --summary
    ```

### 3. Baseline Comparison Harness
To guard against regressions, use the `compare_baseline` tool.

1.  **Build the tool**: `cargo build --release -p compare_baseline`
2.  **Run comparison**:
    ```bash
    target/release/compare_baseline --baseline ./path/to/baseline_bins --current ./path/to/current_bins
    ```

## Submitting Changes

### Pull Request Process

1. **Ensure your code follows all coding standards**:
   ```bash
   cargo fmt
   cargo clippy --release -- -D warnings
   cargo test
   ```

2. **Update documentation** if needed (README.md, CHANGELOG.md)

3. **Write a clear pull request description**:
   - What changes were made
   - Why the changes were necessary
   - How to test the changes
   - Any breaking changes or migration notes

4. **Link related issues** in the PR description

5. **Be responsive** to code review feedback

### Commit Message Format

Use clear, descriptive commit messages:

```
feat: add support for 8K video analysis
fix: resolve memory leak in frame processing
docs: update installation instructions
perf: optimize histogram calculation by 25%
test: add unit tests for PQ conversion functions
```

## Reporting Issues

### Bug Reports

When reporting bugs, please include:

- **Clear description** of the issue
- **Steps to reproduce** the problem
- **Expected vs actual behavior**
- **System information** (OS, Rust version, FFmpeg version)
- **Sample files** or command lines (if possible)
- **Error messages** or logs

### Feature Requests

For feature requests, please provide:

- **Clear description** of the desired functionality
- **Use case** or problem it solves
- **Proposed implementation** (if you have ideas)
- **Willingness to contribute** the implementation

### Using Issue Templates

Please use the provided issue templates when available:
- **Bug Report Template**: For reporting bugs
- **Feature Request Template**: For suggesting new features

## Development Tips

### Performance Considerations

- **Profile before optimizing** - use `cargo bench` for benchmarks
- **Consider memory usage** - HDR analysis can be memory-intensive
- **Test with large files** - 4K/8K videos stress-test the system
- **Optimize hot paths** - frame processing is the critical path

### Testing with Real Content

- **Test with various HDR formats** (HDR10, HDR10+, etc.)
- **Use different resolutions** (1080p, 4K, 8K)
- **Try different frame rates** (24fps, 30fps, 60fps)
- **Test edge cases** (very dark/bright content, rapid scene changes)

### Performance Profiling

The analyzer supports internal profiling metrics.

1. **Build release binary**: `cargo build --release -p hdr_analyzer_mvp`
2. **Run with profiling**:
   ```bash
   ./target/release/hdr_analyzer_mvp \
       --input sample_hdr10.mkv \
       --profile-performance \
       --analysis-threads 8
   ```
3. **Analyze Output**: Look for "Analysis FPS" and "Rayon worker count".

**Comparison**: Run with `--analysis-threads 1` to establish a baseline.

### Debugging

- **Use `RUST_LOG=debug`** for detailed logging
- **Add temporary print statements** for complex debugging
- **Use `cargo run --release`** for performance testing
- **Profile with tools** like `perf` or `cargo flamegraph`

## Questions?

If you have questions about contributing:

- **Check existing issues** and discussions
- **Open a new issue** with the "question" label
- **Join community discussions** (if available)

Thank you for contributing to HDR-Analyze! Your contributions help make HDR video processing more accessible and powerful for everyone.
