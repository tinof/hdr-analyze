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

- **Native Video Processing**: Built with `ffmpeg-next` for direct access to high-bit-depth video data, eliminating external process overhead and enabling precise 10-bit PQ luminance mapping.
- **Accurate Per-Frame Analysis**: Measures key metrics like Peak Brightness (MaxCLL) and Average Picture Level (APL) with direct access to 10-bit YUV420P10LE frame data.
- **Precision PQ-Based Histogram**: Generates a 256-bin luminance histogram using native 10-bit values that directly correspond to the ST.2084 Perceptual Quantizer (PQ) curve for maximum accuracy.
- **Native Scene Detection**: Real-time histogram-based scene detection using Sum of Absolute Differences algorithm, eliminating external parsing overhead.
- **Advanced Dynamic Metadata Optimizer**: An optional, state-of-the-art optimizer that generates per-frame dynamic target nits for superior tone mapping.
    - **Temporal Smoothing**: Uses a 240-frame rolling average to prevent abrupt changes in brightness and ensure smooth visual transitions.
    - **Highlight Management**: Detects the "highlight knee" (99th percentile) to make intelligent decisions about preserving highlight detail versus overall brightness.
    - **Scene-Aware Heuristics**: Applies different logic for dark, medium, and bright scenes to preserve artistic intent.
- **High Performance**: Native Rust pipeline with direct memory access to video frames, maintaining ~13-14 FPS processing speed while eliminating external process coordination overhead.
- **Hardware Acceleration Support**: CUDA, VAAPI, and VideoToolbox acceleration with graceful fallback to software decoding.
- **Professional Output**: Generates madVR-compatible `.bin` measurement files, ready for use in Dolby Vision workflows.

## Native Pipeline Architecture

The HDR analyzer features a fully native Rust pipeline using the `ffmpeg-next` crate, providing direct access to video data and eliminating external process overhead:

### Native Video Processing Benefits

**Direct Memory Access:**
- Native `ffmpeg-next` integration eliminates external FFmpeg process spawning
- Direct access to high-bit-depth video frame data in memory
- No pipe-based communication or external process coordination overhead
- Type-safe video processing with compile-time guarantees

**Accurate 10-bit PQ Luminance Mapping:**
- Direct access to YUV420P10LE frame data with native 10-bit precision
- **Critical Accuracy Fix:** 10-bit luma values (0-1023) directly correspond to PQ curve
- Formula: `pq_value = luma_10bit / 1023.0` for precise measurement parity
- Eliminates previous 8-bit quantization errors and mapping discrepancies

**Native Scene Detection:**
- Real-time histogram-based scene detection using Sum of Absolute Differences
- Threshold of 15 optimized for HDR content sensitivity
- Direct histogram comparison without external process parsing
- Integrated processing during frame analysis for efficiency

### Hardware Acceleration Support

**Multi-Platform Acceleration:**
- **CUDA**: NVIDIA GPU acceleration with `hevc_cuvid` decoder
- **VAAPI**: Intel/AMD GPU acceleration on Linux systems
- **VideoToolbox**: Native macOS hardware acceleration (Intel and Apple Silicon)
- **Graceful Fallback**: Automatic software decoding when hardware acceleration unavailable

### Performance Characteristics

**Maintained Performance:**
- ~13-14 FPS average processing speed (consistent with previous implementation)
- Reduced memory overhead from eliminating external processes
- Direct frame access eliminates pipe buffer management
- Single-threaded processing with better error handling and reliability

## Prerequisites

- **Rust Toolchain**: Install from [rustup.rs](https://rustup.rs/).
- **FFmpeg Development Libraries**: Required for `ffmpeg-next` crate compilation.
  - **macOS**: `brew install ffmpeg pkg-config`
  - **Ubuntu/Debian**: `sudo apt install libavformat-dev libavcodec-dev libavutil-dev libavfilter-dev libavdevice-dev pkg-config`
  - **Windows**: Install FFmpeg development libraries or use vcpkg
- **Build Tools**: C compiler and build tools for native dependencies.
  - **macOS**: Xcode Command Line Tools (`xcode-select --install`)
  - **Linux**: `build-essential` package
  - **Windows**: Visual Studio Build Tools or MSVC

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

The native pipeline supports GPU-accelerated video decoding through the `ffmpeg-next` crate's hardware acceleration capabilities. Hardware acceleration provides improved performance while maintaining the accuracy benefits of native 10-bit processing.

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

**Hardware Acceleration Features:**
- **CUDA**: Attempts `hevc_cuvid` decoder for NVIDIA GPUs with automatic fallback
- **VAAPI/VideoToolbox**: Software decoder with hardware acceleration hints
- **Graceful Fallback**: Automatically uses software decoding if hardware acceleration fails
- **Native Integration**: Hardware acceleration handled entirely within the native pipeline

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

## The Native Pipeline Algorithm

The native pipeline processes video in a single integrated pass for maximum efficiency and accuracy:

1. **Native Video Initialization**: Opens video files using `ffmpeg-next` and extracts metadata (resolution, frame rate, duration) directly from stream parameters.

2. **Integrated Scene Detection & Frame Analysis**: Processes each frame through the native pipeline:
   - Decodes frames using hardware-accelerated or software decoders
   - Scales frames to YUV420P10LE format for consistent 10-bit analysis
   - Extracts Y-plane (luminance) data directly as 16-bit values
   - Performs real-time scene detection using histogram comparison (Sum of Absolute Differences)
   - Calculates accurate PQ values using native 10-bit precision: `pq_value = luma_10bit / 1023.0`
   - Generates 256-bin PQ-based luminance histograms

3. **Scene Statistics Computation**: Calculates per-scene statistics from the collected frame data including peak nits and average PQ values.

4. **Optimizer Pass (Optional)**: If enabled, performs a final pass over the frame data using scene-based statistics, rolling averages, and highlight knee detection to calculate optimal target_nits for each frame.

## Roadmap & Contributing

This tool is a robust V2.0 featuring a fully native Rust pipeline with `ffmpeg-next` integration for maximum accuracy and reliability. Future enhancements may include:

- Enhanced hardware acceleration with proper device context setup for VAAPI/VideoToolbox
- Parallel frame processing for multi-core performance improvements
- Support for additional HDR formats (HDR10+, Dolby Vision)
- SIMD optimizations for histogram calculations
- Real-time processing capabilities and streaming input support
- Automated black bar detection for even more accurate APL measurements
- User-configurable parameters for optimizer heuristics and scene detection

Contributions are welcome! Please feel free to open an issue or submit a pull request.

## Acknowledgements & Dependencies

This project is built with the help of several excellent open-source libraries. We extend our gratitude to their authors and contributors.

- **`ffmpeg-next`**: Native Rust bindings for FFmpeg, enabling direct video processing without external processes.
  - **License:** MIT
  - **Provides:** Native video decoding, hardware acceleration, and frame access
- **`madvr_parse`**: The core library used for reading and writing madVR measurement files. This project would not be possible without it.
  - **License:** MIT
  - **Copyright:** (c) 2025 quietvoid
- **`clap`**: For robust and user-friendly command-line argument parsing.
- **`anyhow`**: For simple and effective error handling and context propagation.

## License

This project is licensed under the MIT License.