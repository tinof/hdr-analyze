# roadmap-next config

Project-specific rules for the global `/roadmap-next` skill
(`~/.claude/skills/roadmap-next/SKILL.md`). Keep the `##` headings exactly as written: the skill
looks them up by name. A heading that is missing or whose body is `none` turns that part off.
Changing this file forces the next `/roadmap-next` run to rank afresh (its hash is recorded in
Next up).

## Probes

- `gpu: nvidia-smi -L || /usr/lib/wsl/lib/nvidia-smi -L` passes when the CUDA host's GPU is
  visible (the RTX 4070 WSL2 box). The Oracle ARM and macOS hosts have none.
- `corpus: test -d ~/mkvdovi-work/corpus/dev` passes when the local development corpus is present.

## Readiness rules

- A change to any path the **clippy-cuda + CUDA parity** gate lists under Gates (the analysis and
  decode path, `dovi84_composer/src/`, `scripts/cuda-parity.sh`,
  `hdr_analyzer_mvp/tests/cuda_parity.rs` and the build inputs) needs `scripts/cuda-parity.sh`
  (probe `gpu`).
- A step whose gate is scored on real content needs the development corpus (probe `corpus`).

## Material

- `~/mkvdovi-work/corpus/dev`: the 45 development cuts (`input.mkv`, `manifest.json`, `truth/`,
  `baseline/`). The holdout cuts under `~/mkvdovi-work/corpus/qual/holdout` are never analyzed
  during development.
- `~/mkvdovi-work/rpu-baseline/`: RPU baselines for `scripts/rpu-baseline.sh compare`.
- `test -e` every media or evidence path an item names.

## Gates

The pre-PR review computes them with `.claude/workflows/pre-pr-panel.js` (`gatesOnly: true`);
name them in the draft so the cost of each step is visible.

- **Tests** (`fmt`, `clippy`, `test`): `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; Rust or
  Cargo files outside `tools/`, plus `.cargo/`, `rust-toolchain.toml`, `clippy.toml` and
  `rustfmt.toml`. Integration tests with media and tools skip without them, and CI fails on an
  unexpected `Skipping` line.
- **Tool crates** (`tool:<crate>`): `cargo fmt`/`clippy -D warnings`/`test` with
  `--manifest-path tools/<crate>/Cargo.toml`; any `.rs` or `Cargo.toml`/`Cargo.lock` under
  `tools/<crate>/`. The tool crates are outside the workspace, so the Tests gate never builds them.
- **clippy-cuda + CUDA parity** (`scripts/cuda-parity.sh`): `hdr_analyzer_mvp/src/analysis/`,
  `hdr_analyzer_mvp/src/{ffmpeg_io,pipeline,cli,crop}` (`.rs` or directory; not mkvdovi's
  `pipeline/`), `dovi84_composer/src/`, `scripts/cuda-parity.sh`,
  `hdr_analyzer_mvp/tests/cuda_parity.rs`, and the build inputs (root `Cargo.toml`/`Cargo.lock`,
  the analyzer and composer manifests, `.cargo/config.toml`, `rust-toolchain.toml`).
- **L1 regression** (`scripts/ci/l1-regression-gate.sh`): `hdr_analyzer_mvp/src/`,
  `dovi84_composer/src/`, `tools/l1_diff/`, the gate script, and the build inputs.
- **gemini-harness**: `.claude/workflows/pre-pr-gemini.*`, `.claude/workflows/tests/`,
  `.agents/agents/pre-pr-reviewer.md`.

## Branching

- Prefixes: `feat/…`, `fix-…`, `docs/…`, `chore/…`, `test/…`, `refactor/…` (check
  `gh pr list --state merged --limit 10`).
- Sharing rule: two steps may share a branch when they are in the same crate, need the same test
  gate and change no L1.
- Hand-off: the step branch goes out with the step's code through `/codex-ship`;
  `/pre-pr-review` and `/codex-ship` work on a branch that already exists.
- Exceptions: a step that moves L1 always has its own branch. A module-size split (E13) is a pure
  move in its own commit and needs no other branch open in the files it moves.

## Release

- Release doc: `docs/RELEASING.md`; classify `Release: minor | patch | none` by its **Versions**
  table, and check its **When to release** triggers for answer item 5.
- Changelog heading for unreleased work: `## [Unreleased]` in `CHANGELOG.md`.
- The **Release plan** table in `ROADMAP.md` names which steps ship in which release.

## Log

- Measurements and long evidence go to `docs/ROADMAP_LOG.md`, not into the item.

## Codex notes

- Codex's sandbox has no GPU and no access to `~/mkvdovi-work`; CUDA parity and corpus checks are
  decided by the session's probes.
- Media file names stay local; titles may be named. Holdout cuts are never analyzed during
  development (`docs/PROVENANCE.md`).

## Push policy

yes

<!-- A "no step can start" run pushes its ROADMAP.md commit to main. Where the pre-commit hooks are
installed, their push stage skips the tests for a Markdown-only push. -->
