---
name: pre-pr-review
description: Review a roadmap change before it ships. Scopes the diff once, runs the pre-pr-panel workflow (three Opus lenses at high effort, Fable 5.1 on high-risk diffs; each finding verified by an Opus skeptic), a Gemini 3.8 Flash pass through Antigravity, optionally a Codex focused pass, fixes confirmed findings, runs the gates the touched paths require, then has the docs-keeper agent update the docs. /codex-ship runs it in ship mode (step 1b); the user can also type /pre-pr-review alone.
argument-hint: "[--base <branch>] [item ID]"
disable-model-invocation: true
allowed-tools: Workflow(pre-pr-panel)
---

# pre-pr-review

**Modes.**
- **Standalone:** the user typed `/pre-pr-review $ARGUMENTS`. Run every step, including both
  passes of step 2 (Codex and Gemini). End as step 8 says.
- **Ship mode:** `/codex-ship` step 1b told you to follow this file. Skip step 2a (Codex focused
  pass): the focus text from step 7 goes to `/codex-ship`'s steered Codex pass instead, so the same
  diff never gets two steered Codex runs. Still run step 2b (Gemini). A blocker from step 4 or 5 stops `/codex-ship` before its push.

Both modes end with the record in step 8. `/codex-ship` step 1b reuses that record instead of
running this review again while HEAD is unchanged, so a standalone run followed by `/codex-ship`
costs one review, not two.

Either way, the user's command is the opt-in for the Gemini pass in step 2b, the `pre-pr-panel`
review run in step 3 (and its resume in step 4), and its `gatesOnly` calls, which start no agents.
Codex runs read-only and follows `~/.claude/rules/codex-routing.md` (`command codex`). Gemini runs
read-only through `.claude/workflows/pre-pr-gemini.sh`. Claude makes every fix.

## 1. Scope, once

1. Base: `--base` argument, else the open PR's base (`gh pr view --json baseRefName`), else
   `main`. `git fetch origin <base>`.
2. Commit the task first, so the review covers a commit and step 8 can record it. If HEAD is on
   the base branch, create a branch in the repo's style first (`feat/…`, `fix-…`; see recent PR
   head names). Then commit the task's staged, unstaged and untracked files in the base's commit
   style (`git log --oneline -15 origin/<base>`). Leave out untracked files that do not clearly
   belong (downloads, media, logs, scratch output). If you cannot tell, ask once. Never delete
   them. In ship mode, `/codex-ship` step 1 has already done this.
   Scope = `git diff --name-only origin/<base>...HEAD`.
   Then remove any record for this HEAD (step 8), so a run that stops early cannot leave an
   older, reusable one behind:
   `D=$(git rev-parse --path-format=absolute --git-common-dir)/pre-pr-review; S=$(git rev-parse HEAD); rm -f "$D/$S.json" "$D/$S.md" "$D/$S.scope.md"`
3. Item ID: the argument, else from the branch name or the commit messages, else ask once.
4. Read the item's section in `ROADMAP.md`: note the step being closed and its **acceptance
   gate** (what evidence closes it, e.g. a development-tier score, precision/recall against
   authored shot lists, a playback test).
5. Write `<scratchpad>/prepr-scope.md`: base, diff command, file list, item, step, acceptance
   gate. Every reviewer, Claude or Codex, gets this same scope.

## 2. External passes

Start them in the background right after step 1, and go on to step 3 at once: the workflow does
not wait for them. Their completion notifications arrive while it runs; do not poll. Collect
their findings before step 4.

### 2a. Codex focused pass (standalone mode only)

```
command codex exec --sandbox read-only "Read CLAUDE.md, then <scratchpad>/prepr-scope.md, and review exactly that scope (the diff origin/<base>...HEAD). Focus on what generic review misses in this repo: cross-binary contracts (L1 sidecar version and fields on both sides, the +cuda version probe, luminance_mapping names, --help option probes such as --hlg-composer), CPU/CUDA bit-identity rules, resume_settings for new artifact-affecting flags, the L1 regression references, 'never re-encode' and 'no silent clamp'. Report only real defects: [P0-P3], file:line, failure scenario, suggested fix. Say plainly if you find none." > <scratchpad>/prepr-codex.md 2>&1
```

Report the `model:` / `reasoning effort:` header lines. A failed run is reported as failed;
never substitute a Claude review for it.

### 2b. Gemini pass (both modes; skipped for a docs-only diff)

Skip it when every scope file matches `\.md$|^docs/|^LICENSE`. Otherwise run, as a background Bash:

```
.claude/workflows/pre-pr-gemini.sh origin/<base> <scratchpad> <scratchpad>/prepr-scope.md
```

The script runs `agy` (Antigravity CLI) with `gemini-3.8-flash-medium` in print mode, which
denies every terminal command and file write. The diff goes inside the prompt; a large diff takes
about 3–4 minutes. It stops `agy` after `PRE_PR_GEMINI_TIMEOUT` seconds (default 900), checks the
result itself (status, schema, no denied action, a nonce that proves Gemini read the diff) and
writes `<scratchpad>/prepr-gemini.result.json`: `ok` with `findings`, or `ok: false` with a
`reason`. Report its `model` and `seconds`, or the `reason`. A failed Gemini pass is not a blocker;
never substitute a Claude review for it.

## 3. Workflow

Call the Workflow tool with `name: "pre-pr-panel"` and `args`:

```json
{"base": "origin/<base>", "files": ["<every file in scope>"], "item": "<ID + step>",
 "acceptanceGate": "<quoted gate>", "scopeFile": "<scratchpad>/prepr-scope.md"}
```

Keep these args: step 4 resumes with them.

It returns `complete`, `failedLenses`, `reviewers` (who ran, model, effort; Fable runs only on
high-risk paths), `findings` (each verified by an independent Opus skeptic), `unverified` and
`requiredGates` (computed in code from the file list). Do not choose the gates by judgment; use
that list.

## 4. Verify and fix

1. If the workflow returns `complete: false` (a Claude reviewer failed), **resume** it once:
   `Workflow({scriptPath, resumeFromRunId, args})` with the script path and run ID from step 3's
   tool result and the identical args. Reviewers that finished come back from the cache; the
   failed reviewer and every skeptic run again (verified 2026-10-07: 2 of 3 Opus lenses cached). If it is still incomplete, report the missing lenses as a blocker.
2. Merge the workflow's `findings` **and** `unverified` lists with the Gemini findings (step 2b)
   and the Codex findings (standalone mode). Gemini and Codex findings have no skeptic: check them
   yourself like `unverified` ones. `unverified` holds findings over the verification cap or whose verifier failed: check
   each of them yourself like any other. Merge two findings only when they describe the same
   defect; distinct defects at the same file:line stay separate.
3. Check every finding against the code yourself. Classify it as confirmed / rejected (with a
   reason that cites code) / deferred (real, out of scope). A deferred or rejected P0/P1 is not a
   blocker here: it goes into the record, and `/codex-ship` asks the user about it at its merge
   gate, the same way it does for Codex findings.
4. Call the `advisor` tool once with the merged list before you settle on the fixes.
5. Fix the confirmed findings; add a regression test where the defect is testable.

## 5. Gates, on the final code

Run the gates after the last fix, on the final file list. Fixes can touch files outside the first
scope, and those files can add gates. So first commit the fixes (step 6 gives the message), then
recompute the list: call the Workflow tool with `name: "pre-pr-panel"` and
`args: {"base": "origin/<base>", "files": [<git diff --name-only origin/<base>...HEAD>], "gatesOnly": true}`.
That returns `requiredGates` from the same path rules and starts no agents. Use the union of
that list and step 3's list. Do the same after every later fix round, here or in `/codex-ship`
steps 4–6, and run the gates again. When the last run passes, save the commit it ran on,
`git rev-parse HEAD`, as `gatedCommit` for step 8. The gate commands:

| Gate | Command |
|------|---------|
| `fmt` | `cargo fmt --all -- --check` |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `test` | the block below |
| `clippy-cuda` | `cargo clippy -p hdr_analyzer_mvp --all-targets --features cuda -- -D warnings` |
| `cuda-parity` | `scripts/cuda-parity.sh` |
| `l1-regression` | `scripts/ci/l1-regression-gate.sh` (check mode) |
| `tool:<name>` | `cargo fmt/clippy -D warnings/test --manifest-path tools/<name>/Cargo.toml` |

The `test` gate, as one Bash call. The build puts the current analyzer next to the debug mkvdovi
the integration tests run. The log goes to a file, not through a pipe, and the last command
returns the gate's status, so a failed build, a failed test or an unexpected skip exits nonzero.
Keep `gate_rc` (zsh reserves `status`) and `grep` (`rg` may exist only as a shell function):

```bash
LOG=<scratchpad>/prepr-test.log
cargo build -p hdr_analyzer_mvp && cargo test --workspace -- --nocapture --skip cuda_output_matches_cpu > "$LOG" 2>&1
gate_rc=$?
grep -n -E 'test result: FAILED|error: test failed' "$LOG"
if grep 'Skipping' "$LOG" | grep -v -E 'HDR_ANALYZE_REAL_SAMPLE|MKVDOVI_FEL_SAMPLE|sample not found at'; then
    echo 'unexpected skip: required coverage did not run'; gate_rc=1
fi
echo "test gate exit $gate_rc"; [ "$gate_rc" -eq 0 ]
```

**Skips.** Expected, and allowed by the block: `HDR_ANALYZE_REAL_SAMPLE`, `MKVDOVI_FEL_SAMPLE`, and the
`integration.rs` "sample not found". The CUDA parity test is skipped by name because its own gate
runs it. Any other `Skipping` line (missing ffmpeg, dovi_tool, mkvmerge, libx265, a sibling
analyzer, an HLG or open-GOP test) fails the gate: required coverage did not run.

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

Commits, in both modes and this order: the code fixes before the gates (step 5) as
`fix: address pre-PR review` with a one-line list of the findings, then the docs edits here as
`docs: …`. The two are never mixed. Commit only task files; leave unrelated untracked files
alone.

## 7. Report

Write `<scratchpad>/pre-pr-review.md`: the reviewers that ran (model and effort, from
`reviewers`, plus the Gemini and Codex passes and any that failed), findings table (priority, file:line, finder, verdict, fix
commit or reason), gate results, blockers, docs changed. Then return **focus text** for the Codex
review: the contract areas this diff touches plus anything still uncertain, in one or two
sentences.

## 8. Record

The record lets `/codex-ship` skip a second review of the same commit. It also carries what
`/codex-ship` needs if it has to rerun the gates or the docs keeper later in another session.

**Tree check.** The gates ran on the working tree, so the record is valid only if that tree was
HEAD. This check is mechanical, not a judgment of which files belong to the task, and
`/codex-ship` step 1b runs the same one. Both commands must print nothing:

```bash
git status --porcelain --untracked-files=no
git ls-files --others --exclude-standard | grep -E '(^|/)(src|tests|benches|examples)/|\.(rs|cu)$|(^|/)(build\.rs|Cargo\.(toml|lock)|rust-toolchain\.toml)$|^\.cargo/|^scripts/'
```

Untracked files outside those paths (downloads, PDFs, notes) do not matter. If either command
prints anything, write no record, and say which files in the report.

The `docs: …` commit after the gates may change only Markdown files:
`git diff --name-only <gatedCommit> HEAD` must list only `*.md`. `<gatedCommit>` is the sha saved
in step 5, never one looked up afterwards. Otherwise write no record.

Use the **full** sha and an **absolute** path. `/codex-ship` looks the record up with
`git rev-parse HEAD`, and the Write tool needs an absolute path:

```bash
S=$(git rev-parse HEAD); D=$(git rev-parse --path-format=absolute --git-common-dir)/pre-pr-review
mkdir -p "$D"; cp <scratchpad>/pre-pr-review.md "$D/$S.md"; cp <scratchpad>/prepr-scope.md "$D/$S.scope.md"
echo "$D/$S.json"
```

The common dir is shared by every worktree of the repo and is never committed. The copies are
needed because the scratchpad belongs to one session, and `/codex-ship` usually runs in another.
Then write the echoed path with the Write tool:

```json
{"head": "<full sha>", "base": "origin/<base>", "item": "<ID + step>",
 "acceptanceGate": "<quoted gate>", "acceptanceEvidence": "<where, or none>",
 "mode": "standalone | ship", "complete": true, "gatedCommit": "<full sha from step 5>",
 "requiredGates": ["<union from step 5>"], "gateResults": {"<gate>": "pass | fail | not-run"},
 "blockers": [], "unfixedConfirmedP0P1": 0,
 "deferredP0P1": [{"where": "file:line", "reason": "..."}],
 "rejectedP0P1": [{"where": "file:line", "reason": "..."}],
 "codexFocusedPass": "done | failed | skipped", "focusText": "...",
 "reviewers": [<the workflow's reviewers list>],
 "geminiPass": "done | failed | skipped", "geminiReason": "<reason when failed>",
 "report": "<full sha>.md", "scope": "<full sha>.scope.md"}
```

- `requiredGates` is the union from step 5, and `gateResults` has one entry for each of its
  gates.
- `complete` is false when a Claude reviewer was still missing after the resume in step 4. `reviewers`, `geminiPass` and `geminiReason` are informational and do not affect reuse.
- `deferredP0P1` and `rejectedP0P1` do not block reuse. `/codex-ship` asks the user about them at
  its merge gate.
- Write the record also when there are blockers. `/codex-ship` reads it and refuses to reuse it.

Standalone mode ends by naming the next command, `/codex-ship`. When the record is reusable
(`complete` true, no blockers, `unfixedConfirmedP0P1` 0, every gate `pass`), add: "It reuses
this review while HEAD stays at <short sha>; a new commit makes it run the review again."
Otherwise name what blocks reuse (no record and why, or the blockers), and say that
`/codex-ship` will stop on it or run the review again.
