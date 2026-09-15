# Installation

HDR-Analyze ships three binaries: `hdr_analyzer_mvp` (analysis), `mkvdovi` (conversion) and
`verifier` (measurement file checks). You can install release archives or build from source.
GPU analysis needs a source build.

## Release binaries

### Install scripts

Linux and macOS:

```bash
curl -fsSL https://github.com/tinof/hdr-analyze/releases/latest/download/install.sh | bash
```

Windows: use the [manual download](#manual-download) and keep the FFmpeg DLLs next to the `.exe` files.

The scripts download the latest release archive for your platform and copy the binaries into an
install directory. Set `INSTALL_DIR` to change it. The defaults are:

| Platform | Default install directory |
|----------|---------------------------|
| Linux, macOS | `$HOME/.local/bin` |
| Windows | `%LOCALAPPDATA%\Programs\hdr-analyze` |

If the directory is not on `PATH`, the script prints the line to add to your shell profile.

A PowerShell script exists for Windows:

```powershell
irm https://github.com/tinof/hdr-analyze/releases/latest/download/install.ps1 | iex
```

The script published with 0.3.0 copies only the three `.exe` files and leaves the FFmpeg DLLs
behind, so `hdr_analyzer_mvp.exe` cannot start unless FFmpeg shared libraries are already on
`PATH`. The 0.4.0 script also copies the DLLs, but it has not been tested on Windows yet. Until it
has, the zip is the recommended way to install on Windows.

### Manual download

Download an archive from the [releases page](https://github.com/tinof/hdr-analyze/releases):

| Platform | Archive |
|----------|---------|
| Linux x64 | `hdr-analyze-<version>-x86_64-unknown-linux-gnu.tar.gz` |
| macOS Intel | `hdr-analyze-<version>-x86_64-apple-darwin.tar.gz` |
| macOS Apple Silicon | `hdr-analyze-<version>-aarch64-apple-darwin.tar.gz` |
| Windows x64 | `hdr-analyze-<version>-x86_64-pc-windows-msvc.zip` |

Each archive contains:

```
hdr-analyze-<version>-<target>/
├── bin/
│   ├── hdr_analyzer_mvp
│   ├── mkvdovi
│   ├── verifier
│   ├── mkvdovi_hifi_workflow.sh          (Linux, macOS)
│   └── FFmpeg LGPL DLLs + FFMPEG-LICENSE.txt   (Windows)
├── docs/                              (the documentation linked from README.md)
├── README.md
├── CHANGELOG.md
├── CONTRIBUTING.md
├── ROADMAP.md
└── LICENSE
```

Extract it and put `bin/` on your `PATH`, or copy its contents to a directory that is already on
`PATH`.

### What release archives do not include

- GPU analysis. Release archives are built with `cargo build --release --workspace`, without the
  `cuda` feature, so the bundled `hdr_analyzer_mvp` analyzes on the CPU only. `mkvdovi` can still
  use NVENC for HLG and FEL re-encodes when your `ffmpeg` has `hevc_nvenc`. For CUDA analysis, see
  [CUDA analysis build](#cuda-analysis-build).
- Linux ARM64. There is no release archive, and `install.sh` stops on that platform. Build from
  source.
- The runtime tools listed in the next section.

## Runtime tools

`mkvdovi` calls external tools. At startup it checks that `ffmpeg`, `mkvmerge`, `dovi_tool`, and
either `mediainfo` or `ffprobe` are on `PATH`, and it stops if one is missing.

| Tool | Needed for | Project page |
|------|------------|--------------|
| `ffmpeg` | Extraction, muxing, HLG and FEL re-encodes | [ffmpeg.org](https://ffmpeg.org/) |
| `mkvmerge` | Final MKV packaging (part of MKVToolNix) | [mkvtoolnix.download](https://mkvtoolnix.download/) |
| `dovi_tool` | RPU generation, injection and inspection | [github.com/quietvoid/dovi_tool](https://github.com/quietvoid/dovi_tool) |
| `mediainfo` | Recommended. Source of MaxCLL, MaxFALL and mastering-display luminance for L6, and of mastering primaries for L9 | [mediaarea.net](https://mediaarea.net/en/MediaInfo) |
| `ffprobe` | Accepted by the startup check in place of MediaInfo. It is used for transfer detection only | Ships with FFmpeg |
| `hdr10plus_tool` | HDR10+ input only | [github.com/quietvoid/hdr10plus_tool](https://github.com/quietvoid/hdr10plus_tool) |

Notes:

- Install MediaInfo for normal conversions. With only `ffprobe`, the startup check passes, but
  `mkvdovi` cannot read the source mastering metadata. L6 then uses default values and L9 assumes
  BT.2020 primaries, each with a warning. `--source-primaries` overrides L9.
- `dovi_tool` 2.3.2 is the minimum version. `mkvdovi` relies on its fix for duplicated end padding
  in `inject-rpu`. The startup check does not verify this version, so check it yourself with
  `dovi_tool --version`.
- `dovi_tool` 2.3.4 or newer lets `mkvdovi` pass the MKV to `dovi_tool` directly and skip a
  full-size HEVC extraction (`--dovi-input auto`, the default). With an older `dovi_tool`,
  `mkvdovi` extracts the stream with `ffmpeg` instead.
- `hdr10plus_tool` is not part of the startup check. If it is missing, an HDR10+ file fails. It
  does not fall back to HDR10 processing.
- On Windows, the DLLs in the zip cover the analyzer only. `mkvdovi` still needs `ffmpeg.exe` on
  `PATH`.
- On macOS, `brew install ffmpeg` provides `ffmpeg` and `ffprobe`. For the other tools, follow the
  install instructions on each project page.

## Build from source

### Requirements

- Rust stable from [rustup.rs](https://rustup.rs/). `rust-toolchain.toml` selects the stable
  channel with `clippy` and `rustfmt`, so rustup installs the right toolchain on the first build.
- FFmpeg development libraries for `ffmpeg-next`. Any FFmpeg from 3.4 through 9.x works. Only
  `libavcodec`, `libavformat`, `libavutil` and `libswscale` are needed. The avfilter, avdevice and
  swresample development packages are not.
- `clang`/`libclang` and `pkg-config`, used by bindgen when it generates the FFmpeg bindings.
- A C toolchain: Xcode Command Line Tools on macOS, `build-essential` on Debian and Ubuntu, MSVC on
  Windows.

### Per-platform setup

These match what CI runs in `.github/actions/setup-ffmpeg`.

Debian and Ubuntu:

```bash
sudo apt install build-essential pkg-config libclang-dev \
  libavcodec-dev libavformat-dev libavutil-dev libswscale-dev
export BINDGEN_EXTRA_CLANG_ARGS="-I/usr/include/$(gcc -dumpmachine)"
```

macOS (Homebrew):

```bash
brew install ffmpeg pkg-config
export BINDGEN_EXTRA_CLANG_ARGS="-I$(brew --prefix)/include"
```

Windows: download an LGPL shared FFmpeg build (CI uses
[BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds), asset `win64-lgpl-shared`). Set
`FFMPEG_DIR` to the extracted folder and `LIBCLANG_PATH` to the LLVM `bin` directory, and keep the
FFmpeg `bin` directory on `PATH` when you run the binaries. vcpkg also works, but it compiles
FFmpeg from source.

### Build

```bash
git clone https://github.com/tinof/hdr-analyze.git
cd hdr-analyze
cargo build --release --workspace
```

The binaries are written to `target/release/`. The release profile uses fat LTO and one codegen
unit, so the link step is slow.

### Linux ARM64

`.cargo/config.toml` sets `-C target-cpu=native` for every target. On `aarch64-unknown-linux-gnu`
it also uses `clang` as the linker with `lld`, so install both:

```bash
sudo apt install clang lld
```

Because of `target-cpu=native`, a binary built on one machine may not run on a CPU with fewer
instruction set extensions. Build on the machine that runs it.

## CUDA analysis build

GPU analysis runs on NVIDIA GPUs on Linux and Windows. Build the analyzer with the `cuda` feature
after the workspace build:

```bash
cargo build --release --workspace
cargo build --release -p hdr_analyzer_mvp --features cuda
```

The order matters. A plain workspace build writes an analyzer without the `cuda` feature to the
same path in `target/release/`, so building the workspace last replaces the CUDA analyzer.

Runtime requirements:

- The NVIDIA driver (`libcuda.so.1` on Linux, `nvcuda.dll` on Windows).
- NVRTC from CUDA 12 or 13 (`libnvrtc.so.12` or `libnvrtc.so.13` on Linux,
  `nvrtc64_120_0.dll`, `nvrtc64_130_0.dll` or `nvrtc64_131_0.dll` on Windows).
- No `nvcc` or CUDA toolkit compiler. The analysis kernel is compiled by NVRTC when the analyzer
  starts.

If a library is missing, `--hwaccel cuda` falls back to CPU analysis.

Check the build:

```bash
target/release/hdr_analyzer_mvp --version
# hdr_analyzer_mvp <version> (+cuda)
```

`mkvdovi` looks for the analyzer in this order: next to the `mkvdovi` executable, then
`target/release/hdr_analyzer_mvp` relative to the current directory, then `PATH`. It enables GPU
analysis only when that analyzer reports `(+cuda)` in `--version` and an NVIDIA GPU is detected.
If you install `mkvdovi` into a directory, put the CUDA analyzer in the same directory.

## Updating

Install script users: run the same one-liner again. It downloads the latest release and
overwrites the installed binaries.

Source users: pull, then rebuild with the same feature flags you used before.

```bash
git pull
./scripts/dev-refresh.sh
```

`scripts/dev-refresh.sh` runs the workspace release build and then, when `nvidia-smi` is present,
rebuilds `hdr_analyzer_mvp` with `--features cuda`, so the CUDA analyzer is not overwritten. It
prints the version of each binary at the end. Without an NVIDIA GPU it is the same as
`cargo build --release --workspace`.

## Checking the install

```bash
mkvdovi --version
hdr_analyzer_mvp --version
dovi_tool --version
```

`mkvdovi` and `hdr_analyzer_mvp` should print the same version. `hdr_analyzer_mvp` prints
`(+cuda)` after the version only for a CUDA build. `dovi_tool` should be 2.3.2 or newer.
