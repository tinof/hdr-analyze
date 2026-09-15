# HDR-Analyze

[![CI](https://github.com/tinof/hdr-analyze/actions/workflows/ci.yml/badge.svg)](https://github.com/tinof/hdr-analyze/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-stable-blue.svg)](https://github.com/rust-lang/rust)

HDR-Analyze measures HDR10 and HDR10+ video and converts it to Dolby Vision Profile 8.1 without re-encoding the picture.

HDR-Analyze is an independent project, not affiliated with or endorsed by Dolby Laboratories, and Dolby Vision is a Dolby trademark ([provenance](docs/PROVENANCE.md)). Current version: 0.4.0.

## What you get

- HDR10 and HDR10+ files become Profile 8.1 MKVs with the video stream copied, not re-encoded. HLG and Profile 7 FEL sources take a re-encode path (see the table below).
- An open-source analysis engine that measures decoded pixels and writes per-scene L1 plus L2, L6, L9 and L11 metadata. `dovi_tool` generates and injects the RPU.
- Direct analysis of the compressed source. You do not need a ProRes or raw intermediate.
- Optional NVDEC decode and CUDA analysis, at approximately 12× the analysis throughput of this project's CPU path on the tested configuration ([details](docs/PERFORMANCE.md)). Release binaries are CPU-only; GPU analysis needs a source build.
- Published validation against synthetic references and Dolby-generated metadata, including an open gap on grainy content ([docs/VALIDATION.md](docs/VALIDATION.md)).

It also reuses existing HDR10+ metadata, audits RPUs with `mkvdovi inspect`, rebuilds metadata on existing Dolby Vision files with `--mdfix`, and runs on the CPU under Linux, macOS and Windows.

## Quick start

Install the release binaries (Linux x64, macOS Intel and Apple Silicon, Windows x64):

```bash
curl -fsSL https://github.com/tinof/hdr-analyze/releases/latest/download/install.sh | bash
```

On Windows, download the zip from the [latest release](https://github.com/tinof/hdr-analyze/releases/latest) and keep the bundled FFmpeg DLLs next to the `.exe` files. A PowerShell install script also exists, but it has not been tested on Windows yet ([details](docs/INSTALLATION.md)).

Convert a file:

```bash
mkvdovi movie.mkv --keep-source
```

> **The source file is deleted after a successful conversion** unless you pass `--keep-source`.
> Inputs that already carry Dolby Vision metadata, and all `--mdfix` runs, keep the source.

Install these tools separately and put them on `PATH`. `mkvdovi` checks for all but the last one at startup:

| Tool | Needed for |
|---|---|
| `ffmpeg` | every conversion |
| `mkvmerge` (MKVToolNix) | final MKV packaging |
| [`dovi_tool`](https://github.com/quietvoid/dovi_tool/releases) 2.3.2 or newer | RPU generation and injection (2.3.4+ reads MKV directly) |
| `mediainfo` (recommended) or `ffprobe` | stream inspection. MediaInfo supplies the L6 and L9 source values; with `ffprobe` alone they fall back to defaults. |
| [`hdr10plus_tool`](https://github.com/quietvoid/hdr10plus_tool/releases) | HDR10+ input. An HDR10+ file fails if the tool is missing. |

To build from source instead, run `cargo build --release --workspace`. For GPU analysis on an NVIDIA host, add `cargo build --release -p hdr_analyzer_mvp --features cuda` (needs the NVIDIA driver and NVRTC, no `nvcc`). FFmpeg dev libraries, clang and per-OS notes are in [docs/INSTALLATION.md](docs/INSTALLATION.md).

## Compatibility

| Input | Output | Picture re-encoded | Maturity |
|---|---|---|---|
| HDR10 | Profile 8.1, CM v4.0 metadata | No | Measurement comparisons published; playback unvalidated |
| HDR10+ | Profile 8.1, L1 taken from HDR10+ | No | Measurement comparisons published; playback unvalidated |
| HLG | Profile 8.1 after HLG to PQ conversion | Yes | Works, less validated |
| Dolby Vision Profile 7 MEL | Profile 8.1 | No | Works |
| Dolby Vision Profile 7 FEL | Profile 8.1 from composited BL+EL | Yes | Experimental, unvalidated |
| Profile 8 or MEL with `--mdfix` | Profile 8.1 with rebuilt metadata | No | Works; not a guaranteed improvement |

Flag-level detail is in [docs/FORMAT_COMPATIBILITY.md](docs/FORMAT_COMPATIBILITY.md) and [docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md).

## Performance and validation

On an RTX 4070 with a 4K source, CUDA analysis ran at 213 fps against 17 fps for the CPU path, with identical L1 output. Several test conditions were not recorded; [docs/PERFORMANCE.md](docs/PERFORMANCE.md) lists them and gives a reproduction recipe.

On synthetic patterns, measured peaks land within 0.25 of one 12-bit PQ code. On an asset with Dolby-generated reference metadata, max-RGB peaks read 12.8 codes high on average. HDR10+-derived L1 matched Dolby's v4 analyzer with a per-shot median error of 1 code (max 17), and scene detection matched 13 of 14 authored cuts while emitting 24 cuts in total.

The weak spot is grain. With the default `max` estimator, two grainy real-content assets read +74 and +93 codes hot against the reference. An opt-in grain-rejecting estimator (`--peak-estimator`, see the CLI reference) narrows this but does not close it.

CPU and GPU agreement shows the two paths are consistent. It says nothing about accuracy. The accuracy evidence is in [docs/VALIDATION.md](docs/VALIDATION.md), and the remaining gaps are in [docs/CM_ANALYZE_PARITY.md](docs/CM_ANALYZE_PARITY.md). No playback comparisons have been published yet.

## Why another HDR analyzer?

Dolby provides its own professional tools for this job. If you already use them and the workflow suits you, keep using it. HDR-Analyze exists for other situations:

- You have HDR10 or HDR10+ files and want Profile 8.1 output from a command-line conversion.
- Reading and changing the analysis code matters to you, down to how each metadata value is computed.
- Your source is a delivered HEVC file and you would rather not export an intermediate first.
- You have an NVIDIA GPU and want to use it for the measurement pass.
- You want published error figures, including where the output is still wrong.

## Limitations

- The default peak estimator is sensitive to film grain (see above). The grain-rejecting estimator is opt-in and CPU-only.
- Profile 7 FEL conversion is experimental and re-encodes the picture. The research notes are in [docs/experimental/](docs/experimental/README.md).
- Hardware decode in the analyzer is CUDA only. VAAPI and VideoToolbox requests fall back to software decode.
- There is no Profile 5 output, no lossless FEL path and no XML metadata export.
- The metadata is format-compatible with CM v4.0. It is produced by this project's own measurements and does not implement Dolby's analysis algorithm.
- HLG is converted to PQ, so the HLG signal is not preserved.
- Linux ARM64 has no release archive; build it from source.
- Playback on real displays has not been compared and published.

## Documentation

These files ship in each release archive next to this README. They are also [online](https://github.com/tinof/hdr-analyze/tree/main/docs).

- [docs/INSTALLATION.md](docs/INSTALLATION.md): release archives, runtime tools, source and CUDA builds
- [docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md): every flag for `mkvdovi`, `hdr_analyzer_mvp` and `verifier`
- [docs/FORMAT_COMPATIBILITY.md](docs/FORMAT_COMPATIBILITY.md): conversion paths, HDR10+ mapping, `--verify`
- [docs/VALIDATION.md](docs/VALIDATION.md): accuracy measurements and method
- [docs/PERFORMANCE.md](docs/PERFORMANCE.md): benchmark record and how to reproduce it
- [docs/CM_ANALYZE_PARITY.md](docs/CM_ANALYZE_PARITY.md): known analysis gaps
- [docs/TECHNICAL_REFERENCE.md](docs/TECHNICAL_REFERENCE.md): analyzer internals
- [docs/PROVENANCE.md](docs/PROVENANCE.md): what the implementation is derived from
- [docs/experimental/README.md](docs/experimental/README.md): FEL compositing and other prototypes
- [ROADMAP.md](ROADMAP.md) and [CHANGELOG.md](CHANGELOG.md)

## Contributing

This is a personal research project shared under the MIT license, with no support commitment. Issues and pull requests are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md). Run the same gates as CI before you commit:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Acknowledgements

quietvoid wrote [`dovi_tool`](https://github.com/quietvoid/dovi_tool), [`hdr10plus_tool`](https://github.com/quietvoid/hdr10plus_tool) and the MIT-licensed `madvr_parse` library. Decoding goes through [FFmpeg](https://ffmpeg.org/) via `ffmpeg-next`, and packaging through [MKVToolNix](https://mkvtoolnix.download/).

## License

MIT.
