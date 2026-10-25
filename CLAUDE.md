# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> Single source of truth for AI agents in this repo. `AGENTS.md` points here, except for its `## Code Review Rules` section, which the Codex reviewers read (Codex's GitHub review natively) and which this file does not repeat. Rust style and lint policy live in the "Lint/style policy" section below — there is no separate Rust guidelines pack.
>
> This file is sent on every turn, so it holds rules and pointers only. The reasons, code paths and history live in the doc each section names; read that doc before you change the area.

## What this repo actually is

- Rust workspace (`resolver = "2"`) with **three shipped binaries**: `hdr_analyzer_mvp` (HDR10/HLG analysis → PQ histograms + DV L1 metadata), `mkvdovi` (MKV container + Dolby Vision metadata injection, CM v4.0), `verifier` (MadVR / RPU measurement validation). Two library crates: `dovi84_composer` (the Profile 8.4 composers `Composer::{Preset, Bt2100V1}`, their sidecar names, the RPU composer rewrite/check, the P8 `canary_mapping` and `check_mapping_writable`, shared by the analyzer and mkvdovi) and `dovi_rpu_builder` (pure-Rust in-process Profile 8.1/8.4 RPU builders, byte-identical to `dovi_tool generate` 2.3.4, no I/O and no panics on input, builds for `aarch64-linux-android`, CI job `rpu-builder`; for the NuvioTV fork, [`docs/RPU_BUILDER.md`](docs/RPU_BUILDER.md); mkvdovi takes its `extra.json` from it and still runs `dovi_tool generate`).
- **Never enable `hdr10plus/json` or `dolby_vision/serde` anywhere in the workspace, not even as a dev-dependency:** both turn on `serde_json/preserve_order`, which reorders mkvdovi's `extra.json` keys in workspace test builds and breaks its byte-exact goldens.
- Utility crates **excluded** from the workspace; build/run them with `--manifest-path tools/<name>/Cargo.toml`: `tools/compare_baseline`, `tools/l1_diff` (L1 scoring, the regression gate), `tools/rpu_builder_check` (`dovi_rpu_builder` fixtures, the serde-path `dovi_tool` equivalence test and the HDR10+ payload reader; includes `dovi_rpu_builder/tests/cases/mod.rs` by `#[path]`), `tools/fit_hlg_composer` (fits the bt2100 constants in `dovi84_composer/src/bt2100_v1.rs`; subcommands `rewrite-rpu`, `canary-rpu`/`canary-check`, and `chroma-siting`, whose `--gate` mode is the P8 acceptance gate; see [`docs/HLG_COMPOSER.md`](docs/HLG_COMPOSER.md)).
- Release profile: `lto = "fat"`, `codegen-units = 1`, `strip = true`, `panic = "abort"`; release builds are slow to link.

## Toolchain and platform quirks

- `rust-toolchain.toml` pins `channel = "stable"` with `clippy` + `rustfmt` and explicit cross targets; CI uses `dtolnay/rust-toolchain@stable`.
- `.cargo/config.toml` sets `build.rustflags = -C target-cpu=native`; on `aarch64-unknown-linux-gnu` it also sets `clang` + `lld`, and those target `rustflags` replace `build.rustflags` (no `target-cpu=native` there). Don't remove either unless you mean to change perf/link behavior. `ci.yml` and `release.yml` set `RUSTFLAGS: ""` so CI and release binaries target the baseline CPU; **keep that override**: without it published binaries crash on other CPUs, and cached proc-macro libraries crash rustc on an older runner.
- `ffmpeg-next` 9 (only `codec`/`format`/`software-scaling`) needs FFmpeg dev libs + `clang`/`libclang` and `BINDGEN_EXTRA_CLANG_ARGS`. CI sets this up per OS in `.github/actions/setup-ffmpeg` ([`docs/CI.md`](docs/CI.md)).

## High-signal commands (use these exact forms)

- Quality gate (matches pre-commit + CI lint): `cargo fmt --all -- --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace --verbose`
- Release binaries: `cargo build --release --workspace`. CUDA analyzer (NVIDIA hosts): `cargo build --release -p hdr_analyzer_mvp --features cuda`; lint it with `cargo clippy -p hdr_analyzer_mvp --all-targets --features cuda -- -D warnings` (no GPU or toolkit needed: cudarc loads the driver and NVRTC at runtime; CI's lint job runs it, the Stop hook does not).
- One crate: `cargo test -p <crate>` (`hdr_analyzer_mvp`, `mkvdovi`, `verifier`, `dovi84_composer`, `dovi_rpu_builder`); one test: `cargo test -p <crate> -- <test_name>`; one binary: `cargo run -p <crate> -- ...`
- **Claude refreshes the dev install itself.** When a change set touches code that ends up in `mkvdovi`, `hdr_analyzer_mvp` or `verifier` (their crates, `dovi84_composer`, `dovi_rpu_builder`, workspace `Cargo.toml`/`Cargo.lock`), run `./scripts/dev-refresh.sh` at the end, unasked, and report the `--version` lines it prints. It builds the workspace release binaries, then the CUDA analyzer last when `nvidia-smi` is found. `~/.local/bin/{mkvdovi,hdr_analyzer_mvp,verifier}` must stay `-> …/target/release/` symlinks, not copies. Do not skip the refresh because tests passed: the user runs the installed commands.
- **A host that does not push** (the Oracle ARM host): at session start `git fetch`; when `main` is checked out, clean apart from untracked files and behind `origin/main`, `git pull --ff-only`, then refresh. A local `--version` check is not enough. Host traps (systemd-run for the long build, stale FFmpeg build cache, `CC=clang`): [`docs/INSTALLATION.md`](docs/INSTALLATION.md) "Development hosts".

## CI behavior that affects edits

- Jobs (`.github/workflows/ci.yml`): `changes` → lint (fmt + clippy, incl. `--features cuda`) → test, `l1-regression`, `rpu-builder` and the release build side by side. Details, apt handling, caches, security checks: [`docs/CI.md`](docs/CI.md).
- A PR that changes only `*.md`, `docs/` or `LICENSE` (the `gate-skip` set) skips every Rust job and `changes` is its one passing check. `/ship` needs at least one check, so never move this filtering to workflow-level `paths:`. Files under `.claude/` are not in the set.
- The `test` job installs the integration-test tools (`.github/actions/setup-test-tools`, shared with `release.yml`, so bump the pins there) and fails on any unexpected `Skipping` line (`scripts/ci/check-test-skips.sh`).
- `cargo audit` and `cargo deny check bans licenses sources` must pass; only `cargo deny` `advisories` may fail (`continue-on-error`). Exceptions live in `deny.toml`.
- CI never exercises the CUDA path (no GPU runner). CI runs on pushes to `main` and `fix-*` and on PRs to `main`; other branches get CI only through a PR.
- Pre-commit hooks (commit: fmt + clippy; push: workspace tests unless docs-only) run only where pre-commit is installed.

## Claude Code feedback loop (committed in `.claude/`)

- Per edit, rust-analyzer pushes diagnostics incl. `cargo check` results (needs `rustup component add rust-analyzer`). At end of turn `.claude/hooks/rust-check.sh` rustfmts touched files and runs clippy (plus `nvcc -ptx` for a touched `.cu`); it feeds back errors anywhere and warnings in touched files, never runs tests, and does not lint `cuda`-gated code. Details: [`docs/CI.md`](docs/CI.md).

## Roadmap workflow (committed in `.claude/`)

- The global `ship` plugin provides `/ship:roadmap-next`, `/ship:roadmap-init` and `/ship`. This repo's contract is `.claude/workflow.md` (base, squash merge, branch pattern, `gate`, path-triggered `## Gates`, Tier 2 paths, `## Routing`, testing rules, release docs, push policy). A change to it makes the next `/ship:roadmap-next` rank afresh and makes the diff Tier 2. Its `## Gates` and `## Tier 2 paths` copy the path rules of `.claude/workflows/pre-pr-panel.js`; change both together.
- **Cycle per roadmap step:** `/ship:roadmap-next` (confirms the **Next up** checkpoint, or on a trigger ranks afresh with Codex as second advisor; the ready queue is `~/.claude/skills/ship/scripts/roadmap-ready.sh`; keeps Next up and the **Checked** bullets in `ROADMAP.md`) → plan mode (Codex plan-review hook) → implement → `/ship --step <ID>`. `/ship` ships only finished work: self-check (the pre-pr-review panel + docs keeper), the `gate` and triggered `## Gates`, the cold cross-family review routed by `## Routing`, then it closes the step in `ROADMAP.md` (step line, Status, Next up handoff) and `docs/ROADMAP_LOG.md`, waits for CI and squash-merges. `cuda-parity` is not in CI: off the CUDA host it stays pending, the PR gets `review:needs-human`, and the owner decides.
- **Handoff:** when an implementation is done, tell the user the one next command, `/ship --step <ID>`. Suggest `/pre-pr-review` alone only for a full local review without a push. Both are user-invoked (`disable-model-invocation`): never start them and never follow their files on your own. Running both costs no second review (the `/pre-pr-review` record is reused while HEAD is unchanged).
- `/pre-pr-review` (`.claude/skills/pre-pr-review/SKILL.md`, workflow `.claude/workflows/pre-pr-panel.js`): three Opus lenses at high, Fable 5.1 on high-risk diffs, an Opus skeptic per finding, a pure-move mode for module splits, and the gates the touched paths require; standalone it adds a Gemini pass (`.claude/workflows/pre-pr-gemini.sh`). A gate that cannot run, an unexpected `Skipping` line or a Claude reviewer that returns nothing after the resume is a blocker. A failed Gemini pass is reported, never a blocker and never replaced by a Claude review.
- `.claude/agents/docs-keeper.md` updates the Markdown docs after the code fixes; it never closes a step (`/ship` step 10 does, only with acceptance-gate evidence). Steps that need the owner are under "Waiting for the owner" in `ROADMAP.md`.

## Cross-binary contracts

Change every side in the same PR; the review rules and their priority are in `AGENTS.md` "Code Review Rules".

- **L1 sidecar** `<measurements>.l1.json`: written by `hdr_analyzer_mvp/src/l1_sidecar.rs` (`version: 5`), read by mkvdovi (`metadata::load_l1_sidecar`, accepts up to `metadata::L1_SIDECAR_MAX_VERSION`) and `tools/l1_diff`. `L1Sidecar` is defined on each side with no crate link. A schema change bumps the version on both sides; a new `analysis.luminance_mapping` value does not. A missing, invalid or unknown-version sidecar is a visible warning and re-analysis, never a silent fallback. Schema per version: [`docs/FORMAT_COMPATIBILITY.md`](docs/FORMAT_COMPATIBILITY.md) "L1 measurement sidecar"; code on each side, v5 stream frames, HLG name checks: [`docs/MKVDOVI_PIPELINE.md`](docs/MKVDOVI_PIPELINE.md) "L1 sidecar contract".
- **`+cuda` probe:** `hdr_analyzer_mvp --version` contains `+cuda` iff built with the `cuda` feature (`cli.rs` `VERSION`); mkvdovi reads it for GPU analysis availability.
- **Analyzer `--help`:** mkvdovi always passes `--transfer hlg` for inputs MediaInfo classifies as HLG and hard-fails on an analyzer without the flag. For HLG it requires `--help` to name the selected composer's `Composer::luminance_mapping()`, for every composer including preset (`external::analyzer_lists_option`, `pipeline/measure.rs` `run_fresh_analysis`), and then always passes `--hlg-composer`; an analyzer that does not name it is refused before any analysis.
- **CPU ↔ CUDA:** `kernels.cu` and `analysis/gpu/mod.rs` keep the result-buffer layout (counts, then u64 sums at `SUMS_WORD`) and the `dovi_params` layout (`DOVI_*`) in sync; the CPU mirrors the kernel's f32 arithmetic operation for operation.
- **Resume fingerprint:** a new mkvdovi flag that changes any artifact goes into `pipeline::resume_settings`; the string fingerprints users' temp directories.

## Real entrypoints and boundaries

- **Analyzer:** `hdr_analyzer_mvp/src/main.rs` → `pipeline::run`; analysis in `analysis/` (frame, histogram, scene, hlg, gpu) plus `crop.rs`, `optimizer.rs`, `ffmpeg_io.rs`, `l1_sidecar.rs`, `writer.rs`. HLG is measured through the Profile 8.4 decode of the composer `--hlg-composer` selects (`bt2100` default, `preset` fallback); since P8 max-RGB follows the spec 4:2:0 structure in `code / 1023` f32, clamped to [62, 3079] on purpose. Only PQ/HLG is analyzed; transfer detection is `ffmpeg_io::resolve_transfer`, overridden by `--transfer`. Details: [`docs/TECHNICAL_REFERENCE.md`](docs/TECHNICAL_REFERENCE.md) §1.5, [`docs/HLG_COMPOSER.md`](docs/HLG_COMPOSER.md).
- **CUDA backend** (`cuda` feature, off by default; `--hwaccel cuda`; `analysis/gpu/` + NVRTC-compiled `analysis/kernels.cu`). Rules ([`docs/CUDA_PIPELINE.md`](docs/CUDA_PIPELINE.md) has the design, the code map and the full list):
  - `GpuAnalyzer::new` stays ahead of `setup_hardware_decoder` (shared primary context).
  - Never hand an `AV_PIX_FMT_CUDA` frame to host code; download through `host_view`.
  - A failed CUDA call stops the run when frames come from NVDEC; a download is no health check.
  - The kernel strides full-resolution frames, so its crop rect is in full-res coordinates (`pipeline/crop_space.rs`).
  - Bit identity with the CPU: same f32 operation order, `_rn` intrinsics, no FMA, order-independent cross-thread reductions. Validated bit-identical L1 vs. CPU, the HLG max-RGB decode too. `--pre-denoise median3` is CPU-only. `HDR_ANALYZER_CUDA_HOST_FRAMES` forces the download path.
  - Benchmark with `--no-crop` and report frames ÷ wall time; `--profile-performance` "Analysis" fps excludes decode.
- **mkvdovi:** `mkvdovi/src/main.rs` → `pipeline::convert_file`; module map in [`docs/MKVDOVI_PIPELINE.md`](docs/MKVDOVI_PIPELINE.md).
- **verifier:** `verifier/src/main.rs`, standalone measurement-validator CLI.

## mkvdovi operational gotchas (easy to miss)

Each rule is one line; the reasons and code paths are in [`docs/MKVDOVI_PIPELINE.md`](docs/MKVDOVI_PIPELINE.md). Read it before changing mkvdovi.

- **Never encodes video:** every path copies the video stream bit-exactly; no compositor, encoder or `--fel-*`/`--encoder` flag. Do not add one back.
- **Deletes the source after a successful conversion** unless `--keep-source`; DV and repair inputs keep their source.
- **Refuses before any temp work:** Profile 7 FEL (with or without `--mdfix`; plan in [`docs/FEL_PLAN.md`](docs/FEL_PLAN.md)); PQ tagged full range for every input whose 8.1 RPU it generates, not the MEL fast path (`metadata::check_pq_range_contract`); HLG tagged full range, with a matrix other than BT.2020 NCL (CL included) or primaries other than BT.2020 (`metadata::check_hlg_colour_contract`; untagged fields pass with a warning). `--mdfix` on an 8.4 input is refused later, in `acquire_measurements`.
- **Never add a silent clamp:** mkvdovi meets out-of-range metadata with a warning or a refusal (HDR10+ scene peaks above 3× the mastering peak only warn; the panel peak is not a `--trim-targets` override). The generator's own L1 clamps (below) are reported by `--verify`.
- **Resume:** interrupted runs auto-resume from `mkvdovi_temp_*` (`resume.rs`, `resume.json` fingerprint); `config::withdraw_stale_rpu` withdraws a stale RPU after re-analysis; `--no-resume` forces a clean run.
- **External tools** are checked at runtime (`ffmpeg`, `mkvmerge`, `dovi_tool` ≥ 2.3.2, `mediainfo` or `ffprobe`; `hdr10plus_tool` for HDR10+). `run_command`/`run_command_with_spinner` must leave the child's pipes attached: taking them out deadlocks above 64 KB of output.
- **Measurements** are reused only when the sidecar validates against the input; `--analysis-quality accurate` is the only preset where CPU and CUDA match. `--hwaccel auto` resolves once in `pipeline::resolve_auto_settings`; it never selects an encoder.
- **Source range:** 8.1 passes `source_min_pq`/`source_max_pq` explicitly (`metadata::source_range_pq_81`); **never pass them for 8.4** (`metadata::source_range_pq_84`).
- **Generator clamps:** `dovi_tool generate` writes L1 min ≤ 12, max ≥ 2081, avg ≥ 819 codes; a measured value outside does not reach the RPU.
- **`--verify`** hard-fails malformed metadata, frame-count mismatches, a wrong CM version, an HLG composer mismatch, and (generated RPUs) a source range or L1 that differs from the sidecar after the clamp.
- **`--mdfix`** replaces authored L2/L3 with neutral ones and drops L4/L8/L10 (under `--cm-version v29` also L2, L3, L9, L11, L254); it prints which levels it replaces or drops.
- **libplacebo** reads only L1 max/avg, the source range and the reshaping, so a libplacebo render cannot test L1 min, L2, L3, L5, L6, L8, L9 or L11.
- The `dolby_vision` crate 3.4.0 cannot parse L253 (re-bump when it can) and stores reshaping pivots delta-coded.
- Generated levels (CM v4.0 default): L1, L2 (100/600/1000-nit trims), L6, L9, L11; `--cm-version v29` writes L1, L5, L6 only.

## Testing quirks

- Many integration tests **skip** without their tools or media (`dovi_tool`, ffmpeg with libx265, mkvmerge, mediainfo, `hdr10plus_tool`, a sibling analyzer, `../tests/hdr-media/`); a skip is not a pass. Per-test conditions: [`docs/TESTING.md`](docs/TESTING.md) §6. Do not assume the workspace tests are hermetic on a clean machine.
- After a change to `analysis/`, `kernels.cu` or `ffmpeg_io.rs`, run `scripts/cuda-parity.sh` on a CUDA host (CPU vs CUDA `.bin` bytes and sidecars, both HLG composers).
- **L1 regression gate** (CI `l1-regression`, `scripts/ci/l1-regression-gate.sh`): a change that moves L1 on purpose needs `--update` and the reference diff in the PR. `scripts/rpu-baseline.sh capture|compare` checks the final RPU of real media against baselines outside the repo.
- **Real-content corpus** (`~/mkvdovi-work/corpus/`, local only): media and file names never enter the repo; do not name the scoring script in files on `main` (a pre-push guard refuses it); Dolby tool output only scores, never tunes; holdout cuts are not analyzed during development. Details: [`docs/TESTING.md`](docs/TESTING.md) §7, [`docs/PROVENANCE.md`](docs/PROVENANCE.md).
- How tests are written, reviewed and mutation-checked: [`docs/TESTING.md`](docs/TESTING.md); audit status: ROADMAP E14.

## Lint/style policy in this repo (non-default)

- Workspace lints in root `Cargo.toml`: clippy `all` allowed but `correctness` denied; `dbg_macro` denied; rustc `rust_2018_idioms` and `unused_qualifications` warn — all fail under `-D warnings` in CI. `unwrap`/`expect`/`print` are allowed everywhere (the `allow-*-in-tests` keys in `clippy.toml` have no effect).
- `unsafe_op_in_unsafe_fn = "deny"`, but `unsafe_code` is **allowed**. `clippy.toml` raises the complexity thresholds.
- `kernels.cu` is only compiled by NVRTC at runtime. The Stop hook's `nvcc -ptx` catches syntax and type errors, but nvcc is not the NVRTC the binary loads, so a `--hwaccel cuda` run (`scripts/cuda-parity.sh`) is the real check.
- Imports: std → third-party → `crate::`, grouped with braces, absolute `crate::` paths internally. `anyhow::Result` + `.context()` at app level; `thiserror` for library error types.

## Module size

- A Rust source file stays at or under about 800 code lines; inline `#[cfg(test)] mod` blocks do not count. The Stop hook notes a touched file over the limit once per session. Long functions matter as much: split a function by phase before it passes a few hundred lines.
- Over the limit, split in its own commit (a pure move), never in the same commit as a behavior change: (1) move the inline test module to `<module>/tests.rs` (`#[cfg(test)] mod tests;`, `use super::*` still reaches private items); (2) turn `foo.rs` into `foo/mod.rs` plus a few cohesive submodules of the same crate, never a new crate (LTO and the CPU/CUDA bit-identity rely on intra-crate inlining); (3) keep every existing path valid with `pub use` re-exports in the parent, because code, this file, `docs/` and `.claude/` name items as `module::item`; widen visibility only to `pub(super)`; (4) simplify in a later commit.
- Proof for a pure move: `git diff --color-moved=dimmed-zebra` shows only moved blocks; the gates of the touched paths pass (`.claude/workflows/pre-pr-panel.js` treats `foo.rs` and `foo/` alike); the L1 gate passes without `--update`; for mkvdovi, `scripts/rpu-baseline.sh compare --require-identical-l1` against the E13 baselines in `~/mkvdovi-work/rpu-baseline/refactor-e6d63ed/` is clean.
- Moves that break silently: `include_str!` paths are file-relative (`gpu/backend.rs` includes `../kernels.cu`); `#[cfg(feature = "cuda")]` items must move with their gating; f32 operation order that mirrors `kernels.cu`; the `resume_settings` string, which fingerprints users' temp directories; line-number references in `ROADMAP.md` and `docs/`.

## Docs vs code

Some prose docs are stale (e.g., references to an old Python `mkvdovi` workflow). Prefer executable truth: root/workspace configs, current Rust crates under `*/src`, and the GitHub Actions workflows — in that order — over narrative docs.

## Releases

[`docs/RELEASING.md`](docs/RELEASING.md) has the process, the version rules, what `release.yml` checks and the bad-release runbook; which roadmap steps ship in which release is the **Release plan** in `ROADMAP.md`. The five shipped crates always carry the same version, an rc included (`resume.rs` fingerprints `CARGO_PKG_VERSION`). Tag only after the `release.yml` dry run on HEAD and `scripts/release-check.sh vX.Y.Z` pass. Never re-tag a published release; ship the next patch.

## Code search and reading

- **Default: `rg` with the exact name**, then read the line range you need. The repo's names are distinctive enough that this is the fast path (a 2026-10-24 audit of 219 sessions: 0 LSP calls, Edit failure rate 0.7%).
- **Load LSP (`ToolSearch` `select:LSP`) for two jobs only:** `findReferences`/`incomingCalls` before changing a pub signature, struct field or enum variant (`incomingCalls` also lists test callers, the cheapest coverage check), and `hover` for a dependency type or signature instead of reading `~/.cargo/registry`. Positions are 1-based and must land on the symbol; "no definition found" usually means a wrong column. `workspaceSymbol` returns empty for about 45 s after a cold start. Skip `goToImplementation`: the repo declares no traits, so it lists the whole ecosystem's impls.
- **Cross-binary contracts are invisible to LSP.** `L1Sidecar` is defined three times (`hdr_analyzer_mvp/src/l1_sidecar.rs`, `mkvdovi/src/metadata/sidecar.rs`, `tools/l1_diff/src/main.rs`) with no crate link between them; the `+cuda` version probe, the `--help` composer names and the `kernels.cu`/`gpu/mod.rs` layouts are the same kind. `rg -w` every side.
- **Big docs: outline first, never a whole-file read.** `ROADMAP.md` (~65 KB), `docs/ROADMAP_LOG.md` (~123 KB) and `CHANGELOG.md` (~64 KB): `rg -n '^#{1,4} ' <file>` (or `rg -n '<item ID>'`), then read only that section.
- Reading code you are about to change is not a search; keep it in the main context. Delegate to Explore only for a "where/how" question across many files whose answer is a conclusion, not code to edit (e.g. plugin or tool config spelunking).
