# HDR-Analyze

[![CI](https://github.com/tinof/hdr-analyze/actions/workflows/ci.yml/badge.svg)](https://github.com/tinof/hdr-analyze/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-stable-blue.svg)](https://github.com/rust-lang/rust)

Convert any HDR10, HLG, or HDR10+ video to dynamic metadata — entirely free and open-source.

HDR-Analyze reads raw 10-bit pixel data frame-by-frame, computes precise per-frame luminance
measurements, and generates dynamic metadata (`.bin`) compatible with existing open-source tools
like `dovi_tool`. The companion `mkvdovi` tool then packages the result into a final MKV with
dynamic tone-mapping metadata.

**Workflow:** `HDR10/HLG MKV → hdr_analyzer_mvp → measurements.bin → mkvdovi → Dynamic HDR MKV`

> **Renamed in v0.3.0:** the converter formerly called `mkvdolby` is now **`mkvdovi`**
> (see [docs/PROVENANCE.md](docs/PROVENANCE.md) for why). For one release, archives ship a
> transitional `mkvdolby` copy and leftover `mkvdolby_temp_*` directories still resume.

For HDR10+ inputs, `mkvdovi` extracts the source HDR10+ metadata directly and passes it to
`dovi_tool`; it does not run `hdr_analyzer_mvp` unless HDR10+ metadata extraction fails and the
workflow falls back to HDR10 analysis.

## Documentation

- **[docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md)** — complete flag reference for all three tools.
- **[docs/DOLBY_VISION.md](docs/DOLBY_VISION.md)** — HDR10+ peak mapping, CM v4.0 metadata, verification.
- **[docs/TECHNICAL_REFERENCE.md](docs/TECHNICAL_REFERENCE.md)** — analysis internals and research.
- **[docs/PROVENANCE.md](docs/PROVENANCE.md)** — clean-room statement: the public standards this is built from.
- **[ROADMAP.md](ROADMAP.md)** · **[CHANGELOG.md](CHANGELOG.md)** · **[CONTRIBUTING.md](CONTRIBUTING.md)**

## Workspace Members

This is a Rust workspace with three shipped binaries:

- **`hdr_analyzer_mvp`** — HDR analysis engine; processes video and writes madVR-compatible `.bin`
  measurement files.
- **`mkvdovi`** — native HDR10/HDR10+/HLG → Dolby Vision Profile 8.1 (CM v4.0) conversion orchestrator.
- **`verifier`** — utility for reading, validating, and inspecting `.bin` measurement files.

## Key Features

- **Native video processing** via `ffmpeg-next` for direct, zero-copy access to high-bit-depth pixel
  data — precise per-pixel 10-bit luminance analysis instead of parsing external tool logs.
- **Accurate per-frame analysis**: MaxCLL and APL from 10-bit YUV420P10LE frames, with active-video
  crop detection to ignore black bars (see [Known Limitations](#known-limitations)).
- **v5/v6 luminance histograms**: 256-bin histogram with SDR/HDR split (64 + 192) and mid-bin averaging.
- **Native scene detection**: histogram-distance-based cut detection with a configurable threshold.
- **Dynamic metadata optimizer**: per-frame `target_nits` from a rolling average, 99th-percentile knee
  detection, scene-aware blending/resets, and bidirectional EMA smoothing (on by default).
- **Noise robustness**: percentile-based peaks (P99/P99.9), per-bin EMA smoothing, optional temporal
  median filtering and pre-analysis denoising for grainy sources.
- **Native HLG workflow**: auto-detects ARIB STD-B67 and converts to PQ histograms in-memory
  (`--hlg-peak-nits`, default 1000).
- **Dolby Vision CM v4.0** output by default (L1/L2/L6/L9/L11/L254) via `mkvdovi`.
- **Cross-platform**: software decoding everywhere; optional CUDA attempt on NVIDIA with graceful
  fallback. ARM64-tuned (NEON, `--sample-rate`/`--downscale` give 3–4× throughput on CPU-limited systems).

See [docs/TECHNICAL_REFERENCE.md](docs/TECHNICAL_REFERENCE.md) for implementation details (PQ-domain
histogram, scene detection, crop detection) and [docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md) for
hardware-acceleration and throughput options.

## Project Status

This is a personal research project, shared as-is under the MIT license. Issues and pull requests are
welcome, but there is no support SLA. Please do not expect production-level maintenance.

## Prerequisites

- **Rust toolchain**: install from <https://rustup.rs/> (the repo pins the stable channel via
  `rust-toolchain.toml`).
- **FFmpeg development libraries** (for compiling `ffmpeg-next`):
  - macOS: `brew install ffmpeg pkg-config`
  - Ubuntu/Debian: `sudo apt install libavformat-dev libavcodec-dev libavutil-dev libavfilter-dev libavdevice-dev libswscale-dev pkg-config`
  - Windows: install FFmpeg dev libraries or use vcpkg
- **Build tools**: C compiler / build tools (Xcode CLT on macOS, `build-essential` on Linux, MSVC on Windows).
- **External tools (NOT included)** — install and place in your `PATH`:
  - [`dovi_tool`](https://github.com/quietvoid/dovi_tool/releases): required for RPU generation/injection.
    **2.3.2+ recommended** (fixes duplicated end-padding in `inject-rpu`).
  - [`hdr10plus_tool`](https://github.com/quietvoid/hdr10plus_tool/releases): required for HDR10+ inputs.
  - `mkvmerge` (from [MKVToolNix](https://mkvtoolnix.download/)): required by `mkvdovi` for final MKV packaging.

## Installation & Setup

Clone and build the workspace (compiles all three binaries):

```bash
git clone https://github.com/tinof/hdr-analyze.git
cd hdr-analyze
cargo build --release --workspace
```

Binaries land in `target/release/`:

- Analyzer: `./target/release/hdr_analyzer_mvp`
- Converter: `./target/release/mkvdovi`
- Verifier: `./target/release/verifier`

After a `git pull`, **always rebuild** so the binaries match the source:

```bash
git pull
cargo build --release --workspace   # CRITICAL
```

Optional local install from a source checkout:

```bash
install -Dm755 target/release/hdr_analyzer_mvp "$HOME/.local/bin/hdr_analyzer_mvp"
install -Dm755 target/release/mkvdovi        "$HOME/.local/bin/mkvdovi"
install -Dm755 target/release/verifier        "$HOME/.local/bin/verifier"
install -Dm755 scripts/mkvdovi_hifi_workflow.sh "$HOME/.local/bin/mkvdovi_hifi_workflow.sh"
```

`mkvdovi_hifi_workflow.sh` is a specialist comparison helper for regenerating files that already
contain Dolby Vision metadata. Use `mkvdovi` directly for HDR10+ sources.

Prebuilt binaries for **Windows**, **macOS** (Intel & Apple Silicon), and **Linux** are published on
the [Releases page](https://github.com/tinof/hdr-analyze/releases).

## Usage

The examples below cover the common paths. For every flag and default, see
**[docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md)**.

### Analyzer

```bash
# Standard analysis (optimizer on by default)
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "measurements.bin"

# v6 output with explicit target peak
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "out_v6.bin" --madvr-version 6 --target-peak-nits 1000

# Tune scene sensitivity / speed up analysis
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --scene-threshold 0.25 --downscale 2

# Full-frame analysis (disable crop detection)
./target/release/hdr_analyzer_mvp -i "video.mkv" -o "out.bin" --no-crop
```

→ Noise-robustness, optimizer, and HLG flags: [docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md#hdr_analyzer_mvp).

### mkvdovi (conversion tool)

Converts HDR10/HDR10+/HLG input to a Profile 8.1 MKV with CM v4.0 metadata.

> **By default, `mkvdovi` deletes the source file after a successful conversion** (and removes temp
> artifacts). Pass `--keep-source` to keep it.
>
> An interrupted run (e.g. a dropped SSH session) keeps its `mkvdovi_temp_*` directory and prints a
> resume hint — just re-run the same command to **resume** from the last completed step (`--no-resume`
> forces a clean run). For long conversions, run under `tmux`/`nohup` so a disconnect can't kill them.

```bash
mkvdovi                                  # convert all .mkv files in the current directory
mkvdovi "input.mkv"                      # convert a specific file
mkvdovi "input.mkv" --keep-source --verify   # recommended first run (A/B safe, validated)
mkvdovi "input.mkv" --analysis-quality accurate   # fast | balanced (default) | accurate
mkvdovi "input.mkv" --encoder videotoolbox        # ~10× faster HLG→PQ on Apple Silicon
mkvdovi "input.mkv" --no-resume                   # ignore a leftover temp dir, start clean
```

→ HDR10+ peak mapping, CM v4.0 metadata, and verification details:
[docs/DOLBY_VISION.md](docs/DOLBY_VISION.md). Full flag list:
[docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md#mkvdovi).

### Verifier

```bash
./target/release/verifier "measurements.bin"
```

Reports version/flags, scene & frame stats, peak brightness and avg PQ, histogram integrity, and
`target_nits` stats (if the optimizer was enabled).

## Known Limitations

- **Crop detection uses a single frame.** The active-area crop is detected **once**, on the first
  frame selected for analysis, and reused for the entire stream. Early black frames, fade-ins,
  full-screen studio logos, pre-roll, or variable-aspect-ratio content (e.g. intermittent IMAX
  scenes) can therefore produce an incorrect crop. Use `--no-crop` to analyze the full frame.
  Stream-level multi-frame probing (and later per-scene crop) is tracked in
  [issue #3](https://github.com/tinof/hdr-analyze/issues/3).
- **HLG/VAAPI/VideoToolbox decode** currently fall back to software decoding; proper device contexts
  are planned (see [Roadmap](#roadmap)).
- **v6 per-gamut peaks** (`peak_pq_dcip3`, `peak_pq_709`) are approximated from BT.2020. These are a
  **madVR measurement-file** feature only and are **not used by the Dolby Vision conversion** (which
  uses the v5 file plus the BT.2020 peak and histogram), so the approximation does not affect DV output;
  it matters only for a standalone v6 `.bin` consumed by madVR. PQ max-RGB peak measurement is now
  implemented; accurate target-gamut transforms remain a follow-up (see [Roadmap](#roadmap)).

## Quick Start Validation

```bash
cargo build --release --workspace
./target/release/hdr_analyzer_mvp -i sample_hdr10.mkv -o measurements_v5.bin
./target/release/verifier measurements_v5.bin
```

Expected: version 5/6 as selected; flags 2 (no optimizer) or 3 (optimizer on); 256-bin histograms
summing ≈ 100; PQ values in `[0,1]`; scenes valid and within frame range.

## Roadmap

See **[ROADMAP.md](ROADMAP.md)**. Near-term highlights: proper VAAPI/VideoToolbox device contexts
and hardware frame transfer, SIMD histogram optimizations, and more robust crop detection (issue #3).

## Contributing & Quality Gates

Contributions welcome — see **[CONTRIBUTING.md](CONTRIBUTING.md)**. Before committing, run the local
gates (also enforced in CI):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Optional pre-commit hooks (config in `.pre-commit-config.yaml`):

```bash
pipx install pre-commit            # or: pip install --user pre-commit
pre-commit install                 # fmt + clippy on commit
pre-commit install --hook-type pre-push   # quick tests on push
```

## Acknowledgements

- **quietvoid** — for `dovi_tool`, `hdr10plus_tool`, and the MIT-licensed `madvr_parse` library.
- **The Doom9 and MakeMKV forum communities** — for the collective research and documentation of
  HDR formats and Dolby Vision packaging that made an open implementation possible.
- `ffmpeg-next`, `clap`, `anyhow` and the wider Rust ecosystem.

## License

MIT License.

## What This Is NOT

- Does not include, redistribute, or reverse-engineer any Dolby Laboratories proprietary code,
  lookup tables, CM v4.0 trims, or binary blobs.
- Does not bypass, circumvent, or interfere with any DRM or content-protection mechanism.
- Not an official Dolby or HDR10+ Technologies product; no trademarks claimed.
- The analyzer outputs generic per-frame luminance data. Final packaging into a playback-compatible
  stream is done by `dovi_tool` and `mkvmerge`, which the user installs independently.

## Legal & Trademarks

This software is a research project for video analysis and is not an official product of Dolby
Laboratories.

- **Dolby Vision** is a trademark of Dolby Laboratories.
- **HDR10+** is a trademark of HDR10+ Technologies, LLC.

This project is not affiliated with, endorsed by, or sponsored by Dolby Laboratories or HDR10+
Technologies, LLC. Reference to these standards is strictly for compatibility and interoperability
purposes.
