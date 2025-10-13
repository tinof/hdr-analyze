# HDR Analyzer MVP - Native Pipeline

A high-performance HDR10 video analysis tool built with native Rust video processing using `ffmpeg-next`. This tool generates dynamic metadata for Dolby Vision conversion by processing HDR video files to create madVR-compatible measurement files with per-frame and per-scene analysis.

## 🚀 Native Pipeline Architecture

**NEW in v2.0**: Complete refactor to a fully native Rust pipeline using `ffmpeg-next`, eliminating external FFmpeg processes and providing direct access to high-bit-depth video data.

### Key Architecture Benefits

- **Native Video Processing**: Direct `ffmpeg-next` integration eliminates external process overhead
- **10-bit Precision**: Direct access to YUV420P10LE frame data for accurate PQ luminance mapping
- **Memory Efficiency**: Direct frame access without pipe buffers or external process coordination
- **Type Safety**: Compile-time guarantees for video processing operations
- **Reliability**: Proper Rust error handling and no external process management

### Critical Accuracy Improvements

The native pipeline provides measurement parity through:

- **Direct 10-bit Access**: Native access to 10-bit luma values (0-1023) from YUV420P10LE frames
- **Correct PQ Mapping**: `pq_value = luma_10bit / 1023.0` where 10-bit values directly correspond to PQ curve
- **Eliminated Quantization**: No more 8-bit intermediate conversions or mapping errors
- **Native Scene Detection**: Real-time histogram-based algorithm using Sum of Absolute Differences

### Performance Characteristics

- **Maintained Speed**: ~13-14 FPS average processing (consistent with previous implementation)
- **Reduced Overhead**: Eliminated external process spawning and pipe management
- **Better Resource Usage**: Direct memory access and single-threaded processing
- **Hardware Acceleration**: Native support for CUDA, VAAPI, and VideoToolbox

## Features

- **Native Video Processing**: Built with `ffmpeg-next` for direct access to high-bit-depth video data
- **Precision Frame Analysis**: Peak brightness (MaxCLL) and Average Picture Level (APL) with native 10-bit accuracy
- **Accurate PQ-Based Histogram**: 256-bin luminance histogram using direct 10-bit to PQ conversion
- **Native Scene Detection**: Real-time histogram-based scene segmentation using Sum of Absolute Differences
- **Advanced Dynamic Optimizer**: Optional per-frame target nits generation with:
  - 240-frame rolling average for temporal smoothing
  - 99th percentile highlight knee detection
  - Scene-aware heuristics for artistic intent preservation
  - Bidirectional EMA smoothing enabled by default for stable target_nits output
- **Hardware Acceleration**: Native support for CUDA, VAAPI, and VideoToolbox with graceful fallback
- **Native HLG Analysis**: Auto-detects ARIB STD-B67 transfers and converts to PQ histograms using configurable peak luminance (`--hlg-peak-nits`, default 1000 nits)
- **Professional Output**: madVR-compatible `.bin` measurement files with measurement parity accuracy

## Prerequisites

- **Rust Toolchain**: Install from [rustup.rs](https://rustup.rs/)
- **FFmpeg Development Libraries**: Required for `ffmpeg-next` crate compilation
  - **macOS**: `brew install ffmpeg pkg-config`
  - **Ubuntu/Debian**: `sudo apt install libavformat-dev libavcodec-dev libavutil-dev libavfilter-dev libavdevice-dev pkg-config`
  - **Windows**: Install FFmpeg development libraries or use vcpkg
- **Build Tools**: C compiler and build tools for native dependencies
  - **macOS**: Xcode Command Line Tools (`xcode-select --install`)
  - **Linux**: `build-essential` package
  - **Windows**: Visual Studio Build Tools or MSVC

## Installation

```bash
# From the hdr_analyzer_mvp directory
cargo build --release

# Or from workspace root
cargo build --release -p hdr_analyzer_mvp
```

## Usage

### Basic Analysis

```bash
# Standard analysis (optimizer + smoothing enabled by default)
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "measurements.bin"

# Disable optimizer/smoothing if you need raw measurements
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "measurements.bin" --disable-optimizer --target-smoother off
```

### Hardware Acceleration (Recommended)

**Native hardware acceleration with automatic fallback:**

```bash
# NVIDIA GPUs (CUDA) - Attempts hevc_cuvid decoder
./target/release/hdr_analyzer_mvp --hwaccel cuda -i "video.mkv" -o "measurements.bin"

# Intel/AMD GPUs (Linux VAAPI) - Software decoder with hardware hints
./target/release/hdr_analyzer_mvp --hwaccel vaapi -i "video.mkv" -o "measurements.bin"

# macOS (VideoToolbox) - Software decoder with hardware hints
./target/release/hdr_analyzer_mvp --hwaccel videotoolbox -i "video.mkv" -o "measurements.bin"
```

### Using Cargo Run

```bash
# From workspace root with hardware acceleration
cargo run -p hdr_analyzer_mvp --release -- --hwaccel cuda -i "video.mkv" -o "measurements.bin"

# Tweak smoothing or HLG peak during development
cargo run -p hdr_analyzer_mvp --release -- -i "video.mkv" -o "measurements.bin" --smoother-alpha 0.15 --hlg-peak-nits 1200
```

## Command Line Arguments

- `-i, --input <PATH>`: Input HDR video file path
- `-o, --output <PATH>`: Output .bin measurement file path  
- `--disable-optimizer`: Disable dynamic metadata optimizer (enabled by default)
- `--hwaccel <TYPE>`: Hardware acceleration (`cuda`, `vaapi`, `videotoolbox`)
- `--target-smoother <off|ema>`: Control target_nits smoothing (default `ema`)
- `--smoother-alpha <float>` / `--smoother-bidirectional`: Tune bidirectional EMA smoothing
- `--hlg-peak-nits <float>`: Override peak luminance used for HLG → PQ conversion (default 1000 nits)

## Performance Characteristics

### Native Pipeline Benefits
- **Processing Speed**: ~13-14 FPS average (maintained from previous implementation)
- **Memory Usage**: Reduced overhead from eliminating external processes
- **Reliability**: No external process coordination or pipe buffer management
- **Accuracy**: Direct 10-bit processing eliminates quantization errors

### Hardware Acceleration Impact
- **CUDA**: Improved decoding performance with `hevc_cuvid` when available
- **VAAPI/VideoToolbox**: Software decoding with hardware acceleration hints
- **Fallback**: Graceful degradation to software decoding maintains compatibility
- **Error Handling**: Proper Rust error propagation and recovery

## Technical Details

### Native Pipeline Architecture
```rust
// Native video processing flow
1. ffmpeg::init() -> Initialize FFmpeg library
2. format::input() -> Open video file natively
3. find_best_video_stream() -> Locate primary video stream
4. setup_decoder() -> Configure hardware/software decoder
5. setup_scaler() -> Convert to YUV420P10LE format
6. process_frames() -> Direct frame analysis loop
```

### Frame Processing Pipeline
1. **Native Video Decode**: Direct `ffmpeg-next` decoding with hardware acceleration
2. **Format Conversion**: Scale frames to YUV420P10LE for consistent 10-bit analysis
3. **Direct Y-Plane Access**: Extract 10-bit luminance data directly from frame memory
4. **Native Scene Detection**: Real-time histogram comparison using Sum of Absolute Differences
5. **Accurate PQ Conversion**: `pq_value = luma_10bit / 1023.0` for precise mapping
6. **Histogram Generation**: 256-bin PQ-based luminance distribution from native data

### Hardware Acceleration Implementation
- **NVIDIA CUDA**: Attempts `hevc_cuvid` decoder with automatic software fallback
- **Linux VAAPI**: Software decoder with hardware acceleration context
- **macOS VideoToolbox**: Software decoder with VideoToolbox acceleration hints
- **Error Recovery**: Graceful fallback ensures compatibility across all systems

## Native Algorithm Overview

1. **Native Video Initialization**: Direct video file opening and metadata extraction using `ffmpeg-next`
2. **Integrated Processing**: Single-pass frame analysis with:
   - Native hardware/software decoding
   - Real-time scene detection using histogram comparison (threshold: 15)
   - Direct 10-bit Y-plane luminance extraction
   - Accurate PQ conversion: `pq_value = luma_10bit / 1023.0`
   - 256-bin PQ-based histogram generation
3. **Scene Statistics**: Computation of per-scene peak nits and average PQ from frame data
4. **Optimizer Pass** (Optional): Dynamic target nits generation using:
   - 240-frame rolling average for temporal smoothing
   - 99th percentile highlight knee detection
   - Scene-aware heuristics for artistic intent preservation

## Output Format

Generates madVR-compatible `.bin` files containing:
- Header with video metadata and processing flags
- Per-scene statistics (start/end frames, peak nits, average PQ)
- Per-frame measurements (peak PQ, average PQ, luminance histogram)
- Optional dynamic target nits (when optimizer enabled)

## Troubleshooting

### Build Issues
- **FFmpeg libraries not found**: Install FFmpeg development libraries (see Prerequisites)
- **Compilation errors**: Ensure C compiler and build tools are installed
- **Linking errors**: Verify pkg-config can find FFmpeg libraries

### Hardware Acceleration Issues
- **CUDA**: NVIDIA drivers required, but CUDA toolkit not necessary for runtime
- **VAAPI**: Available on Linux with Intel/AMD GPUs, verify with `vainfo`
- **VideoToolbox**: Available on macOS 10.11+ with compatible hardware
- **Automatic Fallback**: Tool gracefully falls back to software decoding on any hardware acceleration failure

### Performance Tips
- Use hardware acceleration (`--hwaccel`) when available for improved decoding performance
- Process videos from fast storage (SSD) when possible
- Ensure sufficient RAM for large 4K/8K files
- Native pipeline eliminates external process overhead

### Common Issues
- **Build failures**: Ensure all prerequisites are installed (FFmpeg dev libraries, build tools)
- **Runtime errors**: Native pipeline provides better error messages and recovery
- **Unsupported format**: Tool works best with H.264/H.265 HDR content
- **Memory errors**: Native pipeline uses less memory than external process approach

## Dependencies

- **ffmpeg-next**: Native Rust bindings for FFmpeg, enabling direct video processing
- **madvr_parse**: Core library for madVR measurement file format
- **clap**: Command-line argument parsing with derive macros
- **anyhow**: Error handling and context propagation

## Contributing

Contributions welcome! Areas for improvement:
- Enhanced hardware acceleration with proper device context setup
- Parallel frame processing for multi-core performance
- SIMD optimizations for histogram calculations
- Support for additional HDR formats (HDR10+, Dolby Vision)
- Real-time processing capabilities
- User-configurable optimizer parameters
- Automated black bar detection

## License

MIT License - See LICENSE file for details.
