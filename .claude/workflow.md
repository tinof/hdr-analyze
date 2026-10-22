# Workflow contract

What `/ship`, `/ship:roadmap-next` and `/ship:roadmap-init` need to know about this repository. The schema,
with every key and section, is `~/.claude/skills/ship/reference/contract.md`. Keep every `##` heading
exactly as written: the skills look sections up by name. A missing section, or one whose body is `none`,
turns that part off. A diff that touches this file is always Tier 2.

## Settings

```
contract:       1
repo:           tinof/hdr-analyze
base:           main
merge:          squash
branch:         <type>/<id>-<slug>
roadmap:        ROADMAP.md
roadmap-log:    docs/ROADMAP_LOG.md
roadmap-memory: on
push-roadmap:   yes
docs-prefix:    "docs(roadmap): "
gate:           cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && scripts/ci/test-gate.sh
gate-focused:   scripts/ci/test-gate.sh target/test-gate-focused.log <pattern>
gate-fallback:  none
gate-skip:      '*.md' 'docs/**'
ci:             pr-checks
logic:          '*.rs' '*.cu' 'Cargo.toml' '*/Cargo.toml' '*.sh' '*.js' '*.py'
tests:          '*/tests/**' '*/tests.rs' '*_tests.rs'
self-check:     skill:pre-pr-review
```

- With `merge: squash` the PR body becomes the commit body: keep it plain prose.

## Gates

- `clippy-cuda`: `cargo clippy -p hdr_analyzer_mvp --all-targets --features cuda -- -D warnings`; paths `'Cargo.toml'` `'Cargo.lock'` `'hdr_analyzer_mvp/Cargo.toml'` `'dovi84_composer/Cargo.toml'` `'.cargo/config.toml'` `'rust-toolchain.toml'` `'hdr_analyzer_mvp/src/analysis/'` `'hdr_analyzer_mvp/src/ffmpeg_io.rs'` `'hdr_analyzer_mvp/src/ffmpeg_io/'` `'hdr_analyzer_mvp/src/pipeline.rs'` `'hdr_analyzer_mvp/src/pipeline/'` `'hdr_analyzer_mvp/src/cli.rs'` `'hdr_analyzer_mvp/src/cli/'` `'hdr_analyzer_mvp/src/crop.rs'` `'hdr_analyzer_mvp/src/crop/'` `'dovi84_composer/src/'` `'scripts/cuda-parity.sh'` `'hdr_analyzer_mvp/tests/cuda_parity.rs'`; where `local`.
- `cuda-parity`: `scripts/cuda-parity.sh`; paths `'Cargo.toml'` `'Cargo.lock'` `'hdr_analyzer_mvp/Cargo.toml'` `'dovi84_composer/Cargo.toml'` `'.cargo/config.toml'` `'rust-toolchain.toml'` `'hdr_analyzer_mvp/src/analysis/'` `'hdr_analyzer_mvp/src/ffmpeg_io.rs'` `'hdr_analyzer_mvp/src/ffmpeg_io/'` `'hdr_analyzer_mvp/src/pipeline.rs'` `'hdr_analyzer_mvp/src/pipeline/'` `'hdr_analyzer_mvp/src/cli.rs'` `'hdr_analyzer_mvp/src/cli/'` `'hdr_analyzer_mvp/src/crop.rs'` `'hdr_analyzer_mvp/src/crop/'` `'dovi84_composer/src/'` `'scripts/cuda-parity.sh'` `'hdr_analyzer_mvp/tests/cuda_parity.rs'`; where `probe:gpu`.
- `l1-regression`: `scripts/ci/l1-regression-gate.sh`; paths `'Cargo.toml'` `'Cargo.lock'` `'hdr_analyzer_mvp/Cargo.toml'` `'dovi84_composer/Cargo.toml'` `'.cargo/config.toml'` `'rust-toolchain.toml'` `'hdr_analyzer_mvp/src/'` `'dovi84_composer/src/'` `'tools/l1_diff/'` `'scripts/ci/l1-regression-gate.sh'`; where `local`.
- `tool:compare_baseline`: `cargo fmt --manifest-path tools/compare_baseline/Cargo.toml -- --check && cargo clippy --manifest-path tools/compare_baseline/Cargo.toml --all-targets -- -D warnings && cargo test --manifest-path tools/compare_baseline/Cargo.toml`; paths `'tools/compare_baseline/*.rs'` `'tools/compare_baseline/*Cargo.toml'` `'tools/compare_baseline/*Cargo.lock'`; where `local`.
- `tool:fit_hlg_composer`: `cargo fmt --manifest-path tools/fit_hlg_composer/Cargo.toml -- --check && cargo clippy --manifest-path tools/fit_hlg_composer/Cargo.toml --all-targets -- -D warnings && cargo test --manifest-path tools/fit_hlg_composer/Cargo.toml`; paths `'tools/fit_hlg_composer/*.rs'` `'tools/fit_hlg_composer/*Cargo.toml'` `'tools/fit_hlg_composer/*Cargo.lock'`; where `local`.
- `tool:l1_diff`: `cargo fmt --manifest-path tools/l1_diff/Cargo.toml -- --check && cargo clippy --manifest-path tools/l1_diff/Cargo.toml --all-targets -- -D warnings && cargo test --manifest-path tools/l1_diff/Cargo.toml`; paths `'tools/l1_diff/*.rs'` `'tools/l1_diff/*Cargo.toml'` `'tools/l1_diff/*Cargo.lock'`; where `local`.
- `tool:rpu_builder_check`: `cargo fmt --manifest-path tools/rpu_builder_check/Cargo.toml -- --check && cargo clippy --manifest-path tools/rpu_builder_check/Cargo.toml --all-targets -- -D warnings && cargo test --manifest-path tools/rpu_builder_check/Cargo.toml`; paths `'tools/rpu_builder_check/*.rs'` `'tools/rpu_builder_check/*Cargo.toml'` `'tools/rpu_builder_check/*Cargo.lock'` `'dovi_rpu_builder/'`; where `local`.
- `gemini-harness`: `.claude/workflows/tests/pre-pr-gemini-test.sh`; paths `'.claude/workflows/pre-pr-gemini.*'` `'.claude/workflows/tests/'` `'.agents/agents/pre-pr-reviewer.md'`; where `local`.

## Tier 2 paths

```
Cargo.toml
Cargo.lock
hdr_analyzer_mvp/Cargo.toml
dovi84_composer/Cargo.toml
.cargo/config.toml
rust-toolchain.toml
hdr_analyzer_mvp/src/**
dovi84_composer/src/**
tools/l1_diff/**
scripts/ci/l1-regression-gate.sh
scripts/cuda-parity.sh
hdr_analyzer_mvp/tests/cuda_parity.rs
mkvdovi/src/metadata.rs
mkvdovi/src/metadata/**
mkvdovi/src/pipeline.rs
mkvdovi/src/pipeline/**
mkvdovi/src/external.rs
mkvdovi/src/external/**
mkvdovi/src/resume.rs
mkvdovi/src/resume/**
dovi_rpu_builder/src/**
```

The build inputs can change measured output without a code change. The analyzer, the composer, `l1_diff` and
the two gate scripts decide the L1 output, CPU/CUDA bit identity and how L1 is scored. The mkvdovi modules
hold the cross-binary contracts (L1 sidecar, `--help` probes, `+cuda` probe) and the `resume_settings`
fingerprint; `dovi_rpu_builder` defines mkvdovi's `extra.json` (which resume compares) and the RPUs the NuvioTV
fork delivers. The set is the `highRisk` set of `.claude/workflows/pre-pr-panel.js`; `l1_sidecar` is inside
`hdr_analyzer_mvp/src/**`.

## Routing

default

## Testing rules

- A skipped test is not a passing test. Many tests print `Skipping` and return when a tool, a sibling
  analyzer or media is missing; a run counts only when `scripts/ci/check-test-skips.sh` passes on its log.
  `scripts/ci/test-gate.sh` and CI apply that check.
- A bug fix starts with a test that fails on the parent commit and passes after the fix. The PR body says so.
- Never delete, skip or loosen an existing test to make a change pass.
- Mock only at external boundaries: tools on `PATH` (a PATH sandbox, as in `mkvdovi/tests/pq_input_contract.rs`;
  the fake-tool shims in `mkvdovi/src/pipeline/tests.rs`), the filesystem and the clock. Do not fake a Dolby
  Vision or HEVC input; build a small fixture with `ffmpeg`/`dovi_tool`, or mark the test env-gated.
- Test both sides of every decision, and take expected values from an independent source, never from the
  function under test.
- Analyzer numerics (`analysis/`, `kernels.cu`) are guarded by `scripts/cuda-parity.sh` and the L1
  regression gate. A change that moves L1 on purpose runs `scripts/ci/l1-regression-gate.sh --update` and
  shows the reference diff in the PR body.
- CI has no GPU. The CUDA path is only tested by `scripts/cuda-parity.sh` on the CUDA host; a finding that
  only it can settle is NEEDS-HUMAN off that host.
- Full rules: `docs/TESTING.md`; skips per test: CLAUDE.md "Testing quirks".

## Probes

- `gpu: nvidia-smi -L || /usr/lib/wsl/lib/nvidia-smi -L` passes when the CUDA host's GPU is visible (the
  RTX 4070 WSL2 box). The Oracle ARM and macOS hosts have none.
- `corpus: test -d ~/mkvdovi-work/corpus/dev` passes when the local development corpus is present.

## Tags

- `gpu`: needs probe `gpu`
- `corpus`: needs probe `corpus`

## Readiness rules

- A change to any `cuda-parity` path under `## Gates` (the analysis and decode path, `dovi84_composer/src/`,
  `scripts/cuda-parity.sh`, `hdr_analyzer_mvp/tests/cuda_parity.rs` and the build inputs) needs
  `scripts/cuda-parity.sh` (probe `gpu`).
- A step whose gate is scored on real content needs the development corpus (probe `corpus`).

## Material

- `~/mkvdovi-work/corpus/dev`: the 45 development cuts (`input.mkv`, `manifest.json`, `truth/`,
  `baseline/`). The holdout cuts under `~/mkvdovi-work/corpus/qual/holdout` are never analyzed during
  development.
- `~/mkvdovi-work/rpu-baseline/`: RPU baselines for `scripts/rpu-baseline.sh compare`.

## Branching

- Types: `feat/`, `fix/`, `docs/`, `chore/`, `test/`, `refactor/` (check `gh pr list -R tinof/hdr-analyze
  --state merged --limit 10`). Older fix branches use `fix-…`; CI also runs on pushes to `fix-*`, but every
  PR to `main` gets CI either way.
- Sharing rule: two steps may share a branch when they are in the same crate, need the same gates and change
  no L1.
- Exceptions: a step that moves L1 always has its own branch. A module-size split (E13) is a pure move in its
  own commit and needs no other branch open in the files it moves.

## Release

- Release doc: `docs/RELEASING.md`; classify `Release: minor | patch | none` by its **Versions** table, and
  check its **When to release** triggers.
- Changelog heading for unreleased work: `## [Unreleased]` in `CHANGELOG.md`.
- The **Release plan** table in `ROADMAP.md` names which steps ship in which release.

## Codex notes

- Codex's sandbox has no GPU and no access to `~/mkvdovi-work`; CUDA parity and corpus checks are decided by
  the session's probes.
- Media file names stay local; titles may be named. Holdout cuts are never analyzed during development
  (`docs/PROVENANCE.md`).

## Reminders

none

## Notes

`## Gates` and `## Tier 2 paths` copy the path rules of `.claude/workflows/pre-pr-panel.js` (the gate set
near the top, `highRisk` below it). Keep the two in sync: a path added on one side goes to the other in the
same PR. `/pre-pr-review` standalone still computes its gates from the workflow; `/ship` reads them here.

`gate` runs on every PR that is not docs-only (`gate-skip`). The workflow ran fmt, clippy and the tests only
when a workspace Rust or Cargo path changed, so a PR that touches only scripts or `.claude/` now runs the
full cargo gate too. `scripts/ci/test-gate.sh` is the `test` block of `/pre-pr-review` step 5 with the skip
check of `scripts/ci/check-test-skips.sh`.

`cuda-parity` is not in CI (no GPU runner). Off the CUDA host it stays pending, the PR gets
`review:needs-human`, and the owner decides. `clippy-cuda`, `l1-regression`, `tool:l1_diff` and
`gemini-harness` also run in CI; they are `local` so that a failure stops `/ship` before the push.

`gate-focused` runs `scripts/ci/test-gate.sh` with a test filter: it builds the analyzer next to mkvdovi and
applies the same skip check, so a focused test that prints `Skipping` fails instead of passing silently. Do not
narrow it with `-p <crate>`: `cargo test -p mkvdovi` would not rebuild the sibling analyzer.

The roadmap item names its media and evidence paths; `test -e` each one before a step starts. Measurements
and long evidence go to `docs/ROADMAP_LOG.md`, not into the item. `push-roadmap: yes`: where the pre-commit
hooks are installed, their push stage skips the tests for a Markdown-only push.
