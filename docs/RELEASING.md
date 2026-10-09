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

Versions follow SemVer before 1.0: `0.MINOR.PATCH`. `hdr_analyzer_mvp`, `mkvdovi`, `verifier` and
`dovi84_composer` always carry the same version. The `tools/*` crates do not ship and keep their
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

## When to release

1. **Within about a week** of merging a fix for delivered output that is wrong for every input of a
   type (ROADMAP priorities 1 to 4), or a changed contract or default.
2. **Otherwise in batches**: a PATCH when fixes have collected, at most every few weeks.
3. **Never for** refactors (E13), research behind an unfinished opt-in flag, or measurement-only
   steps.
4. **Release candidate first** (`vX.Y.0-rc.N`) only when the release needs a test outside CI
   before it becomes latest: playback on a device, CUDA on Windows, a changed release workflow.

`/roadmap-next` tags each **Next up** entry `Release: minor | patch | none` and keeps the Release
plan current, so the decision is made per step, not afterwards.

## Checklist

1. **CHANGELOG.** Merge `[Unreleased]` into one heading per kind (`Added`, `Changed`, `Fixed`,
   `Removed`). Put an **Upgrade notes** list at the top: the Breaking items, and anything users must
   do. Rename `[Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` and add a new empty `## [Unreleased]`.
2. **Versions.** Set `version` in `hdr_analyzer_mvp`, `mkvdovi`, `verifier` and `dovi84_composer`
   (with the `-rc.N` suffix for a candidate), then `cargo build` to update `Cargo.lock`. Also set
   "Current version" in `README.md` and `version` in `CITATION.cff`, and refresh the tool
   lockfiles that pin `dovi84_composer`:
   `cargo update -p dovi84_composer --offline --manifest-path tools/fit_hlg_composer/Cargo.toml`.
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
  mkvtoolnix, mediainfo and a pinned `dovi_tool` installed. Any `Skipping` line outside an
  allow-list fails the job (`scripts/ci/check-test-skips.sh`), so the HLG and open-GOP tests cannot
  pass by skipping.
- **build**: Windows x64, macOS Intel and Apple Silicon, Linux x64 and ARM64. Each archive's
  binaries must start and report the tagged version.
- **publish**: writes `SHA256SUMS`, attests build provenance (`actions/attest`), creates the
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
