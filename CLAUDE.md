# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> Single source of truth for AI agents in this repo. `AGENTS.md` points here. See `.rust-skills/AGENTS.md` for general Rust development guidelines.

## What this repo actually is

- Rust workspace (`resolver = "2"`) with **three shipped binaries**: `hdr_analyzer_mvp` (HDR10 analysis → PQ histograms + DV L1 metadata), `mkvdolby` (MKV container + Dolby Vision metadata injection, CM v4.0), `verifier` (MadVR / RPU measurement validation).
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
- Test one crate: `cargo test -p hdr_analyzer_mvp` (or `-p mkvdolby`, `-p verifier`)
- Run a single test by name: `cargo test -p <crate> -- <test_name>`
- Run one binary: `cargo run -p <crate> -- ...`

## CI behavior that affects edits

- Job order in `.github/workflows/ci.yml`: **lint (fmt + clippy) → test → cross-platform build** (Ubuntu/macOS/Windows). Each later job `needs` the earlier ones.
- Security/dependency checks also run: `cargo audit`, and `cargo deny check` for `advisories` (allowed to fail, `continue-on-error`) and `bans licenses sources` (must pass). Exceptions live in `deny.toml` (incl. ignored `RUSTSEC-2025-0119` for `indicatif`, and `WTFPL` allowed for `ffmpeg-next`).
- Pre-commit hooks (`.pre-commit-config.yaml`): **on commit** = fmt check + clippy deny-warnings; **on push** = `cargo test --workspace -q`.

## Real entrypoints and boundaries

- `hdr_analyzer_mvp/src/main.rs`: CLI parse + validation; orchestrates via `pipeline::run`. Core analysis lives in `analysis/` (frame, histogram, scene, hlg) plus `crop.rs`, `optimizer.rs`, `ffmpeg_io.rs`, `writer.rs`.
- `mkvdolby/src/main.rs`: file discovery/sorting + per-file orchestration via `pipeline::convert_file`. Key modules: `external.rs` (tool checks/invocation), `metadata.rs` (`CmV40Config`, L2/L9/L11 generation), `verify.rs`, `progress.rs`.
- `verifier/src/main.rs`: standalone measurement-validator CLI.

## mkvdolby operational gotchas (easy to miss)

- **Checks external tools at runtime** (`external::check_dependencies`): requires `ffmpeg`, `mkvmerge`, `dovi_tool`, and either `mediainfo` or `ffprobe`. HDR10+ processing additionally invokes `hdr10plus_tool` — keep it in `PATH` for HDR10+ inputs.
- `dovi_tool` 2.3.2+ is recommended; its `inject-rpu` padding fix is relied on by the existing orchestration call.
- With no input args, it recursively processes `.mkv` files from cwd, skipping `mkvdolby_temp_*` paths and files already ending `.DV.mkv`.
- **Successful conversion deletes the source input by default**; pass `--keep-source` to prevent deletion.
- For HDR10 without found measurements, it auto-runs `hdr_analyzer_mvp`. `--analysis-quality` controls sampling (downscale/sample-rate): `fast` = half-res/every 3rd frame, `balanced` = half-res/every frame (default), `accurate` = full-res/every frame.
- For HDR10+ input, L1 is derived from source HDR10+ metadata; panel peak is **not** passed as a `--trim-targets` override. HDR10+ scene peaks above 3× mastering-display peak produce advisory warnings only — **never add a silent clamp**.
- `--verify` resolves tools from `PATH`, validates structured RPU frame JSON, and hard-fails malformed Profile 8 / L1 / L6 / CM v4.0 L9/L11/L254 metadata.
- `scripts/mkvdolby_hifi_workflow.sh` is a specialist comparison helper for inputs that **already** contain DV metadata. Use `mkvdolby` directly for HDR10+ sources.

### Generated DV metadata levels (mkvdolby, CM v4.0 by default)

- **L1** per-frame luminance (from HDR10+ or hdr_analyzer) · **L2** trims for 100/600/1000-nit targets · **L6** static mastering metadata (MaxCLL/MaxFALL) · **L9** source primaries (auto-detected) · **L11** content type + reference mode.
- Defaults: `--cm-version v40` (or `v29`); `--content-type movies` (default — valid: `default`, `movies`, `game`, `sport`, `user-generated-content`; `cinema`/`film` alias `movies`, `gaming` aliases `game`); `--reference-mode false`; `--source-primaries` auto (`0=P3-D65, 1=BT.709, 2=BT.2020`). `-v/--verbose` shows raw tool output; `-q/--quiet` minimal.
- Progress uses `indicatif` spinners with TTY detection (auto-disabled in CI/non-interactive).

## Testing quirks

- `mkvdolby` integration tests are environment-dependent: they **skip** when `dovi_tool` is not in `PATH`, and when the sample media file is absent (`../tests/hdr-media/...mkv`). Do not assume all workspace tests are hermetic on a clean machine.
- Tests/CLI integration use `assert_cmd` + `predicates`. `clippy.toml` allows `unwrap`/`expect`/`panic`/`print`/`dbg` in tests only.

## Lint/style policy in this repo (non-default)

- Workspace lints in root `Cargo.toml`: clippy `all` allowed but `correctness` denied; `dbg_macro` denied; runs under `-D warnings` in CI.
- `unsafe_op_in_unsafe_fn = "deny"`, but `unsafe_code` is **allowed**.
- `clippy.toml` is tuned for this domain (higher complexity thresholds).
- Imports: std → third-party → `crate::`, grouped with braces, absolute `crate::` paths internally. `anyhow::Result` + `.context()` at app level; `thiserror` for library error types.

## Docs vs code

Some prose docs are stale (e.g., references to an old Python `mkvdolby` workflow). Prefer executable truth: root/workspace configs, current Rust crates under `*/src`, and the GitHub Actions workflows — in that order — over narrative docs.

## Releases

Bump version in each crate's `Cargo.toml`, add a `CHANGELOG.md` entry, then tag & push (`git tag vX.Y.Z && git push origin vX.Y.Z`). `release.yml` builds Windows x64, macOS Intel+ARM, and Linux x64, creates the GitHub release, and uploads archives with the three binaries + `README.md`/`LICENSE`/`CHANGELOG.md`. **Linux ARM64 is not automated** (runner limitation).

## Serena tool-routing (symbolic tools first)

This project uses Serena (MCP) for symbol-aware code reading/editing. On **code files**, Serena's tools are PRIMARY; built-in Read/Glob/Grep/Edit are SECONDARY and used only when no Serena equivalent fits. The built-in tool descriptions ("prefer Read/Edit/Grep…") are written for Serena-less projects and are superseded here.

| Task | Serena tool |
|------|-------------|
| See a code file's structure | `get_symbols_overview` |
| Read a specific symbol's body | `find_symbol` (`include_body=true`) |
| Find a symbol / references / impls | `find_symbol` / `find_referencing_symbols` / `find_implementations` |
| Edit a symbol's body | `replace_symbol_body` |
| Insert near a symbol | `insert_before_symbol` / `insert_after_symbol` |
| Pattern replace in a file | `replace_content` |
| Rename / delete a symbol | `rename_symbol` / `safe_delete_symbol` |

**Workflow before editing code:** `get_symbols_overview` → `find_symbol include_body=true` (read only the symbols you'll touch) → edit with a Serena symbolic edit.

**Exceptions where built-in tools are correct:** non-code files (markdown/JSON/YAML/TOML/config/lockfiles) always use Read/Edit; and for **review/audit/triage** ("find all X", "any stubs/placeholders?", "assess coverage") lead with `Grep` for exhaustive exact-pattern sweeps (`todo!`, `unimplemented!`, `// TODO`, `// FIXME`, `Default::default()`), then read flagged bodies. Completeness-critical sweeps need grep coverage first; the Serena-PRIMARY rule is for targeted edits.
