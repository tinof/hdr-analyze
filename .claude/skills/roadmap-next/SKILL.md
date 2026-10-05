---
name: roadmap-next
description: Pick the next ROADMAP.md step to work on. Checks readiness of each priority (media on disk, GPU, owner decisions, open PRs), drafts a ranking, gets an independent ranking from Codex as second advisor, reconciles both with the advisor tool, and recommends one step plus two alternates. Writes nothing. Run only when the user types /roadmap-next.
argument-hint: "[item ID or theme to focus on]"
disable-model-invocation: true
---

# roadmap-next

The user typed `/roadmap-next $ARGUMENTS`. Recommend the next roadmap **step** (item ID plus
step, e.g. "P11: dropped-levels notice"), not just an item. This skill only reads: it never
edits files, creates branches or opens PRs.

If `$ARGUMENTS` names an item ID or a theme, rank only steps under it, but still report what
is waiting for the owner.

## 1. Gather state (one Bash call)

- `ROADMAP.md`: the **Priorities** list, the **Waiting for the owner** list, and the item
  sections the priorities name. Read the item sections, not only the priority lines: the gate
  and the open steps live there.
- `git log --oneline -15 origin/main`, `git status --short`, current branch.
- `gh pr list --state open --json number,title,headRefName`.
- GPU: `nvidia-smi -L || /usr/lib/wsl/lib/nvidia-smi -L`.

## 2. Readiness per step

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
- files or crates likely touched, and which gates `/pre-pr-review` will require (CUDA parity,
  L1 regression, integration tests with media);
- branch name in the repo style (`feat/…`, `fix-…`; check `gh pr list --state merged --limit 10`);
- whether it can share a branch with another candidate. Rule from ROADMAP: same crate, same test
  gate and no L1 change. A step that moves L1 always has its own branch.

Write the draft to `<scratchpad>/roadmap-next-draft.md`.

## 4. Codex second advisor

Run in the background (`run_in_background: true`) and wait for the completion notification.
Follow `~/.claude/rules/codex-routing.md`: `command codex`, never the bare alias.

```
command codex exec --sandbox read-only "Read ROADMAP.md and <scratchpad>/roadmap-next-draft.md in this repo. Rank the startable roadmap steps yourself, independently of the draft. Then: name risks the draft misses, prerequisites it assumed but did not check, and any step that should come first and why. Be specific: item ID, step, file paths." > <scratchpad>/roadmap-next-codex.md 2>&1
```

Report the `model:` and `reasoning effort:` header lines. If the run fails (usage limit,
unsupported model, sandbox error), say so and continue without it. Never replace it with a
Claude subagent and call that the Codex opinion.

## 5. Reconcile

Compare the two rankings. Check every factual claim Codex makes (a missing file, a dependency)
against the repo before you accept it. Then call the `advisor` tool once, with both rankings in
context, before you settle on the recommendation.

## 6. Answer

Keep it short:

1. **Recommended:** item + step, why now, its acceptance gate, the branch name, and the first
   action (usually "enter plan mode for <ID> <step>").
2. **Alternates:** two, one line each.
3. **Where Codex disagreed** and what decided it (one or two lines; omit if they agreed).
4. **Waiting for the owner:** the list, one line each, so the owner sees what only they can
   unblock.
