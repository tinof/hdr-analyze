---
name: pre-pr-review
description: Review a roadmap change before it ships. Scopes the diff once, runs the pre-pr-panel workflow (Claude reviewers on three lenses, each finding verified), optionally a Codex focused pass, fixes confirmed findings, runs the gates the touched paths require, then has the docs-keeper agent update the docs. /codex-ship runs it in ship mode (step 1b); the user can also type /pre-pr-review alone.
argument-hint: "[--base <branch>] [item ID]"
disable-model-invocation: true
allowed-tools: Workflow(pre-pr-panel)
---

# pre-pr-review

**Modes.**
- **Standalone:** the user typed `/pre-pr-review $ARGUMENTS`. Run every step, including step 2
  (Codex focused pass). End with "Ready for `/codex-ship <focus text>`".
- **Ship mode:** `/codex-ship` step 1b told you to follow this file. Skip step 2: the focus text
  from step 7 goes to `/codex-ship`'s steered Codex pass instead, so the same diff never gets two
  steered Codex runs. A blocker from step 4 or 5 stops `/codex-ship` before its push.

Either way, the user's command is the opt-in for the one `pre-pr-panel` workflow run in step 3.
Codex runs read-only and follows `~/.claude/rules/codex-routing.md` (`command codex`). Claude
makes every fix.

## 1. Scope, once

1. Base: `--base` argument, else the open PR's base (`gh pr view --json baseRefName`), else
   `main`. `git fetch origin <base>`.
2. Scope = committed `origin/<base>...HEAD` **plus** staged, unstaged and untracked files that
   belong to the task. Leave out untracked files that do not clearly belong (downloads, media,
   logs, scratch output); never delete them.
3. Item ID: the argument, else from the branch name or the commit messages, else ask once.
4. Read the item's section in `ROADMAP.md`: note the step being closed and its **acceptance
   gate** (what evidence closes it, e.g. a development-tier score, precision/recall against
   authored shot lists, a playback test).
5. Write `<scratchpad>/prepr-scope.md`: base, diff command, file list, item, step, acceptance
   gate. Every reviewer, Claude or Codex, gets this same scope.

## 2. Codex focused pass (standalone mode only)

Background Bash; wait for the completion notification, do not poll:

```
command codex exec --sandbox read-only "Read CLAUDE.md, then <scratchpad>/prepr-scope.md, and review exactly that scope (the committed diff and the listed uncommitted files). Focus on what generic review misses in this repo: cross-binary contracts (L1 sidecar version and fields on both sides, the +cuda version probe, luminance_mapping names, --help option probes such as --hlg-composer), CPU/CUDA bit-identity rules, resume_settings for new artifact-affecting flags, the L1 regression references, 'never re-encode' and 'no silent clamp'. Report only real defects: [P0-P3], file:line, failure scenario, suggested fix. Say plainly if you find none." > <scratchpad>/prepr-codex.md 2>&1
```

Report the `model:` / `reasoning effort:` header lines. A failed run is reported as failed;
never substitute a Claude review for it.

## 3. Workflow

Call the Workflow tool with `name: "pre-pr-panel"` and `args`:

```json
{"base": "origin/<base>", "files": ["<every file in scope>"], "item": "<ID + step>",
 "acceptanceGate": "<quoted gate>", "scopeFile": "<scratchpad>/prepr-scope.md"}
```

It returns `findings` (each verified by an independent skeptic) and `requiredGates` (computed in
code from the file list). Do not choose the gates by judgment; use that list.

## 4. Verify and fix

1. Merge the workflow findings with the Codex findings (standalone mode). Dedupe by file:line.
2. Check every finding against the code yourself. Classify it as confirmed / rejected (with a
   reason that cites code) / deferred (real, out of scope).
3. Call the `advisor` tool once with the merged list before you settle on the fixes.
4. Fix the confirmed findings; add a regression test where the defect is testable.

## 5. Gates, on the final code

Run every gate in `requiredGates`, after the last fix. If a later fix round changes code (here,
or in `/codex-ship` steps 4–6), recompute the gate list for the new file list and run the gates
again. The gate commands:

| Gate | Command |
|------|---------|
| `fmt` | `cargo fmt --all -- --check` |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `test` | `cargo build -p hdr_analyzer_mvp && cargo test --workspace -- --nocapture > <scratchpad>/prepr-test.log 2>&1; status=$?; rg -n -e 'Skipping' -e 'test result: FAILED' -e 'error: test failed' <scratchpad>/prepr-test.log; echo "cargo test exit $status"` (the build puts the current analyzer next to the debug mkvdovi the integration tests run; the log goes to a file, not through a pipe, so the exit status is cargo's) |
| `clippy-cuda` | `cargo clippy -p hdr_analyzer_mvp --all-targets --features cuda -- -D warnings` |
| `cuda-parity` | `scripts/cuda-parity.sh` |
| `l1-regression` | `scripts/ci/l1-regression-gate.sh` (check mode) |
| `tool:<name>` | `cargo fmt/clippy -D warnings/test --manifest-path tools/<name>/Cargo.toml` |

**Skips.** In the `test` log these skips are expected: `cuda_parity` (run by its own gate),
`HDR_ANALYZE_REAL_SAMPLE`, `MKVDOVI_FEL_SAMPLE`, and the `integration.rs` "sample not found".
Any other `Skipping` line (missing ffmpeg, dovi_tool, mkvmerge, libx265, a sibling analyzer, an
HLG or open-GOP test) means required coverage did not run: a blocker.

**L1 references.** `l1-regression` runs in check mode. Use `--update` only when the step is meant
to move L1; say so in the PR body with the reference diff.

**Blockers.** A nonzero exit from any gate command is a blocker, whatever a log grep shows. A gate that fails or cannot run (no GPU, missing tool or media) is a blocker. Never
report it as passed. In ship mode it stops `/codex-ship` before the push.

**Acceptance gate.** The commands above do not prove the step's acceptance gate (a corpus score,
precision/recall, a playback result). The evidence must be in the PR body or in
`docs/ROADMAP_LOG.md`. Without it the step is not marked done; the docs keeper records it as
still open.

## 6. Docs

Run the `docs-keeper` agent (Agent tool, `subagent_type: "docs-keeper"`) with: the scope file,
the item and step, the fixes made, the gate results, and whether the acceptance-gate evidence
exists and where. Read its diff (`git diff -- '*.md'`) before you commit it; revert any edit you
cannot verify against the code.

## 7. Report

Write `<scratchpad>/pre-pr-review.md`: findings table (priority, file:line, verdict, fix commit
or reason), gate results, blockers, docs changed. Then return **focus text** for the Codex
review: the contract areas this diff touches plus anything still uncertain, in one or two
sentences. Standalone mode ends with "Ready for `/codex-ship <focus text>`".
