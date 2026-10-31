# HDR-Analyze Roadmap

This is the single source of truth for active `hdr-analyze` work. Completed changes belong in
[`CHANGELOG.md`](CHANGELOG.md); current user-facing behavior belongs in
[`docs/FORMAT_COMPATIBILITY.md`](docs/FORMAT_COMPATIBILITY.md); technical accuracy analysis belongs in
[`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md). The full text of each progress step and the
measurement detail behind the items are in [`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md).

> **North star:** a fully open-source, research-based analyzer whose measurement accuracy stands up
> against the reference tooling it is compared to. Parity is measured, never asserted.

Status meanings: **Open** has not shipped; **Partial** has useful pieces in place but does not meet
the stated outcome; **Core complete** meets the original gate but retains named follow-up work;
**Deferred** is intentionally not scheduled. Items with no follow-up left move to the changelog.

**Step format.** Every open step is one line that `/ship:roadmap-next` reads:
`- [<ID>][tags] <step>. Gate: <evidence>.` with an optional `Needs: <ID>[, <ID>]` for a prerequisite
(`Needs: <ID> (code)` also accepts a step that has landed with its gate pending). IDs are
`<item>.<n>` (for example `P1.2`), assigned once and never reused or renumbered; the numbers are
the ones the items used before the step format (E14 step 5 → `E14.5`). A new step takes 1 + the
highest number ever used for that item in `ROADMAP.md`, `docs/ROADMAP_LOG.md` and `Roadmap-Step:`
commit trailers. A step without a gate, or with `Gate: TODO(owner)`, waits for the owner. Done
steps from before the step format are cited in the log as "<item> step <n>", not by step ID, so a
`Needs:` names only open steps.

| Tag | Meaning |
|---|---|
| `desk` | Code or doc work; needs nothing |
| `owner` | Needs the owner: a decision, material or a test only they can run |
| `reassess` | A stop point: its outcome decides what follows, so `/ship` writes a **Reassess** line in Next up when it closes the step and the next `/ship:roadmap-next` re-plans |
| `gpu` | Needs probe `gpu` (the RTX 4070 CUDA host): the step touches a `cuda-parity` path or its gate runs `scripts/cuda-parity.sh` |
| `corpus` | Needs probe `corpus`: the gate is scored on real content (`~/mkvdovi-work/corpus/dev`; the RPU baselines run on its cuts) |

**How to update.** Format: `~/.claude/skills/ship/reference/roadmap-format.md`; project rules:
[`.claude/workflow.md`](.claude/workflow.md).
- This file holds open work and the context its steps cite. Closed steps, Done notes and
  measurements are in
  [`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md) (the progress-log table and the Done notes moved there
  on 2026-10-26).
- When a step ships, `/ship` removes its line, updates the item's status, and adds the step's full
  text with the gate evidence to [`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md) under
  `## <date>: <ID> <title> (PR #<n>)`.
- A step whose gate needs evidence that does not exist yet stays, with `Landed: PR #<n>; gate
  pending.` appended.
- Measurements go to the log or to the doc the item links, not into the item. An item with no
  steps left moves to the log.

**Next up** is a checkpoint. When a step ships, `/ship` removes it from Next up and promotes the
rest, or writes a **Reassess** line when what it learned changes the plan; `/ship:roadmap-next`
confirms the block instead of re-ranking unless something changed (its ready queue,
`~/.claude/skills/ship/scripts/roadmap-ready.sh`, decides). A **Checked** bullet (date, commit,
`file:line`) on each examined item lets a later run re-check only what changed since that commit.
Only IDs inside an owner-list entry's bold lead count as waiting for the owner.

### Next up (checked 2026-10-31 at main@2a7bb35)
<!-- roadmap-next: rev=3 scope=all config=b3103c892048 probes=gpu:ok,corpus:ok ready=b5eb0ee05687 -->

1. **P9.2 Sidecar v6** (priority 2). Gate: the step's gate. Why: a hybrid fallback piece takes the
   whole analyzer scene's max (`hybrid.rs:340`); fix it before the flip, while "default RPU
   baselines byte-identical" still means the old path. Priority 1 has no startable step.
   Branch `feat/p9-2-sidecar-v6`. Release: minor
2. **P9.3 Flip the `--hdr10plus-l1` default to hybrid** (priority 2). Gate: the step's gate. Why:
   after P9.2, so the default ships the exact fallback peak; the order is stated here and in the
   item, not as a `Needs:`. v0.8.0 with E12.1 and P9.2. Own branch `feat/p9-3-hybrid-default`
   (moves HDR10+ L1). Release: minor
3. **P1.1 Decide the CPU fallback default** (priority 4). Gate: the item gate. Why: priority 4
   measures this first, and a flip is MINOR, so the answer should exist before v0.8.0 is cut. A
   kept `balanced` needs no code. Branch `fix/p1-1-cpu-default`, only if it flips. Release: none
   (minor if CPU `auto` flips)

### Waiting for the owner

Steps that need a decision, material or a test only the owner can provide. Other steps of the
same item stay startable. `/ship:roadmap-next` reads this list; remove an entry when it is settled.

Owner decisions of 2026-10-17 settled the earlier entries (P9 scoring rule, P13 L2, E14.2,
P8 second device, E12 NVRTC, P10 evidence, and the gates of every `TODO(owner)` step then listed; detail in
[`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md) 2026-10-17). What is left: the owner's eyes or
hardware after a startable step has prepared it, a gate not stated, and steps timed to a release:

- **WS6.9: metered, on the "Metered tests (later)" list below** (owner, 2026-10-29).
- **WS1.4: acceptance gate not stated.** True target-gamut peaks: found without a gate when the
  roadmap moved to the step format; the owner left it open (2026-10-17).
- **FEL:** on hold by the owner (2026-10-13, confirmed 2026-10-17: revisit after 1.0.0); no gate
  until the hold is lifted.
- **E15.1: not in the Priorities list.** It has a gate and is startable, but no priority names it
  (priority 10 only says it is not held); the owner places it (found 2026-10-25).

### Metered tests (later)

Tests that need the owner's X-Rite i1Display Pro on the Windows PC. The owner runs them when the
roadmap reaches them; by-eye tests come first (owner, 2026-10-29). Each one also has an entry
under "Waiting for the owner", which is the list `/ship:roadmap-next` reads.

- **WS6.9: meter the neutral-finder probe** (the WS6.8 files, 15 runs, about 40 minutes;
  ArgyllCMS 3.5.0 on the PC). The by-eye reading of 2026-10-29 did not meet its gate; the
  `code / 1023` question no longer depends on it (WS6.6 answered it), so it measures the
  per-gain neutral with an instrument.

### Release plan (2026-10-14)

Which steps ship in which release. Rules for the bump and the timing: [`docs/RELEASING.md`](docs/RELEASING.md).
`/ship:roadmap-next` keeps this table current. Latest release: v0.7.0 (2026-10-18; P7, P9, P11 and P13, tagged without an rc).

| Release | Content | Trigger / gate |
|---|---|---|
| none | P1.1 measurement (if `balanced` stays; a flip of CPU `auto` is MINOR and goes into the next minor), E13.6–E13.21, E14.3–E14.6 (tests and the pre-PR gate only; order in E14) | No user-visible change |
| next patch | P12.1 `--mdfix` source L6 (PATCH-class fix; not merged before the v0.7.0 release PR) | PATCH, batched (`docs/RELEASING.md` rule 2) |
| v0.8.0 | E12.1 CUDA release binaries (Linux x64, Windows x64; the Windows zip bundles NVRTC); P9.2 sidecar v6 (cross-binary contract; exact fallback peak, before P9.3, 2026-10-27); P9.3 hybrid becomes the HDR10+ default (owner decision 2026-10-17: after v0.7.0) | MINOR: delivered L1 changes for NVIDIA users. `v0.8.0-rc.1` tested on the Windows side of the RTX 4070 dev host with the CUDA toolkit hidden (owner decision 2026-10-17). Owner decision 2026-10-14: E12 moved out of v0.7.0 |
| a later minor | E8.1 and E9.1 (they move the default `.bin`; unblocked by E13.5, #59) | MINOR |
| 1.0.0 | P8 playback verified on the LG OLED C9 (2026-10-13) and the WS6 `code / 1023` question answered by the discriminating canary or the neutral-finder probe (WS6.3–WS6.9, WS6 gate (3); answered 2026-10-29 for the C9: `code / 1023`, WS6.6); a second device is not required (none available, owner 2026-10-17); sidecar schema and CLI unchanged for one minor cycle; no open priority-1 defect | Owner decision |

## Priorities

Order (2026-10-03, confirmed by the owner 2026-10-16): defects that make delivered output wrong for
every file of an input type; then the evaluation infrastructure every later decision needs; then
improvements with measured evidence; then research. Items 1 to 5 are about what is delivered, 6
and 7 about how it is judged. The evidence for each priority is in its items and in
[`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md#priority-table).

1. **HLG → Profile 8.4 composer** (P8, WS6.9). `--hlg-composer bt2100` and the input refusal landed
   2026-10-03; bt2100 is the default by owner decision. Gate: the WS6 playback test shows that
   devices apply a mapping that is not the preset; `--hlg-composer preset` is the fallback. Met for
   one device, an LG OLED C9, on 2026-10-13 ([log](docs/ROADMAP_LOG.md)); no second device is
   available, so other devices stay untested (owner, 2026-10-17). The WS6 `code / 1023` question
   is answered for the C9: `code / 1023`, so the bt2100 fit stays (WS6.6 canary, 2026-10-29).
   WS6.9 (the metered per-gain reading) stays on the metered list.
2. **HDR10+ → Profile 8.1 hybrid mode** (P9): scene list = union of HDR10+ starts and the analyzer's
   cuts (chosen 2026-10-13), peak per piece from HDR10+; average, minimum and crop from pixels; the
   opt-in flag landed 2026-10-14; the holdout scoring passed 2026-10-17 and the default flips
   after v0.7.0 (owner). Gate: the P9.3 gate. **Prerequisite:** E13.5 before the
   sidecar v6 step P9.2 (met 2026-10-17, #59).
3. **Deliver what was measured** (P10): source range and the measured-against-delivered report
   landed 2026-10-04; open: decide whether to write L1 in-process (P10.3). Gate: the WS7 simulator
   decides (owner, 2026-10-17).
4. **Narrow code defects** (E8, E9, P1, E10, P6, E7): histogram percentile reader, `--pre-denoise`
   values, half-resolution CPU default, L1 scaling note (the three L6 defects found 2026-10-07 were
   fixed 2026-10-11). Gate: unit tests for the reader and the option (E8.1, E9.1); measure the CPU
   default first (P1.1). **Prerequisite:** E13.5 (analyzer split) before E8.1 + E9.1, because
   E8.1 moves the default `.bin` (met 2026-10-17, #59).
5. **`--mdfix` fidelity** (P12): full regeneration stays (owner decision 2026-10-13) and the
   notice that names the dropped levels shipped 2026-10-15 (`CHANGELOG.md` `[Unreleased]`). Open:
   `--mdfix` ignores the source RPU's L6 (P12.1). P13 closed 2026-10-17 (v29 stays without L2).
   Gate: the P12.1 gate.
6. **Evaluation in displayed-picture units** (WS7): histogram-domain display-mapping simulator, then
   ColorVideoVDP on a few cuts (WS7.1, WS7.2). Gate: the simulator reproduces the recomputed
   numbers in WS7.
7. **Test material** (WS8): grain on synthetic highlights, unaligned positions, a same-master grainy
   twin, transitions. Needed before 9 can be scored. Real material per format (item 5) landed
   2026-10-05. Gate: per item in WS8.
8. **Scene detection** (WS2.1, E4): second signal for whole-picture motion; strong cuts closer than
   12 frames. Gate: precision and recall against authored shot lists, no loss on the other cuts.
9. **Research, default unchanged** (WS1, WS2.2, WS2.4, WS4): grain-robust peak; per-frame L1 inside detected
   transitions; L4; automatic trims. Each stays opt-in or unbuilt until its gate is met. Gate: per
   item (WS4: blinded A/B on a device; WS1.2–WS1.4 and WS2.2–WS2.4: in the items).
10. **Profile 7 FEL: placeholder** (FEL). See [`docs/FEL_PLAN.md`](docs/FEL_PLAN.md); on hold by the
    owner (2026-10-13). Gate: none until the hold is lifted.
11. **Engineering backlog** (E13, E14, E12, E1, E2, E3, E6, E11, E17, E18, P3, P5, WS0, WS6): the
    remaining E13 PRs (E13.6–E13.21), the E14 test audit, CUDA release binaries (E12.1, after
    v0.7.0), CI and corpus gates (E1, WS0, WS6.1), crop and L5 per scene (E2, P3), debug options,
    provenance, open-GOP scoring and the HDR10+ extract fallback (E3, E6, E11, P5), `--verify`
    checks for authored CM v4.0 blocks (E17), hot-file byte budgets (E18). Ranked below 1–10; added
    2026-10-18 so `/ship:roadmap-next` can reach them (owner). Gate: per item.

- **Sequencing (E13, 2026-10-07):** the module-size refactor is no priority of its own; its steps
  are prerequisites of priorities 2 and 4 (see there). Each move is a pure-move PR with the
  acceptance gate in its item; no other branch should be open while a move is in flight. The
  remaining steps E13.6–E13.21 are behaviour-neutral PRs, not moves.
- **Conditional** (R2, WS5): Profile 5 through an established encoder, only if matched playback
  tests justify it.
- **Kept by decision:** a successful conversion deletes a non-DV source (source retention).
  The WS6 corpus and playback procedure grow alongside items 1 to 3, which
  each need a playback check.
- **Deferred:** display-target optimizers (P4), automatic creative trims (WS4, behind the WS7 gate),
  and broader hardware acceleration (E5). Neutral trims stay.

## Conversion quality

### P1: CPU analysis default

- **Status:** Partial. Source-honest generation is the default; `accurate` is the only
  parity-checked preset; on CUDA `balanced`/`fast` differ by backend, no gate covers them, and they
  lose the measured MaxCLL. History in [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: P1.1.
- **Gate:** the CPU cost of `accurate` and the effect of `FAST_BILINEAR` on L1 max and MaxCLL are
  measured (priority 4: measure the CPU default first); then CPU `auto` flips to `accurate` or
  `balanced` stays with a visible note.
- [P1.1][corpus] Decide the CPU fallback default. `auto` resolves to `accurate` only with CUDA,
  otherwise `balanced`, which downsizes with `FAST_BILINEAR` at half resolution and averages small
  highlights. Measure the CPU cost of `accurate` and the effect of `FAST_BILINEAR` on L1 max, then
  flip CPU `auto` to `accurate` (the same file then gives the same L1 on any host) or keep
  `balanced` with a visible note. `fast` puts cuts on a 3-frame grid and misses peaks on skipped
  frames. Gate: the item gate (CPU cost of `accurate` and the `FAST_BILINEAR` effect on L1 max and
  MaxCLL measured, forced CPU and fresh analyses, then flip or keep); a flip adds
  `cargo mutants --in-diff`. Between E13 moves.
- **Checked 2026-10-17 @941489d:** the visible note for CPU `auto` already exists
  (`analysis_quality_notice`, `mkvdovi/src/pipeline/analyzer.rs:246`), so keeping `balanced` needs
  no code. Measure with CPU analysis forced and fresh analyses for both presets (`auto` picks
  `accurate` with GPU analysis, and an existing sidecar can be reused); compare measured MaxCLL
  from the sidecar, since a source-stated MaxCLL wins in L6.

### P3: L5 from the crop

- **Status:** Core complete. L5 comes from the full-resolution crop; sampled source L5 keeps
  precedence for Dolby Vision inputs; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29).
  Open: P3.1.
- **Gate:** the E2 (1) gate; per-scene L5 offsets come from the per-scene crop (owner,
  2026-10-17).
- [P3.1][gpu][corpus] Changing aspect ratios need per-scene L5 offsets; done together with E2.1,
  on the same branch. Gate: the item gate (the E2.1 gate; per-scene L5 offsets come from the
  per-scene crop).

### P4: display-targeted workflow

- **Status:** Deferred. Opt-in `--target-nits` display-targeted workflow wired into optimizer
  behavior.
- **Gate:** reopen only when the WS7 simulator shows that display-targeted L1 lowers the
  displayed-luminance error on a target panel (owner, 2026-10-17).

### P5: L9 primaries

- **Status:** Partial. L9 detection prefers mastering-display primaries and has an override.
- **Gate:** step 1: a test in which the normal `hdr10plus_tool extract` fails and the
  `--skip-reorder` fallback produces a JSON whose frame count equals the video's, and one in which
  the fallback is not used (both sides; owner, 2026-10-17).
- [P5.1][desk] `hdr10plus_tool extract --skip-reorder` fallback (not in the code at 2026-10-17).
  Gate: the item gate (step 1). (The HLG→PQ mastering-primaries item is gone: HLG now converts to
  Profile 8.4 without re-encoding.)

### P6: source consistency warnings

- **Status:** Partial. Warnings exist for L6/L9 fallbacks and suspicious HDR10+ scene peaks.
- **Gate:** step 1: a written list of concrete checks (for example MaxCLL above the mastering
  peak, HDR10+ and ST 2086 disagreeing) in `docs/ROADMAP_LOG.md`, from which the owner picks.
  Step 2: each picked check warns on a clip that has the problem and stays silent on one that does
  not, and all RPU baselines stay byte-identical (owner, 2026-10-17).
- [P6.2][corpus] Implement the five picked checks, warn-only: C5 source RPU L6 against the
  container, C1 measured MaxCLL above 1.25 × the stated MaxCLL, C4 HDR10+ or RPU L1 peak above
  1.25 × MaxCLL, C7 HDR10+ MaxScl 0 on every frame, C3 stated MaxFALL above MaxCLL. Gate: the
  item's step 2 gate (each check warns on a clip that has the problem and stays silent on one that
  does not; all RPU baselines byte-identical).
- **Checked 2026-10-11 (fix-l6-zero-light-levels):** none of the seven saved rpu-baseline inputs
  has a container 0, so the baselines do not cover the container-0 fix; unit and clip tests do,
  including the L6 of a generated `extra.json`.

### P8: HLG → Profile 8.4 composer

- **Status:** Core complete. Default bt2100, playback confirmed on an LG OLED C9 (2026-10-13);
  history in [the log](docs/ROADMAP_LOG.md) (2026-10-29). The WS6 `code / 1023` question is
  answered for the C9 (below).
- **Gate:** the WS6 playback test shows that devices apply a mapping that is not the preset
  (priority 1); met for the LG OLED C9 (2026-10-13). Step 1: the same canary test on a second
  device shows that the display applies the RPU composer: dropped 2026-10-17, no second device is
  available (owner); the C9 result stands alone.
- The WS6 `code / 1023` question (WS6.3–WS6.6) is answered for the C9: `code / 1023`, so the
  bt2100 fit stays (WS6.6 canary, 2026-10-29). A second device stays a non-blocking note: test one
  if it becomes available.

### P9: HDR10+ → Profile 8.1 L1

- **Status:** Partial. Step 0, scene list, opt-in hybrid flag and peak parity done by
  2026-10-14; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: P9.2, then P9.3
  (owner, 2026-10-17: flip the default after v0.7.0).
- **Finding for the fallback:** both HDR10+ Profile A discs (Alien, Alita) carry MaxSCL 0 on every
  frame, but the percentile distribution (99th percentile 90 to 129 nits per scene on Alita) and
  the average are present. A peak taken from MaxSCL is unusable on such sources, so the pixel
  fallback is required, not optional; whether the percentiles can stand in is open.
- **Plan:** opt-in hybrid mode (scene list = union of HDR10+ starts and the analyzer's cuts, peak
  per piece from HDR10+; average, minimum and L5 from pixels),
  scored on the development tier before any default change, with a pixel fallback for missing or
  implausible HDR10+ statistics. The panel peak is still not passed as a trim target, and
  suspicious scene peaks still only warn.
- **Gate:** the P9.3 gate (priority 2). Step 1's gate (owner, 2026-10-17), done: score the four
  holdout cuts with retail DV L1 (`q-alita-01/02`, `q-the-shining-01/02`) against it; Alien and
  Deadloch stay unseen. Pre-registered rule: hybrid becomes the default if the average's pooled
  clamped MAE (above the floor and over all frames) is lower than today's HDR10+ path, and the
  per-frame MAE of L1 max and of L1 min against retail is not higher than today's path; the owner
  confirms the flip. Done 2026-10-17: PASS on every clause, flip confirmed for the minor after
  v0.7.0.
- [P9.2][gpu][corpus] Sidecar v6 with a per-frame max for an exact fallback peak, also recording
  the analyzer's resolved `--peak-source` so `check_measurements` can refuse a percentile peak.
  Gate: v6 written with a per-frame max and the resolved `--peak-source`; mkvdovi and
  `tools/l1_diff` accept 1–6; `check_measurements` refuses a percentile-peak sidecar under hybrid;
  the fallback peak equals the per-frame max over the piece; default RPU baselines byte-identical;
  CUDA parity; the L1 gate without `--update`.
- [P9.3][corpus] Flip the default, after the v0.7.0 release PR (Release plan: v0.8.0). Gate:
  `--hdr10plus-l1` defaults to `hybrid` and the old path stays selectable by name; HDR10+ RPU
  baselines are re-captured and every other baseline stays byte-identical; CHANGELOG entry under
  Changed. The checks that today fire on the flag value alone, whatever the input, must not turn
  on for every user, each with a test on both sides: `--legacy-madvr-l1` without `--hdr10plus-l1`
  still runs, and only an explicit `--hdr10plus-l1 hybrid` conflicts with it (`Args::flag_conflict`,
  `mkvdovi/src/cli.rs`; the existing test
  `hdr10plus_l1_defaults_to_hdr10plus_and_hybrid_excludes_legacy_madvr_l1` is extended, not
  flipped); a non-HDR10+ input prints no "applies only to HDR10+ inputs; ignored" line under the
  default (`convert_file`, `pipeline/mod.rs`); an HDR10+ input under `--analysis-quality fast`
  without an explicit flag still converts, through the `hdr10plus` path with a visible notice,
  and only an explicit `hybrid` keeps today's refusal (`hybrid_sampling_refusal`).
- Peak against retail is not scored: hybrid takes the same HDR10+ peak as `dovi_tool`, about 104
  codes below retail on the holdout cuts.
- **Checked 2026-10-31 @2a7bb35:** P9.2 first: a fallback piece takes the scene max (`hybrid.rs:340`).
  Serialize `pq_to_12bit(frame.peak_pq_2020)` after `select_peak_pq` (`scenes.rs:209`), the value
  of the scene max (`l1_sidecar.rs:292`); `peak_source` is local (`scenes.rs:163`), thread it to
  `pipeline/mod.rs:282`. Under hybrid refuse v5 (`measure.rs:410`). P9.3: resolve one mode (`cli.rs:48`,
  `pipeline/mod.rs:130`, `measure.rs:185`), hint `config.rs:162`, `sha256sum`; holdout had 0 fallbacks.

### P10: measured against delivered

- **Status:** Partial. Steps 1 and 2 (2026-10-04) and step 4 (2026-10-17) done; history in
  [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: P10.3, which waits for WS7.1 (owner,
  2026-10-17: WS7 first, no playback test).
- `dovi_tool generate` clamps L1 (minimum ≤ 12, maximum ≥ 2081, average ≥ 819). Numbers:
  [`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md).
- **Gate:** the P10.3 gate (priority 3). Step 4's gate, done 2026-10-17: a blinded A/B on the LG
  OLED C9 of `ms_weight` 2048 against 512; no visible difference closes the step with 2048
  unchanged.
- [P10.3][corpus] Decide whether to write L1 in-process so the measured minimum and sub-100-nit
  maxima reach the RPU; raising the average floor to 1229 is not planned. Gate: the WS7 simulator
  shows that unclamped L1 is better (owner, 2026-10-17: WS7 first, no playback test). Needs: WS7.1.

### P12: `--mdfix` ignores the source RPU's L6

- **Status:** Open. Defect found 2026-10-15, not fixed; history in
  [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: P12.1, for the next patch release.
- Under `--mdfix` the mastering min/max come from the container or the defaults
  (`metadata::read_static_metadata` + `apply_static_defaults`, `mkvdovi/src/pipeline/config.rs:43-44`),
  never from the source RPU's L6. On r5 Inside Out 2 (P8.1, no ST 2086 in the container) the RPU's
  0.0001/1000 nits becomes 0.005/1000 nits and `source_min_pq` 7 becomes 62 on every frame
  (baseline `~/mkvdovi-work/rpu-baseline/0.6.0/mdfix/r5-inside-out-2-mind-console`). The only
  message is a generic "not found" warning.
- **Gate:** (owner, 2026-10-16) a unit test that is red on the parent; the r5 Inside Out 2
  `--mdfix` RPU carries the source L6 (0.0001/1000 nits, `source_min_pq` 7); every other RPU
  baseline byte-identical.
- [P12.1][corpus] Under `--mdfix`, fall back to the sampled source RPU's L6 before the defaults.
  Changes delivered RPUs: own branch (`fix-p12-mdfix-source-l6`), RPU baselines updated; between
  E13 moves. Gate: the item gate, "every other RPU baseline byte-identical" checked with `sha256sum`
  of the `RPU.bin` files (`rpu-baseline.sh compare` ignores the CRC), plus `cargo mutants --in-diff`.
- **Checked 2026-10-31 @2a7bb35:** the sampled L6 values are not kept: the authored-levels scan
  records only `acc.l6 = true` (`mkvdovi/src/rpu_check.rs:286`), `SampledRpu`
  (`rpu_check.rs:177`) has no L6 field, and `prepare_configuration` (`pipeline/config.rs:30`) gets
  no sampled metadata, so the fix threads the values through and tests the pipeline wiring.
  `scripts/rpu-baseline.sh compare` strips the CRC (`:248`), so "byte-identical" needs `sha256sum`.

## `cm_analyze` parity

The detailed gap table and validation method live in
[`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md). These workstreams are dependency ordered.

### WS0: L1 scoring tools

- **Status:** Core complete. `tools/l1_diff`, ground-truth tests and the CI L1 gate on a six-shot
  PQ and HLG clip shipped; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: WS0.1,
  which needs WS8.1.
- **Gate:** the CI L1 gate covers the WS8 synthetic cases (grain on highlights, a flash, a fade, an
  aspect-ratio change), each with committed references (owner, 2026-10-17).
- [WS0.1][desk] Grow the corpus: the WS8 synthetic cases in the CI L1 gate (this step also covers
  the former E1 step 1, which had the same gate; feeds WS6). Gate: the item gate. Needs: WS8.1 (code).

### WS1: grain-robust peak

- **Status:** Partial. Measurement core, per-scene minimum (clamped by the generator, P10) and the
  max-RGB-mean average domain have shipped. HLG max-RGB shipped in v0.5.0.
- A grain-robust peak is missing. Raw max-RGB reads hot against `cm_analyze` v2
  ([VALIDATION.md §7](docs/VALIDATION.md)). The current opt-in `robust` rule
  ([TECHNICAL_REFERENCE.md §2.4](docs/TECHNICAL_REFERENCE.md)) removes part of the grain bias but
  lowers small highlights near the grain and clean content, and has not been scored against
  `cm_analyze`. No histogram-only or spatial statistic tried so far passes.
- **Finding (2026-10-05):** the default `max` peak also reads high on clean retail sources:
  per-shot bias against retail L1 is +148 to +162 codes on Alita: Battle Angel (digital, largely
  computer-generated) and +88 and +162 on Skyfall (both Profile 7 MEL). Grain does not explain
  that. Find what does (for example the reference's spatial or temporal treatment of small
  highlights) before more grain-estimator work.
- **Rules for a new estimator:** identical output on CPU and CUDA (see
  [`docs/CUDA_PIPELINE.md`](docs/CUDA_PIPELINE.md)); its frozen synthetic gate includes a small
  specular, a one-frame flash, a highlight on the first or last frame of a shot, a 2–3 frame
  specular and a fade.
- **Gate:** (owner, 2026-10-17) a new estimator becomes the default only if, on the development
  tier, its pooled per-shot peak MAE against retail L1 is lower than `max`'s; no case of the
  frozen synthetic gate is worse than `max` by more than 1 code; CPU = CUDA; and the result is
  confirmed on unseen holdout cuts. Step (1) closes with a written explanation of the clean-source
  over-read that a test reproduces. Do not enable the robust estimator by default before this.
- [WS1.1][corpus] Explain the clean-source over-read. Gate: a written explanation of the
  clean-source over-read that a test reproduces.
- [WS1.2][corpus] The display-unit scale (WS7). Gate: the peak errors of `max` and `robust`
  reported in WS7 simulator units (displayed-luminance change, 1000-nit panel) beside the code
  errors on the development tier (owner, 2026-10-17). Needs: WS7.1.
- [WS1.3][gpu][corpus] A grain-robust peak. Gate: the item gate (beats `max` on pooled per-shot
  peak MAE against retail on the development tier, no frozen synthetic case worse by more than
  1 code, CPU = CUDA, confirmed on unseen holdout cuts). Needs: WS1.1.
- [WS1.4][owner] True target-gamut peaks. Gate: TODO(owner).

### WS2: shots and scene averages

- **Status:** Partial. Shot-constant L1 and unbiased scene averages (sidecar v4) are in place;
  the `cm_analyze` v2 average comparison is not repeated for v4; history in
  [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: WS2.1, WS2.2, WS2.4; step 3 (L4) deferred.
- **Gate:** step 1: precision and recall against authored shot lists, no loss on the other cuts
  (priority 8). Step 2: on WS8 transition material, per-frame L1 inside detected dissolves and
  fades is closer to the truth than shot-constant L1, and the WS7 simulator shows no added
  frame-to-frame steps (no pumping); opt-in only. Step 3: reopen when an open renderer or a tested
  device reads L4. Step 4: the WS1 gate (owner, 2026-10-17).
- Steps in this order:
- [WS2.1][gpu][corpus] A second scene signal for whole-picture motion (E4.1, E4.2), and strong cuts
  closer than `--min-scene-length` 12, by a confidence exception, not a lower global minimum.
  Gate: precision and recall against authored shot lists, no loss on the other cuts (priority 8).
- [WS2.2][gpu][corpus] Research, opt-in: per-frame L1 inside detected dissolves and fades; needs a
  per-frame peak in the sidecar (a version bump; P9.2 adds a per-frame max in sidecar v6) and
  transition test material (WS8.3), and can cause pumping. Gate: on WS8 transition material, per-frame L1 inside detected dissolves and fades
  is closer to the truth than shot-constant L1, and the WS7 simulator shows no added
  frame-to-frame steps (no pumping); opt-in only. Needs: WS8.3, WS7.1, P9.2 (code).
- Step 3, deferred (no step line, like the deferred items): L4 emission, until something can
  validate it (no open renderer reads L4). Reopen when an open renderer or a tested device reads
  L4 (owner, 2026-10-17); it then becomes WS2.3.
- [WS2.4][gpu][corpus] Robust aggregation (investigated with WS1.3). Gate: the WS1 gate. Needs:
  WS1.1.

### WS4: automatic trims

- **Status:** Deferred; experimental. Optional L2/L8 trims from an open tone-mapping baseline such
  as ITU-R BT.2390. Not started before WS7 exists, and WS7 alone cannot pass it: no open renderer
  applies trims, so a trim can only be validated on a device.
- **Gate:** blinded A/B testing on a device demonstrates an improvement; until then neutral trims
  remain the default.

### WS5: CM metadata XML export

- **Status:** Deferred. Nothing built; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29).
  Worthwhile only if the external Profile 5 authoring route (R2) is chosen.
- **Gate:** only if R2 is reopened; R2's gate covers it (owner, 2026-10-17).

### WS6: final-RPU corpus and playback procedure

- **Status:** Open. WS6.3, WS6.4 (PR #66), WS6.8 (PR #78), WS6.5 and WS6.6 (PR #80) closed; the
  `code / 1023` question is answered for the C9 (`code / 1023`, the bt2100 fit stays). History in
  [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: WS6.9, metered with the i1Display Pro in Dolby
  Vision Cinema on the "Metered tests (later)" list (owner decision;
  `~/mkvdovi-work/ws6-neutral-probe-2026-10-27/`; method in [`docs/PLAYBACK.md`](docs/PLAYBACK.md)
  "Metered tests"); WS6.1.
- Final-RPU regression corpus covering grain, saturated highlights, raised blacks, fades, flashes,
  rapid cuts and changing aspect ratios; checks run on the RPU extracted from the muxed file.
- A Shield/TV playback procedure comparing matched material from the same master, recording player,
  firmware, TV picture mode and HDMI path. It is the gate for P8, the WS6 normalization question
  and P10's device steps.
- **Gate:** (owner, 2026-10-17) (1) `scripts/rpu-baseline.sh` holds one captured baseline per
  listed feature; (2) the procedure is written (a docs section plus a `VIEWING_GUIDE.txt`
  template recording player, firmware, picture mode and HDMI path) and used once, for (3); (3) a
  discriminating canary, a composer that is neutral under one normalization and visibly coloured
  under the other, checked in libplacebo first, viewed on the LG OLED C9, answer recorded; or
  (owner, 2026-10-22) the neutral-finder probe (WS6.9), when its fit of neutral code against
  gain separates the two normalizations, answer recorded.
- [WS6.1][corpus] The final-RPU regression corpus. Gate: `scripts/rpu-baseline.sh` holds one
  captured baseline per listed feature.
- [WS6.9][owner][reassess] View or measure the neutral-finder probe on the LG OLED C9. Read by eye
  on 2026-10-29 (status above): gate not met; what remains is the metered run on the "Metered
  tests (later)" list (owner, 2026-10-29), with `probe_meter.py check` first and the WS6.8 build in
  `~/mkvdovi-work/ws6-neutral-probe-2026-10-27/build/`, re-delivered as a Plex show. Gate: the apparent
  neutral BL chroma code per gain recorded in the log (to about ±0.5 code by eye, ±0.25 metered),
  with the fit of neutral code against gain. Its intercept at gain 0 is the input-side reading
  (arm A: 911.6 under `code / 1023`, 912.4 under `code / 1024`, 912.2 under a 12-bit `4·code / 4095`
  tunnel), the slope the gain-dependent shift; the outcome follows the probe's pre-registered
  rules (`PREREG.md` draft 2). Needs: WS6.8. Landed: PR #80; gate pending.
- **Checked 2026-10-25 @3c0c913:** `canary_mapping` moved to `dovi84_composer/src/canary.rs:18`
  (E16) and still only offsets constants; the ladder's gain composers live in the local `ws6ladder`
  evidence crate (`~/mkvdovi-work/ws6-slope-ladder-2026-10-18/`, `run.sh`), so a probe built there
  keeps the PR docs-only (touching `dovi84_composer/src/` adds CUDA parity and the L1 gate).
  Pre-register the fit's residual limit and an inconclusive outcome (the intercepts differ by one
  code); fix the picture mode (ladder Cinema, canaries Cinema Home; log 2026-10-22). No `spotread` in WSL.
- **The `code / 1023` question (2026-10-06):** do devices evaluate the 8.4 composer at `code / 1023` (as
  libplacebo and the `bt2100` fit) or at the spec's `code / 1024`? Under the latter `bt2100` is
  5.9 codes off neutral, probably too little to see on a neutral ramp, so the test needs a pattern
  that separates the two. A `code / 1024` answer means a refit (new composer and RPU). The P8
  canary test (2026-10-13) does not answer it.

### WS7: evaluation in displayed-picture units

- **Status:** Open. Nothing has shipped; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29).
  Open: WS7.1, then WS7.2.
- **Plan:** (1) a histogram-domain display-mapping simulator in a tool crate outside the workspace:
  per-frame PQ histograms and an L1 stream through libplacebo's spline, the SMPTE ST 2094-10 curve
  and the ITU-R BT.2390 EETF for 100-, 600- and 1000-nit panels, comparing candidate with reference
  metadata as change in displayed luminance, clipped highlight share and frame-to-frame steps. It
  needs a per-frame histogram dump from the analyzer (integer counts, identical on CPU and CUDA).
  (2) ColorVideoVDP on rendered output of a few cuts.
- **Gate:** the simulator reproduces the numbers recomputed on 2026-10-03
  ([`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md)): for a 1000-nit shot on a 1000-nit
  panel, L1 max +75 / +140 codes costs 14.6% / 24.4% at the peak with the spline, and nothing while
  L1 max stays below the panel peak.
- [WS7.1][gpu] The simulator, with the analyzer's per-frame histogram dump it needs (a
  `cuda-parity` path). Gate: the item gate.
- [WS7.2][corpus] ColorVideoVDP on a few cuts. Gate: ColorVideoVDP JOD scores for candidate against
  reference metadata on three development cuts, renderer and versions pinned, recorded in the log
  next to the simulator's numbers for the same cuts (owner, 2026-10-17). Needs: WS7.1.
- **Limits to state with every result:** open curves are not a Dolby display; libplacebo reads only
  L1 max/avg and the source range, so trims, L1 min, L4 and L11 cannot be evaluated in the open;
  pin the libplacebo version; host limits in [`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md#ws7-open).

### WS8: test material

- **Status:** Partial. Real material per format landed 2026-10-05; the other fixes the 2026-10-03
  review asked for are open.
- **Gate:** per item: synthetic material comes from a committed, reproducible generator; truth
  files are present; the L1 gate or the development corpus uses the material (owner, 2026-10-17).
- [WS8.1][desk] Grain on the synthetic highlights and odd offsets (today they carry no grain and
  fill exactly one chroma quad). Gate: the item gate.
- [WS8.2][corpus] A grainy twin from the same master as the clean one (today "clean is truth"
  holds only to about ±50 codes). Gate: the item gate.
- [WS8.3][corpus] Synthetic cross-dissolves and fades built from real shots (blended in PQ code and
  in linear light) with their intervals as truth, and retail cuts with authored transitions. Gate:
  the item gate.
- [WS8.4][desk] A held-out split: synthetic references and open implementations for development,
  licensed reference output for scoring only ([`docs/PROVENANCE.md`](docs/PROVENANCE.md)). Gate:
  the item gate.

## Dolby Vision profile work

### FEL: Profile 7 FEL

- **Status:** Deferred; on hold by the owner (2026-10-13). The compositor was removed and Profile 7
  FEL inputs are refused; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29). No step until the
  hold is lifted; a fixed-pixel 8.1 chroma canary then needs new code (`canary-rpu` takes only
  bt2100 8.4).
- The only FEL work of interest keeps the base layer bit-exact and carries FEL information, as far
  as possible, in the Profile 8.1 RPU: [`docs/FEL_PLAN.md`](docs/FEL_PLAN.md). This item replaces
  F1, F2 and R1. Profile 7 MEL conversion is unchanged.
- **Gate:** none while on hold; when the owner lifts the hold, the F0 gate of the local
  `FEL_ROADMAP.md` applies (owner, 2026-10-17).
- Not held: E15.1 keeps a malformed source RPU (Profile 7, or one found by the RPU sniff) from
  aborting `mkvdovi` before the FEL refusal.

### R2: Profile 5

- **Status:** Deferred; conditional. Genuine Profile 5 uses IPT, so HDR10 BL → Profile 5 needs
  decoding, colour transformation and re-encoding: feasible through an established encoder
  (Resolve Studio, Dolby Encoding Engine), not in this project. Changing profile flags on untouched
  HEVC is not a target.
- **Gate:** a matched HDR10 / P8.1 / correctly encoded P5 playback comparison from the same master
  with the playback chain recorded (the precondition for any work).

## Engineering backlog

### E1: regression gates

- **Status:** Core complete. CI runs the L1 regression gate, the `--features cuda` clippy and
  synthetic accuracy; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: E1.2, E1.3.
- **Gate:** (owner, 2026-10-17) (1) the WS0 gate (step 1 merged into WS0.1). (2) A self-hosted runner on the RTX 4070 WSL2
  host runs `scripts/cuda-parity.sh` for pushes to `main`/`fix-*` and for PRs from branches of
  this repository, never for fork PRs (a self-hosted runner on a public repository would otherwise
  run outside code), and blocks on failure; it runs only while the host is on. (3) A CI job
  compiles `kernels.cu` with NVRTC from the pip `nvidia-cuda-nvrtc` wheel, without a GPU, and is
  red on a deliberately broken kernel.
- [E1.2][gpu] A self-hosted GPU runner for `scripts/cuda-parity.sh`, now a prerequisite because the
  main analysis path is untested in hosted CI. Gate: the item's gate (2).
- [E1.3][desk] Spike: an NVRTC compile-only check of `kernels.cu` without a GPU. Gate: the item's
  gate (3).

### E2: crop detection

- **Status:** Partial. Seven-position crop probing, modal voting and variable-AR union shipped
  (PR #4); history in [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: E2.1 (with P3.1) and E2.2.
- **Gate:** (owner, 2026-10-17) (1) together with P3: on clips whose aspect ratio changes (WS8),
  each scene's crop and L5 match the visible picture area within 2 px; constant-ratio RPU
  baselines byte-identical; CPU = CUDA. (2) The probe's share of wall time on CUDA is measured on a
  full episode; above 10%, an NVDEC probe that reproduces `detect_crop` exactly (parity test).
- [E2.1][gpu][corpus] Per-scene crop application, a continuity-sensitive follow-up; together with
  P3.1. It needs a second pass, because the crop is fixed before the histograms are built
  (affordable on NVDEC, not on the CPU). Gate: the item's gate (1).
- [E2.2][gpu][corpus] The crop probe decodes on the CPU in CUDA runs: measure its cost on a full
  episode, then decide an NVDEC probe. A kernel crop reduction must reproduce `detect_crop` exactly
  (every 10th sample, at least 10% non-black per row and column, `crop.rs`). Gate: the item's
  gate (2).

### E3: `mkvdovi` debug options

- **Status:** Open. Nothing has shipped; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29).
  Open: E3.1.
- **Gate:** integration tests: `--dry-run` writes nothing and lists the planned steps;
  `--keep-temp` and `--keep-logs` leave their files after a successful run; without the flags the
  behaviour is unchanged (owner, 2026-10-17).
- [E3.1][desk] Add `mkvdovi --dry-run`, `--keep-temp` and `--keep-logs`. Gate: the item gate.

### E4: second scene signal

- **Status:** Open. `--scene-metric hybrid` is still a histogram-only placeholder; history in
  [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: E4.1 to E4.4; both residuals are
  sensitive to flashes, exposure changes and low texture. Full optical-flow fusion stays deferred.
- **Gate:** precision and recall on held-out authored shot lists, no loss on the other cuts
  (priority 8).
- [E4.1][gpu][corpus] Compare a spatial-thumbnail residual. Gate: the item gate, scored for this
  signal.
- [E4.2][gpu][corpus] Then a motion-compensated residual. Gate: the item gate, scored for this signal.
  Needs: E4.1.
- Prior art for E4.3 and E4.4 (2026-10-26): the CB1 engine of CroqueMr/libreelec-x86-DV
  (`CB1/src/cb1_hdr10_temporal.c:49, 81` at commit 5e63e16) uses both in its live cut detector. It
  is GPL-3.0: the method only, no code copied. Either changes cuts and so L1: the L1 regression
  gate needs `--update` with the reference diff in the PR.
- [E4.3][gpu][corpus] Exposure veto: fit one least-squares gain per channel between the
  downscaled frames on either side of a candidate cut; a gain inside (0.25, 4) with a small
  residual marks a fade or exposure change, not a cut. Targets the flash and exposure sensitivity
  named in the status. Gate: the item gate, scored for this veto.
- [E4.4][gpu][corpus] Shape statistic: the mean-removed frame difference on the downscaled frames
  as a second cut channel next to the chi² histogram score; it catches cuts whose histograms look
  alike (one cut: 7 analyzer scenes against 32 authored shots, `docs/FEL_PLAN.md` section 8). Can
  share E4.1's thumbnail. Gate: the item gate, scored for this signal.

### E5: hardware decode contexts

- **Status:** Deferred. Nothing built; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29).
  It would add VAAPI and VideoToolbox decode device contexts and hardware-frame transfer.
- **Gate:** reopen on a user request; then `.bin` and sidecar byte-identical to software decode on
  the PQ and HLG clips (owner, 2026-10-17).

### E6: provenance

- **Status:** Core complete. Sidecar v2 records analyzer version, input name and size, dimensions,
  transfer, downscale, sample rate, GPU use and crop. Resume temp directories carry a fingerprint
  of input name, size and mtime, mkvdovi version and artifact-affecting settings.
- **Gate:** a global MKV tag on the output (for example `MKVDOVI_TOOLS`) records the versions; a
  test reads it back, and the video stream stays bit-identical (owner, 2026-10-17: in the file,
  not a sidecar).
- [E6.1][desk] Record external tool versions (`dovi_tool`, `mkvmerge`, `ffmpeg`) for each output.
  Gate: the item gate.

### E7: completeness

- **Status:** Partial. `--verify` checks the RPU frame count against the muxed track and the
  sidecar; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: E7.1, E7.2.
- **Gate:** (owner, 2026-10-17) (1) a test in which a truncated direct-MKV output is caught by
  frame count and falls back to ffmpeg extraction; (2) the RPU frame count is compared with the
  muxed video on every run (full `--verify` stays opt-in), tested on both sides, with the added time
  per file measured and reported.
- [E7.1][desk] Direct-MKV `dovi_tool` steps still accept exit status plus non-empty output. Gate:
  the item's gate (1).
- [E7.2][corpus] An always-on frame-count check. Gate: the item's gate (2) (the added time per file
  is measured on real files).

### E8: histogram percentile reader

- **Status:** Open. Readers convert a bin index with bin/255, though the bins split 64/192 at
  PQ(100 nits), so 1000 nits reads as about 305; history in [the log](docs/ROADMAP_LOG.md)
  (2026-10-29). Open: E8.1, in a later minor release.
- **Gate:** a bin-edge round-trip unit test and an optimizer unit test (CUDA parity and the L1 gate
  run with `--disable-optimizer`, so they do not cover it). **Prerequisite:** E13 step 5 (met 2026-10-17, #59).
- **Checked 2026-10-06 @df54ad4:** `find_highlight_knee_nits` feeds the optimizer's `target_nits`
  on every default run (`optimizer.rs:152`), so the fix changes the default `.bin`. CUDA parity
  (`cuda_parity.rs:94`) and the L1 gate (`scripts/ci/l1-regression-gate.sh:76`) both pass
  `--disable-optimizer`, so a round-trip and optimizer unit test is the gate. The bin edges are
  written in `frame.rs:667` (`analyze_native_frame_cropped`) and `gpu.rs`
  `build_luminance_bin_lut`; `frame.rs:970` (`analyze_native_frame`) bins uniformly. Line numbers
  are from before the E13 step 5 split; the function names stay valid.
- [E8.1][gpu] One shared bin-edge function for writer and readers, with a round-trip test. Gate:
  the item gate.

### E9: `--pre-denoise` values

- **Status:** Open. Only `median3` works, other values do nothing; history in
  [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: E9.1.
- **Gate:** a unit test for the option (priority 4). **Prerequisite:** E13 step 5 (met 2026-10-17, #59).
- [E9.1][gpu] Restrict the values to `off` and `median3`. Gate: the item gate.

### E10: L1 code scaling

- **Status:** Open; unverified. The analyzer converts normalized PQ to a 12-bit code with ×4095;
  ETSI GS CCM 001 §6.2.2 encodes L1 as `clip(round(x × 4096))`. The difference is at most one
  code. The generator's own conversion (`dolby_vision::utils::nits_to_pq_12_bit`, also used for
  the P10 source range) is ×4095 too, so analyzer and generator agree (checked 2026-10-04).
- **Gate:** retail RPUs of 4000-nit masters, where the two scalings differ (`source_max_pq` 3696
  against 3697), show which one the reference uses; ×4096 → a one-code fix in analyzer and
  generator conversion, otherwise close (owner, 2026-10-17).
- [E10.1][corpus] The check against retail RPUs. Gate: the item gate.

### E11: `l1_diff` and open-GOP cuts

- **Status:** Core complete. `tools/l1_diff` re-bases open-GOP cuts on the sidecar v5 skip
  count (2026-10-06); history in [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: E11.1, the v5
  re-analysis of Joker and the Champions League cut.
- **Gate:** Joker and the Champions League cut re-analyzed with a v5 analyzer, and their per-frame
  `l1_diff` scores in the log (owner, 2026-10-17).
- [E11.1][corpus] Supporting, not the original gate: Joker and the Champions League cut (G4) have
  only v4 runs; scoring Joker's retail L1 per frame needs a re-analysis with a v5 analyzer. Gate:
  the item gate.

### E12: CUDA release binaries

- **Status:** Open. Nothing landed; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29).
  `release.yml` builds without `--features cuda`, so `auto` resolves to CPU `balanced` for
  release users. Open: E12.1, for v0.8.0.
- **Plan:** build Linux x64 and Windows x64 with the feature (no toolkit needed at build time;
  macOS already returns an error from `load_cuda_libraries`), smoke-test `--version` for `+cuda`
  and a clean CPU fallback without libcuda. Windows `--features cuda` is untested. NVIDIA's
  driver does not ship NVRTC, which compiles `kernels.cu` at runtime (`gpu/backend.rs`), so the
  Windows zip bundles the NVRTC DLLs (NVRTC is on NVIDIA's redistributable list; owner decision
  2026-10-17); Linux users still need the NVRTC library installed.
- **Effect:** changes delivered L1 and MaxCLL for NVIDIA release users (`balanced` → `accurate`);
  needs a CHANGELOG entry.
- **Gate:** the release archives report `+cuda` in `--version` and fall back cleanly to the CPU
  without libcuda; the Windows archive runs GPU analysis with only the driver present;
  `v0.8.0-rc.1` tested on the Windows side of the RTX 4070 dev host with the CUDA toolkits hidden
  from `PATH` (Release plan).
- [E12.1][gpu] Build the Linux x64 and Windows x64 release archives with `--features cuda` as in
  the plan (the Windows zip bundles NVRTC), with the smoke tests, for v0.8.0. Gate: the item gate.

### E13: module size refactor

- **Status:** Partial. Steps 2-5 and E13.11 done, no production file over the 800-line
  limit; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: E13.6-E13.10 and
  E13.12-E13.21; each function over 150 lines carries an `#[expect]` marker naming its step.
- **Gate:** per move (E13.18, E13.19): fmt, clippy (also `--features cuda`), tests;
  the L1 regression gate without `--update`; `scripts/cuda-parity.sh` for analysis paths; for
  mkvdovi, `scripts/rpu-baseline.sh compare --require-identical-l1` against the baselines captured
  at main@e6d63ed in `~/mkvdovi-work/rpu-baseline/refactor-e6d63ed/` (HDR10, HDR10+, HLG, two MEL
  incl. an open-GOP start; `--mdfix` on MEL and Profile 8; HLG with `--hlg-composer preset`, the
  path without the RPU rewrite), and
  `MKVDOVI_CORPUS_DIR=~/mkvdovi-work/corpus/dev cargo test -p mkvdovi corpus_cuts_are_classified_like_their_manifests`,
  the only test that reaches the Profile 7 MEL/FEL branch of `check_hdr_format`, which the step 3
  moves cut through. Each command's result goes in the move's PR body: the pre-PR panel computes
  only fmt, clippy and test (plus the CUDA gates) for mkvdovi paths. An automatic rpu-baseline gate
  would need a baseline-path convention and an update mode for PRs that change the RPU on purpose;
  it is not planned.
- [E13.6][gpu][corpus] Split `run_native_analysis_pipeline` (455 lines, `pipeline/analyze.rs`) by
  phase. Gate: fmt, clippy (also `--features cuda`), tests; the L1 regression gate without
  `--update`; `scripts/cuda-parity.sh` PASS; optimizer-enabled CPU and CUDA `.bin` and sidecar
  byte-identical to main on the step 5 run set; for mkvdovi code,
  `scripts/rpu-baseline.sh compare --require-identical-l1` (owner, 2026-10-17: the step 5
  evidence).
- [E13.7][gpu] Chore PR (comments and strings only): stale `gpu.rs` references, now
  `analysis/gpu/mod.rs`, in `kernels.cu:23`, `:52` and the `:54` static_assert message,
  `.claude/workflows/pre-pr-panel.js:133` and `pre-pr-gemini.prompt.md:25`. Gate: a CUDA parity run
  (for the `kernels.cu` edit) and `gemini-harness` (for the prompt edit) pass.
- [E13.8][gpu][corpus] Split `analyze_native_frame_cropped` (276 lines, `analysis/frame/mod.rs`) by
  phase, after E8.1, which edits its bin edges. Gate: as E13.6. Needs: E8.1.
- [E13.9][gpu][corpus] Split `run` (281 lines, `pipeline/mod.rs`) by phase. Gate: as E13.6.
- [E13.10][gpu][corpus] Split `launch_and_collect` (243 lines, `analysis/gpu/backend.rs`) by phase.
  Gate: as E13.6.
- Code lines below are clippy's count (blank and comment lines skipped), 2026-10-28 @dc4882d. The
  mkvdovi gate: fmt, clippy, tests, and `scripts/rpu-baseline.sh compare --require-identical-l1`
  against `~/mkvdovi-work/rpu-baseline/e13-step5/`.
- [E13.12][corpus] Split `convert_file` (`mkvdovi/src/pipeline/mod.rs`) by phase; step 4 left it at
  281 lines. Gate: the mkvdovi gate.
- [E13.13][corpus] Split `verify_post_mux_with_options` (`mkvdovi/src/verify.rs`, 256 lines) by
  phase. Gate: the mkvdovi gate.
- [E13.14][corpus] Split `acquire_measurements` (`mkvdovi/src/pipeline/measure.rs`, 221 lines) by
  phase. Gate: the mkvdovi gate.
- [E13.15][corpus] Split `prepare_configuration` (`mkvdovi/src/pipeline/config.rs`, 211 lines) by
  phase. Gate: the mkvdovi gate.
- [E13.16][corpus] Split mkvdovi's `main` (`mkvdovi/src/main.rs`, 188 lines) by phase. Gate: the
  mkvdovi gate.
- [E13.17][corpus] Split `mdfix_notice` (`mkvdovi/src/pipeline/mdfix_notice.rs`, 160 lines) by
  phase. Gate: the mkvdovi gate.
- [E13.18][desk] Split `tools/fit_hlg_composer/src/siting.rs` (803 lines, limit 800) as a pure move.
  Gate: `tool:fit_hlg_composer`, and a diff that shows only moved blocks.
- [E13.19][desk] Split `tools/l1_diff/src/main.rs` (948 lines, limit 800) as a pure move, after the
  local `feat/dev-corpus` branch that depends on it has landed. Gate: `tool:l1_diff` and the L1
  regression gate without `--update`, and a diff that shows only moved blocks.
- [E13.20][desk] Function lint in the excluded `tools/` crates: `[lints.clippy] too_many_lines =
  "warn"` in each `Cargo.toml` (the root `clippy.toml` threshold applies), with step-named markers;
  `l1_diff` (its `main` is 312 lines) after `feat/dev-corpus` has landed. Gate: the `tool:*` gates.
- [E13.21][gpu] Turn on clippy's `style`, `complexity` and `perf` groups (warn) and fix what they
  report: about 30 warnings at @dc4882d (doc list indentation, empty format literals, a regex
  compiled in a loop, one function with 11 arguments). Gate: fmt, clippy (also `--features cuda`),
  tests; the L1 gate without `--update`; `scripts/cuda-parity.sh` when analysis code changes.
- **Simplification candidates for later PRs (not in a move):** `read_static_metadata` stores
  `md_*` mastering primaries that nothing reads, and its parser does not match MediaInfo 24.01's
  spellings anyway; two `impl L1Sidecar` blocks.
- **Checked 2026-10-17 @941489d:** before the split the Stop hook's count (`.claude/hooks/rust-check.sh`)
  gave `pipeline.rs` 1348, `analysis/frame.rs` 1026 and `analysis/gpu.rs` 951 lines; both gates pass
  `--disable-optimizer`, so the step diffed optimizer-enabled `.bin` files. Baselines from
  2026-10-11 in `~/mkvdovi-work/rpu-baseline/l6-fix-e052c3d/`.

### E14: test-suite audit

- **Status:** Partial. Steps 1-2 done; steps 3-4 and step 5's high-risk groups done for
  `verify.rs` and the riskiest modules (main@c74effb, #55); history in [the
  log](docs/ROADMAP_LOG.md) (2026-10-29). Open: E14.3-E14.6; smell list for E14.4/E14.5:
  `~/mkvdovi-work/test-audit-2026-10-15/smells-*.md`. Owner, 2026-10-17: in-session tests plus the
  E14.6 mutation gate, no separate test author.
- **Gate:** a clean `check-test-skips` log on the audit host; per-module mutant counts (caught /
  missed / unviable / timeout) before and after; every survivor in a bucket.
- **Order (2026-10-16):** none of it gates a release.
  1. Steps 3 and 4 (mutation runs, triage) are local, in a detached worktree, and need no branch,
     so they can run at any time. Steps 5 and 6 are PRs: they open only when no E13 move PR is in
     flight (the E13 sequencing rule).
  2. Step 3 on the mkvdovi contract modules: `resume.rs`, `external.rs`,
     `metadata/{mod,probe}.rs` and `pipeline/{mod,config,analyzer,hybrid,hdr10plus,temp_dir,mdfix_notice}.rs`
     (413 mutants, about 70 min at `-j 3`), then `metadata/static_metadata.rs` (323, about
     55 min). Triage, then step 5 for the 17 open medium-risk groups (115 mutants) of the c74effb
     run and the medium-risk groups of both new runs.
  3. Step 3 on the analyzer contract files `l1_sidecar.rs`, `leading_pictures.rs`, `ffmpeg_io.rs`
     and `writer.rs` (273; `cargo mutants --list` gives no mutants in `cli.rs`). E13 step 5 does
     not move them.
  4. Step 6, the `--in-diff` gate. Run it by hand
     on two PRs first to measure its cost per PR.
  5. Low-risk groups get no sweep of their own: they are fixed when the step 6 gate lists them in
     code a PR changes. `main.rs`, `progress.rs` and `cli.rs` (142 mutants, mostly output text)
     run last, if at all.
  6. Analyzer numerics after E13 step 5, with E8 as the validation case.
- [E14.3][desk] Mutation baseline (`cargo-mutants`) on `mkvdovi/src`, `verifier/src`,
  `dovi84_composer/src` and the analyzer contract files (`l1_sidecar.rs`, `leading_pictures.rs`,
  `ffmpeg_io.rs`, `writer.rs`, `cli.rs`), after a clean `check-test-skips` baseline on the host.
  Done for `verify.rs` and the riskiest modules; open: the runs in Order 2 and 3 above. Analyzer
  numerics (Order 6) now that E13 step 5 is done, with E8.1 as the validation case. Local, in a
  detached worktree, no branch. Gate: the item gate (clean `check-test-skips` log; per-module
  mutant counts; every survivor in a bucket) for the modules run.
- [E14.4][desk] Triage every survivor (missing test / equivalent / actual bug / dead code /
  characterization); actual-bug survivors go to the owner before any test. Gate: every survivor of
  the E14.3 runs in a bucket. Needs: E14.3.
- Step 5, test-only PRs per crate area (each new test names the mutant it kills or the decision
  side it adds): high-risk groups done (#55). One step per PR from here: E14.5 takes the medium
  groups of the c74effb run; the medium-risk groups of the E14.3 runs get their own step IDs after
  E14.4 (low-risk groups per Order 5). They open only when no E13 move PR is in flight.
- [E14.5][gpu] Tests for the 17 medium-risk groups (115 mutants) of the c74effb run (they include
  `dovi84_composer`, a `cuda-parity` path). Gate: the targeted mutants caught (per-module counts
  before and after), each new test naming its mutant or decision side.
- [E14.6][desk] A `cargo mutants --in-diff` gate in the pre-PR panel for mkvdovi, verifier and
  dovi84_composer logic (fails only on infrastructure errors; survivors listed with their bucket),
  and red-on-parent evidence for fix PRs. Run it by hand on two PRs first to measure its cost per
  PR (Order 4). Gate: the gate runs in the pre-PR panel, fails only on infrastructure errors,
  lists survivors with their bucket, and its cost on two PRs is recorded.

### E15: abort-safe RPU parsing

- **Status:** Open. Defect found, not fixed; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29).
  Open: E15.1: `dolby_vision` 3.4.0's `unimplemented!` on a polynomial piece with
  `poly_order_minus1 == 0` and `linear_interp_flag` set (`src/rpu/rpu_data_mapping.rs:415`) aborts
  `classify_rpu_format`, `analyze_rpu_file` and `sample_rpu_windows` under `panic = "abort"`.
- [E15.1][desk] Pre-scan each source RPU's data mapping for that piece before `parse_rpu_file` in the
  three functions, and return an error instead; re-check the scan when the crate is bumped. Gate: a
  unit test with an RPU carrying such a piece (built bit by bit, since the crate's writer panics on it
  too) gets an error from each of the three functions where the parent panics; a multi-file run
  continues with the next file; the existing mkvdovi tests pass unchanged.

### E17: `--verify` checks for authored CM v4.0 blocks

- **Status:** Open. `--verify` checks that the generated levels exist (`assert_rpu_invariants`),
  not whether authored L8 and L10 blocks from Dolby Vision inputs are consistent; prior art in
  [the log](docs/ROADMAP_LOG.md) (2026-10-29). Open: E17.1.
- **Gate:** each check has a unit test where a crafted RPU fails and a well-formed authored-style
  RPU passes; the existing `--verify` tests and the RPU baselines are unchanged.
- [E17.1][corpus] Hard-fail `--verify` on duplicate L8 `target_display_index` values, duplicate L10
  target indices, an L8 target that resolves to neither a preset nor an L10 block of the same
  frame, and optional L8 fields whose presence disagrees with the block length. First check what
  the `dolby_vision` crate (3.4.0) already rejects at parse time, and drop the checks it covers.
  Gate: the item gate.

### E18: hot-file budgets

- **Status:** Core complete; no step open. The budgets are in `.claude/workflow.md` "## Size limits"
  (history: [the log](docs/ROADMAP_LOG.md), 2026-10-30). E18 stays as the `item` of those bullets,
  which collects Trim steps.
- **Gate:** the roadmap at or under its byte budget, with nothing lost (moved text in
  `docs/ROADMAP_LOG.md`) and the ready queue unchanged.

## Explicit non-goals

- No proprietary Dolby code, LUTs, tone curves, or binary blobs in this repository; see
  [`docs/PROVENANCE.md`](docs/PROVENANCE.md) for the provenance statement and its stated limits.
- No silent peak clamping; suspicious values produce advisory warnings.
- No `cm_analyze` parity claim without reproducible measurements.
- No video encoding in `mkvdovi`. Every path copies the video stream bit-exactly; inputs that would
  need a re-encode (Profile 7 FEL today) are refused.
- No Dolby Vision 2 or HDR10+ Advanced output. Neither has public technical content: the
  announcements name features, and no bitstream or metadata specification is published. Revisit
  when one is.

During the `0.x` series, minor releases may include breaking changes under the project's documented
[semantic-versioning](https://semver.org/) policy: the Versions table in
[`docs/RELEASING.md`](docs/RELEASING.md).

## Development principles

- **Three achievements, three kinds of evidence.** Measuring source luminance accurately is shown by
  synthetic ground truth and reference-analyzer scores. Generating valid metadata is shown by
  structural checks on the final muxed file. Better-looking playback is shown only by matched playback
  tests on real hardware. The project has evidence for the first two; claims about the third need WS6.
- **Parity is a benchmark, not proof of better playback.** Matching a reference analyzer's numbers
  does not by itself show that the resulting tone mapping looks better.
- **Correctness before features.** Conversion correctness and playback validation come before new
  capabilities.
- **Delivered output decides the order.** A defect that makes the delivered file wrong for every
  input of a type comes before any accuracy improvement. What counts is the value in the final RPU
  and the pixels a display reconstructs from it, not the value the analyzer measured.
- **The video stream is never re-encoded.** Every `mkvdovi` path copies the video bit-exactly; a
  feature that needs an encode is out of scope.
- **No silent degradation.** A path that produces weaker metadata must be explicit and visible.
- **Source-faithful defaults.** Measured L1 and neutral trims stay the default; display-targeted
  behavior stays opt-in.
- **Measured research claims only.** Unmeasured expectations, such as the retired "80-90% of FEL
  intent" estimate, are not targets.
