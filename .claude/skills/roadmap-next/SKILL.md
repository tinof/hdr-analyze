---
name: roadmap-next
description: Pick the next ROADMAP.md step to work on. Checks readiness of each priority (media on disk, GPU, owner decisions, open PRs), drafts a ranking, gets an independent ranking from Codex as second advisor, reconciles both with the advisor tool, and recommends one step plus two alternates. Starts from the Next up block and the Checked bullets in ROADMAP.md and re-checks only what changed since their commit; writes what it verified back to ROADMAP.md and commits that on the recommended step's new branch, so it lands with the step's PR (to main only when no step can start). Run only when the user types /roadmap-next.
argument-hint: "[item ID or theme to focus on]"
disable-model-invocation: true
---

# roadmap-next

The user typed `/roadmap-next $ARGUMENTS`. Recommend the next roadmap **step** (item ID plus
step, e.g. "P11: dropped-levels notice"), not just an item. The only file it edits is
`ROADMAP.md` (step 6), and typing the command authorizes that one docs commit: on a new branch
for the recommended step, or to `main` when no step can start. That branch is the only one it
creates; it never pushes it, opens PRs or touches code.

**Roadmap memory.** What a run verifies is kept in `ROADMAP.md`, so the next run does not
re-derive it: the **Next up** block (ranking, date, `main@<sha>`) and a **Checked** bullet in each
item it examined (date, `@<sha>`, findings with `file:line`). Trust them unless the files they
cite changed since their commit.

If `$ARGUMENTS` names an item ID or a theme, rank only steps under it, but still report what
is waiting for the owner.

## 1. Gather state (one Bash call)

- `ROADMAP.md`: **Next up** (note its `main@<sha>`), the **Priorities** list, the **Waiting for
  the owner** list, and the item sections the priorities name, with their **Checked** bullets.
  Read the item sections, not only the priority lines: the gate and the open steps live there.
- What changed since Next up was written: `git log --oneline <sha>..origin/main` and
  `git diff --stat <sha>..origin/main`.
- `git log --oneline -15 origin/main`, `git status --short`, current branch.
- `gh pr list --state open --json number,title,headRefName`.
- GPU: `nvidia-smi -L || /usr/lib/wsl/lib/nvidia-smi -L`.

## 2. Readiness per step

**Confirm mode.** If `$ARGUMENTS` is empty (the Next up ranking is unscoped, so a scoped run
always ranks afresh) and no commit since the Next up `<sha>` touches `ROADMAP.md`'s Priorities or
owner list, or the files the Next up candidates and their Checked bullets cite, and no open PR
or merged commit covers a candidate, then only re-run the cheap checks below (open PRs, branches,
GPU, media paths) and go to step 6 with the existing ranking. Skip steps 3–5: no new draft, no
Codex run, no advisor call. Say "confirmed from Next up @<sha>" in the answer.

Otherwise, for a **Checked** bullet whose cited files changed since its `@<sha>`
(`git diff --quiet <sha>..origin/main -- <files>` fails), re-verify that finding; keep the rest.

For each open step of the priority items, in priority order, decide:

- **Blocked on the owner:** listed under "Waiting for the owner". Other steps of the same item
  stay candidates (for example P8's chroma-siting comparison while the P8 playback test waits).
- **Blocked on a prerequisite:** the step's gate names material, a tool or another item that is
  missing. Check media paths the item names with `test -e`; the development corpus lives in
  `~/mkvdovi-work/corpus/dev`.
- **Needs the GPU:** a change to the analysis or decode path needs `scripts/cuda-parity.sh`.
- **Already in progress:** an open PR or a local branch covers it.
- **Startable:** none of the above.

Do not spawn agents for this; it is a handful of reads.

## 3. Draft ranking

Up to three startable steps. For each:
- item ID + step, and the priority number it belongs to;
- the **acceptance gate** quoted from the item (what evidence closes the step);
- files or crates likely touched, and which gates the pre-PR review in `/codex-ship` will require
  (CUDA parity, L1 regression, integration tests with media);
- branch name in the repo style (`feat/…`, `fix-…`; check `gh pr list --state merged --limit 10`);
- whether it can share a branch with another candidate. Rule from ROADMAP: same crate, same test
  gate and no L1 change. A step that moves L1 always has its own branch.

Write the draft to `<scratchpad>/roadmap-next-draft.md`.

## 4. Codex second advisor

Run in the background (`run_in_background: true`) and wait for the completion notification.
Follow `~/.claude/rules/codex-routing.md`: background Codex runs go through its stall watchdog
(it kills a silent run after 8 minutes, retries once, and adds `--json` and `-o <prefix>.md`).

```
~/.claude/skills/codex-ship/scripts/codex-watch.sh <scratchpad>/roadmap-next-codex exec --sandbox read-only "Read ROADMAP.md and <scratchpad>/roadmap-next-draft.md in this repo. Rank the startable roadmap steps yourself, independently of the draft. Then: name risks the draft misses, prerequisites it assumed but did not check, and any step that should come first and why. Be specific: item ID, step, file paths."
```

The ranking is in `<scratchpad>/roadmap-next-codex.md`. `--json` prints no header, so report
`model`, `effort`, `outcome` and `attempts` from `<scratchpad>/roadmap-next-codex.result.json`.
If `ok` is false (stall, usage limit, unsupported model, sandbox error), say so with its `reason`
and continue without it. Never replace it with a Claude subagent and call that the Codex opinion.

Known false alarm: Codex runs in a read-only sandbox without GPU access, so its `nvidia-smi` or
CUDA probes fail there. Ignore claims that the GPU is unavailable; step 1's probe decides.

## 5. Reconcile

Compare the two rankings. Check every factual claim Codex makes (a missing file, a dependency)
against the repo before you accept it. Then call the `advisor` tool once, with both rankings in
context, before you settle on the recommendation.

## 6. Write back and commit

Edit `ROADMAP.md` only with facts you verified in this run:

- **Next up:** replace the block: `### Next up (checked <YYYY-MM-DD> at main@<short sha of origin/main>)`,
  then the recommended step and the two alternates, each with its gate and why, at most four lines
  each. In confirm mode, leave it as it is.
- **Checked bullets:** for each item you examined and found something not already in it, add or
  replace one bullet `- **Checked <date> @<sha>:** …` with the finding and its `file:line`. One
  Checked bullet per item: replace the old one, do not stack them. At most five lines; numbers and
  measurements go to `docs/ROADMAP_LOG.md`, not the item.
- **Corrections:** fix an item line the code shows is stale (for example an "Open" step that
  already shipped), citing the function or `file:line`.
- **New defects:** add a new item with the next free ID in its section (Status, the defect with
  `file:line`, Fix). Do not fix code here.
- **Owner list:** remove an entry only when a merged commit or the owner settled it.

The roadmap memory goes into `main` with the step's PR (PRs are squash-merged), not as a
separate commit on `main`:

- **Confirm mode with nothing new:** write nothing and commit nothing. The next run diffs from
  the existing Next up `<sha>`.
- **A step can start (the usual case):** only on `main`, after `git pull --ff-only`, with no
  other uncommitted change to `ROADMAP.md`: `git switch -c <branch>` (the branch named in the
  answer, in the repo's style: `feat/…`, `fix-…`, `docs/…`, `chore/…`), `git add ROADMAP.md`,
  `git commit -m "docs(roadmap): next up <ID> <step> (checked @<sha>)"`. Do not push: the branch
  goes out with the step's code through `/codex-ship`, and `pre-pr-review` and `/codex-ship` keep
  working on a branch that already exists. If the owner picks an alternate, rename the branch
  (`git branch -m`); if they drop the step, cherry-pick the commit onto `main`.
- **No step can start** (every candidate waits for the owner or is blocked), but the run wrote
  corrections, new defects or Checked bullets: commit on `main` as above without the branch, then
  `git push origin main` (where the pre-commit hooks are installed, their push stage skips the
  tests for a Markdown-only push).
- **Any other branch or a dirty `ROADMAP.md`:** leave the edits uncommitted and say so.

Never commit other files.

## 7. Answer

Keep it short:

1. **Recommended:** item + step, why now, its acceptance gate, the branch (created and checked
   out in step 6, or the name to use when it stayed uncommitted), and the first action (usually
   "enter plan mode for <ID> <step>").
2. **Alternates:** two, one line each.
3. **Where Codex disagreed** and what decided it (one or two lines; omit if they agreed).
4. **Waiting for the owner:** the list, one line each, so the owner sees what only they can
   unblock.
5. **Roadmap memory:** what you wrote back (Next up, Checked bullets, corrections, new items) and
   the commit and its branch, or why nothing was written or it stayed uncommitted.
