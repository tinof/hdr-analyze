# CI and local checks

What runs where, and why. `CLAUDE.md` ("CI behavior that affects edits") keeps the rules that change
how an edit is made; the release workflow is in [RELEASING.md](RELEASING.md).

## `ci.yml`

- **Triggers:** pushes to `main` and `fix-*`, and PRs to `main`. Other branches (for example
  `perf/*`) get CI only through a PR.
- **Job order:** `changes` → lint (fmt + clippy, including clippy with the analyzer's `cuda`
  feature) → test, `l1-regression`, `rpu-builder` and the cross-platform release build
  (Ubuntu/macOS/Windows) side by side. The build does not wait for test.
- **`changes`** (`scripts/ci/ci-changes.sh`, tested by `scripts/ci/tests/ci-changes-test.sh` in the
  same job) reads the PR's files. A PR that changes only `*.md`, `docs/` or `LICENSE` (the
  `gate-skip` set) skips every Rust job, and `changes` is then its one passing check. `/ship` needs
  at least one check, so never move this filtering to workflow-level `paths:`. `gemini-harness`
  runs only when its gate paths change; `cargo audit`/`cargo deny` on PRs only when a manifest, a
  `Cargo.lock` or `deny.toml` changes. When the file list cannot be read, every job runs.
- Push to main and `workflow_dispatch` run everything. A weekly `schedule` runs only the advisory
  jobs. The concurrency group includes the event, and `scripts/release-check.sh` counts only
  push/dispatch runs.
- **apt:** every apt install goes through `.github/actions/apt-install` (`scripts/ci/apt-install.sh`).
  The hosted Ubuntu mirrors can trickle for minutes (actions/runner-images#14823; 304–531 s per job
  before), so `update` and the download run under a time limit with a retry on another mirror, and
  the `.deb` files are cached per image and package set (saved outside PRs only).
- **Caches:** rust-cache saves only on main (`save-if`); PRs restore main's caches.
- **test** installs the integration-test tools (`.github/actions/setup-test-tools`: ffmpeg CLI,
  mkvtoolnix, mediainfo, pinned `dovi_tool` and `hdr10plus_tool`; `release.yml` uses the same
  action, so bump the pins there) and fails on any unexpected `Skipping` line
  (`scripts/ci/check-test-skips.sh`), as `release.yml` does. Before 2026-10-15 it installed none of
  them, so the HLG, open-GOP, P7 full-range refusal (`pq_input_contract`) and clip tests passed as
  skips in PR CI.
- **`l1-regression`:** `scripts/ci/l1-regression-gate.sh` plus fmt/clippy/test for `tools/l1_diff`
  (outside the workspace); see `CLAUDE.md` "Testing quirks".
- **`gemini-harness`** is the fake-agy test of the Gemini pre-PR pass
  (`.claude/workflows/tests/pre-pr-gemini-test.sh`; no Rust).
- **Security and dependencies:** `cargo audit`, and `cargo deny check` for `advisories` (allowed to
  fail, `continue-on-error`) and `bans licenses sources` (must pass). Exceptions live in
  `deny.toml` (including the ignored `RUSTSEC-2025-0119` for `indicatif`, and `WTFPL` allowed for
  `ffmpeg-next`).
- **FFmpeg setup:** one composite action, `.github/actions/setup-ffmpeg`, sets up FFmpeg dev libs,
  `clang`/`libclang` and `BINDGEN_EXTRA_CLANG_ARGS` per OS for both workflows:
  - Linux: apt including `libclang-dev` through `.github/actions/apt-install`, with
    `extra-packages` for the ffmpeg CLI or the ARM64 `clang`.
  - macOS: `brew install ffmpeg`, with `FFMPEG_DIR` set to the version-independent
    `$(brew --prefix ffmpeg)` symlink, so a rust-cache-restored `ffmpeg-sys-next` build does not
    link a Cellar path that a Homebrew bump removed.
  - Windows: a checksum-verified BtbN LGPL shared build through `FFMPEG_DIR`. The Windows release
    zip bundles the FFmpeg DLLs through `scripts/ci/bundle-windows-ffmpeg.sh`, which CI smoke-tests
    on every Windows build.
  - `ffmpeg-next` 9 is built with only `codec`/`format`/`software-scaling`, so the
    avfilter/avdevice/swresample dev libs are not needed.

## Pre-commit hooks

`.pre-commit-config.yaml`: **on commit**, fmt check + clippy deny-warnings; **on push**,
`cargo test --workspace -q`, skipped when every pushed file matches `\.md$|^docs/|^LICENSE$`. The
hooks run only where pre-commit is installed
(`pre-commit install --hook-type pre-commit --hook-type pre-push`).

## Claude Code feedback loop (`.claude/`)

- **Per edit:** rust-analyzer (the `rust-analyzer-lsp` plugin) pushes diagnostics after each
  Edit/Write, including `cargo check` flycheck results (type and borrow errors, tagged `(rustc)`).
  It needs the rustup component: `rustup component add rust-analyzer`. Without it,
  `~/.cargo/bin/rust-analyzer` is only the rustup proxy, the LSP server crashes with exit code 1,
  and no diagnostics arrive.
- **End of turn:** `.claude/hooks/rust-check.sh`, wired in `.claude/settings.json`. PostToolUse
  records touched `.rs`/`.cu`/Cargo/lint-config files. The Stop hook then rustfmts those files
  without reporting, and runs `cargo clippy --workspace --all-targets` (plus
  `--manifest-path tools/<crate>/Cargo.toml` for a touched tool crate). It feeds back only errors
  from any file and warnings in touched files. A touched `.cu` kernel is also compiled with
  `nvcc -ptx` (from PATH or `/usr/local/cuda/bin`; skipped when there is no toolkit), and its errors
  and warnings are fed back. Turns without Rust or `.cu` edits never start cargo. After 3 feedback
  rounds it tells the user and stays quiet until the next Rust or `.cu` edit. State lives in
  `${TMPDIR:-/tmp}/claude-rust-check/<session_id>/`. An incremental run takes about 2–4 s on the
  `/mnt/c` checkout. Tests do not run here; they run in the pre-push hook (where installed), the
  pre-PR test gate and CI.
- **Size note:** the global ship plugin hook (`~/.claude/skills/ship/scripts/size-check.sh`,
  wired in `~/.claude/settings.json`) reads `.claude/workflow.md` "## Size limits" and notes a
  touched `.rs` file over 800 code lines once per session; it never blocks. It is not part of this
  repo: a host without the synced plugin gets no note, and `/ship` still runs the same check.
- The hook does not lint the `cuda`-gated Rust code; CI's lint job does
  (`cargo clippy -p hdr_analyzer_mvp --all-targets --features cuda -- -D warnings`).
