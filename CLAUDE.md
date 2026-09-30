# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> Single source of truth for AI agents in this repo. `AGENTS.md` points here. Rust style and lint policy live in the "Lint/style policy" section below — there is no separate Rust guidelines pack.

## What this repo actually is

- Rust workspace (`resolver = "2"`) with **three shipped binaries**: `hdr_analyzer_mvp` (HDR10 analysis → PQ histograms + DV L1 metadata), `mkvdovi` (MKV container + Dolby Vision metadata injection, CM v4.0), `verifier` (MadVR / RPU measurement validation).
- `tools/compare_baseline` and `tools/l1_diff` are separate utility crates, **excluded** from the workspace. Build/run them explicitly with `--manifest-path tools/<name>/Cargo.toml`.
- Release profile is tuned: `lto = "fat"`, `codegen-units = 1`, `strip = true`, `panic = "abort"` — release builds are slow to link; expect it.

## Toolchain and platform quirks

- `rust-toolchain.toml` pins `channel = "stable"` (not a fixed version number) with components `clippy` + `rustfmt` and explicit cross targets. CI uses `dtolnay/rust-toolchain@stable`.
- `.cargo/config.toml` sets `-C target-cpu=native` globally; on Linux ARM64 (`aarch64-unknown-linux-gnu`) it also forces `clang` + `lld`. This host is Oracle ARM/Ampere. Don't remove unless you mean to change perf/link behavior.
- `ffmpeg-next` is used, so local/CI builds need FFmpeg dev libs + `clang`/`libclang` and `BINDGEN_EXTRA_CLANG_ARGS` configured — one composite action, `.github/actions/setup-ffmpeg`, does this per OS for both workflows (apt on Linux, `brew install ffmpeg` on macOS, a checksum-verified BtbN LGPL shared build via `FFMPEG_DIR` on Windows; libclang/pkg-config come preinstalled on the runner images). `ffmpeg-next` 9 is built with only `codec`/`format`/`software-scaling`, so avfilter/avdevice/swresample dev libs are not needed. The Windows release zip bundles the FFmpeg DLLs via `scripts/ci/bundle-windows-ffmpeg.sh`, which CI smoke-tests on every Windows build.

## High-signal commands (use these exact forms)

- Fast local quality gate (matches pre-commit + CI lint intent):
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace --verbose`
- Build all release binaries: `cargo build --release --workspace`
- CUDA-enabled analyzer (NVIDIA hosts): `cargo build --release -p hdr_analyzer_mvp --features cuda` — also lint it with `cargo clippy -p hdr_analyzer_mvp --all-targets --features cuda -- -D warnings` (CI only covers the default feature set)
- Refresh the dev install after changes: `./scripts/dev-refresh.sh` — rebuilds the workspace release binaries **then** the CUDA analyzer last (a plain workspace build overwrites the analyzer without the cuda feature). On this dev box `~/.local/bin/{mkvdovi,hdr_analyzer_mvp,verifier}` are symlinks into `target/release/`, so the system-wide commands always run the latest build.
- Test one crate: `cargo test -p hdr_analyzer_mvp` (or `-p mkvdovi`, `-p verifier`)
- Run a single test by name: `cargo test -p <crate> -- <test_name>`
- Run one binary: `cargo run -p <crate> -- ...`

## CI behavior that affects edits

- Job order in `.github/workflows/ci.yml`: **lint (fmt + clippy) → test → cross-platform build** (Ubuntu/macOS/Windows). Each later job `needs` the earlier ones.
- Security/dependency checks also run: `cargo audit`, and `cargo deny check` for `advisories` (allowed to fail, `continue-on-error`) and `bans licenses sources` (must pass). Exceptions live in `deny.toml` (incl. ignored `RUSTSEC-2025-0119` for `indicatif`, and `WTFPL` allowed for `ffmpeg-next`).
- Pre-commit hooks (`.pre-commit-config.yaml`): **on commit** = fmt check + clippy deny-warnings; **on push** = `cargo test --workspace -q`.

## Claude Code feedback loop (committed in `.claude/`)

- **Per edit:** rust-analyzer (the `rust-analyzer-lsp` plugin) pushes diagnostics after each Edit/Write, including `cargo check` flycheck results (type and borrow errors, tagged `(rustc)`). It needs the rustup component: `rustup component add rust-analyzer`. Without it, `~/.cargo/bin/rust-analyzer` is only the rustup proxy, the LSP server crashes with exit code 1, and no diagnostics arrive.
- **End of turn:** `.claude/hooks/rust-check.sh` (wired in `.claude/settings.json`). PostToolUse records touched `.rs`/Cargo/lint-config files. The Stop hook then rustfmts those files without reporting, and runs `cargo clippy --workspace --all-targets` (plus `--manifest-path tools/<crate>/Cargo.toml` for a touched tool crate). It feeds back only errors from any file and warnings in touched files. Turns without Rust edits never start cargo. After 3 feedback rounds it tells the user and stays quiet until the next Rust edit. State lives in `${TMPDIR:-/tmp}/claude-rust-check/<session_id>/`. An incremental run takes about 2–4 s on the `/mnt/c` checkout. Tests are not run here; they stay on pre-push.

## Real entrypoints and boundaries

- `hdr_analyzer_mvp/src/main.rs`: CLI parse + validation; orchestrates via `pipeline::run`. Core analysis lives in `analysis/` (frame, histogram, scene, hlg, gpu) plus `crop.rs`, `optimizer.rs`, `ffmpeg_io.rs`, `l1_sidecar.rs`, `writer.rs`. HLG is mapped to PQ through the Dolby Vision Profile 8.4 decode (`analysis/hlg.rs`, from the `dolby_vision` crate's `Profile84` preset): luma goes through the 8.4 luma reshaping curve (one 1024-entry LUT shared by CPU and GPU), and max-RGB (`Dovi84Decoder`, the HLG default like PQ) reconstructs the full decode per pixel (luma curve + the two order-3 chroma MMR curves + the RPU `ycc_to_rgb` matrix/offsets), then takes max(R′,G′,B′). Both are clamped to the RPU's source_min/max_pq [62, 3079] on purpose, so L1 stays in the declared range. Neutral HLG reads about 2% higher in max-RGB than in luma (the preset's chroma MMR tints neutrals slightly blue). The transfer comes from the first decoded frame when the stream tag is not PQ/HLG (`ffmpeg_io::resolve_transfer`): that picks up HLG signalled via the alternative-transfer SEI (VUI says BT.2020) or only in the MKV Colour element. Input contract: only PQ/HLG transfers are analyzed (`TransferFunction::Unsupported` covers tagged SDR curves, incl. BT2020_10/12); samples are assumed limited-range BT.2020 NCL, with warnings for other range/matrix tags.
- **Optional CUDA backend** (`cuda` cargo feature, off by default): `analysis/gpu.rs` + NVRTC-compiled `analysis/kernels.cu`. Activated at runtime by `--hwaccel cuda`; NVDEC decode via FFmpeg `AVHWDeviceContext` in `ffmpeg_io.rs` (cuvid → software fallbacks). Decoder and analyzer share device 0's **primary context** (`AV_CUDA_USE_PRIMARY_CONTEXT`; `GpuAnalyzer::new` sets `CU_CTX_SCHED_BLOCKING_SYNC` first, so it must stay ahead of `setup_hardware_decoder`), and P010 NVDEC frames are analyzed in place (`GpuAnalyzer::analyze_device`); `pipeline.rs` downloads a frame (`host_view`) only for crop detection/scene-cut sampling or CPU fallback — never hand an `AV_PIX_FMT_CUDA` frame to host code. `HDR_ANALYZER_CUDA_HOST_FRAMES=1` forces the download path for parity checks. The kernel analyzes **full-resolution** frames with a sampling stride (`--downscale` = stride, no swscale), so its crop rect lives in full-res coordinates — `pipeline.rs` scales rects between spaces (`scale_rect`/`shrink_rect`). `--pre-denoise median3` and `--peak-estimator robust` are CPU-only (robust needs the cross-quad diff histogram); GPU `FramePeakStats` report neutral sigma/n_eff. Validated bit-identical L1 output vs. CPU (the HLG max-RGB decode too: f32, fixed op order, non-contracting `__fmul_rn`/`__fadd_rn`). Keep the kernel result-buffer layout (counts, then u64 sums at `SUMS_WORD`) and the `dovi_params` buffer layout (`DOVI_*` constants) in sync between `kernels.cu` and `gpu.rs`. Cross-thread reductions in the kernel must stay order-independent (integer counts, u64 fixed-point sums, max of non-negative f32 bits) so launch-shape changes stay bit-identical. Benchmark with `--no-crop` (or subtract the crop probe) and report frames ÷ wall time; `--profile-performance`'s "Analysis" fps excludes decode.
- `mkvdovi/src/main.rs`: file discovery/sorting + early `inspect`/`composite-pipe` dispatch + per-file orchestration via `pipeline::convert_file`. Key modules: `fel_composite.rs` (Profile 7 BL+EL processing), `rpu_check.rs` (MEL/FEL/P8 classification and RPU diagnostics), `external.rs` (tool checks/invocation), `metadata.rs` (`CmV40Config`, L2/L5/L9/L11 generation), `verify.rs`, `progress.rs`.
- `verifier/src/main.rs`: standalone measurement-validator CLI.

## mkvdovi operational gotchas (easy to miss)

- **Checks external tools at runtime** (`external::check_dependencies`): requires `ffmpeg`, `mkvmerge`, `dovi_tool`, and either `mediainfo` or `ffprobe`. HDR10+ processing additionally invokes `hdr10plus_tool` — keep it in `PATH` for HDR10+ inputs.
- `dovi_tool` 2.3.2 is the floor (its `inject-rpu` padding fix is relied on by the existing orchestration call); 2.3.4+ lets mkvdovi pass the MKV directly to remove/convert/demux (`--dovi-input auto|raw|mkv`, auto = on when 2.3.4+ detected via `dovi_tool --version`), with automatic per-step fallback to ffmpeg extraction; direct attempts log to `*_mkv.log` in the temp dir.
- `dovi_tool` 2.3.4 parses Level 253 ext-metadata blocks, but the `dolby_vision` crate 3.4.0 used in-process (inspect sampling, FEL NLQ parsing) does not yet — re-bump when the crate releases L253 support.
- With no input args, it recursively processes `.mkv` files from cwd, skipping `mkvdovi_temp_*`/legacy `mkvdolby_temp_*` paths and files already ending `.DV.mkv`. Explicit `--mdfix` allows a DV input and writes a distinct `*.mdfix.DV.mkv` candidate.
- **Successful conversion deletes the source input by default**; pass `--keep-source` to prevent deletion.
- **Robust to interruption:** extract/inject/mux/encode show a live byte-progress bar (throughput + ETA) and warn after `--stall-timeout` (default 300s, `0` disables) if the output file stops growing. An interrupted run (e.g. SSH `SIGHUP`) preserves `mkvdovi_temp_*` and prints a resume hint; a re-run **auto-resumes** by reusing completed steps, gated by `<artifact>.done` sentinels plus a `resume.json` fingerprint (input name/size/mtime, mkvdovi version, artifact-affecting settings from `pipeline::resume_settings`); a temp dir with a different fingerprint is discarded, while one with no fingerprint (older mkvdovi, legacy `mkvdolby_temp_*`) resumes with a warning (`resume.rs`). Add any new artifact-affecting flag to `resume_settings`. `--no-resume` forces a clean run. Run long conversions under `tmux`/`nohup`.
- For HDR10 without **valid** measurements, it auto-runs `hdr_analyzer_mvp`: every existing measurements candidate is tried in turn (the analyzer's own `<stem>_measurements.bin` first, a shared `measurements.bin` last), and the first whose `.l1.json` sidecar validates against the input (name, size, contiguous scenes, MediaInfo frame count within `metadata::frame_count_tolerance`, because MediaInfo estimates counts from duration for untagged MKVs) is reused; otherwise analysis re-runs. Scenes outside min <= avg <= max are a warning, not a rejection: `--peak-domain luma` and percentile/robust estimators legitimately produce them. `--legacy-madvr-l1` is the only way to reach the old `--madvr-file --use-custom-targets` L1 (optimizer targets as max, placeholder avg). `--analysis-quality` controls sampling (downscale/sample-rate): `auto` (default) = `accurate` when GPU analysis is available else `balanced`; `fast` = half-res/every 3rd frame, `balanced` = half-res/every frame, `accurate` = full-res/every frame.
- **L1 sidecar contract:** the analyzer writes per-scene L1 stats to `<measurements>.l1.json` (`hdr_analyzer_mvp/src/l1_sidecar.rs`); mkvdovi reads it via `metadata::load_l1_sidecar` for source-honest per-scene L1. The analyzer writes `version: 3` (v2 added provenance + full-resolution `crop`, which mkvdovi turns into L5 offsets; v3 added `analysis.luminance_mapping`: `pq`, or for HLG `dovi84-v2` (full 8.4 decode, written for every HLG run) and the older luma-only `dovi84-v1`); mkvdovi and `tools/l1_diff` accept 1–3, and HLG inputs require v3 + `metadata::DOVI84_LUMINANCE_MAPPING` (`dovi84-v2`; luma-only `dovi84-v1` sidecars from pre-release builds are re-analyzed; adding `dovi84-v2` kept version 3 because it is a new value of a field in the still-unreleased v3). A missing/invalid/unknown-version sidecar produces a visible warning and re-analysis, never a silent fallback — a schema change must still bump the version on both sides (same class of cross-binary contract as the `+cuda` version probe).
- `--hwaccel` defaults to **`auto`**: `pipeline::resolve_auto_settings` (called once from `main.rs`) probes `nvidia-smi` (incl. `/usr/lib/wsl/lib/nvidia-smi` on WSL2) and resolves to `cuda` or `none` before any file processing — downstream code only ever sees concrete values. GPU analysis availability is probed via `hdr_analyzer_mvp --version` containing `+cuda` (set from the analyzer's `cuda` feature in its `cli.rs` VERSION const — keep that contract if you touch either side). NVENC selection for FEL re-encodes is additionally guarded by `external::ffmpeg_has_encoder("hevc_nvenc")` with a warn+libx265 fallback.
- Explicit `mkvdovi --hwaccel cuda` is forwarded to the spawned `hdr_analyzer_mvp` (GPU analysis if that binary was built with `--features cuda`) and selects NVENC for FEL re-encodes. mkvdovi prefers `target/release/hdr_analyzer_mvp` relative to cwd over PATH (`pipeline::analyzer_executable`).
- For HDR10+ input, L1 is derived from source HDR10+ metadata; panel peak is **not** passed as a `--trim-targets` override. HDR10+ scene peaks above 3× mastering-display peak produce advisory warnings only — **never add a silent clamp**.
- `--verify` resolves tools from `PATH`, validates structured RPU frame JSON, and hard-fails malformed Profile 8 / L1 / L6 / CM v4.0 L9/L11/L254 metadata, or an RPU frame count that differs from the muxed video track (MediaInfo `FrameCount`) or the L1 sidecar.
- `scripts/mkvdovi_hifi_workflow.sh` is a specialist comparison helper for inputs that **already** contain DV metadata. Use `mkvdovi` directly for HDR10+ sources.
- `inspect` and `composite-pipe` dispatch before dependency checks. Keep `composite-pipe` stdout raw-frame-only; diagnostics belong on stderr.
- **HLG → Profile 8.4, no re-encode:** the HLG stream is copied bit-exact and gets an 8.4 RPU (`metadata::dv_profile_for`); L1 comes from the analyzer's 8.4-decode measurement. There is no HLG→PQ encode path any more. `--mdfix` on an 8.4 input is refused.
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
| What does this function call? | `outgoingCalls` (resolves into deps too, not just this crate) |
| Where is this defined? | `goToDefinition` (or `workspaceSymbol` from a name) |
| What's the type/signature? | `hover` (returns the full signature **plus** the doc comment) |
| What's in this file? | `documentSymbol` |

- LSP is a deferred tool: load it early with `ToolSearch` query `select:LSP` (a SessionStart hook reminds you).
- **Warm the index first, and expect a wait.** One cheap `documentSymbol` call on an entrypoint (e.g. `hdr_analyzer_mvp/src/main.rs`) kicks off indexing, but it is not instant: on this host the first `workspaceSymbol` still returned empty and only resolved after ~45s. `documentSymbol` works immediately; treat an empty `workspaceSymbol` as "still indexing", not "no such symbol", and retry once.
- **Positions are 1-based on both axes and must land exactly on the symbol.** `goToImplementation` at `mkvdovi/src/progress.rs:127:11` (inside the `for` keyword) returned "no definition found"; the same call at `127:6` (on `Drop`) returned 182 impls. A "no definition found" result usually means a bad column, not a missing symbol — recount before concluding anything.
- **`goToImplementation` has little local value here: the repo declares no traits of its own** (`rg '^(pub )?trait '` → zero hits). Every `impl X for Y` implements a std or third-party trait, so the operation dumps the whole ecosystem's impl list (182 entries for `Drop`, of which 2 are ours). For repo-local work use `findReferences` and the call-hierarchy pair instead.
- Cross-crate and cross-dep resolution both work: `findReferences` on `l1_sidecar::write_l1_sidecar` finds its `pipeline.rs` call sites, and `outgoingCalls` resolves through to `anyhow::Context::with_context`, `serde_json::to_writer_pretty` and `std::fs::File::create`. `incomingCalls` also surfaces **test** callers, which is the cheapest way to find a function's existing coverage.
- **Review/audit/triage sweeps** ("find all X", "any stubs?") lead with `Grep` for exhaustive exact-pattern coverage (`todo!`, `unimplemented!`, `// TODO`, `// FIXME`), then read flagged bodies.
