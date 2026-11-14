# Releasing

How and when `hdr-analyze` makes a release. Which roadmap steps go into which release is in the
**Release plan** section of [`ROADMAP.md`](../ROADMAP.md).

## Terms

- **Tag**: a name for one commit, such as `v0.6.0`. Pushing a `v*.*.*` tag starts
  `.github/workflows/release.yml`.
- **Release**: the GitHub page for a tag, with notes and the downloadable archives.
- **Draft**: a release only maintainers can see. The workflow keeps the release a draft until
  every asset is attached, then publishes it.
- **Pre-release**: a published release that is never "latest". Tags with a suffix
  (`v0.6.0-rc.1`) become pre-releases.
- **Latest**: the newest published release that is not a pre-release. `install.sh` installs it,
  so whatever becomes latest reaches users at once.
- **Immutable release** (repository setting): after publishing, the tag cannot move and the assets
  cannot change. A mistake is fixed by a new release, never by re-tagging.

## Versions

Versions follow SemVer before 1.0: `0.MINOR.PATCH`. `hdr_analyzer_mvp`, `mkvdovi`, `verifier`,
`dovi84_composer` and `dovi_rpu_builder` always carry the same version. The `tools/*` crates do not ship and keep their
own versions.

| Change since the last release | Bump |
|---|---|
| A CHANGELOG entry marked **Breaking**; a removed or renamed CLI flag; a changed default | MINOR |
| A cross-binary contract: L1 sidecar `version` or a new `luminance_mapping` value, the `+cuda` version probe, analyzer `--help` text that mkvdovi checks, the `resume_settings` string | MINOR |
| Delivered L1/L6/RPU moves because a default or method changed (estimator, composer, sampling preset) | MINOR |
| A correctness fix, also one that moves delivered output, with no changed default, flag or contract | PATCH, with an "output changes" line in the notes and new RPU baselines |
| Docs, refactors, tooling, measurements, `.claude/` | no release by itself |

A release candidate carries its suffix in the manifests too (`version = "0.6.0-rc.1"`). mkvdovi
fingerprints resume directories with its version, so an rc and the final release must differ.

### Library crates

Decided by the owner on 2026-11-14 (ROADMAP P16.3). **Not in force yet:** the rules below start
with the split PR (ROADMAP P16.7). Until it merges, the five crates above keep one version and
`scripts/ci/release-version-check.sh` checks them as before.

- **Their own version line.** `dovi84_composer`, `dovi_rpu_builder` and every later library crate
  (the analysis core, ROADMAP P16.1) share one version, starting at 0.1.0. The binaries
  (`hdr_analyzer_mvp`, `mkvdovi`, `verifier`) keep theirs. The table above measures flags,
  contracts and delivered output; a library is judged by its Rust API and its C header. The
  binaries can then reach 1.0.0 without promising a library API that is still moving.
- **Git tags, not crates.io.** A library release is a tag `lib-vX.Y.Z` on `main`, and a consumer
  pins the tag instead of a bare commit. The crates stay `publish = false`. Publishing is a later
  owner step (ROADMAP P16.8), considered when all of these hold: the C ABI exists; the NuvioTV
  fork has taken one library bump without an API break; the exact `hdr10plus = "=2.1.5"` pin no
  longer blocks a consumer's dependency resolution. A published version cannot be removed.
- **Bumps.**

  | Change since the last library release | Bump |
  |---|---|
  | A removed or changed public item or signature; a `dolby_vision` major version (`dovi84_composer` exposes its types) | MINOR |
  | RPU bytes change for the same input because a method, a default or the reference `dovi_tool` version changed | MINOR, with an "output changes" line in the notes and new fixtures |
  | A correctness fix, also one that moves RPU bytes; a new public item | PATCH, with the "output changes" line when bytes move |

- **Formats keep their own versions.** The playback manifest version, the L1 sidecar version and
  the `Error::code` numbers are not the crate version. They only grow: a new format version or a
  new code is a MINOR, and an existing one never changes meaning.
- **C ABI** (ROADMAP P16.2). The first releases are labelled unstable: the header carries version
  macros, and it may change in any MINOR. Once the fork has used it for one release cycle, the
  owner declares it additive within a minor: functions are added, never removed or changed.
- The binaries build against the library crates by path, so a binary release needs no library tag.
  The split PR settles where library release notes go in `CHANGELOG.md` and how the version check
  reads two lines.

## When to release

1. **Within about a week** of merging a fix for delivered output that is wrong for every input of a
   type (ROADMAP priorities 1 to 4), or a changed contract or default.
2. **Otherwise in batches**: a PATCH when fixes have collected, at most every few weeks.
3. **Never for** refactors (E13), research behind an unfinished opt-in flag, or measurement-only
   steps.
4. **Release candidate first** (`vX.Y.0-rc.N`) only when the release needs a test outside CI
   before it becomes latest: playback on a device, CUDA on Windows, a changed release workflow.

`/ship:roadmap-next` tags each **Next up** entry `Release: minor | patch | none` and keeps the Release
plan current, so the decision is made per step, not afterwards.

## Checklist

1. **CHANGELOG.** Merge `[Unreleased]` into one heading per kind (`Added`, `Changed`, `Fixed`,
   `Removed`). Put an **Upgrade notes** list at the top: the Breaking items, and anything users must
   do. Rename `[Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` and add a new empty `## [Unreleased]`.
2. **Versions.** Set `version` in `hdr_analyzer_mvp`, `mkvdovi`, `verifier`, `dovi84_composer` and
   `dovi_rpu_builder` (with the `-rc.N` suffix for a candidate), then `cargo build` to update
   `Cargo.lock`. Also set "Current version" in `README.md` and `version` in `CITATION.cff`, and
   refresh the tool lockfiles that pin a shipped crate:
   `cargo update -p dovi84_composer --offline --manifest-path tools/fit_hlg_composer/Cargo.toml` and
   `cargo update -p dovi84_composer -p dovi_rpu_builder --offline --manifest-path tools/rpu_builder_check/Cargo.toml`.
   `scripts/ci/release-version-check.sh` fails on any of them left behind.
3. **PR.** One commit `chore(release): vX.Y.Z`, a pull request, CI green, merge.
4. **Dry run.** `gh workflow run release.yml --ref main`, then wait for it to pass
   (`gh run watch`). It runs the release gate on the merged commit without publishing.
5. **Pre-flight.** On an up-to-date `main`: `scripts/release-check.sh vX.Y.Z`. It checks the
   versions, `Cargo.lock`, the CHANGELOG section, that the tag is new, that CI passed and that a
   Release dry run (a non-push `release.yml` run) passed on HEAD. When the
   release changes analysis or mkvdovi, also run `scripts/cuda-parity.sh` and
   `scripts/rpu-baseline.sh compare` on the CUDA host.
6. **Tag.** `git tag -a vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z`.
7. **Watch** the Release run. The release appears only after every job passed. Then open the page,
   try `install.sh` on Linux and the Windows zip.
8. **Baselines.** Capture the RPU baselines for the new version
   (`~/mkvdovi-work/rpu-baseline/<version>/`).

For a candidate, after the test: a commit sets the manifests to `X.Y.Z` (plus `Cargo.lock`), then
steps 4 to 6 for `vX.Y.Z`: the final-version commit needs its own dry run. A fix found in `rc.1` goes to `main` with the manifests at `-rc.2`.

## What the workflow does

- **check**: the tag must be `vX.Y.Z` or `vX.Y.Z-rc.N`, equal to every shipped crate's version and
  `Cargo.lock`, to every `tools/*/Cargo.lock` entry for a shipped crate, to `Current version: X.Y.Z.`
  in `README.md` and to `version: X.Y.Z` in `CITATION.cff`, with a dated, non-empty CHANGELOG
  section that repeats no heading (`scripts/ci/release-version-check.sh`).
- **test**: every cargo call here and in **build** passes `--locked`, so a `Cargo.lock` that does
  not match the manifests fails before any build. fmt, clippy (also with `--features cuda`), then the full test suite with ffmpeg,
  mkvtoolnix, mediainfo and a pinned, checksummed `dovi_tool` and `hdr10plus_tool` installed
  (`.github/actions/setup-test-tools`, the same action PR CI uses). Any `Skipping` line outside an
  allow-list fails the job (`scripts/ci/check-test-skips.sh`), so the HLG and open-GOP tests cannot
  pass by skipping.
- **build**: Windows x64, macOS Intel and Apple Silicon, Linux x64 and ARM64 (on
  `ubuntu-24.04-arm`). Each archive's binaries must start and report the tagged version. The Linux
  x64 and Windows x64 analyzers are built with the `cuda` feature (embedded PTX, driver only): their
  `--version` must contain `(+cuda)`, the others' must not, and a `--hwaccel cuda` run on a small
  FFV1 PQ clip must fall back to CPU analysis on the driverless runner (`analysis.gpu == false`).
  The runners have no GPU, so GPU analysis itself is tested on the CUDA host before an rc.
- **publish**: writes `SHA256SUMS`, attests build provenance (`actions/attest`, pinned by SHA), creates the
  release as a draft with every asset, then publishes it. The notes are a fixed header plus the
  CHANGELOG section (`scripts/ci/release-notes.sh`).

A pull request that changes the release workflow, its scripts, `.cargo/config.toml` or
`rust-toolchain.toml` runs everything except publishing, for the version in the manifests (a dry
run). So does a manual run (`workflow_dispatch`). `Cargo.lock` and the manifests are left out on
purpose, so a dependency bump does not start five release builds; the pre-flight requires a dry
run on the commit to be tagged instead.

## When something goes wrong

- **The run failed before publishing, for a flaky reason** (network, runner): use "Re-run failed
  jobs" on the same run.
- **The run failed before publishing, and code must change**: nothing was public. For a candidate,
  fix on `main` and release `-rc.N+1`. For a final version, the owner (the bypass actor of the tag
  ruleset) deletes the tag (`git push --delete origin vX.Y.Z`); fix on `main`; tag again. A
  leftover draft is replaced by the next run.
- **A defect is found after publishing**: never delete or re-tag. Release `X.Y.(Z+1)`. Put a
  warning at the top of the bad release's notes, or mark it a pre-release so it stops being latest.
  Both stay editable on an immutable release; only the assets and the tag are locked. Deleting a
  published immutable release does not free its tag name for reuse.
