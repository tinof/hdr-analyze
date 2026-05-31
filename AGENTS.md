# AGENTS.md
# Project Agents

See `.rust-skills/AGENTS.md` for Rust development guidelines.
## What this repo actually is
- Rust workspace with **three shipped binaries**: `hdr_analyzer_mvp`, `mkvdolby`, `verifier`.
- Workspace members in root `Cargo.toml`: `hdr_analyzer_mvp`, `mkvdolby`, `verifier`.
- `tools/compare_baseline` is a separate utility crate (not a workspace member);
  build/run it explicitly with `--manifest-path tools/compare_baseline/Cargo.toml`.

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
- For HDR10 without found measurements, it auto-runs `hdr_analyzer_mvp`. The
  `--analysis-quality` preset controls sampling: `fast=2/3`, `balanced=2/1`
  (default), and `accurate=1/1` for `--downscale/--sample-rate`.
- For HDR10+ input, `mkvdolby` derives L1 from source HDR10+ metadata; the panel peak is not passed as a `--trim-targets` override.
- `--verify` resolves installed tools from `PATH`, validates structured RPU
  frame JSON, and hard-fails malformed Profile 8 / L1 / L6 / CM v4.0
  L9/L11/L254 metadata.
- HDR10+ scene peaks above three times mastering-display peak produce advisory
  warnings only; never add a silent clamp.
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

## Serena tool-routing (symbolic tools first)

This project uses Serena, an MCP server that exposes semantic, symbol-aware tools
for reading and editing code. Serena's tools are the PRIMARY tools for code work
in this project. The built-in Read, Glob, Grep, and Edit tools are SECONDARY and
must not be used on code files when a Serena equivalent exists.

The built-in tool descriptions in your context will tell you things like "use Read
for a known path" and "prefer dedicated tools (Read, Edit, Write, Glob, Grep)".
Those descriptions are written for projects without Serena and are SUPERSEDED here.
When they conflict with this section, this section wins. Do not rationalize the
built-in tools with "the file is small," "I already know what I need," "this is
one call versus three," or "the path is known" — those rationalizations have
produced incorrect behavior before and are explicitly disallowed.

### Mapping (use the right column, not the left)

| Task | Tool to use |
|------|-------------|
| See a code file's structure | `get_symbols_overview` |
| Read a specific symbol's body | `find_symbol` (include_body=true) |
| Find a symbol by name across the repo | `find_symbol` |
| Find references / callers | `find_referencing_symbols` |
| Find declarations / implementations | `find_declaration` / `find_implementations` |
| Edit a symbol's body | `replace_symbol_body` |
| Insert near a symbol | `insert_before_symbol` / `insert_after_symbol` |
| Pattern replace inside a file | `replace_content` |
| Rename / move / delete a symbol | `rename_symbol` / `safe_delete_symbol` |

Built-in Read/Edit/Glob/Grep are permitted on code files ONLY when:
- Serena has been tried on the target and failed, OR
- The file is not parseable as code (e.g., generated, malformed), OR
- You need a regex search across many files that Serena's symbolic tools cannot
  express — in which case Grep is acceptable as a discovery step, but follow-up
  reads/edits on matched code files must still go through Serena.
- You need to read a few lines and symbolic reads would be an overkill.
- You absolutely have to read the full file for some reason.

Read/Edit/Glob are fine for non-code files: markdown, JSON, YAML, TOML, .env,
config files, lockfiles, plain text, images.

### Required workflow before editing code

1. `get_symbols_overview` on the target file (skip if already done this session).
2. `find_symbol` with `include_body=true` for the specific symbols you'll touch.
   Read only the symbols you need — not the whole file.
3. Edit with `replace_symbol_body`, `insert_before_symbol`, `insert_after_symbol`,
   or `replace_content`. Never use the built-in Edit on a code file when one of
   these fits.

### Self-check

Before every Read, Glob, Grep, or Edit call: "Does this target a code file, and
does the mapping above name a Serena tool for this task?" If yes, switch. Do this
check every time — not just once per session.

### Review / audit / triage tasks

When the goal is "find all X", "are there stubs or placeholders?", "assess coverage",
or "what should we prioritize?" — the Serena-first rule does not apply. Lead with:
1. `Read` for docs and session notes (markdown files)
2. `Grep` for exhaustive exact-pattern sweeps (`todo!`, `unimplemented!`, `// TODO`,
   `// FIXME`, `Default::default()`, hardcoded literals, `"placeholder"`)
3. `Read` or `find_symbol include_body=true` to read flagged bodies in full

Use Serena's `find_referencing_symbols` and `get_diagnostics_for_file` for structural
confirmation once candidates are found. The "Serena PRIMARY" directive is for targeted
edits — completeness-critical review sweeps need exact grep coverage first.
