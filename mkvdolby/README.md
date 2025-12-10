# mkvdolby

A command-line tool to convert HDR10/HDR10+ videos to Dolby Vision Profile 8.1.

## Installation

### Prerequisites

Ensure the following command-line tools are installed and available in your system's `PATH`:
- `ffmpeg`
- `mkvmerge`
- `dovi_tool`
- `hdr10plus_tool` (optional, for HDR10+ sources)
- `hdr_analyzer_mvp` (optional, for generating measurements from HDR10 sources)

### Installing the tool

It is recommended to install this tool using `pipx` to ensure its dependencies are isolated from your system Python environment.

```bash
# Navigate to the root of this project
cd /path/to/mkvdolby

# Install using pipx
pipx install .
```

## Usage

Once installed, the `mkvdolby` command will be available system-wide.

```bash
# Process a single file
mkvdolby /path/to/your/video.mkv

# Process multiple files
mkvdolby file1.mkv file2.mkv

# If no input files are provided, it will process all .mkv files
# in the current directory.
mkvdolby
```

For a full list of options, run:
```bash
mkvdolby --help
```

### Notable options

- `--verify`: After muxing, runs verification:
  - Validates the measurements with our Rust `verifier`.
  - Extracts DV RPU and prints summary via `dovi_tool info`.
  - Cross-checks DV container frame count vs. measurements; fails on mismatch.

- `--hlg-peak-nits <nits>`: For HLG sources, passes the nominal peak luminance to the analyzer (native HLG path) and uses it for the HLG→PQ encode used as the DV base layer.

- `--peak-source <max-scl-luminance|histogram|histogram99>`: Controls `dovi_tool generate` peak source for HDR10+.\n  Default is `histogram99`, which typically produces a brighter, less \"gray\" result by ignoring extreme highlight outliers.

- `--boost` / `-b`: Convenience preset for darker HDR10+ titles when you explicitly set a more conservative `--peak-source`. If `--peak-source` is `max-scl-luminance` or `histogram`, `--boost` switches it to `histogram99`. With the default `histogram99`, this flag has no additional effect.

- `--boost-experimental`: Experimental shot-by-shot boost mode for HDR10/HLG sources. When mkvdolby needs to generate measurements itself, this flag tells `hdr_analyzer_mvp` to use its `aggressive` optimizer profile so that per-scene `target_nits` are pushed harder (similar in spirit to Dolby’s `cm_analyzer`). Existing `*_measurements.bin` files are not modified; remove them if you want to regenerate with this mode.

## Performance

By default, `mkvdolby` uses "Fast Mode" for HDR analysis (when using `hdr_analyzer_mvp`):
- **Sample Rate**: Analyzes every 3rd frame (`--sample-rate 3`).
- **Downscale**: Analyzes at half resolution (`--downscale 2`).

This reduces analysis time by ~4-5x on CPU-limited systems (like ARM64) with negligible impact on the resulting Dolby Vision metadata quality.
