# Docs rules

How this repo's Markdown docs are brought in line with a finished code change. Two writers work
from this file: the agy `docs-writer` agent on Gemini (the default, on the model that
`~/.claude/skills/ship/scripts/gemini-model.sh docs` prints; the `pre-pr-review` skill, step 6,
puts this file into its brief) and the `docs-keeper` agent
(`.claude/agents/docs-keeper.md`, the fallback).

You update documentation so it matches the code on this branch. You edit Markdown files only.
Never edit code, tests, scripts, config or reference data. Read with `git diff`, `git log` and
`rg`. A binary's `--help` text is in the brief when the diff touches a CLI; the `docs-keeper`
agent may also run a debug binary that is already built (`target/debug/<bin> --help`).
The agy writer's guard refuses `AGENTS.md` and every file under `.claude/`: it lists such an edit
as a gap, and the caller makes it.

## Input

The caller gives you: the scope file (base, diff command, file list), the roadmap item and step,
the fixes made in review, the gate results, and whether the acceptance-gate evidence exists and
where. When `/ship:land` runs the review as its self-check, the gates run after you and the caller has
no gate results. Read the diff first. Everything you write must follow from the diff or from those
inputs.

You never close a roadmap step. `/ship:land` step 10 does that after the review: it removes the step
line, updates the item's Status, writes the `docs/ROADMAP_LOG.md` entry for the step and hands off
the Next up block (promotes it, or writes a Reassess line).

## Where each fact belongs

| Change in the code | Doc to update |
|--------------------|---------------|
| Any step of a roadmap item | Nothing: `/ship:land` step 10 closes the step. Fix only text in `ROADMAP.md` that the diff makes wrong (a path, a flag, a line reference), not the step line, Status or Next up. |
| The step's full story, measurements, numbers | Nothing in `docs/ROADMAP_LOG.md`: `/ship:land` step 10 writes the step's entry. Name the evidence in your output so the caller can put it in the PR body. |
| User-visible behavior, flags, defaults, refusals | `CHANGELOG.md` `[Unreleased]`, in the section style already used there. |
| A fact `README.md` states: a capability, an install step, a required tool, a Compatibility row, a headline number, a limitation | `README.md`, by the **README** rule below. |
| A CLI flag or its help text | `docs/CLI_REFERENCE.md`; compare with the `--help` text (in the brief, or `target/debug/<bin> --help` when you can run it). |
| What inputs are accepted, refused or converted | `docs/FORMAT_COMPATIBILITY.md`. |
| A new or changed rule: a cross-binary contract, an invariant ("never", "must", "keep X ahead of Y"), a gate, a command | `CLAUDE.md`, the section that already covers the topic, as one sentence plus a pointer to the doc that explains it. Edit the sentence that is now wrong; do not append a second, contradicting one. |
| mkvdovi internals: the reason for a rule, code paths, resume, measurements reuse, `--verify`, RPU generation | `docs/MKVDOVI_PIPELINE.md`. |
| L1 sidecar fields or version | `docs/FORMAT_COMPATIBILITY.md` "L1 measurement sidecar" (schema per version) and `docs/MKVDOVI_PIPELINE.md` "L1 sidecar contract" (code on each side). |
| Per-test skip conditions, corpus, baselines | `docs/TESTING.md` §6 and §7. |
| CI jobs, apt and caches, pre-commit, the Stop hook | `docs/CI.md`. |
| Dev-host build traps, the dev install | `docs/INSTALLATION.md` "Development hosts". |
| Analyzer and CUDA internals | `docs/TECHNICAL_REFERENCE.md` §1, `docs/CUDA_PIPELINE.md`. |
| Other topic internals | the topic doc: `docs/HLG_COMPOSER.md`, `docs/CM_ANALYZE_PARITY.md`, `docs/VALIDATION.md`, `docs/FEL_PLAN.md`, `docs/RPU_BUILDER.md`, `docs/RELEASING.md`. |
| "Waiting for the owner" in `ROADMAP.md` | Remove an entry only when the diff or the caller shows it is settled. |
| "Next up" and "Checked" bullets in `ROADMAP.md` (the roadmap memory `/ship:roadmap-next` keeps) | Leave Next up alone. Delete or correct an item's Checked bullet when this change makes it wrong; do not add new ones (that is `/ship:roadmap-next`'s job). |

## Rules

- **`CLAUDE.md` is sent on every turn of every session; keep it small.** It holds rules and
  pointers, not explanations. A fact goes there only when not knowing it before an edit breaks
  something; its reason, its code path, the numbers and the history ("since P8", what an incident
  showed) go to the topic doc in the table above. Never add a version-by-version history to
  `CLAUDE.md`. Budgets: the `claude-md`, `agents-md` and `roadmap` bullets of `.claude/workflow.md`
  "## Size limits" (`wc -c`; `CLAUDE.md` 24,000 bytes now). If your edit would take `CLAUDE.md` or
  `AGENTS.md` past its budget, move detail out of the section you touch into its topic doc, and report
  the size before and after. Move reasons, code paths and numbers only: never cut a clause that
  says what must or must never happen, and name each clause you moved in your output. Do not trim
  `ROADMAP.md` (`/ship:land` step 10 checks its budget) or a file
  already over budget before your edit: report it. `/ship:land` adds a Trim step under E18 only when the
  PR grows the file; otherwise the report is what reaches the owner.
- Keep the `CLAUDE.md` headings that other files cite by name: "Testing quirks", "CI behavior that
  affects edits", "Module size", "Lint/style policy".
- **Acceptance gate.** Never mark a step done. Report whether the caller named the
  acceptance-gate evidence and where (PR body, a doc section); `/ship:land` step 10 closes the step
  only with it.
- Search before you write: `rg` the old name, flag, value or version across `*.md` and fix every
  stale mention in scope, not only the first.
- **README.** `README.md` is a designed page that also ships in every release archive. Fix the
  sentence, table cell or command that the diff made wrong, in place, everywhere it appears. Keep
  the section order, the headings, the image embeds, `<details>` blocks and a Mermaid diagram's
  `classDef` lines; in a diagram, fix only the label or edge that is now wrong. Image files under
  `assets/readme/` are not yours to edit: `rg -n '<old value>' assets/readme`, and report a hit as a
  gap so the caller rebuilds the asset. Add no section, badge or diagram; a change that needs one
  is a gap. A redesign of the page is `/readme`'s job, not yours.
- Keep each doc's voice: short declarative sentences, numbers with units, dates as YYYY-MM-DD.
  Wrap prose at about 100 columns; no line over 200 characters.
- Do not invent numbers, results or dates. If a fact is not in the diff or the inputs, leave it
  out and list it as a gap.
- Do not touch `docs/ROADMAP_LOG.md`.

## Output

Return a short list: each file you changed, what you changed and why (one line each); then the
gaps you could not fill (facts missing, evidence missing). Return "no docs change needed" when
the diff changes nothing that a doc states.
