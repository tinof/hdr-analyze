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

**How to update.** Change the item's status and open steps in place. Add one row to the progress
log (one sentence plus the PR link) and the full text of the step to
[`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md). Measurements go to the log or to the doc the item
links, not into the item. `/roadmap-next` keeps **Next up** current and adds a **Checked** bullet
(date, commit, `file:line`) to each item it examined, so a later session re-checks only what
changed since that commit. Format: `~/.claude/skills/roadmap-next/reference/roadmap-format.md`;
project rules: [`.claude/roadmap-config.md`](.claude/roadmap-config.md).

### Next up (checked 2026-10-17 at main@941489d)
<!-- roadmap-next: rev=1 scope=all config=73415b8825d0 probes=gpu:ok,corpus:ok -->

1. **E13 step 5: analyzer split** (prerequisite of priorities 2 and 4: E8, E9, P9 step 2). Gate:
   per move fmt, clippy (also `--features cuda`), tests, the L1 gate without `--update`,
   `scripts/cuda-parity.sh`, and an optimizer-enabled default `.bin` captured on main@941489d
   before the first move and byte-identical after each. Branch `refactor/e13-analyzer-split`; no
   other branch open while a move is in flight (Sequencing). Release: none.
2. **P1 step 1: measure the CPU default** (priority 4). Gate: CPU cost of `accurate` and the
   `FAST_BILINEAR` effect on L1 max and MaxCLL measured (forced CPU, fresh analyses); then flip or
   keep (the note exists); a flip adds `cargo mutants --in-diff`. Between E13 moves. Release: minor
   if CPU `auto` flips, none if `balanced` stays.
3. **P12: `--mdfix` source L6** (priority 5). Gate: as in P12, "every other RPU baseline
   byte-identical" checked with `sha256sum` of the `RPU.bin` files (`rpu-baseline.sh compare`
   ignores CRC), plus `cargo mutants --in-diff`. Own branch `fix-p12-mdfix-source-l6`, between E13
   moves. Release: patch (rides v0.7.0 if merged before it).

### Waiting for the owner

Steps that need a decision, material or a test only the owner can provide. Other steps of the
same item stay startable. `/roadmap-next` reads this list; remove an entry when it is settled.

- **P9: default change.** Whether hybrid mode becomes the default, after holdout HDR10+ scoring
  (development tier scored 2026-10-14).
- **P10 step 3: write L1 in-process.** Needs WS7 or playback evidence first.
- **P13: L2 under `--cm-version v29`.** Should v29 RPUs carry neutral L2 trims, as v40 does? v29
  never wrote L2 and still does not. A question, not a decision.
- **E14 step 2: separate test author.** Should a separate session (Codex, without the diagnosis)
  write the acceptance tests for steps whose gate is a stated behaviour? The P13 pilot found
  nothing an in-session test missed and missed the pipeline wiring. Recommendation: not mandatory;
  try it once more on a new feature's gate. E14 step 6 does not wait for this.
- **P8 step 1: playback test on a second device.** Needs the owner's hardware and the WS6
  written procedure (WS6 open (2)).
- **E12: `v0.8.0-rc.1` on a Windows NVIDIA host.** The release-candidate test in E12's gate; the
  build, the `+cuda` smoke test and the CPU fallback stay startable.
- **Acceptance gate not stated** (`**Gate:** TODO(owner)`; `/roadmap-next` never recommends these
  steps): P3, P4, P5, P6, P9 step 2 (sidecar v6), P10 L2 `ms_weight`, P13, WS0, WS1, WS2 steps 2–4,
  WS5, WS6, WS8, FEL, E1, E2, E3, E5, E6, E7, E10, E11.

### Release plan (2026-10-14)

Which steps ship in which release. Rules for the bump and the timing: [`docs/RELEASING.md`](docs/RELEASING.md).
`/roadmap-next` keeps this table current. Latest release: v0.6.0 (2026-10-13; with the L6 fixes of #41 and the ffprobe HLG fix of #43, both merged after rc.1).

| Release | Content | Trigger / gate |
|---|---|---|
| none | E13 step 5 (refactor), P1 step 1 measurement, E14 remaining steps (tests and the pre-PR gate only; order in E14) | No user-visible change |
| **v0.7.0** | P7 full-range PQ refusal (#49); P9 `--hdr10plus-l1 hybrid` opt-in flag (#50); P11 `--mdfix` dropped-levels notice (#52); P13 `--cm-version v29` writes CM v2.9 (#53; owner decision 2026-10-15); P12 `--mdfix` source L6 (PATCH-class fix) only if merged before the release PR, else the next patch | MINOR (new flag). P7 was priority 1, so due about 2026-10-21 (`docs/RELEASING.md` rule 1). No rc: no test outside CI. Owner decision 2026-10-14: no separate 0.6.1 |
| v0.8.0 | E12 CUDA release binaries (Linux x64, Windows x64) | MINOR: delivered L1 changes for NVIDIA users. `v0.8.0-rc.1` tested on a Windows NVIDIA host first. Owner decision 2026-10-14: E12 moved out of v0.7.0 |
| a later minor | E8 and E9 (they move the default `.bin`); P9 sidecar v6 (cross-binary contract), both after E13 step 5 | MINOR |
| 1.0.0 | P8 playback verified (WS6; device application shown 2026-10-13 on an LG OLED C9 only, `code / 1023` question open), sidecar schema and CLI unchanged for one minor cycle, no open priority-1 defect | Owner decision |

## Progress log

Newest first. One line per step that changed the state of a roadmap item. The full text is in
[`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md); details are in the pull request and in
[`CHANGELOG.md`](CHANGELOG.md).

| Date | Step | Items | Where |
|------|------|-------|-------|
| 2026-10-15 | E14 step 5, first PR: tests for the 19 high-risk survivor groups (composer, `--verify`, sidecar/RPU/HLG probes, pipeline delivery steps); 0 of the targeted mutants missed. Steps 1, 2 and 3–4 (partial) recorded. | E14 | `test/e14-high-risk` |
| 2026-10-15 | P13: `--cm-version v29` writes a CM v2.9 RPU (L1, L5, L6) and `--verify` checks the requested CM version; CM v4.0 RPUs identical to 0.6.0. | P13 | `fix-p13-cm-v29` |
| 2026-10-15 | P11 notice: `--mdfix` names the authored levels it replaces or drops, from the sampled source RPU windows; r4/r5 `--mdfix` RPUs identical to 0.6.0; two defects found (P12, P13). | P11, P12, P13 | `feat/p11-mdfix-notice` |
| 2026-10-14 | P9 peak parity: under `histogram99`, `max-scl` and `max-scl-luminance` the hybrid max equals `dovi_tool`'s on every non-fallback frame of six cuts (0 mismatches), fallbacks carry the analyzer scene's max, `--boost` equals `histogram99`. | P9 | `docs/p9-peak-parity` |
| 2026-10-14 | P9 hybrid flag: opt-in `--hdr10plus-l1 hybrid` (union pieces, HDR10+ peak, measured average and minimum); default RPUs byte-identical to 0.6.0, Alita pooled clamped MAE 37.58 / 18.91 as pre-registered. | P9 | `feat/p9-hdr10plus-hybrid` |
| 2026-10-14 | P7: mkvdovi refuses PQ input tagged full range before any temp work (HDR10, HDR10+, `--mdfix`), including a full-range stream the container overrides as limited; 9/9 RPU baselines byte-identical to 0.6.0. | P7 | `fix-p7-full-range-pq` |
| 2026-10-13 | E13 step 4: `convert_file` split by phase into `pipeline/{temp_dir,measure,config,deliver}.rs`, `mod.rs` 1037 to 409 lines, 27 new unit tests; all 9 RPU baselines byte-identical to 0.6.0. | E13 | [log](docs/ROADMAP_LOG.md) |
| 2026-10-13 | P9 scene list for the pixel average: pixel average 19 codes from retail L1 (Alita, pooled, clamped) against 338 for today's HDR10+ average; HDR10+ scenes, analyzer cuts and their union within about 1.5 codes; the union carries it by the pre-registered rule. | P9 | [log](docs/ROADMAP_LOG.md) |
| 2026-10-13 | v0.6.0 released (#46): #41 and #43 on top of rc.1; CUDA parity and all ten RPU baselines identical before the tag. | P6, E7, P8 | [v0.6.0](https://github.com/tinof/hdr-analyze/releases/tag/v0.6.0) |
| 2026-10-13 | P8 playback test: on an LG OLED C9 (Plex on the TV and on a Shield) a green canary composer renders green and the preset's grey tint is visible, bt2100 neutral; bt2100 stays the default. | P8, WS6 | [log](docs/ROADMAP_LOG.md) |
| 2026-10-11 | L6 fixes: MaxCLL/MaxFALL read from the stream behind a container 0, `Details.txt` thousands separators, `--verify` warns on L6 0 in a passed-through MEL RPU; r4 MEL default baseline captured. | P6, E7, E13 | `fix-l6-zero-light-levels` |
| 2026-10-08 | P9 step 0: HDR10+ scene starts line up with the picture (offset 0 on six cuts, v5 runs, mkvdovi extraction path); 24% of frames in HDR10+ scenes holding several shots. | P9 | [log](docs/ROADMAP_LOG.md) |
| 2026-10-07 | E13 step 3: pure-move split of mkvdovi `metadata.rs` and `pipeline.rs` into submodules, tests in `tests.rs`; all 8 RPU baselines identical. | E13 | `feat/e13-mkvdovi-split` |
| 2026-10-07 | E13 safety net: 34 characterization tests for mkvdovi (metadata, pipeline, HLG source deletion), CUDA gate on module dirs, final-RPU baselines at main@e6d63ed; no production code changed. | E13 | this branch |
| 2026-10-06 | CUDA as main pipeline: warn on `balanced`/`fast` with GPU, re-analyze coarser sidecars under `accurate`, warn on CPU-analyzed CUDA run; CI lints `cuda`. | P1, E1 | [#31](https://github.com/tinof/hdr-analyze/pull/31) |
| 2026-10-06 | Review with CUDA as the main pipeline: `accurate` is the only parity-checked preset; release binaries and hosted CI never build the `cuda` feature; P1, E1, E2 amended, E12 opened. | P1, E1, E2, E12 | this file |
| 2026-10-06 | `l1_diff` lines references up with open-GOP cuts by `leading_skipped_frames`; exports are labelled in stream frames. | E11 | [#30](https://github.com/tinof/hdr-analyze/pull/30) |
| 2026-10-06 | Analyzer measures HLG max-RGB through the spec 4:2:0 decode (`spec-float`); gate passed at 0.000 codes, CPU = CUDA; new sidecar names, older sidecars re-analyzed. | P8 | [#29](https://github.com/tinof/hdr-analyze/pull/29) |
| 2026-10-06 | Spec fixed-point arithmetic measured: `bt2100` is not neutral at `code / 1024` (spread 5.9 codes); the decode change targets the spec structure in `code / 1023` (`spec-float`). | P8, WS6 | [log](docs/ROADMAP_LOG.md) |
| 2026-10-05 | 4:2:0 chroma of the HLG decode measured against the spec composer and libplacebo: the analyzer differs from the spec by up to 18 codes of L1 max per scene; spec decode step opened. | P8 | [log](docs/ROADMAP_LOG.md) |
| 2026-10-05 | Real material per format added: four HDR10+ titles (one same-master DV MEL + HDR10+ pair), four HLG cuts, six HDR10/MEL titles; 29 holdout cuts. | WS8, P9, P8, WS1 | [log](docs/ROADMAP_LOG.md) |
| 2026-10-05 | Coverage of real material per format reviewed; HDR10+ and HLG material comes before P9 and P8 work. | WS8, P9, P8 | this file |
| 2026-10-04 | L1 lines up with the picture in open-GOP cuts (RASL leading pictures); sidecar v5. | E7 | [#26](https://github.com/tinof/hdr-analyze/pull/26) |
| 2026-10-04 | 8.1 source range from the mastering display; `--verify` compares delivered with measured L1. | P10, E10 | [#25](https://github.com/tinof/hdr-analyze/pull/25) |
| 2026-10-03 | FEL compositor and re-encode removed; FEL inputs refused. | FEL | [FEL_PLAN](docs/FEL_PLAN.md) |
| 2026-10-03 | Review of all conversion paths; priorities reordered; new items. | P8–P11, E8–E10, WS7, WS8 | this file |
| 2026-10-02 | Robust peak estimator reads the shape of the histogram top. | WS1, WS2 | [#22](https://github.com/tinof/hdr-analyze/pull/22) |
| 2026-10-02 | `--peak-estimator robust` in the CUDA kernel, identical to CPU. | WS1 | [#21](https://github.com/tinof/hdr-analyze/pull/21) |
| 2026-10-02 | Scene cuts chosen after analysis; 142 of 168 authored cuts matched. | WS2, E1 | [#20](https://github.com/tinof/hdr-analyze/pull/20) |
| 2026-10-01 | Grain-robust peak measurement study started. | WS1, WS2 | in progress |
| 2026-10-01 | Unfiltered scene averages (sidecar v4); stale RPU regenerated on resume. | WS2 | [#18](https://github.com/tinof/hdr-analyze/pull/18) |
| 2026-10-01 | Final RPU checked against the sidecar; 0.5.1 RPU baselines captured. | P0, WS6 | [#17](https://github.com/tinof/hdr-analyze/pull/17) |
| 2026-10-01 | `l1_diff` limits, L1 gate in CI, CUDA parity check, RPU baseline script. | E1, WS0 | [#17](https://github.com/tinof/hdr-analyze/pull/17) |


## Priorities

Order (2026-10-03, confirmed by the owner 2026-10-16): defects that make delivered output wrong for
every file of an input type; then the evaluation infrastructure every later decision needs; then
improvements with measured evidence; then research. Items 1 to 5 are about what is delivered, 6
and 7 about how it is judged. The evidence for each priority is in its items and in
[`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md#priority-table).

1. **HLG → Profile 8.4 composer** (P8). `--hlg-composer bt2100` and the input refusal landed
   2026-10-03; bt2100 is the default by owner decision. Gate: the WS6 playback test shows that
   devices apply a mapping that is not the preset; `--hlg-composer preset` is the fallback. Met for
   one device, an LG OLED C9, on 2026-10-13 ([log](docs/ROADMAP_LOG.md)); other devices untested.
2. **HDR10+ → Profile 8.1 hybrid mode** (P9): scene list = union of HDR10+ starts and the analyzer's
   cuts (chosen 2026-10-13), peak per piece from HDR10+; average, minimum and crop from pixels; the
   opt-in flag landed 2026-10-14. Gate: opt-in, scored on the development tier and the holdout
   HDR10+ cuts before any default change. **Prerequisite:** E13 step 5 before the sidecar v6 step.
3. **Deliver what was measured** (P10): source range and the measured-against-delivered report
   landed 2026-10-04; open: decide whether to write L1 in-process. Gate: unclamped L1 needs WS7 or
   a playback test.
4. **Narrow code defects** (E8, E9, P1, E10, P6, E7): histogram percentile reader, `--pre-denoise`
   values, half-resolution CPU default, L1 scaling note (the three L6 defects found 2026-10-07 were
   fixed 2026-10-11). Gate: unit tests for the reader and the option; measure the CPU default
   first. **Prerequisite:** E13 step 5 (analyzer split) before E8 + E9, because E8 moves the
   default `.bin`.
5. **`--mdfix` fidelity** (P12, P13): full regeneration stays (owner decision 2026-10-13) and the
   notice that names the dropped levels shipped 2026-10-15 (`CHANGELOG.md` `[Unreleased]`). Open:
   `--mdfix` ignores the source RPU's L6 (P12), and whether v29 RPUs carry neutral L2 (P13, owner
   question). Gate: the P12 gate.
6. **Evaluation in displayed-picture units** (WS7): histogram-domain display-mapping simulator, then
   ColorVideoVDP on a few cuts. Gate: the simulator reproduces the recomputed numbers in WS7.
7. **Test material** (WS8): grain on synthetic highlights, unaligned positions, a same-master grainy
   twin, transitions. Needed before 9 can be scored. Real material per format (item 5) landed
   2026-10-05. Gate: TODO(owner).
8. **Scene detection** (WS2, E4): second signal for whole-picture motion; strong cuts closer than
   12 frames. Gate: precision and recall against authored shot lists, no loss on the other cuts.
9. **Research, default unchanged** (WS1, WS2, WS4): grain-robust peak; per-frame L1 inside detected
   transitions; L4; automatic trims. Each stays opt-in or unbuilt until its gate is met. Gate: per
   item (WS4: blinded A/B on a device; WS1, WS2 steps 2–4: TODO(owner)).
10. **Profile 7 FEL: placeholder** (FEL). See [`docs/FEL_PLAN.md`](docs/FEL_PLAN.md); on hold by the
    owner (2026-10-13). Gate: TODO(owner).

- **Sequencing (E13, 2026-10-07):** the module-size refactor is no priority of its own; its steps
  are prerequisites of priorities 2 and 4 (see there). Each is a pure-move PR with the acceptance
  gate in its item; no other branch should be open while a move is in flight.
- **Conditional** (R2, WS5): Profile 5 through an established encoder, only if matched playback
  tests justify it.
- **Landed before this order:** dependable HDR10/HDR10+ → Profile 8.1 with source-derived L1, L5,
  provenance, completeness checks and the input contract (P0, P1, P3, P7, E6, E7; code 2026-09-15;
  source retention reviewed and the default, delete a non-DV source after success, **kept by
  decision**); regression gates (E1, WS0; 2026-10-01); unbiased scene averages (WS2; 2026-10-01).
  The final-RPU regression corpus and the written Shield/TV playback procedure (WS6) grow alongside
  items 1 to 3, which each need a playback check.
- **Deferred:** display-target optimizers (P4), automatic creative trims (WS4, behind the WS7 gate),
  and broader hardware acceleration (E5). Neutral trims stay.

## Conversion quality

### P1: CPU analysis default

- **Status:** Partial. Source-honest generation is the default. CUDA is the main pipeline;
  `accurate` (downscale 1, sample-rate 1) is the only parity-checked measurement. On CUDA,
  `balanced`/`fast` sample with a stride and the CPU resizes, so those presets differ by backend and
  no gate covers them ([`docs/CUDA_PIPELINE.md`](docs/CUDA_PIPELINE.md)). They also save no time on
  CUDA, where the run is NVDEC-bound, and they lose the measured MaxCLL (`fast` also MaxFALL).
- **Gate:** the CPU cost of `accurate` and the effect of `FAST_BILINEAR` on L1 max and MaxCLL are
  measured (priority 4: measure the CPU default first); then CPU `auto` flips to `accurate` or
  `balanced` stays with a visible note.
- **Open:**
  1. Decide the CPU fallback default. `auto` resolves to `accurate` only with CUDA, otherwise
     `balanced`, which downsizes with `FAST_BILINEAR` at half resolution and averages small
     highlights. Measure the CPU cost of `accurate` and the effect of `FAST_BILINEAR` on L1 max, then
     flip CPU `auto` to `accurate` (the same file then gives the same L1 on any host) or keep
     `balanced` with a visible note. `fast` puts cuts on a 3-frame grid and misses peaks on skipped
     frames.
- **Checked 2026-10-17 @941489d:** the visible note for CPU `auto` already exists
  (`analysis_quality_notice`, `mkvdovi/src/pipeline/analyzer.rs:246`), so keeping `balanced` needs
  no code. Measure with CPU analysis forced and fresh analyses for both presets (`auto` picks
  `accurate` with GPU analysis, and an existing sidecar can be reused); compare measured MaxCLL
  from the sidecar, since a source-stated MaxCLL wins in L6.
- **Done 2026-10-06** (former open items 2–4; evidence: unit tests `coarser_sampling`,
  `rejects_coarser_sidecar`, `analysis_quality_notice`, `gpu_analysis_missing` in
  `mkvdovi/src/pipeline/tests.rs`, e2e test `accurate_reanalyzes_coarser_measurements` in
  `mkvdovi/tests/hlg_profile84.rs`): mkvdovi warns on an explicit `balanced`/`fast` with GPU
  analysis; under `accurate` it re-analyzes a coarser sidecar instead of reusing it; it warns when a
  run that expected GPU analysis comes back with `analysis.gpu: false`.

### P3: L5 from the crop

- **Status:** Core complete. L5 offsets come from the committed full-resolution crop (sidecar v2);
  sampled source L5 keeps precedence for Dolby Vision inputs.
- **Gate:** TODO(owner)
- **Open:** (1) changing aspect ratios need per-scene offsets, treated as a separate validation
  problem.

### P4: display-targeted workflow

- **Status:** Deferred. Opt-in `--target-nits` display-targeted workflow wired into optimizer
  behavior.
- **Gate:** TODO(owner)

### P5: L9 primaries

- **Status:** Partial. L9 detection prefers mastering-display primaries and has an override.
- **Gate:** TODO(owner)
- **Open:** (1) `hdr10plus_tool extract --skip-reorder` fallback. (The HLG→PQ mastering-primaries
  item is gone: HLG now converts to Profile 8.4 without re-encoding.)

### P6: source consistency warnings

- **Status:** Partial. Warnings exist for L6/L9 fallbacks and suspicious HDR10+ scene peaks.
- **Gate:** TODO(owner)
- **Open:** (1) broader missing/inconsistent-source detection.
- Fixed 2026-10-11: a container MaxCLL/MaxFALL of 0 no longer hides the stream's value
  (`track_light_level` reads `MaxCLL_Original`/`MaxFALL_Original`; a non-zero container value
  still wins), and `Details.txt` thousands separators are read (`parse_details_number`).
- **Checked 2026-10-11 (fix-l6-zero-light-levels):** none of the seven saved rpu-baseline inputs
  has a container 0, so the baselines do not cover the container-0 fix; unit and clip tests do,
  including the L6 of a generated `extra.json`.

### P8: HLG → Profile 8.4 composer

- **Status:** Core complete; default bt2100; playback confirmed on an LG OLED C9 (2026-10-13).
- `--hlg-composer bt2100` (2026-10-03) is fitted to the BT.2100 / BT.2408 1000-nit HLG-to-PQ
  conversion and keeps neutrals neutral; the HLG colour input contract is enforced. Design and
  measurements: [`docs/HLG_COMPOSER.md`](docs/HLG_COMPOSER.md). The preset it replaces tints
  neutrals (75% grey ΔE_ITP 6.8, up to 10.3 near black) and decodes nominal white to about
  1150 nits.
- **Gate:** the WS6 playback test shows that devices apply a mapping that is not the preset
  (priority 1); met for the LG OLED C9 (2026-10-13). Step 1: the same canary test on a second
  device shows that the display applies the RPU composer.
- **Open:** (1) the playback test on devices other than the C9 (procedure in WS6; the WS6
  `code / 1023` question is tracked there).
- **Done 2026-10-13: playback test.** An LG OLED C9 (Plex on the TV, and Plex on an Nvidia Shield TV Pro) shows a
  canary composer (bt2100 with both chroma MMR constants shifted, `fit_hlg_composer canary-rpu`)
  fully green, with the Dolby Vision badge, on a byte-identical HLG base layer: the display applies
  the composer in the RPU. The preset's tint is visible on greys (the owner saw yellow), bt2100 is
  neutral. bt2100 stays the default. Not covered: devices other than the C9, and the WS6
  `code / 1023` question. Evidence: [log](docs/ROADMAP_LOG.md) 2026-10-13.
- **Done 2026-10-06: spec 4:2:0 decode in the analyzer** (both composers;
  [#29](https://github.com/tinof/hdr-analyze/pull/29)). Max-RGB runs the chroma MMR at chroma resolution on down-sampled
  luma in `code / 1023` f32 (`spec-float`; `code / 1024` leaves `bt2100` 5.9 codes off neutral) and
  upsamples the composed chroma bilinearly at the stream's chroma location; CPU and CUDA identical.
  New sidecar names `dovi84-v3` / `dovi84-bt2100-v1-spec420`; older sidecars are re-analyzed, and
  mkvdovi refuses an analyzer whose `--help` does not name the mapping. Gate
  (`scripts/validate_hlg_chroma_siting.sh --gate`): 0.000 codes on the synthetic patterns and the
  five cuts. Scene L1 max moves by up to 19 codes on the two cuts compared. Evidence: [log](docs/ROADMAP_LOG.md) 2026-10-06.

### P9: HDR10+ → Profile 8.1 L1

- **Status:** Partial; step 0 (frame alignment) done 2026-10-08; scene list chosen 2026-10-13; the
  hybrid flag done 2026-10-14 (opt-in); peak parity done 2026-10-14.
- `dovi_tool generate --hdr10plus-json` takes L1 from the first frame of each HDR10+ scene
  (minimum 0, average rounded to whole nits, no measured crop). On the HDR10+ test cut the average
  reads 156 to 505 codes above the analyzer's mean; the cause is not separated.
- **Prerequisite met (2026-10-05):** four HDR10+ titles from two studios and one streamer
  (WS8 item 5). Alita: Battle Angel carries Dolby Vision MEL and HDR10+ on the same disc, so its
  retail L1 can score an HDR10+-derived L1 frame by frame.
- **Finding for the fallback:** both HDR10+ Profile A discs (Alien, Alita) carry MaxSCL 0 on every
  frame, but the percentile distribution (99th percentile 90 to 129 nits per scene on Alita) and
  the average are present. A peak taken from MaxSCL is unusable on such sources, so the pixel
  fallback is required, not optional; whether the percentiles can stand in is open.
- **Plan:** opt-in hybrid mode (scene list = union of HDR10+ starts and the analyzer's cuts, peak
  per piece from HDR10+; average, minimum and L5 from pixels),
  scored on the development tier before any default change, with a pixel fallback for missing or
  implausible HDR10+ statistics. The panel peak is still not passed as a trim target, and
  suspicious scene peaks still only warn.
- **Gate:** step 1: the holdout HDR10+ cuts are scored and the owner decides the default (priority
  2: opt-in until scored). Step 2: TODO(owner).
- **Open:** (1) score holdout HDR10+ cuts before any default change (owner list), where the union
  could show whether HDR10+ starts rescue cuts the analyzer misses; (2) sidecar v6 with a per-frame
  max for an exact fallback peak, also recording the analyzer's resolved `--peak-source` so
  `check_measurements` can refuse a percentile peak (after E13 step 5). Peak against retail is not
  scored.
- **Done 2026-10-08: step 0, HDR10+ scene boundaries against the picture.** HDR10+ frame n is
  picture frame n: offset 0 on all six multi-scene HDR10+ cuts (five by the scan, one by
  corroboration), no confirmed off-by-one (Alien 298 ambiguous), v5 runs and mkvdovi's extraction path
  ([log](docs/ROADMAP_LOG.md) 2026-10-08). The scene list is not a shot
  list: 24% of the frames sit in HDR10+ scenes holding more than one shot (Alita, The Shining), and
  some HDR10+ starts split a shot (Alien).
- **Done 2026-10-13: which scene list carries the pixel average.** The union of HDR10+ starts and
  the analyzer's cuts, by the pre-registered rule; the HDR10+ scenes, the analyzer's cuts and the
  union score within about 1.5 codes of each other (pooled clamped MAE above the floor 38.3 / 36.8 /
  37.6), and the analyzer's cuts are the numerical best. The gain is the measured average itself:
  pooled clamped MAE over all frames 18.9 codes against 338.4 for today's HDR10+ path, which writes
  1229 where retail sits at the 819 floor. The 18.9 holds after the generator's 819 floor; raw, the
  measured average is 147.6 codes off over all frames (about 259 below on floor frames). Two cuts from one title; peak and minimum not
  evaluated ([log](docs/ROADMAP_LOG.md) 2026-10-13).
- **Done 2026-10-14: the hybrid flag.** `--hdr10plus-l1 hybrid` (opt-in, default unchanged):
  union pieces, HDR10+ peak (`--peak-source`, whole nits like `dovi_tool`) with a reported fallback
  to the analyzer scene peak, measured average and minimum. Default path 9/9 RPUs byte-identical to
  0.6.0; the four HDR10+-only cuts (h1, h5) convert with `--verify` passing and 0 fallbacks. On
  DV-free remuxes of the Alita and The Shining cuts (`accurate`): 0 L1 mismatches against the
  pre-registered rule, max equal to `dovi_tool`'s HDR10+ path on every frame, Alita pooled clamped
  MAE of the average 37.58 above floor / 18.91 all frames (pre-registered 37.6 / 18.9). Evidence:
  `~/mkvdovi-work/p9-hybrid-2026-10-14/` ([log](docs/ROADMAP_LOG.md) 2026-10-14).
- **Done 2026-10-14: peak parity for the other `--peak-source` values.** `histogram99`, `max-scl`
  and `max-scl-luminance`: the hybrid max equals `dovi_tool`'s on every non-fallback frame of six
  cuts; fallbacks (all Profile A pieces under `max-scl*`) carry the analyzer scene's max; `--boost`
  equals `histogram99`. Evidence: `~/mkvdovi-work/p9-peak-parity-2026-10-14/`
  ([log](docs/ROADMAP_LOG.md) 2026-10-14). The unit test `hdr10plus_peak_sources_follow_dovi_tool`
  pins the mirror, not parity: re-run that evidence's `run.sh` after a `dovi_tool` bump.
- **Checked 2026-10-14 @f5d4ccc:** mirror `hdr10plus_peak_nits` (`pipeline/hdr10plus.rs:81`);
  `--boost` remaps to `histogram99` (`main.rs:167`); the report prints only `MAX_FALLBACK_LINES`
  ranges (`pipeline/hybrid.rs:93`); `check_measurements` admits a hand-made percentile sidecar
  (`hybrid.rs:115`), so parity runs use fresh analyses. Of the HDR10+ development cuts, MaxSCL is
  nonzero on h1 Deadloch, h4 The Shining and d2 Patriot (not run), 0 on every frame of the Alita
  and Alien cuts (every `max-scl*` piece falls back there).

### P10: measured against delivered

- **Status:** Partial. Steps 1 and 2 landed 2026-10-04.
- `dovi_tool generate` clamps L1 (minimum ≤ 12, maximum ≥ 2081, average ≥ 819). Numbers:
  [`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md).
- **Done:**
  - (1) Profile 8.1 RPUs carry `source_min_pq` / `source_max_pq` converted from the mastering
    display, instead of the generator's L6 lookup. Implausible mastering values (peak outside
    100–10000 nits, minimum above 1 nit) keep the lookup, with a warning.
  - (2) `--verify` checks the source range and the clamped measured L1 on every frame of a
    generated RPU, and reports per field how many scenes the generator's limits changed.
- **Gate:** step 3: evidence from WS7 or a playback test that unclamped L1 is better (priority 3).
  The L2 `ms_weight` step: TODO(owner).
- **Open:** (3) decide whether to write L1 in-process so the measured minimum and sub-100-nit
  maxima reach the RPU; raising the average floor to 1229 is not planned. (4) Generated L2 carries
  `ms_weight` 2048 where retail RPUs carry 512 or 0; the effect is unverified.

### P12: `--mdfix` ignores the source RPU's L6

- **Status:** Open; defect found 2026-10-15, not fixed.
- Under `--mdfix` the mastering min/max come from the container or the defaults
  (`metadata::read_static_metadata` + `apply_static_defaults`, `mkvdovi/src/pipeline/config.rs:42-43`),
  never from the source RPU's L6. On r5 Inside Out 2 (P8.1, no ST 2086 in the container) the RPU's
  0.0001/1000 nits becomes 0.005/1000 nits and `source_min_pq` 7 becomes 62 on every frame
  (baseline `~/mkvdovi-work/rpu-baseline/0.6.0/mdfix/r5-inside-out-2-mind-console`). The only
  message is a generic "not found" warning.
- **Gate:** (owner, 2026-10-16) a unit test that is red on the parent; the r5 Inside Out 2
  `--mdfix` RPU carries the source L6 (0.0001/1000 nits, `source_min_pq` 7); every other RPU
  baseline byte-identical.
- **Fix:** under `--mdfix`, fall back to the sampled source RPU's L6 before the defaults. Changes
  delivered RPUs: own branch, RPU baselines updated.
- **Checked 2026-10-17 @941489d:** the sampled L6 values are not kept: the authored-levels scan
  records only `acc.l6 = true` (`mkvdovi/src/rpu_check.rs:286`), `SampledRpu`
  (`rpu_check.rs:177`) has no L6 field, and `prepare_configuration` (`pipeline/config.rs:42`) gets
  no sampled metadata, so the fix threads the values through and tests the pipeline wiring.
  `scripts/rpu-baseline.sh compare` strips the CRC (`:245`), so "byte-identical" needs `sha256sum`.

### P13: `--cm-version v29` writes a CM v4.0 RPU

- **Status:** Core complete 2026-10-15 (#53; `CHANGELOG.md` `[Unreleased]` / Fixed, ships in
  v0.7.0); the owner question on L2 remains.
- Fixed: `--cm-version v29` writes a CM v2.9 RPU (L1, L5, L6) and `--verify` checks the requested
  CM version. v40 RPUs carry neutral L2 trims; v29 never wrote L2.
- **Gate:** TODO(owner)
- **Open:** (1) the owner decides whether v29 RPUs carry neutral L2 trims (see "Waiting for the
  owner").

## `cm_analyze` parity

The detailed gap table and validation method live in
[`docs/CM_ANALYZE_PARITY.md`](docs/CM_ANALYZE_PARITY.md). These workstreams are dependency ordered.

### WS0: L1 scoring tools

- **Status:** Core complete. `tools/l1_diff`, synthetic ground-truth tests, embedded-L1
  comparison and licensed `cm_analyze` scoring have shipped. `l1_diff` takes limits
  (`--max-peak-bias`, `--max-peak-error` and the same for minimum and max-RGB average) and exits
  nonzero on a breach; CI runs it on a generated six-shot PQ and HLG clip against committed
  references.
- **Gate:** TODO(owner)
- **Open:** (1) grow the corpus.

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
- **Gate:** TODO(owner). Do not enable the robust estimator by default.
- **Open:** (1) explain the clean-source over-read; (2) the display-unit scale (WS7); (3) a
  grain-robust peak; (4) true target-gamut peaks.

### WS2: shots and scene averages

- **Status:** Partial. The shot maximum is the maximum of its frame peaks. Scene averages are
  unbiased since sidecar v4 ([VALIDATION.md §9](docs/VALIDATION.md)); the `cm_analyze` v2 average
  comparison has not been repeated for v4. Cuts are chosen after analysis (2026-10-02). Retail RPUs
  change L1 only at shot starts, so shot-constant L1 is kept.
- **Gate:** step 1: precision and recall against authored shot lists, no loss on the other cuts
  (priority 8). Steps 2–4: TODO(owner).
- **Open, in this order:**
  1. A second scene signal for whole-picture motion (E4), and strong cuts closer than
     `--min-scene-length` 12, by a confidence exception, not a lower global minimum.
  2. Research, opt-in: per-frame L1 inside detected dissolves and fades; needs a per-frame peak in
     the sidecar (a version bump) and transition test material (WS8), and can cause pumping.
  3. L4 emission, deferred until something can validate it (no open renderer reads L4).
  4. Robust aggregation (investigated with WS1).

### WS4: automatic trims

- **Status:** Deferred; experimental. Optional L2/L8 trims from an open tone-mapping baseline such
  as ITU-R BT.2390. Not started before WS7 exists, and WS7 alone cannot pass it: no open renderer
  applies trims, so a trim can only be validated on a device.
- **Gate:** blinded A/B testing on a device demonstrates an improvement; until then neutral trims
  remain the default.

### WS5: CM metadata XML export

- **Status:** Deferred; conditional. Worthwhile only if the external Profile 5 authoring route (R2)
  is chosen.
- **Gate:** TODO(owner)

### WS6: final-RPU corpus and playback procedure

- **Status:** Open.
- Final-RPU regression corpus covering grain, saturated highlights, raised blacks, fades, flashes,
  rapid cuts and changing aspect ratios; checks run on the RPU extracted from the muxed file.
- A Shield/TV playback procedure comparing matched material from the same master, recording player,
  firmware, TV picture mode and HDMI path. It is the gate for P8 and for the default changes of P9
  and P10.
- **Gate:** TODO(owner)
- **Open:** (1) the final-RPU regression corpus; (2) the written playback procedure; (3) the
  `code / 1023` question below.
- **Open question (2026-10-06):** do devices evaluate the 8.4 composer at `code / 1023` (as
  libplacebo and the `bt2100` fit) or at the spec's `code / 1024`? Under the latter `bt2100` is
  5.9 codes off neutral, probably too little to see on a neutral ramp, so the test needs a pattern
  that separates the two. A `code / 1024` answer means a refit (new composer and RPU). The P8
  canary test (2026-10-13) does not answer it.
- **P8 result (2026-10-13):** an LG OLED C9 applies the 8.4 composer in the RPU (canary method in
  [log](docs/ROADMAP_LOG.md)). Not recorded for that test: firmware, picture mode, HDMI path. The
  preset's grey tint looked yellow on the C9, where the libplacebo SDR render predicted lavender
  (mid greys) and olive (dark greys).

### WS7: evaluation in displayed-picture units

- **Status:** Open.
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
- **Open:** (1) the simulator; (2) ColorVideoVDP on a few cuts.
- **Limits to state with every result:** open curves are not a Dolby display; libplacebo reads only
  L1 max/avg and the source range, so trims, L1 min, L4 and L11 cannot be evaluated in the open;
  pin the libplacebo version; host limits in [`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md#ws7-open).

### WS8: test material

- **Status:** Partial. Real material per format landed 2026-10-05; the other fixes the 2026-10-03
  review asked for are open.
- **Gate:** TODO(owner)
- **Open:**
  1. Grain on the synthetic highlights and odd offsets (today they carry no grain and fill exactly
     one chroma quad).
  2. A grainy twin from the same master as the clean one (today "clean is truth" holds only to
     about ±50 codes).
  3. Synthetic cross-dissolves and fades built from real shots (blended in PQ code and in linear
     light) with their intervals as truth, and retail cuts with authored transitions.
  4. A held-out split: synthetic references and open implementations for development, licensed
     reference output for scoring only ([`docs/PROVENANCE.md`](docs/PROVENANCE.md)).
- **Done 2026-10-05 (former item 5):** real material per format. The development tier holds 45
  entries (it lives outside the repository, see CLAUDE.md "Testing quirks"); 29 further cuts are
  held out, unanalyzed, for a `cm_analyze` qualification run. Added on 2026-10-04 and 2026-10-05,
  no Profile 7 FEL:
  - **HDR10+:** Deadloch (Amazon WEB-DL, Profile B), The Shining (disc, DV MEL + HDR10+),
    Alien (disc, Profile A) and Alita: Battle Angel (disc, DV MEL + HDR10+ Profile A on the same
    master, so its retail L1 is an authored reference for exactly the frames the HDR10+
    metadata describes).
  - **HLG:** Wimbledon 2024 and Glastonbury 2025 (iPlayer), The Green Planet (iPlayer, nature),
    a Champions League broadcast capture (50p, starts at an open-GOP CRA).
  - **HDR10 and Profile 7 MEL / 8.1:** Joker: Folie à Deux (MEL, 1000 nits, starts at a CRA),
    Skyfall (MEL, per-shot L1), Exodus: Gods and Kings (1100-nit master), Star Trek (2009, no
    MaxCLL), Inside Out 2 (Disney+ 8.1, animation; its L1 is static and is not used as a
    reference) and The Revenant (bright snow; an injected hybrid, its metadata is not used).
  Measurements and what was rejected as a reference: [`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md).

## Dolby Vision profile work

### FEL: Profile 7 FEL

- **Status:** Deferred; placeholder, on hold by the owner (2026-10-13). The BL+EL compositor and
  its re-encode were removed on 2026-10-03 because the composed picture did not match ETSI GS CCM
  001. `mkvdovi` refuses Profile 7 FEL inputs, keeps the source and continues with the next file.
- The only FEL work of interest keeps the base layer bit-exact and carries FEL information, as far
  as possible, in the Profile 8.1 RPU: [`docs/FEL_PLAN.md`](docs/FEL_PLAN.md). This item replaces
  F1, F2 and R1. Profile 7 MEL conversion is unchanged.
- **Gate:** TODO(owner)

### R2: Profile 5

- **Status:** Deferred; conditional. Genuine Profile 5 uses IPT, so HDR10 BL → Profile 5 needs
  decoding, colour transformation and re-encoding: feasible through an established encoder
  (Resolve Studio, Dolby Encoding Engine), not in this project. Changing profile flags on untouched
  HEVC is not a target.
- **Gate:** a matched HDR10 / P8.1 / correctly encoded P5 playback comparison from the same master
  with the playback chain recorded (the precondition for any work).

## Engineering backlog

### E1: regression gates

- **Status:** Core complete. `tools/l1_diff` runs in CI (`scripts/ci/l1-regression-gate.sh`,
  references in `tools/l1_diff/corpus`); `scripts/cuda-parity.sh` checks CPU against CUDA on a GPU
  host; `scripts/rpu-baseline.sh` captures and compares final RPUs. Synthetic accuracy runs in
  workspace CI. Since 2026-10-06 the CI lint job runs clippy on the analyzer with `--features cuda`
  (no GPU or toolkit; first hosted run passed on #31).
- **Gate:** TODO(owner)
- **Open:** (1) expand the corpus (feeds WS6); (2) a self-hosted GPU runner for
  `scripts/cuda-parity.sh`, now a prerequisite because the main analysis path is untested in hosted
  CI; (3) spike: an NVRTC compile-only check of `kernels.cu` without a GPU.

### E2: crop detection

- **Status:** Partial. Seven-position crop probing, low-signal rejection, modal voting and
  variable-AR union shipped in PR [#4](https://github.com/tinof/hdr-analyze/pull/4), closing issue
  [#3](https://github.com/tinof/hdr-analyze/issues/3).
- **Gate:** TODO(owner)
- **Open:** (1) per-scene crop application, a continuity-sensitive follow-up; (2) the crop probe
  decodes on the CPU in CUDA runs: measure its cost on a full episode, then decide an NVDEC probe.
  A kernel crop reduction must reproduce `detect_crop` exactly (every 10th sample, at least 10%
  non-black per row and column, `crop.rs`); per-scene crop application needs a second pass, because the crop is fixed before the histograms
  are built (affordable on NVDEC, not on the CPU).

### E3: `mkvdovi` debug options

- **Status:** Open.
- **Gate:** TODO(owner)
- **Open:** (1) add `mkvdovi --dry-run`, `--keep-temp` and `--keep-logs`.

### E4: second scene signal

- **Status:** Open. `--scene-metric hybrid` is a histogram-only placeholder. Missed cuts on
  whole-picture motion (WS2) need a signal that explains motion; both candidates are sensitive to
  flashes, exposure changes and low texture. Full optical-flow fusion stays deferred.
- **Gate:** precision and recall on held-out authored shot lists, no loss on the other cuts
  (priority 8).
- **Open:** (1) compare a spatial-thumbnail residual; (2) then a motion-compensated residual.

### E5: hardware decode contexts

- **Status:** Deferred. Add proper VAAPI and VideoToolbox decode device contexts and hardware-frame
  transfer.
- **Gate:** TODO(owner)

### E6: provenance

- **Status:** Core complete. Sidecar v2 records analyzer version, input name and size, dimensions,
  transfer, downscale, sample rate, GPU use and crop. Resume temp directories carry a fingerprint
  of input name, size and mtime, mkvdovi version and artifact-affecting settings.
- **Gate:** TODO(owner)
- **Open:** (1) record external tool versions (`dovi_tool`, `mkvmerge`, `ffmpeg`) for each output.

### E7: completeness

- **Status:** Partial. `--verify` fails when the RPU frame count differs from the muxed video track
  or the L1 sidecar, and warns when output and input frame counts differ.
- Since 2026-10-04 the analyzer accounts for every picture of the stream, so measured L1 lines up
  with the picture. It counts the undecodable RASL pictures at the start of an open-GOP cut, and
  any other loss is an error. A measured RPU whose length differs from the video is refused at
  inject.
- Fixed 2026-10-11: `--verify` warns, not fails, on L6 MaxCLL/MaxFALL 0 in a passed-through
  Profile 7 MEL RPU (`delivery` is `None`); generated RPUs, missing or malformed fields and MaxDML 0
  still fail.
- **Gate:** TODO(owner)
- **Open:** (1) direct-MKV `dovi_tool` steps still accept exit status plus non-empty output; (2)
  verification is opt-in.

### E8: histogram percentile reader

- **Status:** Open. The 256-bin luma histogram is split into 64 bins below PQ(100 nits) and 192
  above, but `compute_histogram_percentile_pq` and `find_highlight_knee_nits`
  (`hdr_analyzer_mvp/src/analysis/histogram.rs`) convert a bin index back with bin/255, so a
  1000-nit value reads as about 305 nits. It reaches the output only with
  `--peak-source histogram99/histogram999` or `--peak-domain luma` with a non-conservative
  optimizer profile.
- **Gate:** a bin-edge round-trip unit test and an optimizer unit test (CUDA parity and the L1 gate
  run with `--disable-optimizer`, so they do not cover it). **Prerequisite:** E13 step 5.
- **Checked 2026-10-06 @df54ad4:** `find_highlight_knee_nits` feeds the optimizer's `target_nits`
  on every default run (`optimizer.rs:152`), so the fix changes the default `.bin`. CUDA parity
  (`cuda_parity.rs:94`) and the L1 gate (`scripts/ci/l1-regression-gate.sh:76`) both pass
  `--disable-optimizer`, so a round-trip and optimizer unit test is the gate. The bin edges are
  written in `frame.rs:667` (`analyze_native_frame_cropped`) and `gpu.rs`
  `build_luminance_bin_lut`; `frame.rs:970` (`analyze_native_frame`) bins uniformly. Line numbers
  are from before the E13 step 5 split; the function names stay valid.
- **Fix:** one shared bin-edge function for writer and readers, with a round-trip test.

### E9: `--pre-denoise` values

- **Status:** Open. `--pre-denoise` accepts any string and its help text names `nlmeans`, but only
  `median3` is implemented; every other value silently does nothing and is still written into the
  sidecar provenance.
- **Gate:** a unit test for the option (priority 4). **Prerequisite:** E13 step 5.
- **Fix:** restrict the values to `off` and `median3`.

### E10: L1 code scaling

- **Status:** Open; unverified. The analyzer converts normalized PQ to a 12-bit code with ×4095;
  ETSI GS CCM 001 §6.2.2 encodes L1 as `clip(round(x × 4096))`. The difference is at most one
  code. The generator's own conversion (`dolby_vision::utils::nits_to_pq_12_bit`, also used for
  the P10 source range) is ×4095 too, so analyzer and generator agree (checked 2026-10-04).
- **Gate:** TODO(owner)
- **Open:** (1) the check against retail RPUs.

### E11: `l1_diff` and open-GOP cuts

- **Status:** Core complete 2026-10-06 (`CHANGELOG.md` `[0.6.0]`). `tools/l1_diff` shifts our
  frames by `source.leading_skipped_frames` (sidecar v5) when the reference covers the stream
  frames, re-bases `--per-shot` and `--scenes`, and labels `--export-reference` rows with stream
  frames. Any other count difference is still refused; no measurements are invented for the skipped
  pictures. Evidence: [`docs/ROADMAP_LOG.md`](docs/ROADMAP_LOG.md) (2026-10-06).
- **Gate:** TODO(owner)
- **Open (supporting, not the original gate):** (1) Joker and the Champions League cut (G4) have
  only v4 runs; scoring Joker's retail L1 per frame needs a re-analysis with a v5 analyzer.

### E12: CUDA release binaries

- **Status:** Open. `release.yml` builds without `--features cuda`, so release users never get GPU
  analysis, and `auto` resolves to CPU `balanced` even on an NVIDIA host.
- **Plan:** build Linux x64 and Windows x64 with the feature (no toolkit needed at build time;
  macOS already returns an error from `load_cuda_libraries`), smoke-test `--version` for `+cuda`
  and a clean CPU fallback without libcuda. Windows `--features cuda` is untested.
- **Effect:** changes delivered L1 and MaxCLL for NVIDIA release users (`balanced` → `accurate`);
  needs a CHANGELOG entry.
- **Gate:** the release archives report `+cuda` in `--version` and fall back cleanly to the CPU
  without libcuda; `v0.8.0-rc.1` tested on a Windows NVIDIA host (Release plan).

### E13: module size refactor

- **Status:** Partial; step 2 (safety net) done 2026-10-07; step 3 (mkvdovi split) done 2026-10-07,
  evidence in the PR body and `~/mkvdovi-work/rpu-baseline/e13-step3-281b927/proof/`; step 4
  (`convert_file` by phase) done 2026-10-13, evidence in
  `~/mkvdovi-work/rpu-baseline/e13-step4-d7c080d/`; step 5 open. Still over the 800-line limit in CLAUDE.md ("Module size"), test modules not counted:
  `hdr_analyzer_mvp/src/pipeline.rs` (1348), `analysis/frame.rs` (1026) and `analysis/gpu.rs` (951).
  The mkvdovi `metadata/` submodules are all under it.
- **Steps, each its own PR:** (1) this item; (2) safety net: CUDA gate paths accept module
  directories, the module-size rule and Stop-hook note, final-RPU baselines, characterization tests
  for the untested high-risk mkvdovi paths (source deletion, format detection, static L6, the whole
  `extra.json`); (3) mkvdovi: inline tests to `tests.rs`, then pure-move splits of `metadata.rs`
  and `pipeline.rs`, before P9 step 0, which works in both files; (4) split `convert_file` by
  phase, before the P9 hybrid flag; (5) analyzer: inline tests to `tests.rs`, pure-move splits of
  `pipeline.rs`, `analysis/gpu.rs` and `analysis/frame.rs` (the peak estimators and grain
  statistics are the candidate submodule; `pixel_pq_bin` and the max-RGB mix mirror `kernels.cu`
  operation for operation and move unchanged), before E8 + E9, because E8 moves the default `.bin`,
  edits `frame.rs` itself, and a pure move can only be proved byte-identical before it. `tools/l1_diff` stays as it is while the
  local `feat/dev-corpus` branch depends on it.
- **Gate:** per move (acceptance gate for steps 3-5): fmt, clippy (also `--features cuda`), tests;
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
- **Open:** (5) the analyzer split (see Steps).
- **Simplification candidates for later PRs (not in a move):** `read_static_metadata` stores
  `md_*` mastering primaries that nothing reads, and its parser does not match MediaInfo 24.01's
  spellings anyway; two `impl L1Sidecar` blocks.
- **Found by the step 2 tests:** two L6 defects under P6 (priority 4), fixed 2026-10-11.
- **Checked 2026-10-17 @941489d:** step 5 files unchanged since 2026-10-06. Non-blank, non-comment
  lines outside test modules: `pipeline.rs` 1171 (test mods at :649, :680, :1410), `gpu.rs` 811
  (:952; `include_str!` of `kernels.cu`), `frame.rs` 782 (:1026, :1066), so `frame.rs` is at the
  limit only by this count. Both gates pass `--disable-optimizer`: diff an optimizer-enabled
  default `.bin` per move. Baselines from 2026-10-11 in `~/mkvdovi-work/rpu-baseline/l6-fix-e052c3d/`.

### E14: test-suite audit

- **Status:** Partial. Rules: [`docs/TESTING.md`](docs/TESTING.md). Step 1 done (#54, CI run
  38753171952, only allowed skips). Step 2 done: 3 of the pilot's tests red on main, nothing new
  found, pipeline wiring missed; whether a separate test author becomes a roadmap step is under
  "Waiting for the owner". Steps 3 and 4 done on main@c74effb for `verify.rs` and the riskiest
  modules only (owner choice): 336 survivors, 0 actual bugs; missing tests in 19 high-risk groups
  (57 mutants), 17 medium (115) and 14 low (43); 121 print-only or equivalent. Step 5: high-risk
  groups done (#55, 0 missed after); medium and low groups open. Step 3 not yet run on 15 mkvdovi
  files (878 mutants) and the analyzer contract files (273; counts from `cargo mutants --list` at
  main@6f92306).
- **Gate:** a clean `check-test-skips` log on the audit host; per-module mutant counts (caught /
  missed / unviable / timeout) before and after; every survivor in a bucket.
- **Order (2026-10-16):** none of it gates a release.
  1. After the v0.7.0 release PR (due about 2026-10-21). Steps 3 and 4 (mutation runs, triage)
     are local, in a detached worktree, and need no branch, so they can run at any time. Steps 5
     and 6 are PRs: they open only when no E13 move PR is in flight (the E13 sequencing rule).
     E13 step 5 keeps the first branch-free slot, so E14 PRs go between its move PRs or after it.
  2. Step 3 on the mkvdovi contract modules: `resume.rs`, `external.rs`,
     `metadata/{mod,probe}.rs` and `pipeline/{mod,config,analyzer,hybrid,hdr10plus,temp_dir,mdfix_notice}.rs`
     (413 mutants, about 70 min at `-j 3`), then `metadata/static_metadata.rs` (323, about
     55 min). Triage, then step 5 for the 17 open medium-risk groups (115 mutants) of the c74effb
     run and the medium-risk groups of both new runs.
  3. Step 3 on the analyzer contract files `l1_sidecar.rs`, `leading_pictures.rs`, `ffmpeg_io.rs`
     and `writer.rs` (273; `cargo mutants --list` gives no mutants in `cli.rs`). E13 step 5 does
     not move them.
  4. Step 6, the `--in-diff` gate. It does not depend on the test-author decision. Run it by hand
     on two PRs first to measure its cost per PR.
  5. Low-risk groups get no sweep of their own: they are fixed when the step 6 gate lists them in
     code a PR changes. `main.rs`, `progress.rs` and `cli.rs` (142 mutants, mostly output text)
     run last, if at all.
  6. Analyzer numerics after E13 step 5, with E8 as the validation case.
- **Why:** a test that has never been red has not been shown to test anything. Until 2026-10-15
  the PR CI test job installed none of the integration-test tools, so the HLG 8.4, open-GOP, P7
  full-range refusal (`pq_input_contract`), HDR10+ hybrid, synthetic-accuracy and clip tests
  passed as skips (0.00 s on main run 38715765973); only `release.yml` ran them. Mutation testing ran once (E13 step 2: 69 of 75
  `metadata.rs` mutants caught) and was never repeated. A read-only sweep on 2026-10-15
  (`~/mkvdovi-work/test-audit-2026-10-15/smells-*.md`, unverified) lists one-sided decisions
  (`classify_rpu_format`, the `inject_rpu` length refusal, `hlg_bitstream_lacks_transfer`, the
  verifier's accept path), assertions that only check `is_ok()`, and tests that restate the
  production formula.
- **Steps:**
  1. PR CI installs the tools (`.github/actions/setup-test-tools`, shared with `release.yml`) and
     fails on unexpected skips.
  2. P13 pilot: Codex writes the P13 acceptance tests from a behaviour-only spec in a worktree
     without the diagnosis; record whether they assert something an in-session test would not,
     and the cost. The owner decides whether a separate test author becomes a roadmap step.
  3. Mutation baseline (`cargo-mutants`) on `mkvdovi/src`, `verifier/src`,
     `dovi84_composer/src` and the analyzer contract files (`l1_sidecar.rs`,
     `leading_pictures.rs`, `ffmpeg_io.rs`, `writer.rs`, `cli.rs`), after a clean
     `check-test-skips` baseline on the host. Analyzer numerics wait for E13 step 5; E8 is then the
     validation case.
  4. Triage every survivor (missing test / equivalent / actual bug / dead code /
     characterization); actual-bug survivors go to the owner before any test.
  5. Test-only PRs per crate area; each new test names the mutant it kills or the decision side it
     adds.
  6. A `cargo mutants --in-diff` gate in the pre-PR panel for mkvdovi, verifier
     and dovi84_composer logic (fails only on infrastructure errors; survivors listed with their
     bucket), and red-on-parent evidence for fix PRs.

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

## Checked and kept (2026-10-03)

The review re-examined these decisions and found no evidence to change them:

- **Max-RGB as the L1 domain** and **the L1 average as the mean of PQ max-RGB**, unfiltered.
- **The 819 average floor** (CM v2.9): retail averages sit at exactly 819 on 24 of 32 and 25 of 35
  shots of two retail cuts.
- **Shot-constant L1**: all nine retail RPUs change L1 only at shot starts.
- **Neutral L2 for analysis output.**
- **Nearest-neighbour chroma** for max-RGB: within about 10 codes of native 4:2:0 ingest
  ([VALIDATION.md §7](docs/VALIDATION.md)). A correctly sited reconstruction has not been compared
  on saturated edges; reopen only with such a measurement.
- **Stream copy for MEL and HLG.** With the FEL compositor gone, no path encodes video.
