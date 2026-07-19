# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> Single source of truth for AI agents in this repo. `AGENTS.md` points here. Rust style and lint policy live in the "Lint/style policy" section below — there is no separate Rust guidelines pack.

## What this repo actually is

- Rust workspace (`resolver = "2"`) with **three shipped binaries**: `hdr_analyzer_mvp` (HDR10 analysis → PQ histograms + DV L1 metadata), `mkvdovi` (MKV container + Dolby Vision metadata injection, CM v4.0), `verifier` (MadVR / RPU measurement validation).
- `tools/compare_baseline` is a separate utility crate, **excluded** from the workspace. Build/run it explicitly with `--manifest-path tools/compare_baseline/Cargo.toml`.
- Release profile is tuned: `lto = "fat"`, `codegen-units = 1`, `strip = true`, `panic = "abort"` — release builds are slow to link; expect it.

## Toolchain and platform quirks

- `rust-toolchain.toml` pins `channel = "stable"` (not a fixed version number) with components `clippy` + `rustfmt` and explicit cross targets. CI uses `dtolnay/rust-toolchain@stable`.
- `.cargo/config.toml` sets `-C target-cpu=native` globally; on Linux ARM64 (`aarch64-unknown-linux-gnu`) it also forces `clang` + `lld`. This host is Oracle ARM/Ampere. Don't remove unless you mean to change perf/link behavior.
- `ffmpeg-next` is used, so local/CI builds need FFmpeg dev libs + `clang`/`libclang` and `BINDGEN_EXTRA_CLANG_ARGS` configured (see `ci.yml` for the exact apt/brew/vcpkg packages and env per OS).

## High-signal commands (use these exact forms)

- Fast local quality gate (matches pre-commit + CI lint intent):
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace --verbose`
- Build all release binaries: `cargo build --release --workspace`
- CUDA-enabled analyzer (NVIDIA hosts): `cargo build --release -p hdr_analyzer_mvp --features cuda` — also lint it with `cargo clippy -p hdr_analyzer_mvp --all-targets --features cuda -- -D warnings` (CI only covers the default feature set)
- Test one crate: `cargo test -p hdr_analyzer_mvp` (or `-p mkvdovi`, `-p verifier`)
- Run a single test by name: `cargo test -p <crate> -- <test_name>`
- Run one binary: `cargo run -p <crate> -- ...`

## CI behavior that affects edits

- Job order in `.github/workflows/ci.yml`: **lint (fmt + clippy) → test → cross-platform build** (Ubuntu/macOS/Windows). Each later job `needs` the earlier ones.
- Security/dependency checks also run: `cargo audit`, and `cargo deny check` for `advisories` (allowed to fail, `continue-on-error`) and `bans licenses sources` (must pass). Exceptions live in `deny.toml` (incl. ignored `RUSTSEC-2025-0119` for `indicatif`, and `WTFPL` allowed for `ffmpeg-next`).
- Pre-commit hooks (`.pre-commit-config.yaml`): **on commit** = fmt check + clippy deny-warnings; **on push** = `cargo test --workspace -q`.

## Real entrypoints and boundaries

- `hdr_analyzer_mvp/src/main.rs`: CLI parse + validation; orchestrates via `pipeline::run`. Core analysis lives in `analysis/` (frame, histogram, scene, hlg) plus `crop.rs`, `optimizer.rs`, `ffmpeg_io.rs`, `writer.rs`.
- **Optional CUDA backend** (`cuda` cargo feature, off by default): `analysis/gpu.rs` + NVRTC-compiled `analysis/kernels.cu`. Activated at runtime by `--hwaccel cuda`; NVDEC decode via FFmpeg `AVHWDeviceContext` in `ffmpeg_io.rs` (cuvid → software fallbacks). The kernel analyzes **full-resolution** frames with a sampling stride (`--downscale` = stride, no swscale), so its crop rect lives in full-res coordinates — `pipeline.rs` scales rects between spaces (`scale_rect`/`shrink_rect`). `--pre-denoise median3` and `--peak-estimator robust` are CPU-only (robust needs the cross-quad diff histogram); GPU `FramePeakStats` report neutral sigma/n_eff. Validated bit-identical L1 output vs. CPU. Keep kernel result-buffer layout in sync between `kernels.cu` and `gpu.rs` constants.
- `mkvdovi/src/main.rs`: file discovery/sorting + early `inspect`/`composite-pipe` dispatch + per-file orchestration via `pipeline::convert_file`. Key modules: `fel_composite.rs` (Profile 7 BL+EL processing), `rpu_check.rs` (MEL/FEL/P8 classification and RPU diagnostics), `external.rs` (tool checks/invocation), `metadata.rs` (`CmV40Config`, L2/L5/L9/L11 generation), `verify.rs`, `progress.rs`.
- `verifier/src/main.rs`: standalone measurement-validator CLI.

## mkvdovi operational gotchas (easy to miss)

- **Checks external tools at runtime** (`external::check_dependencies`): requires `ffmpeg`, `mkvmerge`, `dovi_tool`, and either `mediainfo` or `ffprobe`. HDR10+ processing additionally invokes `hdr10plus_tool` — keep it in `PATH` for HDR10+ inputs.
- `dovi_tool` 2.3.2+ is recommended; its `inject-rpu` padding fix is relied on by the existing orchestration call.
- With no input args, it recursively processes `.mkv` files from cwd, skipping `mkvdovi_temp_*`/legacy `mkvdolby_temp_*` paths and files already ending `.DV.mkv`. Explicit `--mdfix` allows a DV input and writes a distinct `*.mdfix.DV.mkv` candidate.
- **Successful conversion deletes the source input by default**; pass `--keep-source` to prevent deletion.
- **Robust to interruption:** extract/inject/mux/encode show a live byte-progress bar (throughput + ETA) and warn after `--stall-timeout` (default 300s, `0` disables) if the output file stops growing. An interrupted run (e.g. SSH `SIGHUP`) preserves `mkvdovi_temp_*` and prints a resume hint; a re-run **auto-resumes** by reusing completed steps, gated by `<artifact>.done` sentinels (`resume.rs`). `--no-resume` forces a clean run. Run long conversions under `tmux`/`nohup`.
- For HDR10 without found measurements, it auto-runs `hdr_analyzer_mvp`. `--analysis-quality` controls sampling (downscale/sample-rate): `fast` = half-res/every 3rd frame, `balanced` = half-res/every frame (default), `accurate` = full-res/every frame.
- `mkvdovi --hwaccel cuda` is forwarded to the spawned `hdr_analyzer_mvp` (GPU analysis if that binary was built with `--features cuda`) and selects NVENC for FEL re-encodes. mkvdovi prefers `target/release/hdr_analyzer_mvp` relative to cwd over PATH.
- For HDR10+ input, L1 is derived from source HDR10+ metadata; panel peak is **not** passed as a `--trim-targets` override. HDR10+ scene peaks above 3× mastering-display peak produce advisory warnings only — **never add a silent clamp**.
- `--verify` resolves tools from `PATH`, validates structured RPU frame JSON, and hard-fails malformed Profile 8 / L1 / L6 / CM v4.0 L9/L11/L254 metadata.
- `scripts/mkvdovi_hifi_workflow.sh` is a specialist comparison helper for inputs that **already** contain DV metadata. Use `mkvdovi` directly for HDR10+ sources.
- `inspect` and `composite-pipe` dispatch before dependency checks. Keep `composite-pipe` stdout raw-frame-only; diagnostics belong on stderr.
- Profile 7 MEL uses a fast metadata-only discard path unless `--mdfix` is requested. Profile 7 FEL composites BL+EL and re-encodes; MEL/Profile 8 `--mdfix` rebuilds metadata from a clean base layer. All DV/repair inputs keep their source by default.

### Generated DV metadata levels (mkvdovi, CM v4.0 by default)

- **L1** per-frame luminance (from HDR10+ or hdr_analyzer) · **L2** trims for 100/600/1000-nit targets · **L6** static mastering metadata (MaxCLL/MaxFALL) · **L9** source primaries (auto-detected) · **L11** content type + reference mode.
- Defaults: `--cm-version v40` (or `v29`); `--content-type movies` (default — valid: `default`, `movies`, `game`, `sport`, `user-generated-content`; `cinema`/`film` alias `movies`, `gaming` aliases `game`); `--reference-mode false`; `--source-primaries` auto (`0=P3-D65, 1=BT.709, 2=BT.2020`). `-v/--verbose` shows raw tool output; `-q/--quiet` minimal.
- Progress uses `indicatif` spinners with TTY detection (auto-disabled in CI/non-interactive).

## Testing quirks

- `mkvdovi` integration tests are environment-dependent: they **skip** when `dovi_tool` is not in `PATH`, and when the sample media file is absent (`../tests/hdr-media/...mkv`). Do not assume all workspace tests are hermetic on a clean machine.
- Tests/CLI integration use `assert_cmd` + `predicates`. `clippy.toml` allows `unwrap`/`expect`/`panic`/`print`/`dbg` in tests only.

## Lint/style policy in this repo (non-default)

- Workspace lints in root `Cargo.toml`: clippy `all` allowed but `correctness` denied; `dbg_macro` denied; runs under `-D warnings` in CI.
- `unsafe_op_in_unsafe_fn = "deny"`, but `unsafe_code` is **allowed**.
- `clippy.toml` is tuned for this domain (higher complexity thresholds).
- Imports: std → third-party → `crate::`, grouped with braces, absolute `crate::` paths internally. `anyhow::Result` + `.context()` at app level; `thiserror` for library error types.

## Docs vs code

Some prose docs are stale (e.g., references to an old Python `mkvdovi` workflow). Prefer executable truth: root/workspace configs, current Rust crates under `*/src`, and the GitHub Actions workflows — in that order — over narrative docs.

## Releases

Bump version in each crate's `Cargo.toml`, add a `CHANGELOG.md` entry, then tag & push (`git tag vX.Y.Z && git push origin vX.Y.Z`). `release.yml` builds Windows x64, macOS Intel+ARM, and Linux x64, creates the GitHub release, and uploads archives with the three binaries + `README.md`/`LICENSE`/`CHANGELOG.md`. **Linux ARM64 is not automated** (runner limitation).

## Symbol navigation: LSP-first (rust-analyzer)

The native LSP tool (rust-analyzer plugin) is PRIMARY for symbol questions in this repo — not grep:

| Task | LSP operation |
|------|---------------|
| Who calls this / uses this field? | `findReferences` / `incomingCalls` |
| Where is this defined? | `goToDefinition` (or `workspaceSymbol` from a name) |
| What's the type/signature? | `hover` |
| What's in this file? | `documentSymbol` |

- LSP is a deferred tool: load it early with `ToolSearch` query `select:LSP` (a SessionStart hook reminds you).
- Warm the index with one cheap `documentSymbol` call on an entrypoint — the first `workspaceSymbol` after a cold start returns empty while rust-analyzer indexes.
- Semantic "where/how" exploration with no known symbol → `mcp__morph-mcp__codebase_search` first (global routing); big multi-question sweeps → the `warp-explorer` agent, never the built-in Explore agent.
- **Review/audit/triage sweeps** ("find all X", "any stubs?") lead with `Grep` for exhaustive exact-pattern coverage (`todo!`, `unimplemented!`, `// TODO`, `// FIXME`), then read flagged bodies.
