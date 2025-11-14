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

- `--peak-source <max-scl-luminance|histogram|histogram99>`: Controls `dovi_tool generate` peak source for HDR10+.

- `--boost` / `-b`: Convenience preset for darker HDR10+ titles. When enabled and `--peak-source` is left at the default, mkvdolby internally uses `--peak-source histogram99`, which typically yields a brighter Dolby Vision mapping by ignoring extreme highlight outliers during peak detection.

- `--boost-experimental`: Experimental shot-by-shot boost mode for HDR10/HLG sources. When mkvdolby needs to generate measurements itself, this flag tells `hdr_analyzer_mvp` to use its `aggressive` optimizer profile so that per-scene `target_nits` are pushed harder (similar in spirit to Dolby’s `cm_analyzer`). Existing `*_measurements.bin` files are not modified; remove them if you want to regenerate with this mode.
