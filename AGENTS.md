# Repository Guidelines

## Project Structure & Module Organization

This is a Rust workspace with three shipped CLI crates: `hdr_analyzer_mvp/` for HDR frame analysis and `.bin` generation, `mkvdolby/` for end-to-end MKV conversion, and `verifier/` for inspecting measurement files. Each crate keeps source in `src/` and crate-level integration tests in `tests/`. Shared media fixtures and baseline outputs live under top-level `tests/`, with documentation in `docs/` and release or install helpers in `scripts/`. Workspace policy is centralized in `Cargo.toml`, `rustfmt.toml`, `clippy.toml`, `deny.toml`, and `.github/workflows/`.

## Build, Test, and Development Commands

- `cargo build --workspace`: build all workspace binaries in debug mode.
- `cargo build --release --workspace`: produce optimized binaries in `target/release/`.
- `cargo run -p hdr_analyzer_mvp -- --help`: run a specific CLI during development.
- `cargo test --workspace`: run all unit and integration tests.
- `cargo fmt --all -- --check`: verify formatting without changing files.
- `cargo clippy --workspace --all-targets -- -D warnings`: run the CI lint gate.
- `cargo audit` and `cargo deny check`: run security and dependency policy checks when installed.

FFmpeg development libraries are required for builds. On macOS, install them with `brew install ffmpeg pkg-config llvm`.

## Coding Style & Naming Conventions

Use Rust 2021 with four-space indentation and `rustfmt` defaults from `rustfmt.toml` (`max_width = 100`). Follow standard Rust naming: `snake_case` for functions and variables, `PascalCase` for types, and `SCREAMING_SNAKE_CASE` for constants. Avoid committed `dbg!` or `todo!`; Clippy denies these. Keep public CLI behavior documented in `README.md` or `docs/` when it changes.

## Testing Guidelines

Place crate-specific integration tests in `<crate>/tests/`, using `assert_cmd`, `predicates`, and `tempfile` where helpful. Use top-level `tests/baseline/`, `tests/hdr-media/`, and `tests/hlg-media/` for regression fixtures, but avoid adding large media unless necessary. Before opening a PR, run `cargo test --workspace` plus the formatting and Clippy checks above.

## Commit & Pull Request Guidelines

Recent history uses concise Conventional Commit-style messages such as `feat(mkvdolby): ...`, `fix ...`, `docs: ...`, and `chore: ...`. Keep commits focused and imperative. PRs should describe the change, why it is needed, exact test commands run, affected binaries, linked issues, and screenshots or logs for user-visible CLI output changes.
