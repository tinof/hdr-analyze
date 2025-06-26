# HDR-Analyze: Dynamic HDR Metadata Generator

A powerful, open-source command-line tool for analyzing HDR10 video files to generate dynamic metadata for Dolby Vision conversion.

This tool implements advanced, research-backed algorithms to analyze video on a per-frame and per-scene basis, creating measurement files that can be used by tools like `dovi_tool` to produce high-quality Dolby Vision Profile 8.1 content from a standard HDR10 source.

## Key Features

- **Accurate Per-Frame Analysis**: Measures key metrics like Peak Brightness (MaxCLL) and Average Picture Level (APL).
- **PQ-Based Histogram**: Generates a 256-bin luminance histogram based on the ST.2084 Perceptual Quantizer (PQ) curve for high-precision analysis.
- **Automated Scene Detection**: Uses `ffmpeg` to intelligently segment the video into scenes for contextual analysis.
- **Advanced Dynamic Metadata Optimizer**: An optional, state-of-the-art optimizer that generates per-frame dynamic target nits for superior tone mapping.
    - **Temporal Smoothing**: Uses a 240-frame rolling average to prevent abrupt changes in brightness and ensure smooth visual transitions.
    - **Highlight Management**: Detects the "highlight knee" (99th percentile) to make intelligent decisions about preserving highlight detail versus overall brightness.
    - **Scene-Aware Heuristics**: Applies different logic for dark, medium, and bright scenes to preserve artistic intent.
- **High Performance**: Optimized for multi-core CPUs and designed to process large 4K/8K files efficiently.
- **Professional Output**: Generates madVR-compatible `.bin` measurement files, ready for use in Dolby Vision workflows.

## Prerequisites

- **Rust Toolchain**: Install from [rustup.rs](https://rustup.rs/).
- **FFmpeg**: Must be installed and available in your system's PATH.

## Installation

Clone the repository and build the release binary:

```bash
git clone https://github.com/your-username/hdr-analyze.git
cd hdr-analyze
cargo build --release
```

The executable will be located at `./target/release/hdr_analyzer_mvp`.

## Usage

The tool is simple to run from the command line.

### Standard Analysis (without optimizer):

```bash
./target/release/hdr_analyzer_mvp -i "path/to/your/video.mkv" -o "measurements.bin"
```

### Analysis with Advanced Optimizer Enabled:

To generate dynamic per-frame target_nits, use the `--enable-optimizer` flag. This is highly recommended for the best quality.

```bash
./target/release/hdr_analyzer_mvp -i "path/to/your/video.mkv" -o "measurements_optimized.bin" --enable-optimizer
```

## Arguments

- `-i, --input <PATH>`: Path to the input HDR video file.
- `-o, --output <PATH>`: Path for the output .bin measurement file.
- `--enable-optimizer`: (Optional) Activates the advanced optimizer to generate dynamic target nits.

## The Algorithm Explained

This tool operates in three distinct phases to ensure the highest quality output:

1. **Scene Detection**: The video is first quickly analyzed to identify the start and end of every scene.
2. **Frame Measurement**: The tool then performs a deep analysis of every single frame, calculating its peak brightness, average brightness, and a detailed 256-bin PQ histogram.
3. **Optimizer Pass (Optional)**: If enabled, a final pass is made over the frame data. Using scene-based statistics, a rolling average of previous frames, and highlight knee detection, it calculates the ideal target_nits for every frame to ensure a smooth, stable, and visually stunning result.

## Roadmap & Contributing

This tool is a robust V1.0, but there is always room for improvement. Future enhancements may include:

- Implementing automated black bar detection for even more accurate APL measurements.
- Using more advanced color science for luminance calculations (e.g., BT.2020 coefficients).
- Allowing user-configurable parameters for the optimizer heuristics.

Contributions are welcome! Please feel free to open an issue or submit a pull request.

## License

This project is licensed under the MIT License.