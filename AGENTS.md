# AGENTS.md
# Project Agents

See `.rust-skills/AGENTS.md` for Rust development guidelines.
## What this repo actually is
- Rust workspace with **three shipped binaries**: `hdr_analyzer_mvp`, `mkvdolby`, `verifier`.
- Workspace members in root `Cargo.toml`: `hdr_analyzer_mvp`, `mkvdolby`, `verifier`.
- `tools/compare_baseline` is a separate utility crate (not a workspace member); build/run it explicitly with `-p compare_baseline`.

## Toolchain and platform quirks
- Toolchain is pinned via `rust-toolchain.toml` to `stable` with required components `clippy` + `rustfmt` and explicit cross targets.
- `.cargo/config.toml` sets `-C target-cpu=native` globally; on Linux ARM64 it also forces `clang` + `lld` linker. Do not remove unless you intend to change perf/link behavior.
- `ffmpeg-next` is used, so local/CI builds need FFmpeg dev libs + clang/libclang configured (see CI workflow for exact packages/env).

## High-signal commands (use these exact forms)
- Fast local quality gate (matches pre-commit + CI lint intent):
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace --verbose`
- Build all release binaries:
  - `cargo build --release --workspace`
- Test one crate:
  - `cargo test -p hdr_analyzer_mvp`
  - `cargo test -p mkvdolby`
  - `cargo test -p verifier`
- Run one binary:
  - `cargo run -p hdr_analyzer_mvp -- ...`
  - `cargo run -p mkvdolby -- ...`
  - `cargo run -p verifier -- ...`

## CI behavior that affects edits
- CI job order is effectively: **fmt/clippy -> tests -> cross-platform build** (`.github/workflows/ci.yml`).
- Security/dependency checks also run:
  - `cargo audit`
  - `cargo deny check` (with configured advisory/license exceptions in `deny.toml`, including `RUSTSEC-2025-0119`).
- Pre-commit hooks:
  - commit: fmt check + clippy deny warnings
  - push: workspace tests (`cargo test --workspace -q`)

## Real entrypoints and boundaries
- `hdr_analyzer_mvp/src/main.rs`: CLI parse + validation; orchestrates via `pipeline::run`.
- `mkvdolby/src/main.rs`: file discovery/sorting + per-file orchestration via `pipeline::convert_file`.
- `verifier/src/main.rs`: standalone measurement validator CLI.

## mkvdolby operational gotchas (easy to miss)
- `mkvdolby` **checks external tools at runtime** (`external::check_dependencies`): requires `ffmpeg`, `mkvmerge`, `dovi_tool`, and either `mediainfo` or `ffprobe`.
- HDR10+ processing additionally invokes `hdr10plus_tool`; keep it in `PATH` when converting HDR10+ inputs.
- `dovi_tool` 2.3.2 or newer is recommended: its `inject-rpu` padding fix is used automatically by the existing orchestration call.
- If no input args are given, it recursively processes `.mkv` files from cwd, skipping `mkvdolby_temp_*` paths and files already ending with `.DV.mkv`.
- Successful conversion deletes the source input by default; pass `--keep-source` to prevent deletion.
- For HDR10 without found measurements, it auto-runs `hdr_analyzer_mvp` and currently injects fast defaults `--downscale 2 --sample-rate 3`.
- For HDR10+ input, `mkvdolby` derives L1 from source HDR10+ metadata; the panel peak is not passed as a `--trim-targets` override.
- `scripts/mkvdolby_hifi_workflow.sh` is a specialist comparison helper for inputs that already contain Dolby Vision metadata. Use `mkvdolby` directly for HDR10+ sources.

## Testing quirks
- Integration tests for `mkvdolby` are environment-dependent:
  - skip when `dovi_tool` is not in PATH
  - skip when sample media file is absent (`../tests/hdr-media/...mkv`)
- Do not assume all workspace tests are hermetic on a clean machine.

## Lint/style policy in this repo (non-default)
- Root `Cargo.toml` sets workspace lint policy; clippy `correctness` is denied and runs under `-D warnings` in CI.
- `unsafe_op_in_unsafe_fn = "deny"`, but `unsafe_code` is allowed.
- `clippy.toml` is tuned for this domain (higher complexity thresholds; unwrap/expect/panic/print/dbg allowed in tests).

## Docs vs code
- Some prose docs are stale (e.g., references to old Python `mkvdolby` workflow). Prefer executable truth from:
  - workspace/root configs
  - current Rust crates under `*/src`
  - GitHub Actions workflows
