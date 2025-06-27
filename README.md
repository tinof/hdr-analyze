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

- **`hdr_analyzer_mvp`**: The main HDR analysis application that processes video files and generates madVR-compatible measurement files
- **`verifier`**: A utility tool for reading, validating, and inspecting madVR measurement files

## Key Features

- **Accurate Per-Frame Analysis**: Measures key metrics like Peak Brightness (MaxCLL) and Average Picture Level (APL).
- **PQ-Based Histogram**: Generates a 256-bin luminance histogram based on the ST.2084 Perceptual Quantizer (PQ) curve for high-precision analysis.
- **Automated Scene Detection**: Uses `ffmpeg` to intelligently segment the video into scenes for contextual analysis.
- **Advanced Dynamic Metadata Optimizer**: An optional, state-of-the-art optimizer that generates per-frame dynamic target nits for superior tone mapping.
    - **Temporal Smoothing**: Uses a 240-frame rolling average to prevent abrupt changes in brightness and ensure smooth visual transitions.
    - **Highlight Management**: Detects the "highlight knee" (99th percentile) to make intelligent decisions about preserving highlight detail versus overall brightness.
    - **Scene-Aware Heuristics**: Applies different logic for dark, medium, and bright scenes to preserve artistic intent.
- **High Performance**: Features revolutionary luminance-only piping optimization delivering 3x I/O improvement and 50-200+ FPS with hardware acceleration. Optimized for multi-core CPUs and designed to process large 4K/8K files efficiently.
- **Professional Output**: Generates madVR-compatible `.bin` measurement files, ready for use in Dolby Vision workflows.

## Prerequisites

- **Rust Toolchain**: Install from [rustup.rs](https://rustup.rs/).
- **FFmpeg**: Must be installed and available in your system's PATH.

## Installation

Clone the repository and build all workspace members:

```bash
git clone https://github.com/your-username/hdr-analyze.git
cd hdr-analyze
cargo build --release --workspace
```

This will build both tools. The executables will be located at:
- Main analyzer: `./target/release/hdr_analyzer_mvp`
- Verifier tool: `./target/release/verifier`

### Building Individual Tools

You can also build specific workspace members:

```bash
# Build only the main analyzer
cargo build --release -p hdr_analyzer_mvp

# Build only the verifier tool
cargo build --release -p verifier
```

## Usage

### HDR Analyzer Tool

The main analysis tool can be run in several ways:

#### Using the built executable:

**Standard Analysis (without optimizer):**
```bash
./target/release/hdr_analyzer_mvp -i "path/to/your/video.mkv" -o "measurements.bin"
```

**Analysis with Advanced Optimizer Enabled:**
```bash
./target/release/hdr_analyzer_mvp -i "path/to/your/video.mkv" -o "measurements_optimized.bin" --enable-optimizer
```

#### Using cargo run (from workspace root):

**Standard Analysis:**
```bash
cargo run -p hdr_analyzer_mvp -- -i "path/to/your/video.mkv" -o "measurements.bin"
```

**With Optimizer:**
```bash
cargo run -p hdr_analyzer_mvp -- -i "path/to/your/video.mkv" -o "measurements_optimized.bin" --enable-optimizer
```

## Hardware Acceleration (Advanced)

For users with compatible hardware, `hdr-analyze` supports GPU-accelerated video decoding to significantly improve performance. Combined with the new **luminance-only piping optimization**, this delivers exceptional performance gains of 50-200+ FPS.

The optimization processes only 1 byte per pixel (luminance) instead of 3 bytes (RGB), providing 3x I/O throughput improvement while maintaining full measurement accuracy.

To use it, provide the `--hwaccel` flag with the appropriate value for your system.

**Usage:**
```bash
# For NVIDIA GPUs on Windows or Linux
./target/release/hdr_analyzer_mvp --hwaccel cuda -i "video.mkv" -o "out.bin"

# For Intel/AMD GPUs on Linux
./target/release/hdr_analyzer_mvp --hwaccel vaapi -i "video.mkv" -o "out.bin"

# For macOS (Intel or Apple Silicon)
./target/release/hdr_analyzer_mvp --hwaccel videotoolbox -i "video.mkv" -o "out.bin"
```

**Using cargo run:**
```bash
# NVIDIA CUDA acceleration
cargo run -p hdr_analyzer_mvp --release -- --hwaccel cuda -i "video.mkv" -o "measurements_gpu.bin"

# Linux VAAPI acceleration
cargo run -p hdr_analyzer_mvp --release -- --hwaccel vaapi -i "video.mkv" -o "measurements_gpu.bin"

# macOS VideoToolbox acceleration
cargo run -p hdr_analyzer_mvp --release -- --hwaccel videotoolbox -i "video.mkv" -o "measurements_gpu.bin"
```

**Note:** Hardware acceleration requires that `ffmpeg` was compiled with support for the chosen method. The tool will automatically select appropriate hardware decoders when available. For CUDA acceleration, it defaults to `hevc_cuvid` decoder for H.265/HEVC content, which is common in HDR videos.

### Verifier Tool

The verifier tool can inspect and validate measurement files:

#### Using the built executable:
```bash
./target/release/verifier "measurements.bin"
```

#### Using cargo run:
```bash
cargo run -p verifier -- "measurements.bin"
```

The verifier will display detailed information about the measurement file including:
- File format validation
- Scene and frame statistics
- Peak brightness analysis
- Histogram integrity checks
- Optimizer data (if present)

## Arguments

- `-i, --input <PATH>`: Path to the input HDR video file.
- `-o, --output <PATH>`: Path for the output .bin measurement file.
- `--enable-optimizer`: (Optional) Activates the advanced optimizer to generate dynamic target nits.

## The Algorithm Explained

This tool operates in three distinct phases to ensure the highest quality output:

1. **Scene Detection**: The video is first quickly analyzed to identify the start and end of every scene.
2. **Frame Measurement**: The tool then performs a deep analysis of every single frame, calculating its peak brightness, perceptually accurate average brightness using industry-standard weighted luminance (Rec. 709/2020 coefficients), and a detailed 256-bin PQ histogram.
3. **Optimizer Pass (Optional)**: If enabled, a final pass is made over the frame data. Using scene-based statistics, a rolling average of previous frames, and highlight knee detection, it calculates the ideal target_nits for every frame to ensure a smooth, stable, and visually stunning result.

## Roadmap & Contributing

This tool is a robust V1.1 featuring the new luminance-only piping optimization for exceptional performance. Future enhancements may include:

- Implementing automated black bar detection for even more accurate APL measurements.
- Allowing user-configurable parameters for the optimizer heuristics.
- Additional hardware acceleration backends and optimizations.
- Advanced scene detection algorithms leveraging the performance improvements.

Contributions are welcome! Please feel free to open an issue or submit a pull request.

## Acknowledgements & Dependencies

This project is built with the help of several excellent open-source libraries. We extend our gratitude to their authors and contributors.

- **`madvr_parse`**: The core library used for reading and writing madVR measurement files. This project would not be possible without it.
  - **License:** MIT
  - **Copyright:** (c) 2025 quietvoid
- **`clap`**: For robust and user-friendly command-line argument parsing.
- **`anyhow`**: For simple and effective error handling.
- **`byteorder`**: For low-level binary data serialization.

## License

This project is licensed under the MIT License.