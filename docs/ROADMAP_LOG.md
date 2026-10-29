# Roadmap log

History for [`ROADMAP.md`](../ROADMAP.md): the full text of each closed step and the measurement
detail that used to sit inside the roadmap items. `ROADMAP.md` keeps the open work (since
2026-10-26 without a one-line log); the detail is here, newest first.

## Progress log

### 2026-10-29: E18.1 Trim the roadmap to open work (PR #81)

Step: - [E18.1][desk] Trim `ROADMAP.md` to open work: Status lines to the current state, finished-step
  notes, dated measurements and closed orders moved verbatim to the log. Gate: every removed line
  is in the log (links rewritten for `docs/`), the `roadmap-ready.sh` rows and `ready` hash
  unchanged, the `###` headings unchanged, and the file at or under 64,000 bytes.

Resolution: closes E18.1. `ROADMAP.md` went from 74,462 bytes on main to under 64,000; each
Status line gives the current state, and the old Status text, finished-step notes, dated
measurements and closed orders are under "Item history moved from ROADMAP.md on 2026-10-29"
below. The branch was rebased across PR #80; the WS6 and P8 Status lines take its answer.

Evidence: checked in the PR #81 session against main `6182649`. Every line removed from
`ROADMAP.md` is in this log, links rewritten for `docs/` (0 missing). The `roadmap-ready.sh` rows
and `ready` hash equal those of the trim's parent, the commit that adds E18; against main they
differ only by E18's own rows and the `### E18` heading (review finding F1, left to the owner).
The `###` headings are otherwise unchanged, and the file was 63,827 bytes before this close.

E18 Status before this close:
- **Status:** Open. Nothing landed; history in [the log](docs/ROADMAP_LOG.md) (2026-10-29).
  `ROADMAP.md` is 74 KB, read on every `/ship:roadmap-next` and `/ship` run. Open: E18.1, then
  E18.2.

### 2026-10-29: WS6.6 View the redesigned canary on the LG OLED C9 (PR #80)

Step: [WS6.6][owner] View the redesigned canary on the LG OLED C9. Gate: the item's gate (3): viewed
  on the LG OLED C9, answer recorded. Needs: WS6.5.

Resolution: closes WS6.6 and meets WS6 gate (3) for the C9: the LG OLED C9 evaluates the Profile
8.4 chroma composer at `code / 1023`, so the bt2100 fit stays and no refit follows. Viewed by the
owner on 2026-10-29 through Plex on the Nvidia Shield TV Pro, Dolby Vision Cinema, Dolby Vision
badge on all seven pages; section 0 by the owner's default ("same devices as all previous tests").

Evidence: `~/mkvdovi-work/ws6-canary-2026-10-29/` (local). `answers.json` holds the answers
verbatim; `score.txt` scores them by the pre-registered rules of `PREREG.md` v2. Every decisive
answer fell on the `code / 1023` side: P1 (911.5 against 912.5) six of six for each composer (RR5,
RR10, RRm10) and Q (911.25 against 912.25) two of two for each. The REF page read "same" on every
row; the texture catch read "same" on 4 of 6 pages and the dithered patch on 2 (the pre-registered
limit). The answers followed the code on mirrored pages (no side bias) and agreed at gain −10 (no
tint preference) and gain 5 (no shift growing with the gain at 912). They match libplacebo's
`code / 1023` answers row for row (`render/answers_libplacebo.json`). Q also separates
`code / 1023` from `(code + 0.5) / 1024`. Scope: one device and one picture mode; the neutral
shift near code 944 (WS6.4, WS6.9) is a device behaviour this result does not explain.

### 2026-10-29: WS6.5 Build the redesigned canary (PR #80)

Step: [WS6.5][desk] Build the redesigned canary around the neutral shift WS6.9 measures (WS6.4 found
  no slope limit) and within the coefficient range real RPUs use. Gate: a discriminating canary, a composer that is
  neutral under one normalization and visibly coloured under the other, checked in libplacebo and
  handed off for the C9 with a `VIEWING_GUIDE.txt`. Needs: WS6.9 (the WS6.4 outcome, 2026-10-21,
  showed a gain-dependent neutral shift larger than the normalization difference; if WS6.9
  answers the question on its own (gate (3)), WS6.5 and WS6.6 are dropped).

Resolution: closes WS6.5; WS6.6 viewed it the same day. A blind forced-choice canary at BL chroma
code 912, away from the shift near 944 (WS6.9). v1 (one gain-10 page per composer) was checked by
an Opus skeptic before any viewing and replaced: a leftover shift and a scale difference were not
separable at one gain, a tint preference was not controlled, the key had an error, and "same"
answers were trusted beyond the eye's resolution. v2: composers RR5, RR10 (gains 5 and 10,
constant −1.25) and RRm10 (gain −10, constant 1.125), all inside the real-RPU envelope (constant
[−1.253, 1.137], coefficients ±18), each on a page and its mirror, plus a gain-0 REF page; five
rows of touching patches per page (P1 three times with balanced sides, Q, and a flat-against-dither
texture catch); delivered as the Plex show "WS6 canary" with a guide that gives no hint of the
expected side.

Evidence: `~/mkvdovi-work/ws6-canary-2026-10-29/` (local). `design.txt`: every composer inside
the envelope, slope at U0 equal to its gain, neutral code 911.555 under `code / 1023`. `run.log`
and `build_checks.txt`: 7 of 7 pages decode to their planned frames, base layers identical, each
page's RPU differs from REF's only in the mapping. `render/check.txt`: libplacebo picks the
`code / 1023` side on every P1 and Q row, reads every T and REF row as equal, and puts the crossing
of each page at 911.555. `score.py` gives the pre-set decision for synthetic observers of each
hypothesis, flags a tint-preferring and an always-left observer, and returns "code/1023 holds" on
libplacebo's own answers.

### 2026-10-29: E13.11 Function-length lint and the size contract (PR #79)

Step: [E13.11][gpu] Function-length lint and the size contract: `too_many_lines = "warn"` in
  `Cargo.toml`, an `#[expect]` naming its step on each of the 10 functions over 150 code lines,
  `clippy.toml` cut to its live key (no `msrv`: clippy reads `dovi_rpu_builder`'s 1.88; the
  `tools/` crates set none and lint at the toolchain's MSRV),
  `## Size limits` in `.claude/workflow.md`, and the size note moved from `rust-check.sh` to the ship
  plugin's global hook. Gate: fmt, clippy (also `--features cuda`), tests; the L1 gate without
  `--update`; `scripts/cuda-parity.sh` (the markers sit in its paths); a 160-statement probe
  function fails clippy.

Resolution: closes E13.11. `clippy::too_many_lines` now fails the gate above 150 code lines. The
10 functions over it carry `#[expect(…, reason = "E13.<n>: split by phase")]` naming E13.6,
E13.8–E13.10 and the new E13.12–E13.17; E13.18–E13.21 hold the two `tools/` files over 800 lines
and the lint follow-ups. Without `msrv`, the `tools/` crates (no `rust-version`, default lint
levels) lint at the toolchain MSRV, so `manual_is_multiple_of` failed the `tool:fit_hlg_composer`
gate (found by the pre-PR panel); the three checks in `tools/fit_hlg_composer/src/siting.rs` use
`is_multiple_of` now. `tools/compare_baseline` already failed clippy on main
(`unnecessary_map_or`, `src/main.rs:56`); it is untouched here.

Evidence: on the CUDA host at the reviewed head, the repo gate (fmt, workspace clippy,
`scripts/ci/test-gate.sh`), `clippy-cuda`, the L1 regression gate without `--update` (5/5 cuts),
`scripts/cuda-parity.sh` PASS and the `tool:fit_hlg_composer` gate passed; a 161-line probe
function in `dovi_rpu_builder` failed clippy with "this function has too many lines (161/150)";
CI on PR #79. Cross-review: Codex (gpt-6.1-sol, high) exec and Codex's GitHub review LGTM; the
pre-PR panel's one finding (the `tools/` MSRV lint) fixed.

### 2026-10-28: WS6.8 Build the neutral-finder probe (PR #78)

Step: [WS6.8][desk] Build the neutral-finder probe (owner, 2026-10-22): composers neutral at the same
  BL chroma code u0 with gains about 1 (bt2100-like), 1.5, 2.5 and 5, each over a fine sweep of
  BL chroma codes covering the shift WS6.4 found (about 938–962), so the code where the C9 shows
  grey can be read per gain. If the owner's X-Rite i1Display Pro can be read from a script
  (ArgyllCMS `spotread -O`), the probe is a timed sequence of full-field codes per file, measured
  automatically; otherwise patch rows read by eye. Gate: the probe checked in libplacebo and handed
  off with a `VIEWING_GUIDE.txt` (plus the measurement script when metered).

Resolution: closes WS6.8; WS6.9 (the metered session on the C9) is next and needs the owner. The
probe is metered (owner decision 2026-10-27): ArgyllCMS 3.5.0 `spotread` reads the i1Display Pro
from this PC, picture mode Dolby Vision Cinema, 15 runs in about 40 minutes. 13 files: a gain-0
reference (played at the start, middle and end), arm A centred at code 912 with gains 0.25, 0.5,
1, 1.5, 2.5, 5 and 10, arm B centred at code 160 with gains 0.25, 0.5, 1 and 2.5, and a control
whose composer ignores the BL chroma. Arm A sits at 912, not 944, so every sweep stays at or below
code 960. Holds play in a shuffled order inside each file and the files in a scrambled order. The
analysis compares ln(R/G) and ln(B/G) with the interpolated reference and fits neutral code
against gain; its outcome rules are pre-registered (`PREREG.md` draft 2; draft 1 was checked by an
Opus skeptic and Codex). Arm A intercepts at gain 0: 911.6 under `code / 1023`, 912.4 under
`code / 1024`, 912.2 under a 12-bit `4·code / 4095` tunnel. The method is in
[`PLAYBACK.md`](PLAYBACK.md) "Metered tests".

Evidence: `~/mkvdovi-work/ws6-neutral-probe-2026-10-27/` (local). `run.log:70`: the libplacebo
gate passed with outcome H1023, decision "code/1023 holds". `render/analysis.txt`: every Cb and Cr
crossing within 0.03 code of `code / 1023` (worst A10 Cb, 911.527 against 911.555) against the
0.1-code gate, arm intercepts 911.558 and 159.925 (expected 911.555 and 159.922). `selftest.txt`:
nine synthetic devices (each hypothesis, drift, a curved shift, a noisy slow meter, an output
offset) give their pre-set outcomes, none a false "code/1023 holds". `build_checks.txt`: each
file's extracted RPU is byte-identical to the written one, and the base layers match the built
ones. The measurement script is `probe_meter.py` (`check`, `run`, `analyze`). Hand-off: forplex
`WS6-neutral-probe-2026-10-27` (13 `.DV.mkv` files and `VIEWING_GUIDE.txt`), listed in
`TESTS_PENDING.txt`. Cross-review: Codex exec (gpt-6.1-sol, high) and Codex's GitHub review
LGTM; the pre-PR panel's two doc findings (the libplacebo tolerance, an unsourced luminance
ratio) were fixed before the review.

### 2026-10-23: E16.2 RPU builder input checks (PR #68)

Step: [E16.2][desk] Validate in `RpuBuilder::new`: refuse those values (and an empty or inverted 8.1
  source range) as `Error::OutOfRange`, with a backstop write of the base so a builder that exists
  writes every shot; add `Level6::clamped` for SEI values and `Error::code` for JNI; correct the
  three doc points. No delivered RPU, `extra.json` or mkvdovi output changes. Gate: unit tests at
  10001, 8192, 9 targets, L9 255, L11 16 and a 4096, empty and inverted source range fail on the
  parent and get `Error::OutOfRange` from `new`; values at the limits write; the fixture tests and
  the live `dovi_tool` test stay byte-identical; the repo gate and `tool:rpu_builder_check` pass.

Why: E16.1 shipped `dovi_rpu_builder` (#67). A review from the NuvioTV fork (its
  `ROADMAP_LOG.md`, 2026-10-23) found that `RpuBuilder::new`
  (`dovi_rpu_builder/src/builder.rs:93`) accepts parameters that every later `ShotRpu::write`
  refuses as the generic `Error::Rpu`: L6 above 10 000, L5 offsets above 8191, L9
  `source_primary_index` 255, L11 `content_type` above 15, more than 8 trim targets, an 8.1 source
  range above 4095. An empty or inverted 8.1 source range is written without complaint. The docs
  overstate `panic = "abort"` (it covers this workspace only), omit the L3 and L254 that every CM
  v4.0 RPU carries, and tell callers to reset the HDR10+ scene hold after a seek, which is the
  caller's policy.

Resolution: closes E16.2 and the item E16. `RpuBuilder::new` builds the L5, L6, L9 and L11 blocks
and runs the `dolby_vision` writer's own `validate` on each, counts trim targets by distinct PQ
code (the crate merges L2 blocks that share a `target_max_pq`, so 9 targets with 8 codes still
write; `MAX_TRIM_TARGETS` 8), refuses an 8.1 source range above 4095 or with min >= max, and
returns `Error::OutOfRange` for all of them. A backstop write of the base at the end of `new`
proves every later write, so `Error::Rpu` is now an internal failure only. On the parent the L2,
L5, L6, L9, L11 and 4096 cases passed `new` and failed at `write` with `Error::Rpu`; the empty and
inverted source ranges wrote without error, so refusing them is this crate's rule, not the
`dolby_vision` crate's (`source_range_81` never produces one). New API: `Error::code` (1 to 7; 7 is
`Rpu`), `Level6::clamped` and `Level6::MAX_VALUE`, `MAX_TRIM_TARGETS`. Docs
([`RPU_BUILDER.md`](RPU_BUILDER.md)): `panic = "abort"` covers this workspace only, an FFI caller
still uses `catch_unwind`; every CM v4.0 RPU, the static 8.4 one included, carries the neutral L3
and the L254 of `CmV40DmData::default_safe()`; `Hdr10PlusHold::reset` leaves seeks to the caller's
policy (the fork's default, mkvdovi parity, has no refresh mid-shot). No delivered RPU,
`extra.json` or mkvdovi output changed.

Evidence: `new_refuses_values_the_rpu_cannot_carry` (red on the parent) and
`values_at_the_limits_are_written` in `dovi_rpu_builder/src/builder.rs`,
`level6_clamped_caps_each_field_at_10000` and `codes_are_fixed`; the fixture tests and the live
`dovi_tool` 2.3.4 test unchanged and passing; the repo gate (fmt, workspace clippy,
`scripts/ci/test-gate.sh`) and `tool:rpu_builder_check` passed; `cargo check --target
aarch64-linux-android` passed; CI on PR #68. Cross-review: Codex (gpt-6.1-sol, high) and Gemini
(3.8 Flash) LGTM; the pre-PR panel confirmed nothing (one Fable P2 refuted).

### 2026-10-22: E16.1 In-process RPU builder library (PR #67)

Step: [E16.1][gpu][corpus] A new workspace crate with the RPU builders: one complete RPU NAL per access
  unit for 8.1 and 8.4 (composers preset, bt2100, canary), HDR10+ ST 2094-40 payload → L1, static
  8.1 and 8.4 (owner, 2026-10-22: static 8.4 L1 max 3079, average the pooled measured bt2100 scene
  average of the four HLG development cuts, `l1_avg_pq_cm_version` V29); `mkvdovi` takes its
  extra.json definition from the crate and still calls `dovi_tool generate`. Gate: `mkvdovi`
  extra.json and RPUs unchanged (`scripts/rpu-baseline.sh compare --require-identical-l1` and
  `sha256sum` of `RPU.bin` on the nine 0.7.0 baselines); the builder's output equals the `RPU.bin`
  records of `dovi_tool` 2.3.4 for every case (8.1 shots V29 and V40, 8.4 preset/bt2100/canary,
  static 8.1 and 8.4, HDR10+ for each peak source); `cargo check --target aarch64-linux-android`
  passes; unit tests for each builder.

Resolution: closes E16.1 and the item E16. A new workspace crate, `dovi_rpu_builder`
([`docs/RPU_BUILDER.md`](RPU_BUILDER.md)), builds one complete RPU NAL per access unit in-process,
byte-identical to `dovi_tool generate` 2.3.4. `mkvdovi` takes its `extra.json` from it
(`RpuParams::extra_json`) and still calls `dovi_tool generate`; no CLI flag, sidecar or delivered
RPU changed. The canary shift moved to `dovi84_composer::canary_mapping`, next to a new
`check_mapping_writable`. The fork's later DV7.3 rewrite entry point still needs E15.1 first.
Cross-review: Codex (gpt-6.1-sol, high) and Gemini (3.8 Flash) LGTM; the pre-PR panel found
nothing.

Why this item: the NuvioTV fork (`tinof/NuvioTV-Fork`, branch `perf/shield-pass`, `ROADMAP.md`
Appendix A, step DV2.1) adds an RPU to each access unit during playback on a Shield TV Pro 2019
and wraps this crate in a JNI staticlib (DV2.2; JNI stays in the fork). Its DV3.2 (HLG → 8.4
static) is gated by the P8 canary turning green through the app's own conversion, so the library
carries the canary. Not in this item: switching `mkvdovi` to in-process generation (an owner
decision tied to P10.3; [`HLG_COMPOSER.md`](HLG_COMPOSER.md) section 7 (c)).

Static 8.4 L1 (owner, 2026-10-22): min 0, avg 1283, max 3079, `l1_avg_pq_cm_version` V29. The
average is the frame-weighted mean of the measured scene averages of the four HLG development
cuts, from fresh `dovi84-bt2100-v1-spec420` runs (accurate sampling, CUDA). The g3 run was
identical to its 0.7.0 baseline sidecar. Scene peaks run from 2293 to 3079, 36 of 65 at 3079.

| Cut | Frames | Mean scene average (12-bit PQ) |
|---|---|---|
| Wimbledon 2024 final rally | 3051 | 874.9 |
| Glastonbury 2025 Supergrass stage | 1550 | 1511.9 |
| Green Planet tropical canopy | 1526 | 1377.1 |
| UCL Bayern–PSG match action | 3033 | 1528.2 |
| Pooled | 9160 | 1282.67, rounded to 1283 |

Evidence (WSL2 RTX 4070 host; `~/mkvdovi-work/e16-1-2026-10-22/EVIDENCE.md`):

- **`mkvdovi` unchanged.** `scripts/rpu-baseline.sh compare --require-identical-l1` against
  `~/mkvdovi-work/rpu-baseline/0.7.0`:
  - cases: default d1, g3, h1, r1, r2, r4; `--mdfix` r4, r5; `--hlg-composer preset` g3
  - all nine pass, every `RPU.bin` sha256-identical, and `mkvdovi`'s own log lines identical
    after normalization
  - captures in `~/mkvdovi-work/rpu-baseline/e16-1-0e07f62/`
  - the byte-exact `extra.json` goldens pass, plus new HDR10+ (V40, no sidecar) and CM v2.9
    goldens captured from main@67632bc before the move
- **Builders = `dovi_tool` 2.3.4**, checked four ways:
  - committed fixtures (`dovi_rpu_builder/tests/fixtures`, from `make_fixtures.sh`; bt2100 and
    canary through `fit_hlg_composer`)
  - the cases: 8.1 with shots (V40 with the V29 floor, V40 with the V40 floor, V29), 8.4 preset,
    bt2100 and canary, static 8.1, static 8.4 bt2100 and canary, and two synthetic HDR10+
    sequences for every peak source under V29 and V40
  - a live `dovi_tool` test (runs in the test gate; CI installs the pinned 2.3.4)
  - a serde-path equivalence over 96 parameter combinations (`tools/rpu_builder_check`)
- **HDR10+ real content.** The eight HDR10+ development cuts × 4 peak sources × V29/V40, the
  stream's own SEI payloads ordered as `hdr10plus_tool` 1.7.2 orders them:
  - 64/64 runs byte-identical over 11,841 frames
  - parsed values equal to the JSON for every frame and peak source
  - scene starts equal to `SceneInfoSummary` on every run
- **Android.** `cargo check --target aarch64-linux-android -p dovi_rpu_builder` passes (new CI job
  `rpu-builder`). No NDK on the host, so no cargo-ndk build (the fork's DV2.2).
- **Unit tests for each builder.** They cover both sides of the clamps, the composer and
  block-level guards, the HDR10+ identity fields, malformed and multi-window payloads, every
  `SceneKey` field and the scene-refresh flag. Two deliberate mutations of the builder turned the
  fixture tests red.
- **Gates on the final code:**
  - the gate (only allowed skips), clippy-cuda, `scripts/cuda-parity.sh` PASS, the L1 gate
    without `--update`
  - `tool:fit_hlg_composer`, `tool:rpu_builder_check`
  - `cargo deny` (Android target added to its graph), `cargo audit`, `release-version-check v0.7.0`

Found on the way: `hdr10plus/json` and `dolby_vision/serde` turn on `serde_json/preserve_order`,
so they live only in the excluded tool. Raising `clippy.toml` msrv to 1.88 turned on new lints in
`fit_hlg_composer`, so it stays at 1.82. `tools/compare_baseline` fails clippy under the current
toolchain on main too (pre-existing).

### 2026-10-21: WS6.4 View the slope ladder on the LG OLED C9 (PR #66)

Step: [WS6.4][owner] View the slope ladder on the LG OLED C9. Gate: the steepest chroma slope the C9
still follows recorded in the log. Needs: WS6.3.

Resolution: closed by owner decision (2026-10-22). Gate answer: the C9 follows the curve's shape at
every slope tested, up to 20.46; the pre-registered g\* is undefined (outcome "anything else")
because the neutral moves with the gain. Follow-up: WS6.8/WS6.9.

Viewed by the owner, who sent one photo per file (kept locally); Claude read each photo in the
guide's terms and the owner confirmed the reading by eye. The confirmed readings are the RESULT
block of the work directory's `VIEWING_GUIDE.txt`; the earlier `owner-report.txt` holds the
2026-10-19 in-person answers for WS6L_1 and unconfirmed drafts for WS6L_2/WS6L_3. Section 0:
Plex on the Nvidia Shield TV Pro, picture mode Dolby Vision Cinema (owner, 2026-10-22). That
differs from Dolby Vision Cinema Home, the mode announced for the 2026-10-19 in-person session
and used for the 2026-10-17 canaries. Not recorded for the photos: TV firmware, Shield software
version, HDMI input, room light (2026-10-19: Shield → C9, dim). Dolby Vision badge on all eight
files.

| File | Rung | Background | Row 1 | Row 2 | Row 3 | Class |
|---|---|---|---|---|---|---|
| WS6L_4 | H2.5 (g 2.5) | dark | (not counted) changes, 5/6 | changes, 5 | changes, 4/5 | O |
| WS6L_5 | HT (g 5) | dark | changes, 7 | changes, 6/7 | changes, 5 | O |
| WS6L_2 | H5 (g 5) | dark | changes, 7 | changes, 7 near grey | changes, 5/6 | O |
| WS6L_7 | H10 (g 10) | dark green | changes, all green, 7 | changes, 7 | changes, 7, none grey | O |
| WS6L_1 | HK10 (g 10, constant −9.28) | faint green | changes, all green, 7 | changes, 7 | changes, 7, none grey | O |
| WS6L_6 | H20 (g 20.46) | clearly green | changes slightly, all green, 7 | changes, 7 | changes, 7, none grey | O |
| WS6L_3 | HA (g 20.46, old structure) | clearly green | almost the same, green | changes slightly, 7 | changes, 7, none grey | X/O |
| control | bt2100 | purple | purple | purple | purple | as predicted |

(Patch numbers are "closest to grey"; row 1 is 941–947 in 1-code steps, row 2 938–950 in
2-code steps, row 3 932–956 in 4-code steps; patch 4 = 944 everywhere. WS6L_1's background:
black in the owner's in-person report of 2026-10-19 (Cinema Home), faint green in the
confirmed photo reading of 2026-10-21 (Cinema); the rows agree.)

**Outcome under the pre-registered table: "anything else".** No H rung passes (H2.5 is O on
row 2), so g\* is not defined, and none of the A, B, C rows fits (no flat or kinked H rung).
By the guide, WS6.5 gets no verdict from this test, and a follow-up probe comes first.

**What the viewing does show:**
- **The C9 follows the curve's shape at every slope up to 20.46.** Every counted row grades
  monotonically from green to the high-code end; nothing is flat or clamped. The 2026-10-17
  "uniform green" was not a slope limit (that viewing was in Cinema Home, this one in Cinema).
- **The neutral point moves to higher BL chroma codes, more for steeper curves.** The
  apparent neutral is about 946 for H2.5 (model 943.5 under /1023, 944.5 under /1024), about
  948.5 for HT and 950 for H5, and above 956 (beyond the last patch) for H10, HK10, H20 and HA.
  In composed chroma that is about 0.006 below the model at g 2.5, 0.024–0.032 at g 5 and more
  than 0.12 at g 10 (more than 0.25 at g 20.46). Code shifts this size are 3 to 14 times the
  0.92-code `code / 1023` vs `code / 1024` difference.
- **The shift follows the gain, not the coefficients.** HT (gain 5, a term of 7.7) matches H5
  (gain 5, terms below 1); HK10 (constant −9.28) matches H10 (constant −1.25); HA matches
  H20. So the out-of-range constant and large terms were not the 2026-10-17 problem.
- **No input transform explains it.** Every rung is neutral at the same u0, so any change of
  how the TV reads the BL chroma code (another normalization, an offset, a clamp, coarser
  quantization) would move all rungs' neutral to the same code. A constant offset of the
  composed chroma would move it less for steeper rungs. The observed shift grows with gain.
- **All three pre-registered device models are out.** The ±8 running sum, and the
  stored-chroma clamps at 940 and 948 each predicted a different pattern (C, B, A); none
  appears. This rests on the ladder rows alone. The 2026-10-17 canaries' surround, which all
  three predicted green, is unresolved: answered green from memory on 2026-10-19
  (`owner-report.txt`), later recalled as black, and the owner is not sure (2026-10-22). The
  ladder's steep files do show a green surround (code 944 at black luma), consistent with the
  shift.

Not covered: why the neutral moves (the C9's composer arithmetic, or its display management
acting on these curves), and whether the shift extrapolates to the production bt2100 composer
(gain about 0.7–1.1; at the observed rate the shift would be under about one code). The
`code / 1023` question stays open: a slope canary needs the shift to be known first, since it
exceeds the normalization difference at every gain tested.

Photos: `docs/in/` in the working tree (not committed); hand-off folder
`WS6-slope-ladder-2026-10-18/`; work directory `~/mkvdovi-work/ws6-slope-ladder-2026-10-18/`.

### 2026-10-18: WS6.3 Build the slope ladder (PR #66)

Step: [WS6.3][desk] Build the slope ladder. Gate: the ladder checked in libplacebo and handed off for
the C9 with a `VIEWING_GUIDE.txt`.

Resolution: gate met. libplacebo v7.374.0 within 0.54 12-bit codes of the model on every decisive
patch (`render/compare.txt`); handed off in forplex `WS6-slope-ladder-2026-10-18/` with
`VIEWING_GUIDE.txt`.

The 2026-10-17 canary gave no verdict. Its chroma composer had a slope of 20.46 times identity
(0.02 per BL code) and a constant of −9.28, and the C9 showed it as one flat green. Composers the
C9 is known to follow have constants between −1.25 and +1.14 (bt2100, the preset). The ladder
measures how steep a chroma curve the C9 still follows. Three probes separate the slope from
term size and from the out-of-range constant. Design: a judge panel of three Opus designers
(least effort, separating the causes, working back from WS6.5), the owner's choice of 8 files,
and two Codex plan reviews.

**Composers.** All keep the bt2100 luma curve. Cb = c + a1·u + a2·u² + a3·u³, with Cr the same
on v and no luma or cross terms. Each is neutral at u0 = 944·(1/1023 + 1/1024)/2, so patch 4
sits 0.46 code from grey under both normalizations and the ladder does not answer WS6.5's
question. Gain g is dCb/du at u0.

| Rung | g | Constant | Coefficients u, u², u³ | Largest term at u0 |
|---|---|---|---|---|
| H2.5 | 2.5 | −1.25 | 1.064, 1.154, −0.271 | 0.98 |
| H5 | 5 | −1.25 | −0.171, 1.121, 1.216 | 0.95 |
| H10 | 10 | −1.25 | −3.103, 2.057, 3.647 | 2.86 |
| H20 | 20.46 | −1.25 | −8.333, 2.057, 9.795 | 7.69 |
| HT | 5 | 0 | −5.853, 9.034, −2.277 | 7.69 (H20's) |
| HK10 | 10 | −9.2807 (the old constant) | 7.271, 7.883, −4.629 | 6.71 |
| HA | 20.46 | −9.1964 | k = 3.791 on all three (the old structure) | 3.50 |

All coefficients lie inside the preset's range [−18.26, 18.76]. The H constants are the
preset's own −1.25.

**Pattern.** One HLG page (x265 lossless, level 5.1, 750 frames). Three rows at BL luma 640
(117 nits grey through bt2100) hold seven patches each, around Cb = Cr = 944, spaced 1, 2 and 4
codes. The background is (64, 944, 944) and there is no centre marker.

The plan had rows at luma 600/680/760 with 4-code spacing. Its own design check failed: the
central patches of H10, H20 and HA fell outside the C9's range, which is ≤ 600 nits and P3. A
row therefore counts for a rung only when patches 3–5 stay inside that range and the
neighbouring steps are ≥ 8 ΔE_ITP. Steep rungs are read on the fine rows, shallow rungs on the
coarse rows.

Patch 4's tint is 2 / 4.5 / 9 / 18 ΔE_ITP from H2.5 to H20. On the 1-code row, patch 3 or 5 is
almost as grey as patch 4, and which one depends on the normalization, so that row accepts any
of 3–5.

**Counterfactual decodes.** Before viewing, the tool decoded the 2026-10-17 canaries and the
ladder under these models:
- order-3 terms dropped;
- coefficient clamps at 2, 4 and 8;
- a ±8 saturating accumulator, terms first or constant first;
- constant clamps at 2 and 4;
- a [0.4, 0.6] output clamp;
- stored-chroma clamps at 940 and 948;
- 17- and 33-node 3-D LUTs.

Three models reproduce the 2026-10-17 patch colours (both canaries uniform green, control
purple): the terms-first ±8 accumulator, and the clamps at 940 and 948. Each predicts a
different ladder vector, so none collide, and each has its own row in the guide's
pre-registered table. All three also predict a green background for the 2026-10-17 canaries,
which that test did not ask about.

**Checks (all passed):**
- **Design:** f(u0) = 0.5 to 2^−24, slope within 0.1%. Patch 4 is greyest on every reading row
  under /1023 float, /1024 spec fixed point and (c+0.5)/1024. Spec composed chroma never clips.
  The background stays ≤ 0.05 nits.
- **Files:** every frame of each extracted RPU carries its composer and none of the other seven.
  Each extracted RPU equals the written one byte for byte. Between rungs only the mapping
  differs. Against the control only the mapping, L1 `avg_pq` (3071 → 2291) and the CRC
  differ. The shared L1 is H20's, measured 62/2291/3079 and written 12/2291/3079 after the
  generator's min clamp; the control's measured min (2214) is clamped to 12 too (`run.log`).
- **Decode:** the base layers of all 8 files are `cmp`-identical, and 750/750 decoded frames
  equal the page.
- **libplacebo v7.374.0** (pinned settings): within 0.54 12-bit codes of the model on every
  decisive patch, and patch 4 is the greyest rendered patch on every reading row.
  - The 1-code tolerance was gated on the decisive patches, not on every patch in [0.02, 0.98]
    as planned. Saturated outer patches with a channel near black differ by up to 53 codes, and
    are identical under `gamut_mode=clip` and `warn`. Clipped patches differ by up to about
    1460 on the rungs and 1833 on the control. Those patches get a hue-order check. The
    bt2100 control has no patch inside the display range, so it got only the hue-order check
    (`render/compare.txt`).

**Viewing guide.** It records the section 0 fields, plays the control last and stops playback
between files. Report form per file: plays, DV badge, background, then for each row changes /
same / partly and the patch closest to grey. The key and the table follow the answers. g* is
the steepest of H2.5–H20 that passes together with every shallower one. It is nominal: WS6.5
checks the gain itself. An Opus reviewer checked the guide against the tool output twice before
the hand-off.

Files: forplex `WS6-slope-ladder-2026-10-18/` (`WS6L_1`…`WS6L_7`, `WS6L_control`,
`VIEWING_GUIDE.txt`, `reference-renders/`). Work directory, `run.sh` (rebuilds the set),
`versions.txt` (mkvdovi and hdr_analyzer_mvp 0.7.0 +cuda, dovi_tool 2.3.4) and the
`ws6ladder` evidence crate: `~/mkvdovi-work/ws6-slope-ladder-2026-10-18/`. Next: WS6.4, the
owner's viewing.

### 2026-10-18: v0.7.0 released

Release PR #63 (squash a65dcd9) set every shipped crate, README and CITATION to 0.7.0 and dated
the CHANGELOG section. The section adds an upgrade note for `--cm-version v29` users. Contents: P7
(#49), P9 opt-in flag (#50), P11 (#52) and P13 (#53). There was no rc, because nothing needed a
test outside CI (owner decision 2026-10-14). P12.1 was not merged and moves to the next patch.

Before the tag, on a65dcd9 (`mkvdovi 0.7.0`, `hdr_analyzer_mvp 0.7.0 (+cuda)`):

- The `release.yml` dry run (run 39118006758) passed on all five targets. `scripts/release-check.sh
  v0.7.0` passed.
- `scripts/cuda-parity.sh`: PASS.
- Nine fresh RPU captures (six default, two `--mdfix`, one `--hlg-composer preset`) against
  `~/mkvdovi-work/rpu-baseline/0.6.0/`: `compare --require-identical-l1` exit 0 and `RPU.bin`
  byte-identical on all nine. mkvdovi's own log lines are identical, with versions normalized,
  except for the P11 notice lines in the two `--mdfix` logs. Script and logs:
  `~/mkvdovi-work/rpu-baseline/0.7.0/` (these captures are also the 0.7.0 baselines).

The tag run published v0.7.0 as the latest release. P9.3 and E12.1 (v0.8.0) can start now.

### 2026-10-17: C9 viewings, WS6 canary and P10 `ms_weight`

Viewed by the owner the same day. Path: Plex on the Nvidia Shield TV Pro into the LG OLED C9,
picture mode Dolby Vision Cinema Home. Not recorded: TV firmware, Shield software version, room
light. Answers are given as reported, before the interpretation or the key were shown.

**WS6 (3), `WS6-normalization-2026-10-17`:** Dolby Vision on all three files. WS6_1 and WS6_2:
"no grey patch", "all green", the same green on all seven patches. WS6_3 (control): strong
pink/purple, as predicted. This falls under the guide's "anything else" row (no conclusion); its "no grey patch at all,
all patches strong purple" row does not match either, since the canaries differed from the
purple control. Under `code / 1023`,
`code / 1024` and `(code + 0.5) / 1024` alike, the model predicts a green-to-magenta row with one
or two near-grey patches (ΔE_ITP 0 to about 130 across the row, `design.txt`). The TV applies
these RPUs (they differ from the control) but does not follow the steep chroma curve: the
composed chroma comes out flat and low. A plausible reading, not tested: the C9 clamps either
large MMR coefficients or the composed chroma, and a clamp to the bottom of the range renders
green. That reading would also fit the P8 green canary, but P8's subtle evidence (the preset's
visible tint against a neutral bt2100) still shows that composers in the normal coefficient range
are applied. No verdict on `code / 1023`. Next: a slope ladder (rows of increasing chroma slope,
from bt2100's magnitude upward) to find where the C9 stops following the curve, then a canary
inside that range, with more patches if the step gets smaller.

**P10 step 4, `P10-ms-weight-2026-10-17`:** Dolby Vision on all four files; "no difference" on
both cuts. Key, opened after the report: storm-X = 2048, storm-Y = 512, firechase-X = 512,
firechase-Y = 2048. By the rule set before the test, generated L2 keeps `ms_weight` 2048 and the
step closes.

### 2026-10-17: P9 holdout scoring, P6 check list, WS6 and P10 hand-offs

**P9 step 1, holdout scoring** (evidence `~/mkvdovi-work/p9-holdout-2026-10-17/`; mkvdovi and
hdr_analyzer_mvp 0.6.0 +cuda, dovi_tool 2.3.4). The one scoring of `q-alita-01/02` and
`q-the-shining-01/02`, under the rule written before it; Alien and Deadloch were not opened. Retail
L1 from each cut's RPU (the parser reproduces the four development `embedded_l1.csv` files byte for
byte); a DV-free remux keeping HDR10+; two conversions each through `scripts/rpu-baseline.sh
capture` (`--analysis-quality accurate --hwaccel cuda`, the second with `--hdr10plus-l1 hybrid`),
all eight `--verify` passed; scored with a copy of the development `score.py` extended with max and
min MAE (it reproduces the development 37.58 / 18.91 / 338.43). Delivered RPU values (after the
generator's clamps) against retail per frame:

| Pooled, today → hybrid | MAE | Clause |
|---|---|---|
| Average, frames above the floor (4,095) | 276.58 → 33.83 | PASS |
| Average, all frames (5,806) | 315.90 → 25.24 | PASS |
| L1 max | 105.58 → 105.58 | PASS (equal by construction) |
| L1 min | 5.98 → 4.91 | PASS |

Per cut (average above the floor / all / max / min): alita-01 236.08 → 33.34 / 236.08 → 33.34 /
27.98 = / 0.00 → 10.86; alita-02 425.73 → 67.11 / 413.57 → 15.24 / 108.32 = / 0.00 → 8.72;
the-shining-01 291.33 → 17.43 / 291.33 → 17.43 / 246.54 = / 12.00 → 0.00; the-shining-02
262.69 → 49.32 / 321.88 → 35.02 / 39.77 = / 12.00 → 0.00. Hybrid: 15, 25, 12 and 14 pieces, 0
fallbacks, 0 mismatches against the hybrid rule. Caveats: the minimum splits by title (retail
writes 0 on every Alita frame, 12 on every Shining frame; libplacebo ignores L1 min); the max
clause cannot fail, since both paths take the HDR10+ peak; two titles, four one-minute cuts. Owner
decision: the default flips in the minor after v0.7.0.

**P6 step 1, check list** (evidence `~/mkvdovi-work/p6-checks-2026-10-17/list.md`, survey of the 45
development cuts, read-only). Every check warns only, so RPU baselines stay byte-identical. Owner
pick: C5, C1, C4, C7, C3.

| Check | Threshold | Development corpus |
|---|---|---|
| **C1** measured MaxCLL above the stated MaxCLL (`light_level`, `sample_rate` and `downscale` 1) | > 1.25 × | 6 of 25 cuts with a stated MaxCLL (The Shining mountain road 2.53×, Joker 972 / 676); Fury Road 1.01× is the negative |
| C2 stated MaxCLL above the mastering peak | > 1.10 × | 7 retail cuts (Fury Road, Saving Private Ryan); not picked, warns on correct discs |
| **C3** stated MaxFALL above MaxCLL (silent today: `resolve_light_levels` returns early) | any | none; needs a synthetic mux |
| **C4** HDR10+ peak or sampled RPU L1 max above MaxCLL | > 1.25 × | The Shining mountain road (MaxScl 3710 / 1633); the RPU form sees only the sampled windows |
| **C5** source RPU L6 against the container (decides the `--mdfix` output; pairs with P12) | MDL any, CLL/FALL > 2 % | Joker (RPU 992 / 285, container 676 / 87); First Man and Saving Private Ryan disagree too but are FEL |
| C6 RPU source range against its own L6 | > 2 codes | The Revenant only; no spec found that requires equality |
| **C7** HDR10+ MaxScl 0 on every frame (L1 max at the 100-nit floor under `max-scl*`, expected, not run) | all frames | Alien ×2, Alita ×2 |
| C8 HDR10+ frame count against the video, outside hybrid | any | none; needs a packet count (MediaInfo `FrameCount` is stale on 16 of 45 cuts) |
| C9 mastering primaries matching no standard set (label fallback picks BT.2020 silently) | > 0.002 | none |
| C10 container static metadata against the stream SEI | > 1 % | none |

Observation, not a check: in all nine HDR10+ JSONs the `DistributionValues` are not a monotonic
percentile list (one entry looks like a percentage). The `histogram` peak source takes their
maximum; whether that reads what it intends is not checked against ST 2094-40.

**WS6 (3), the `code / 1023` canary** (evidence `~/mkvdovi-work/ws6-normalization-2026-10-17/`).
Feasible: the chroma coefficient range (−65536 to 65535) lets a steep chroma composer (3.73 on u,
u², u³ and v, v², v³; constant about −9.3; bt2100 luma) turn the 0.93-code difference between the
conventions at BL chroma 956 into one patch step of ΔE_ITP 39–40. Pattern: three rows (about 90,
150 and 270 nits) of seven patches at BL chroma 953–959. WS6_1 is neutral under `code / 1024`,
WS6_2 under `code / 1023`, WS6_3 is the normal bt2100 control. Under `/1023` WS6_1 is grey at
patch 3 and WS6_2 at patch 4; under `/1024` at patches 4 and 5; a `(code + 0.5) / 1024` device
shows two equally near-grey patches. Lossless base layer, every decoded frame equal to the source,
identical base layers, RPUs checked per frame. libplacebo (`/1023`) matches the prediction within
0.47 of a 12-bit code with `upscaler=bilinear`; its default chroma upscaler moves the canary by
about half a code, so the test measures the device's effective chroma offset at the composer
input. Untested: lossless decode on the C9 (the guide has a "does not play" outcome).

**P10 step 4, `ms_weight` A/B** (evidence `~/mkvdovi-work/p10-ms-weight-2026-10-17/`, key in
`KEY.txt` there only). Two Fury Road cuts (4000-nit master, peaks above 800 nits); the 512 files
are rebuilt from mkvdovi's own `extra.json` with only `ms_weight` changed. A field diff of every
frame shows only the three `Level2.ms_weight` values and the CRC; base layers byte-identical. 2048
is the `dolby_vision` crate's zero point (`round(t·2048 + 2048)`), not documented as neutral for a
device. Retail RPUs in the development corpus carry 512 (First Man, Saving Private Ryan, The
Shining), 0 (Top Gun: Maverick, Alita, Skyfall) and 2048 (House of the Dragon, Joker, Inside Out 2).

**WS6 (2):** the procedure is [`PLAYBACK.md`](PLAYBACK.md), from how P8 ran and what it did
not record. After the C9 viewings the same day, at the owner's request, Claude corrected it: the
WS6 and P10 paths and picture mode, where to find the C9 firmware, lossless streams play through
the Shield, stay inside the coefficient range the device follows, an interpretation row for every
observation, how Claude asks the report questions, and finished test folders deleted only on
request (the WS6, P10 and P8 folders were deleted 2026-10-17).

### 2026-10-17: owner decisions on the waiting list and the missing gates

No code change. The owner settled every "Waiting for the owner" entry and stated a gate for
every `TODO(owner)` step; the gates themselves are in the `ROADMAP.md` items.

- **P9 holdout scoring:** score the four holdout cuts that carry retail DV L1 (`q-alita-01/02`,
  `q-the-shining-01/02`); Alien and Deadloch stay unseen (scoring them would need `cm_analyze`).
  The rule is written before any scoring: hybrid becomes the default if the average's pooled
  clamped MAE (above the floor and over all frames) is lower than today's HDR10+ path and the
  per-frame MAE of L1 max and L1 min against retail is not higher; the owner confirms the flip.
- **P13:** v29 RPUs stay without L2. A display treats a missing L2 for its target like a neutral
  trim, and v29 output never carried L2. P13 has no follow-up left and leaves the roadmap; its fix
  is in `CHANGELOG.md` `[Unreleased]`.
- **E14 step 2:** a separate test author is dropped as a rule; in-session tests plus the step 6
  `cargo mutants --in-diff` gate stay.
- **P8 step 1:** no second Dolby Vision device is available. The LG OLED C9 result stands alone;
  1.0.0 needs the WS6 `code / 1023` answer instead.
- **WS6:** Claude builds a discriminating canary (neutral under one normalization, visibly
  coloured under the other), checks it in libplacebo and hands A/B files to the owner for the C9;
  Claude drafts the written playback procedure for the owner to correct.
- **E12:** the Windows zip bundles the NVRTC DLLs, because the driver ships none and the
  analyzer compiles `kernels.cu` at runtime. The rc test runs on the Windows side of the RTX 4070
  dev host through WSL interop, with the CUDA toolkits (12.4 to 13.1 installed there) hidden from
  `PATH`, so it stands in for a driver-only user host.
- **P10 step 3:** decided by the WS7 simulator, not by a playback test.
- **E1 (2):** a self-hosted runner on the WSL2 host, for trusted runs only (no fork PRs).
- **E7 (2):** an always-on RPU frame-count check; full `--verify` stays opt-in.
- **E6:** tool versions go into a global MKV tag on the output.
- **P6:** Claude drafts a list of concrete source checks; the owner picks.
- **WS1:** a new peak estimator must beat `max` on pooled per-shot MAE against retail on the
  development tier, lose no frozen synthetic case by more than 1 code, match on CPU and CUDA, and
  hold on unseen holdout cuts.
- **Deferred items** got reopen conditions instead of gates: P4 (WS7 shows a gain), WS5 (R2),
  E5 (a user request), FEL (the owner lifts the hold), WS2 step 3 (something reads L4).

### 2026-10-17: E13 step 5, analyzer split

Pure move in `hdr_analyzer_mvp`; no behaviour change. Hook line counts (`.claude/hooks/rust-check.sh`)
before → after: `pipeline.rs` 1348 → `pipeline/mod.rs` 338, `analyze.rs` 629, `crop_space.rs` 162,
`scenes.rs` 252; `analysis/frame.rs` 1026 → `frame/mod.rs` 643, `peak.rs` 397; `analysis/gpu.rs` 951
→ `gpu/mod.rs` 224, `backend.rs` 728. Test modules keep their names as files.

- Proof (`~/mkvdovi-work/rpu-baseline/e13-step5/proof/`): `split.py` and `verify_move.py` from step 3,
  extended with `unwrap` ranges (an inline `mod x { }` moved to a file, body dedented by exactly four
  spaces) and `import_only` files (only `use` lines added). Each of the six move commits prints
  PURE MOVE OK against its parent; the fmt commit is reproduced by `cargo fmt --all` at its parent.
  Allowed edits: 23 `pub(super)` widenings, `include_str!("../kernels.cu")`, the rewritten import
  block of `pipeline/mod.rs`, one `use` line in four test files.
- `cargo test -p hdr_analyzer_mvp -- --list`: the same 125 names before and after.
- `proof/run-proofs.sh` reruns every proof in clean worktrees (`E13 STEP 5 PROOF OK`).
  `git diff --color-moved=plain --color-moved-ws=allow-indentation-change 9d33d85 28d341e`: the only
  non-moved lines are `mod` lines, imports, the widenings, the `include_str!` path and two rustfmt
  re-wraps (`CudaContext::new(0)` in `backend.rs`, one `kernel_composed` call in `gpu/tests.rs`).
- Gates on 28d341e: fmt, clippy, clippy `--features cuda`, `cargo test --workspace` CI-style
  (`--nocapture`, 429 passed, only the 7 allowed `Skipping` lines), `scripts/ci/l1-regression-gate.sh`
  without `--update`, `scripts/cuda-parity.sh` PASS.
- Optimizer on (both gates pass `--disable-optimizer`): release CPU and `+cuda` builds of main@941489d
  and 28d341e, analyzer defaults, on the synthetic PQ and HLG gate clips, d1 HOTD, g3 Green Planet
  (bt2100 and preset) and r1 Joker; every CUDA run `analysis.gpu: true`. All 12 `.bin` and `.l1.json`
  pairs byte-identical (`proof/evidence.sh`, `premove/`, `postmove/`).

### 2026-10-15: E14 steps 1 to 5, high-risk survivors covered

Test-only; no production code changes. E14 stays open.

- Step 1 (#54, 0c68d2a): PR CI installs the tool-dependent test tools. CI run 38753171952
  `Test (Ubuntu)`: synthetic_accuracy 10.51 s, hlg_profile84 5.99 s, open_gop_start 3.24 s,
  hdr10plus_hybrid 7.36 s; only allowed skips.
- Step 2 (P13 pilot): Codex wrote the P13 acceptance tests in 148 s; 3 were red on main. They found
  nothing an in-session test missed, and missed the pipeline wiring. Owner decision open: does a
  separate test author become mandatory?
- Steps 3 and 4: `cargo-mutants` on main@c74effb for `verify.rs` and the riskiest modules (owner
  choice), not the full step-3 list. 336 survivors: dovi84_composer 11, `verify.rs` 20, verifier
  142, pipeline (`deliver.rs`, `measure.rs`, `dovi_steps.rs`) 42, metadata 30, `rpu_check.rs` 91.
  Triage: 0 actual bugs, 120 print-only or equivalent, 215 missing tests, 19 high-risk groups.
- Step 5, first PR (`test/e14-high-risk`): tests for the 19 high-risk groups. `mappings_equal` and
  `check_header` in dovi84_composer; `--verify` frame-count, HLG sidecar and compatibility-ID checks
  on real muxed outputs; sidecar identity and luminance mapping, RPU profile classification and HLG
  transfer probes in metadata and `rpu_check.rs`; pipeline delivery steps (`extract_base_layer`
  bsf decision, `mux_output` resume predicate, `run_dovi_video_step`) against PATH-shimmed fake
  tools in `pipeline/tests.rs`. The shims live in one hash-keyed directory next to the test binary
  (`target/<profile>/deps/mkvdovi-shims-<hash>`), written by staged write and rename. The
  `verify.rs` sibling-analyzer helper skips a stale analyzer whose `--help` lacks a
  `Composer::ALL` luminance mapping name.
- After counts, 7 targeted runs at the branch's tests: composer 46 caught; pipeline dovi_steps 30;
  pipeline deliver 12; verify 11; classify-format 6 caught, 1 unviable; classify-rpu 2 caught, 1
  unviable; classify-sidecar 2 caught. All 0 missed, 0 timeout.
- Gates on e8658ee: fmt, clippy, clippy-cuda, test (429 passed, no unexpected `Skipping` lines),
  l1-regression (check mode), cuda-parity: all pass.
- Evidence: `~/mkvdovi-work/test-audit-2026-10-15/` (`triage.md`, `triage.json`, `mutants/`,
  `mutants-after/SUMMARY.txt`).
- Open: the medium- and low-risk groups, and the step-3 modules not yet run.

### 2026-10-15: P13, `--cm-version v29` writes a CM v2.9 RPU

Closes P13. Changes delivered output only under `--cm-version v29`.

- Before: `generate_extra_json` wrote `cm_version` only for v40, and `dovi_tool generate` defaults
  to V40. v29 output was CM v4.0 with the generator's L3, L9 (DCI-P3), L11 (cinema) and L254 and no
  L2; `--verify` passed no expected CM version for v29.
- Code: the v29 branch of `generate_extra_json` writes `"cm_version": "V29"`
  (`mkvdovi/src/metadata/rpu_config.rs`). `pipeline::expected_cm_version` passes the requested
  version to `--verify` for both values; `verify::assert_rpu_invariants` fails CM v4.0 metadata on
  frame 0 under V29 and a missing CM v4.0 block under V40. The `--mdfix` notice
  (`pipeline/mdfix_notice.rs`) no longer names generator-default L9/L11/L254 under v29; it names
  L3, L9, L11 and L254 as dropped ("a CM v2.9 RPU has no CM v4.0 levels"). The `V29_GENERATED_*`
  constants are gone.
- v29 output: r2 HDR10, g3 HLG (bt2100 composer) and the r4 MEL `--mdfix` rebuild report "DM
  version: 1 (CM v2.9)" in `dovi_tool info --summary`, carry no CM v4.0 data (cmv29 blocks L1, L5,
  L6; no L2) and pass `--verify`.
- CM v4.0 unchanged: the default d1 RPU and the r4/r5 `--mdfix` RPUs are identical to the 0.6.0
  baselines (`scripts/rpu-baseline.sh compare --require-identical-l1`).
- Resume: a 0.6.0 v29 temp dir regenerates its RPU, because `config::withdraw_stale_rpu` sees the
  changed `extra.json`.
- Tests: `extra_json_names_the_requested_cm_version` (hermetic). The `cm_version_acceptance_*`
  tests in `metadata/tests.rs` and `verify.rs` were written by Codex from a behaviour-only spec (the
  E14 pilot, `~/mkvdovi-work/test-audit-2026-10-15/p13-pilot.md`); three were red on main. The
  `dovi_tool`-backed ones skip without `dovi_tool`.
- Gates: fmt, clippy `-D warnings`, full workspace test on 38ff64f; only allowed skips.
- Evidence: `~/mkvdovi-work/p13-cm-v29-2026-10-15/` (`run.sh`, `run.log`, `run-v29.log`,
  `capture-v29-gen.log`, `compare-*.log`).
- Open for the owner: should v29 RPUs carry neutral L2 trims? v40 writes them; v29 never did.

### 2026-10-15: P11, `--mdfix` dropped-levels notice

Closes P11's open step. Notice only: no change to any generated RPU, `extra.json` or resume
fingerprint.

- Code: `rpu_check::sample_rpu_windows` also collects `AuthoredLevels` (non-neutral L2 targets,
  non-neutral L3, L4, distinct L5, L6, non-neutral L8 indices, L9, L10 custom targets, L11, L254,
  L255). After each "Rebuilding … (--mdfix)." line (MEL and Profile 8),
  `pipeline/mdfix_notice.rs::print_mdfix_notice` prints a warning header and one line per level it
  replaces or drops, compared with what the rebuild writes (L9 from `--source-primaries` or
  MediaInfo, L11 from `--content-type`/`--reference-mode`, L254 (0,2)). A failed sample gives a
  general notice; `-q` hides it. `Args::trim_targets_nits` is now the one `--trim-targets` parser.
- Corpus census (18 development cuts with retail RPUs): of the `--mdfix`-eligible ones, 8 Profile 7
  MEL cuts are CM v2.9 with non-neutral L2 (plus L4, L5, L6); 2 Profile 8.1 cuts are CM v4.0 and
  also carry L3, L8, L9, L11 and L254.
- Real inputs: r4 Skyfall (P7 MEL, CM v2.9) names L2 at 100 nits replaced, L4 dropped, L6 not taken
  from the source RPU. r5 Inside Out 2 (P8.1, CM v4.0) names L2 at 100/600/1000 nits replaced, L3
  neutral, L4 dropped, L6, L8 at 100 nits dropped; L9, L11 and L254 equal, no line. Profile 8
  without `--mdfix` prints no notice. r4 and r5 `--mdfix` and the d1-hotd default capture are
  identical to the 0.6.0 baselines (`scripts/rpu-baseline.sh compare --require-identical-l1`,
  PASS).
- Gates: fmt, clippy `-D warnings`, full workspace test, no unexpected skips. Unit tests:
  `rpu_check::tests::authored_levels_*`, `distinct_l5_and_l11_values_are_all_kept`,
  `custom_l8_targets_come_with_their_l10`, `windows_accumulate_into_one_summary`, 13 tests in
  `pipeline/mdfix_notice.rs`, `cli::tests::trim_targets_parse_as_nits_in_order`.
- Found on the way, not fixed: P12 (`--mdfix` ignores the source RPU's L6; r5 `source_min_pq` 7
  becomes 62) and P13 (`--cm-version v29` writes a CM v4.0 RPU).
- Evidence: `~/mkvdovi-work/p11-mdfix-notice-2026-10-15/` (`run.sh`, `run.log`, `compare-*.log`).

### 2026-10-14: P9, peak parity for the other `--peak-source` values

Closes P9's open item "peak parity with `dovi_tool` is measured for the `histogram` source only".
Measurement only, no code change. Run on main f5d4ccc's code (mkvdovi and hdr_analyzer_mvp 0.6.0
+cuda, dovi_tool 2.3.4, hdr10plus_tool 1.7.2).

- Method: per source X and cut, the default HDR10+ path with `--peak-source X` (dovi_tool computes
  the peak) against `--hdr10plus-l1 hybrid --peak-source X --analysis-quality accurate` with a fresh
  analysis. The scorer ports `hdr10plus_peak_nits`, `hdr10plus_peak_code` and `build_pieces` and
  rebuilds every piece and fallback (range and reason) from `truth/hdr10plus.json` and the sidecar,
  independently of the log, which prints only the first 10 fallback ranges. Self-tested on the
  `histogram` captures of the hybrid-flag run (0 mismatches).
- Cuts: h1 Deadloch beach-day and shoreline-night, the DV-free remuxes of the two The Shining cuts
  (MaxSCL nonzero), and h4 Alita airlock-night and h5 Alien derelict-chamber (MaxSCL 0 on every
  frame).
- Result, PASS on all 18 runs: the derived fallbacks equal mkvdovi's report (counts per reason and
  the printed prefix); the hybrid max equals `dovi_tool`'s on every non-fallback frame (8,938 frames
  under `histogram99`, 6,034 under each `max-scl*`); fallback frames carry the enclosing analyzer
  scene's max after the 2081 floor (76 pieces); min and avg follow the rule after the generator's
  clamps. The source changes the peak on 37, 41 and 40 of the 42 parity-cut pieces, so the runs
  discriminate. Under `max-scl*`, every Alita and Alien piece falls back with "HDR10+ peak is 0"
  (18/18, 20/20). `--boost` writes the same RPU as `--peak-source histogram99`.
- Not triggered by any cut: the `missing`, `malformed` and below-average fallbacks (unit tests in
  `hybrid.rs` cover them). Peak against retail is still not scored.
- Evidence: `~/mkvdovi-work/p9-peak-parity-2026-10-14/` (README, `run.sh`, `score.py`, `score.md`).
- Guard: the unit test `hdr10plus_peak_sources_follow_dovi_tool` (`mkvdovi/src/pipeline/tests.rs`)
  pins the mirror for `histogram99` (last distribution value, not the largest), `histogram` and
  `max-scl`. It does not prove parity with `dovi_tool`; re-run `run.sh` and `score.py` after a
  `dovi_tool` bump. Of the HDR10+ development cuts, d2 Patriot also has nonzero MaxSCL; it was not
  run.

### 2026-10-14: P9, the hybrid flag

Closes P9's open step "the hybrid flag" as the opt-in `mkvdovi --hdr10plus-l1 hdr10plus|hybrid`
(default `hdr10plus`, output unchanged). Branch `feat/p9-hdr10plus-hybrid`; code in
`mkvdovi/src/pipeline/hybrid.rs`, e2e `mkvdovi/tests/hdr10plus_hybrid.rs`.

- Design (owner decisions 2026-10-14): an HDR10+ input under the flag runs the analyzer. The
  sidecar's scenes become the union pieces (HDR10+ starts ∪ analyzer cuts), so `generate_extra_json`,
  `--verify`, the stale-RPU check, L5 and L6 take them unchanged. Average: mean of the per-frame
  max-RGB codes, rounded half away from zero (the 2026-10-13 pre-registered rule). Minimum: min of the
  per-frame codes. Peak: `nits_to_pq_12_bit(round(nits))` of the HDR10+ scene's first frame, as
  `dovi_tool generate --hdr10plus-json` does. Fallback to the enclosing analyzer scene's max when
  the peak is missing, 0, not finite, above 10000 nits, or not above the average after the 2081
  floor; each fallback is reported.
- Refusals per file, source kept: `--analysis-quality fast`; a malformed HDR10+ scene list or one
  whose frame count differs from the stream's; measurements that cannot carry the hybrid (reused
  candidates are re-analyzed). `hybrid` with `--legacy-madvr-l1` is refused at startup. Resume
  appends ` hdr10plus_l1=hybrid` only for an HDR10+ input under the flag. `release.yml` pins
  `hdr10plus_tool` 1.7.2 so the e2e test runs in the release test job.
- Evidence: `~/mkvdovi-work/p9-hybrid-2026-10-14/` (README, `C/score.md`, `versions.txt`; mkvdovi
  and hdr_analyzer_mvp 0.6.0 +cuda at e14e514, dovi_tool 2.3.4, hdr10plus_tool 1.7.2).
  - Default path: `scripts/rpu-baseline.sh compare --require-identical-l1` against 0.6.0, 9/9 exit 0
    (h1 HDR10+ without the flag included).
  - HDR10+-only cuts (integration gate; `truth.l1` is null, so pipeline and plausibility only): h1
    beach-day 14 pieces, h1 shoreline-night 23, h5 derelict-chamber 20, h5 nostromo-corridor-dark 1.
    All convert, `--verify` passes, 0 fallbacks under the default histogram peak source.
  - Development tier: DV-free remuxes (`dovi_tool remove`, mkvmerge; HDR10+ equal to
    `truth/hdr10plus.json`) of the two Alita and two The Shining cuts at `--analysis-quality
    accurate`. Per-frame arrays equal to the 2026-10-13 run; 0 mismatches between delivered min/avg
    and the rule after the generator clamps; delivered max equal to the default HDR10+ path on every
    frame; 0 fallbacks; `--verify` passes.

| Delivered avg vs retail (clamped MAE, codes) | Above floor | All frames | 2026-10-13 U |
|---|---|---|---|
| Alita pooled (1,469 / 2,920 frames) | 37.58 | 18.91 | 37.6 / 18.9 |
| airlock | 29.56 | 3.86 | 29.6 |
| Iron City | 38.78 | 34.00 | 38.8 |
| Shining hedge maze | 79.87 | 79.87 | 79.9 |
| Shining mountain road | 21.49 | 21.49 | 21.5 |

- Not scored: peak against retail (not pre-registered). Peak parity with `dovi_tool` holds for the
  `histogram` source only. Open: holdout HDR10+ scoring before a default change; sidecar v6 with a
  per-frame max and the analyzer's resolved `--peak-source`.
- Gates on d93de4d: fmt, clippy and test pass. Pre-PR fixes: overflow check on the scene frame
  numbers; only `hybrid` conflicts with `--legacy-madvr-l1`, checked after parsing.

### 2026-10-14: P7, full-range PQ refused in mkvdovi

Closes P7's open step ("for PQ, normalize full-range input or refuse it") as a refusal in
`mkvdovi`; the analyzer's own full-range warning (`hdr_analyzer_mvp/src/pipeline.rs:412`) is
unchanged.

- Scope (owner decisions 2026-10-14): every input whose Profile 8.1 RPU mkvdovi generates, i.e.
  HDR10 (with or without measurements), HDR10+, and Profile 7 MEL / Profile 8 only under `--mdfix`.
  Range only: the matrix and primaries of PQ input are not checked. Untagged range is accepted
  silently. The MEL fast path (authored RPU passed through) is not checked. A Profile 8 input with an
  HLG base layer under `--mdfix` keeps the existing 8.4 `--mdfix` refusal.
- Refusal point: in `convert_file` next to the HLG contract (`needs_pq_range_check`), before the
  resume fingerprint, so no `mkvdovi_temp_*` directory is created, resumed or discarded, nothing is
  extracted, measured or written, and the source is kept. `resume_settings` is unchanged.
- Sources: MediaInfo effective and `_Original` range, the ffprobe stream, and the first decoded
  video frame from a video-only ffprobe call (`metadata::get_ffprobe_video_json`), because an MKV
  can override a full-range stream as limited in the container (ffprobe stream `tv`, frame `pc`)
  and MediaInfo is optional. Any source naming full range refuses; the error lists every tag read.
  Code: `metadata::pq_range_contract` / `check_pq_range_contract` and the shared
  `resolve_colour_field` in `mkvdovi/src/metadata/format.rs`; the HLG contract uses the same
  resolver and is unchanged.
- Tests: six unit tests in `mkvdovi/src/metadata/tests.rs` (every spelling and source refused,
  conflicting tags refused, limited and untagged accepted, one tool missing, matrix and primaries
  ignored), `pq_range_check_covers_exactly_the_generated_8_1_rpus` in
  `mkvdovi/src/pipeline/tests.rs`, and e2e `mkvdovi/tests/pq_input_contract.rs`
  (`full_range_pq_is_refused_before_any_work`;
  `container_override_of_full_range_pq_is_refused_without_mediainfo`, audio track first, MediaInfo
  hidden).
- Gates on a733804: fmt, clippy and `cargo test --workspace` pass (344 tests, only allowed skips).
  Pre-PR panel (three Opus lenses, Fable 5.1) and Gemini 3.8 Flash: no findings.
- RPU baselines: `~/mkvdovi-work/rpu-baseline/p7-full-range-pq-a733804/` against 0.6.0, 9/9
  byte-identical, gate exit 0. Hand check with the installed binary: full range refused with the
  source kept, limited range converts.
- No CUDA parity or L1 regression gate: no analyzer path changed.

### 2026-10-13: E13 step 4, `convert_file` split by phase

Behavior-preserving function extraction in `mkvdovi/src/pipeline/`; no user-visible change.
`pipeline/mod.rs` went from 1037 to 409 lines; `convert_file` is about 270 lines of orchestration.

- New modules: `temp_dir.rs` (`locate_temp_dir`, `settle_leftover_temp_dir`, `prepare_temp_dir`),
  `measure.rs` (detection report, unsupported exit, `acquire_measurements`,
  `find_reusable_measurements`, `run_fresh_analysis` with the analyzer `--help` composer probe, the
  seam for the P9 hybrid flag), `config.rs` (`prepare_configuration`, `withdraw_stale_rpu`) and
  `deliver.rs` (`extract_base_layer`, `inject_rpu`, `mux_output`). The MEL fast path moved to
  `dovi_steps::convert_mel_fast_path`. `convert_file` ends through `finish_success`, which folds the
  duplicated cleanup (an E13 simplification candidate).
- Tests: 27 new unit tests (4 tool-free `convert_file` characterization, 8 temp-dir, 7 measure,
  4 `withdraw_stale_rpu`, 4 delivery); 181 mkvdovi unit tests in total.
- Gates on d7c080d: fmt, clippy and `cargo test --workspace` pass (log
  `~/mkvdovi-work/rpu-baseline/e13-step4-d7c080d/workspace-test.log`); `hlg_profile84` and
  `open_gop_start` ran without skips; the corpus classification test passed (174 s).
- RPU baselines: 9 stems (default d1, g3, h1, r1, r2, r4; mdfix r4, r5; preset g3) captured in
  `~/mkvdovi-work/rpu-baseline/e13-step4-d7c080d/` and compared with
  `~/mkvdovi-work/rpu-baseline/0.6.0/`: `compare --require-identical-l1` exit 0 and `RPU.bin`
  byte-identical for all 9; mkvdovi's own log lines identical after normalizing timings and paths.
  Covers Profile 7 MEL, Profile 8 `--mdfix`, HDR10, HDR10+ and HLG bt2100 and preset. The only full-log
  difference is one analyzer stderr line interleaved differently on g3.
- No CUDA parity or L1 regression gate: no analyzer path changed.

### 2026-10-13: P8 playback test, the LG OLED C9 applies the RPU composer

The owner viewed the P8 test set on an LG OLED C9 with Plex on the TV and with Plex on an Nvidia
Shield TV Pro; the result was the same on both. The set was built on 2026-10-11 with 0.6.0-rc.1
plus #41 (main 8db4ad6), and `--verify` passed on every mkvdovi output.

- Files: a broadcast HLG clip (Blue Lights, 3026 frames) and a synthetic test pattern (800 frames,
  four still pages: superwhite greys, 100% and 75% colours, dark greys with 25% colours, sub-black),
  each as the HLG source, `--hlg-composer preset` (A), `--hlg-composer bt2100` (B) and a canary (C).
  Within a group the HLG base layer is byte-identical; only the RPU differs. The canary is B's RPU
  with both chroma MMR constants shifted by −0.0625 (`fit_hlg_composer canary-rpu`), muxed like
  mkvdovi does. A decoder that applies the RPU composer shows the picture strongly green (libplacebo:
  mid grey 1808/1808/1808 becomes 1377/2023/1258 in 12-bit PQ R/G/B); one that ignores the composer
  shows normal colours. `canary-check` confirmed the shifted mapping on every frame of the RPU
  extracted from both muxed canary files.
- The owner's report, per file:
  - Canary, clip: "full green", Dolby Vision badge.
  - Canary, pattern: "first green bars, then different colors, then green boxes, with two red and one
    dark blue, then green columns", Dolby Vision badge.
  - A (preset), clip and pattern: yellow tint on greys, Dolby Vision badge.
  - B (bt2100), clip and pattern: no tint, "looks good", Dolby Vision badge.
  - Pattern HLG source: played as SDR (HLG is signalled in the VUI; a player detection matter, not
    followed up).
- Result: the C9 applies the composer in the RPU. The canary turns an unchanged base layer green,
  and A and B differ the way their composers do (A tinted, B neutral). `--hlg-composer bt2100` stays
  the default; the release that would have switched the default back to `preset` is not needed.
- Hue: the libplacebo SDR render predicted lavender mid greys and olive dark greys for the preset;
  the owner saw yellow on the C9. Recorded, not explained.
- Not covered: whether devices evaluate the composer at `code / 1023` or `code / 1024` (WS6; 5.9
  codes, below what this test can show); TV firmware, picture mode and HDMI path (not recorded);
  devices other than the C9.
- Work files, viewing guide and reference renders: `~/mkvdovi-work/p8-playback-2026-10-11/`
  (local).

### 2026-10-13: P9, scene list for the pixel average

The P9 design step is done. A hybrid HDR10+ → Profile 8.1 mode that takes the average from pixels
lands about 19 codes from the retail L1 average per frame on the two Alita cuts (pooled MAE over
all frames, after the generator's 819 average floor). Today's HDR10+-derived average lands
338 codes away. The 19 codes depend on that floor: the raw measured average is 147 to 148 codes from
retail over all frames, because on the floor frames it reads about 259 codes below 819. Above the
floor, raw and clamped scores are the same for H, A, U and R. Which scene list carries the average
matters little on these cuts: the HDR10+ scenes (H), the analyzer's cuts (A) and their union (U)
are within about 1.5 codes of each other (1.51). By the pre-registered rule the hybrid mode
uses U. A is the numerical best, and no HDR10+ boundary added a picture cut that the analyzer
missed.

**Method.** The rule was registered in `method.md` before any scoring. Each candidate list
partitions the cut into pieces. A piece's average is the mean of the frame codes
`avg_max_rgb_pq_12bit`, rounded half away from zero; every frame of the piece gets that value.

- **Cuts.** The two Alita: Battle Angel cuts (airlock night, Iron City day) carry Dolby Vision MEL
  and HDR10+ from the same master. Their retail L1 is the reference per frame (scoring only). The
  two The Shining cuts support the result but take no part in the decision.
- **Analyzer.** v5 runs with mkvdovi's `accurate` flags, crop on. Every per-frame array and scene
  list is identical to the step-0 runs.
- **Today's path (S).** A DV-free remux of each cut (`dovi_tool remove`; the HDR10+ SEI equals the
  corpus JSON) went through the installed `mkvdovi` ("Detected: HDR10+"); the L1 was exported from
  its RPU.
- **Frame alignment.** Frame counts agree across decoder, sidecar, HDR10+, retail CSV and S RPU,
  with `leading_skipped_frames` 0.
- **References outside the decision.** R is the retail scene refreshes, an oracle. F is the
  per-frame values with no piece average.
- **Scores.** Errors against the retail average, raw and clamped (average ≥ 819, as the generator
  writes). The primary score is clamped MAE on frames where the retail average is above 819.
- **Rule.** Near-best = within 5 codes of the best primary score; U is preferred inside that set.
  The winner must also pass an all-frame regression gate, be near-best on each Alita cut, and win
  under both rounding variants. V1 is the unrounded mean; V2 uses the sidecar's own scene averages
  for A.
- **Scoring.** Two scorers implemented it independently and agree to 0.01 code on every number.
  A methods review, a provenance review (22 piece values recomputed by hand) and Codex
  (gpt-6.1-sol, high) found no error that changes the verdict.

| Alita pooled, clamped | H | A | U | R (oracle) | F (per frame) | S (today) |
|---|---|---|---|---|---|---|
| MAE, retail above floor (1,469 frames) | 38.3 | **36.8** | 37.6 | 32.0 | 63.8 | 267.7 |
| MAE, all frames (2,920) | 19.3 | **18.5** | 18.9 | 16.1 | 32.5 | 338.4 |
| Bias, above floor | −19.3 | −19.3 | −19.3 | −19.2 | −12.4 | +267.7 |
| p95 / max \|error\|, above floor | 70 / 137 | 86 / 152 | 86 / 152 | 59 / 86 | 231 / 331 | 347 / 405 |
| Floor frames lifted above 819 | 0 | 0 | 0 | 0 | 35 | 1,451 |
| *Raw (no floor):* MAE, all frames | 148.0 | 147.2 | 147.6 | 144.8 | 165.2 | 338.4 |
| *Raw:* bias, floor frames (1,451) | −259.1 | −259.0 | −259.0 | −259.0 | −259.1 | +410.0 |
| *Raw:* MAE, above floor | 38.3 | 36.8 | 37.6 | 32.0 | 70.7 | 267.7 |

Per cut (clamped MAE above floor; H / A / U):

- airlock: 28.4 / 29.6 / 29.6 (191 frames);
- Iron City: 39.8 / 37.9 / 38.8 (1,278 frames).

On the Shining cuts (all frames above floor), the scores are:

- hedge maze: 83.0 / 79.9 / 79.9;
- mountain road: 21.5 for all three.

S stays 336 to 460 codes away on both Shining cuts.

Findings:

- **The measured average is the gain; the scene list is not.** H, A and U differ by about 1.5 codes on
  the primary score, against a 5-code margin. A is lowest on primary and all-frame MAE; H has the
  lowest p95 and maximum error. U wins only by the pre-registered preference: it keeps the HDR10+
  boundaries a hybrid peak needs. Read it as "U does no measurable harm", not "HDR10+ boundaries
  help".
- **What decided it is a handful of boundaries in one cut.** On airlock, A equals R exactly
  (18 pieces). Five H scenes hold several shots. Four of them have retail 819, so the clamp hides
  them. The fifth, 1271–1461, holds all 191 above-floor frames of the cut (retail 940 and 953 in
  its two shots); there H's single value, 912, lands closer than A's split at 1456 (910 / 969),
  which is why H scores 28.4 against 29.6 on airlock. On Iron City, U is A plus one H start, frame 1044. That start lies inside retail shot
  999–1081, and splitting there costs U 1,125 code-frames. Iron City contributes 1,278 of the
  1,469 primary frames.
- **Today's HDR10+ average reads high everywhere.** S is +268 codes above floor and +410 on every
  floor frame, where it writes 1229 against retail 819; none of its floor frames stay at 819.
  This repeats the 2026-10-03 finding on four more cuts from two titles.
- **Rounding does not matter.** V1 and V2 pick U too; V2 differs from the main variant in one A
  piece by one code.
- **A constant crop or calibration shift does not change the winner under the rule.** Shifts of
  0 to 40 codes keep the spread under 2 codes. The numerical order can change: A is best up to
  +35 codes, U at +40.
- **Minimum.** Retail min is 0 on every frame, so this data cannot choose a rule for the minimum.
  The generator writes it as at most 12 codes anyway.

Limits:

- Two cuts from one title decide the result, and the analyzer was developed on this corpus, so
  A's near-oracle partition may be in-sample. A case where the union rescues a cut the analyzer
  misses was never observed. Holdout HDR10+ cuts would test it.
- The crop convention of the retail L1 is unknown. The scores hold for the committed crop;
  another crop changes the mean's denominator and needs rescoring.
- Peak was not evaluated. mkvdovi's default HDR10+ peak source is the histogram
  (`DistributionValues`), not MaxSCL (`pipeline/hdr10plus.rs:84`).
- The scores measure agreement with retail metadata in PQ codes: neither displayed-picture units
  (WS7) nor playback.
- The model omits the generator's upper average clamp (`max_pq − 1`); every frame average here is
  far below it.

Where: `method.md`, the runs, both scorers' scripts and results, and the review checks are in
`~/mkvdovi-work/p9-scene-list-2026-10-13/`, outside the repository.

### 2026-10-11: P6 and E7, L6 light-level fixes

The three L6 defects the E13 step 2 tests found on 2026-10-07 are fixed on branch
`fix-l6-zero-light-levels`. No L1 change.

- P6, container 0: when the MKV Colour element states MaxCLL or MaxFALL 0, MediaInfo reports that
  0 and moves the stream's value to `MaxCLL_Original`/`MaxFALL_Original`. `track_light_level`
  (`mkvdovi/src/metadata/static_metadata.rs`) reads the `_Original` field when the container value
  is missing or 0; a non-zero container value still wins. The pinned tests were flipped and renamed
  (`static_metadata_reads_the_stream_max_cll_behind_a_zero_container_value` and its MaxFALL twin, a
  clip with 812/213 nits, plus a check of the L6 in the generated `extra.json`), and
  `track_light_level_reads_the_stream_value_behind_a_zero_container_value` is a hermetic test.
- P6 minor, `Details.txt`: `parse_details_number` reads either decimal mark. With both marks the
  last one is the decimal mark; a lone `,` or `.` that groups three digits is a thousands separator
  (`1,000`, `1.000` → 1000), else a decimal mark. A value with three fraction digits (`812.500`) is
  read as grouped, because light levels are whole nits; this rule came from the pre-PR review (one
  P3, found by all four reviewers). The Codex review then found that it read `812.000` (812 on
  main) as 812000 (P2); 251b294 bounds it by the PQ peak: grouping applies only while the value
  stays within 10,000 nits (`10.000` → 10000), above it the mark is a decimal mark (`812.000` →
  812, `812.500` → 812.5, `12,500` → 12.5), and `1,000,000` / `1.000.000` do not parse.
- E7: `assert_rpu_invariants` (`mkvdovi/src/verify.rs`) takes `passthrough` (`delivery.is_none()`;
  only the MEL path calls `verify_post_mux` without a delivery). There, L6 MaxCLL/MaxFALL 0 is a
  warning; generated RPUs, missing or malformed fields and MaxDML 0 still fail. Tests:
  `rpu_invariants_warn_on_zero_light_levels_only_in_a_passed_through_rpu` and
  `rpu_invariants_reject_zero_mastering_peak_and_missing_light_level_in_a_passed_through_rpu`.
- Gates on 251b294: fmt, clippy, workspace tests (303 passed, only allowed skips). No analyzer or
  decode path touched, so no CUDA parity or L1 gate.
- End to end: `scripts/rpu-baseline.sh capture` with `--verify` of the r4 Skyfall MEL cut
  (`~/mkvdovi-work/rpu-baseline/refactor-e6d63ed/inputs/r4-skyfall-macau-casino-entrance.mkv`)
  passes with the two L6 0 warnings (it failed at e6d63ed); 1457 RPU frames, in
  `~/mkvdovi-work/rpu-baseline/l6-fix-e052c3d/default/r4-skyfall-macau-casino-entrance`. This is
  the r4 MEL default baseline E13 step 4 waited for. The d1 HDR10 capture
  `~/mkvdovi-work/rpu-baseline/l6-fix-e052c3d/default/d1-hotd-s01e06` is identical to
  `refactor-e6d63ed/default/d1-hotd-s01e06` (`compare --require-identical-l1` PASS).

### 2026-10-08: P9 step 0, HDR10+ scene boundaries against the picture

P9 step 0 is done: on every HDR10+ cut with enough boundaries, HDR10+ metadata frame n describes
picture frame n (offset 0). The HDR10+ scene list is a separate question: it lines up with the
picture, but it is not a shot list.

Method. The eight HDR10+ development cuts (Deadloch ×2, Alita ×2, The Shining ×2, Alien ×2) were
re-analyzed with the v5 analyzer (`0.5.1 (+cuda)`) in two configurations: the corpus peak flags
(`--hwaccel cuda --transfer pq --downscale 1 --disable-optimizer --no-crop`) and what
`run_hdr_analyzer` passes under `accurate` (`--downscale 1 --sample-rate 1 --optimizer-profile
conservative --hwaccel cuda`). Every sidecar is version 5 with `analysis.gpu: true`, and on every cut
decoded frames = `source.stream_frames` = HDR10+ JSON length, with `leading_skipped_frames` 0. The
HDR10+ JSON was extracted the way mkvdovi does it (`ffmpeg -c:v copy -f hevc`, then `hdr10plus_tool
extract -i`, `pipeline/hdr10plus.rs`). It is identical to the corpus JSON on all eight cuts, down to
every per-frame `LuminanceParameters`.

`AverageRGB` holds one value per HDR10+ scene on all eight cuts, so per-frame correlation cannot
show an offset; only scene boundaries can. For each cut, HDR10+ scene starts (frame 0 excluded) were
compared with the analyzer's cuts at offsets −5…+5. `aligned` means best offset 0, at least 3
matches, and every other offset at most half as many; fewer than 3 non-zero starts is
`inconclusive` in the scan. A cut with fewer than 3 starts still counts as aligned by
corroboration when every start matches at offset 0, no other offset matches, and each start is
also in the retail RPU and visibly a cut exactly at n. The Shining mountain road (2 starts, scan
verdict `inconclusive`) is the one cut aligned this way. 25 boundaries that at least one source
lacks (HDR10+, the analyzer or, on the Alita and Shining cuts, the retail RPU's scene refreshes),
plus the two Shining mountain road starts, were checked by eye on four labelled frames n−2…n+1. Frame indices were
verified by joining `showinfo` before and after `select` on pts. One reviewer per cut classified the
frames, and a second reviewer tried to refute the verdicts.

| Cut | Non-zero HDR10+ starts | Match at k = 0 | Any other k (−5…+5) | Verdict |
|---|---|---|---|---|
| Deadloch beach day | 13 | 13 | 0 | aligned; the delivered RPU (E13 baseline) has the same 14 starts |
| Deadloch shoreline night | 21 | 20 | 0 | aligned |
| Alita airlock night | 12 | 12 | 0 | aligned; also 12/12 against the retail RPU |
| Alita Iron City day | 15 | 14 | 0 | aligned; 13 of 15 against the retail RPU, which lacks 1044 and 1168 (1168 is in the analyzer's cuts) |
| The Shining mountain road | 2 | 2 | 0 | aligned by corroboration: 264 and 1001 are in the RPU, in the analyzer's cuts and visually a cut exactly at n |
| Alien derelict chamber | 18 | 12 | 0 | aligned |
| The Shining hedge maze | 0 | – | – | inconclusive (one HDR10+ scene) |
| Alien Nostromo corridor | 0 | – | – | inconclusive (one HDR10+ scene) |

Both analyzer configurations give the same cuts on every cut. The scan and the visual check found no
systematic shift and no confirmed off-by-one; Alien derelict 298 stays ambiguous (the change may start
at 297, or be a whip pan), and the analyzer has no cut there to compare.

Boundary coverage, after the visual check:

- **HDR10+ scenes that span several shots:** 11 real picture cuts lie inside an HDR10+ scene:
  Alita airlock 58, 640, 926, 1120, 1456 (5 of 13 scenes, 54% of the frames); Alita Iron City 108,
  1082, 1123, 1381 (3 of 16 scenes, 38%); Shining hedge maze 1178 (the single scene); Alien
  derelict 292 (scene 270–297). In total 2,835 of 11,841 frames (24%) sit in an HDR10+ scene that
  holds more than one shot. A per-scene average taken over such a scene mixes shots.
- **HDR10+ starts without a picture cut:** Deadloch shoreline 709 and Alien derelict 323, 332,
  1008, 1393 split a shot (motion, a flare). Alien 1026 is ambiguous: a camera move with a lens
  flare, most likely the same shot. Alien 298 is a cut at low confidence (fast, dark handheld
  motion).
- **HDR10+ is the only source with a real cut:** Alita Iron City 1044, a different take of the
  same close-up (medium confidence, confirmed by the second reviewer), which both the analyzer and
  the retail RPU miss. Alita Iron City 1168, which HDR10+ and the analyzer share but the RPU lacks,
  was looked at on one sheet after the two-reviewer check, not by the reviewers: same framing, the
  light drops, undecided. It does not affect the alignment verdict.
- **The analyzer:** it misses Alita 1044, 1123 and 1381 (all `below-threshold`: they fail
  `score > 3.0` or `score > 16 × baseline`, `analysis/scene.rs:125-129`; no miss is due to the
  12-frame minimum scene length) and cuts once inside a shot (Deadloch shoreline 456, an arm
  passing the camera). Evidence for E4.
- **The retail RPU shot list is not a shot list:** on The Shining mountain road, 5 of its 7
  non-zero scene refreshes fall inside a shot where the opening credits change: 122 in the shot
  0–263, 303, 374 and 551 in 264–1000, 1381 in 1001–1460. Only 264 and 1001 are picture cuts. Corpus `truth/shotlist.txt` with `shotlist_origin: rpu-scene-refresh`
  needs that caveat when it scores scene detection.

What this means for P9. Taking pixel measurements onto the HDR10+ frame grid needs no shift. But a
hybrid mode that takes the scene list from HDR10+ and the average from pixels would average across
shots in about a quarter of these frames and split shots elsewhere. The next P9 design step has to
decide which scene list carries the pixel average: HDR10+, the analyzer's cuts, or their union with
the HDR10+ peak spread over the pieces. It also has to say what that does to the L1 average on the
Alita cuts, where the retail RPU is the reference.

Limits. HDR10+ values are scene-constant, so the order of frames inside a scene cannot be tested,
and does not matter for L1. Alita and The Shining carry Dolby Vision as well and take the MEL path
in mkvdovi (`metadata/format.rs:52`), so they test the alignment, not mkvdovi's HDR10+ path.
Three HDR10+ titles from discs and one from a streamer; no open-GOP start among them.

Where: evidence (runs, `align.py`, `alignment.json`, `disputed.json`, `visual.json`, frame sheets)
in `~/mkvdovi-work/p9-step0-2026-10-08/`, outside the repository.

### 2026-10-07: E13 step 3, mkvdovi module split

E13 step 3 is done. Four commits, each a pure move with no behavior change:

- Inline tests moved to `mkvdovi/src/metadata/tests.rs` and `mkvdovi/src/pipeline/tests.rs`.
- `metadata.rs` split into `metadata/{mod,format,probe,rpu_config,sidecar,static_metadata}.rs`;
  `pipeline.rs` into `pipeline/{mod,analyzer,dovi_steps,hdr10plus}.rs`. Re-exports keep every
  `metadata::X` and `pipeline::X` path valid.
- `pipeline/mod.rs` stays at 1037 lines, over the 800-line limit: `convert_file` alone is about
  900 lines, and step 4 splits it by phase.
- Gates on 281b927: fmt, clippy (also `--features cuda`), workspace tests (result lines and the 5
  `Skipping` lines identical to main@f104c78), the corpus classification test
  (`MKVDOVI_CORPUS_DIR`), the L1 regression gate without `--update`: all pass. All 8
  `scripts/rpu-baseline.sh compare --require-identical-l1` runs against
  `~/mkvdovi-work/rpu-baseline/refactor-e6d63ed/` pass (default d1, g3, h1, r1, r2; mdfix r4, r5;
  preset g3); unchanged main@f104c78 also passed all 8 before the move. Logs in
  `~/mkvdovi-work/rpu-baseline/e13-step3-281b927/proof/` and in the PR body.
- Still open: E13 steps 4-5.

### 2026-10-07: E13 step 2, refactor safety net

E13 step 2 is done. It adds the checks the pure-move splits of steps 3-5 are judged by, and
changes no production code: every Rust hunk is in a `#[cfg(test)]` module or under `tests/`.

- 34 new characterization tests: `mkvdovi/src/metadata.rs` (format detection, static L6, the whole
  `extra.json`), `mkvdovi/src/pipeline.rs` unit tests (source retention, output naming), and one
  e2e test in `mkvdovi/tests/hlg_profile84.rs`,
  `successful_hlg_conversion_deletes_the_source_and_the_temp_dir`.
- Mutation-checked during development. `metadata.rs`: 69 of 75 mutants caught; the survivors are
  equivalent or reachable only by the opt-in corpus test. `pipeline.rs`: the unit tests on
  `finish_success`/`should_keep_source` missed two mutants, both in the production deletion path.
  For non-DV inputs, source deletion and temp cleanup are an inline copy at the end of
  `convert_file`; `finish_success` is called only by the Profile 7 MEL fast path. The new e2e test
  covers that copy: removing its `fs::remove_file(input_file)` or `remove_dir_all(&temp_dir)` makes
  it fail.
- The opt-in test `corpus_cuts_are_classified_like_their_manifests` (`MKVDOVI_CORPUS_DIR`)
  classifies all 45 development cuts like their manifests.
- Final-RPU baselines captured at main@e6d63ed in
  `~/mkvdovi-work/rpu-baseline/refactor-e6d63ed/{default,mdfix,preset}`: HDR10, HDR10+, HLG with
  `bt2100` and `preset`, two MEL incl. an open-GOP start, `--mdfix` on MEL and Profile 8.
  `scripts/rpu-baseline.sh compare --require-identical-l1` of each baseline against itself exits 0.
  The capture of `--mdfix` runs now finds the `*.mdfix.DV.mkv` output.
- Tooling: the CUDA gate in `.claude/workflows/pre-pr-panel.js` also matches module directories
  (`pipeline/` like `pipeline.rs`); the Stop hook notes a touched `.rs` file over 800 code lines
  once per session and compiles a touched `.cu` kernel with `nvcc -ptx`; module-size rule in
  CLAUDE.md.
- Gates on f950ca8: fmt, clippy, test (no unexpected skips); all pass. No CUDA or L1 gate required
  (no analyzer path touched).
- Still open: E13 steps 3-5.

### 2026-10-06: P1, E1, CUDA as the main pipeline

mkvdovi now treats CUDA as the main analysis pipeline. An explicit `--analysis-quality balanced` or
`fast` with GPU analysis is kept but warned about: it saves no time (the run is NVDEC-bound), drops
the measured MaxCLL (`fast` also MaxFALL) and is not parity-checked. Under `accurate` (the `auto`
default with GPU analysis), measurements analyzed more coarsely are re-analyzed instead of reused
with a warning; other presets keep the warning. mkvdovi warns when a run that expected GPU analysis
comes back with sidecar `analysis.gpu: false`, and when `--hwaccel cuda` resolves with an analyzer
built without the `cuda` feature. CI lints the analyzer with `--features cuda`; cudarc loads the
driver and NVRTC at runtime, so no GPU or toolkit is needed.

- Evidence: unit tests `coarser_sampling`, `rejects_coarser_sidecar`, `analysis_quality_notice` and
  `gpu_analysis_missing` (`mkvdovi/src/pipeline.rs`); e2e test
  `accurate_reanalyzes_coarser_measurements` (`mkvdovi/tests/hlg_profile84.rs`).
- Gates on d459681: fmt, clippy, clippy-cuda, test (no unexpected skips), `scripts/cuda-parity.sh`,
  L1 regression; all pass, L1 references unchanged. No measurement changes on either backend.
- The CI `cuda` clippy step passed locally with cudarc's build script rerun without nvcc or CUDA
  environment variables, and in hosted CI on #31 (run 37530882715).
- Still open: P1 item 1 (CPU fallback default); E1 GPU runner and NVRTC compile-only spike; E2
  addition; E12.

### 2026-10-06: E11, `l1_diff` and open-GOP cuts

E11 is done. A cut that starts at a CRA has RASL pictures no decoder outputs; sidecar v5 records
them, and our frame `i` is stream frame `i + leading`. `tools/l1_diff` paired rows by position
and stopped on the count difference, so such cuts could only be compared in decoded coordinates.

- The sidecar is checked against the `.bin` first: v5 stream fields present,
  decoded + leading = `stream_frames`, every per-frame array of length n.
- Reference frame labels must count up by one. A reference that ends at the last stream picture
  is read in stream coordinates and its rows for the leading pictures are skipped. One labelled
  from 0 with a row per decoded frame (older exports) keeps decoded coordinates and refuses
  `--per-shot` and `--scenes`. Every other count difference is an error, with a hint when no v5
  sidecar was read.
- `--per-shot` and `--scenes` are re-based by the same offset; shots and cuts in the leading
  pictures are dropped and counted, and a cut beyond the reference's frames is an error.
- `--export-reference` labels rows with stream frames (unchanged without leading pictures).

Acceptance gate:

- Synthetic test `tools/l1_diff/tests/open_gop.rs`: a `.bin` of 10 decoded frames with a v5
  sidecar recording 2 leading pictures. A 12-row stream reference scores zero error with its first
  2 rows skipped; `--per-shot` and `--scenes` with a stream shot list match every cut; a
  stream-labelled export reads back with zero error. S+1 rows, a wrong `stream_frames`, a v5
  sidecar without its source fields and a cut beyond the stream are refused; an older sidecar with
  a stream reference gets a hint.
- Gates on 0e5f6da: `tools/l1_diff` fmt, clippy `-D warnings`, 22 unit + 7 integration tests
  pass. `l1-regression` passes in check mode (PQ and HLG synthetic clips, 5/5 scene cuts each);
  `tools/l1_diff/corpus` is unchanged, so export output without leading pictures is
  byte-identical.
- Real material, no new analysis: the three Blade Runner 2049 development cuts have v5 sidecars
  with 2 leading pictures (bitstream: CRA, then 2 RASL pictures, checked with ffmpeg
  `trace_headers`). With a stream-labelled export from this `l1_diff` and their shot lists in
  stream frames, scene cuts match 14/14, 5/5 and 7/7, the same counts as the original
  decoded-frame lists against our decoded scene starts.

Corpus convention (owner decision, 2026-10-06): truth shot lists count stream frames, like the
RPU. Four development lists that were on decoded frames (the three Blade Runner 2049 cuts and G4)
were moved to stream frames.

Still open, supporting evidence only: Joker (the cut with retail L1: 1448 stream frames, 2 RASL,
1446 decoded) and the Champions League cut (G4, 3 RASL) have only v4 runs. Scoring Joker's retail
L1 per frame needs a re-analysis with a v5 analyzer.

### 2026-10-06: P8, spec 4:2:0 decode in the analyzer

The analyzer now measures HLG max-RGB through the spec's 4:2:0 structure (ETSI GS CCM 001
§5.4.2.3.3): the chroma MMR runs once per chroma sample on luma down-sampled with `[1 2 1]` across
two rows, in `code / 1023` f32 arithmetic (`spec-float`), and the composed chroma is upsampled
bilinearly at the stream's chroma location (left, also when unspecified, or top-left; other
locations warn and use left). CUDA runs the chroma pass in a second kernel, `compose_chroma`. New
sidecar names `dovi84-v3` (preset) and `dovi84-bt2100-v1-spec420` (bt2100); the old names are
legacy and re-analyzed; the sidecar stays at version 5. mkvdovi requires the analyzer's `--help` to
name the selected composer's mapping, preset included.

**Gate** (`scripts/validate_hlg_chroma_siting.sh <analyzer +cuda from #29> --gate --cuts
~/mkvdovi-work/corpus/dev`): exit 0, "P8 gate ok", wall 2:34:33. A NaN hole in the script's checks,
found in review, was fixed afterwards in the same PR, with two more check fixes from the Codex
review; the saved outputs re-evaluated with the fixed checks give the same verdicts.

- Synthetic clips (256×256 patterns, left and top-left siting; a letterbox clip with 32-px bars),
  both composers, CPU and CUDA: analyzer against the tool's `spec-float` at its own siting, worst
  |max| 0.000 and |avg| 0.000 codes over 13 frames; per pixel 0 of 851,968 differ, and inside the
  detected crop of the letterbox clip (256×192 at 0,32) 0 of 638,976. CPU and CUDA identical
  (`.bin` bytes, sidecar frames/scenes/light_level, frame statistics). Anchor 2 (tool renderer
  against libplacebo bilinear) within its 4-code tolerance.
- Five HLG cuts, both composers, CPU and CUDA (`gpu: true` on every CUDA sidecar), worst |max| /
  |avg| per frame against `spec-float` at the cut's own siting:

| Cut | Siting | Frames | Worst \|max\| / \|avg\| | CPU = CUDA |
|-----|--------|--------|------------------|-----------|
| d11 Blue Lights S01E02 | left | 2976 | 0.000 / 0.000 | identical |
| g1 Wimbledon 2024 final | top-left | 3051 | 0.000 / 0.000 | identical |
| g2 Glastonbury 2025 | top-left | 1550 | 0.000 / 0.000 | identical |
| g3 The Green Planet | left | 1526 | 0.000 / 0.000 | identical |
| g4 UCL Bayern-PSG (open GOP) | left | 3033 decoded (stream_frames 3036, 3 leading skipped) | 0.000 / 0.000 | identical |

- `HDR_ANALYZER_CUDA_HOST_FRAMES=1` (CUDA analyzes frames downloaded from NVDEC; every CUDA run
  printed that it took this path), g2, the synthetic clips, both composers: "P8 gate ok", 0.000 codes
  worst |max| and |avg| over 1550 frames, per pixel 0 differ, CPU = CUDA identical.

Per-scene diagnostics, variant minus new analyzer, 12-bit codes, largest |difference| over scenes
(`spec-float-nearest` = spec structure with nearest-neighbour upsampling; `spec-fixed` =
`code / 1024`):

| Cut | Composer | `spec-float-nearest` L1 max / avg | `spec-fixed` L1 max / avg |
|-----|----------|------|------|
| d11 | bt2100 | 9.86 / 0.06 | 4.71 / 2.25 |
| d11 | preset | 9.57 / 0.18 | 5.37 / 1.77 |
| g1 | bt2100 | 13.45 / 0.05 | 4.94 / 3.14 |
| g1 | preset | 22.73 / 0.08 | 6.09 / 3.22 |
| g2 | bt2100 | 0.00 / 0.35 | 0.00 / 4.63 |
| g2 | preset | 0.00 / 0.36 | 0.00 / 3.36 |
| g3 | bt2100 | 11.87 / 0.60 | 5.02 / 1.02 |
| g3 | preset | 3.10 / 0.66 | 5.35 / 1.36 |
| g4 | bt2100 | 0.00 / 0.15 | 0.00 / 2.14 |
| g4 | preset | 0.00 / 0.14 | 0.00 / 2.44 |

g2 and g4 sit at the 3079 source-max clamp in every scene, so their L1 max cannot move.

**What it does to L1** (main 71fd03d against this branch; CUDA, `--no-crop --downscale 1
--disable-optimizer`; sidecar per-scene values, new minus old, 12-bit codes; scene boundaries
identical):

| Cut | Composer | Scenes | L1 max: max abs (mean) | avg max-RGB: max abs (mean) | avg luma | min | MaxCLL old → new |
|-----|----------|--------|------------------|------------------|----------|-----|-------------------|
| d11 | bt2100 | 32 | 9 (−0.38) | 0 | 0 | 0 | 905 → 903 nits |
| d11 | preset | 32 | 11 (−0.44) | 1 (−0.16) | 0 | 0 | 925 → 925 |
| g1 | bt2100 | 22 | 15 (−1.41) | 0 | 0 | 1 (−0.09) | 1001 → 1001 |
| g1 | preset | 22 | 19 (−2.18) | 1 (−0.14) | 0 | 1 (−0.14) | 1001 → 1001 |

Luma averages do not move (the luma path is unchanged). Scene L1 max moves by up to 19 codes,
mostly down.

**Other checks.**

- `scripts/cuda-parity.sh`: PASS.
- PQ unchanged: `.bin` and sidecar (timestamps removed) byte-identical between main (71fd03d) and
  this branch on 16 configurations (CPU/CUDA × crop/no-crop × downscale 1/2 × max-RGB/luma) of a
  134-frame 4K PQ clip.
- L1 regression reference: only `tools/l1_diff/corpus/hlg.reference.csv` shot 1 (frames 61–89)
  average 1360 → 1359 (#29).
- mkvdovi by hand: a pre-P8 analyzer is refused up front; a legacy-name sidecar is re-analyzed with
  the "pre-spec 4:2:0" message; `--verify` passes; the source is kept.
- CUDA throughput (RTX 4070, WSL2; `--hwaccel cuda --no-crop --disable-optimizer --transfer hlg`,
  frames ÷ wall time, best of 2 interleaved runs): −0.03% to −1.12% on g1 and d11, both composers,
  downscale 1 and 2 (table in [`CUDA_PIPELINE.md`](CUDA_PIPELINE.md)). NVDEC-bound. Below the 10%
  follow-up threshold; no follow-up item.

Where: [#29](https://github.com/tinof/hdr-analyze/pull/29); [`HLG_COMPOSER.md`](HLG_COMPOSER.md) §9

### 2026-10-06: P8, spec arithmetic measured; decode target `spec-float`

Step 0 of the P8 decode change measured the `bt2100` composer's section 6 criteria
([`HLG_COMPOSER.md`](HLG_COMPOSER.md)) through the spec composer's fixed-point arithmetic, on flat
fields (`fit_hlg_composer report --spec-fixed`). Two deciding criteria fail, so the planned
`spec-fixed` target was dropped by owner decision.

- `bt2100` under `spec-fixed`: neutral R′G′B′ spread 5.891 codes (float decode 0.005), neutral luma
  error 3.57 codes at code 902 (0.64), ΔE_ITP 1.05 (0.11); superwhite 0.03 and the source clamp
  pass. Neutral composed chroma at code 721 is (32751, 32750), 17 LSB below 32768. The preset
  barely moves (spread 172 → 173 codes); it is tinted under either arithmetic.
- Cause: the normalization, not the 16-bit output. ETSI GS CCM 001 §5.4.2.3.2–3 (pp. 18–20) feeds a
  10-bit code into the polynomial and the MMR as `s << 10` on a 2^20 scale, i.e. `code / 1024`;
  the composer was fitted at `code / 1023` (`tools/fit_hlg_composer/src/model.rs`). About 5.78 of
  the 5.89 codes of spread are the moved neutral point, 0.1 the quantization; the luma error is
  entirely the moved variable.
- Checked three ways: the spec text read against `SpecComposer` term by term; an independent
  Python re-derivation from the constants (5.891 and 3.57 exactly); libplacebo, which scales the
  pivots by `1 / ((1 << bl_bit_depth) − 1)` and evaluates on texture values, i.e. `code / 1023`
  with no correction (`libav_internal.h:967-968`). No open-source decoder runs the spec's fixed
  point. CCM 001 defines only BT.1886 and PQ base layers, so spec arithmetic for an HLG base layer
  is an extrapolation.
- Decision: the analyzer takes the spec's structure (MMR at chroma resolution on down-sampled luma,
  bilinear upsampling at the stream's chroma location) with the `code / 1023` f32 arithmetic the
  composer was fitted for and libplacebo uses (`spec-float`). The structure is unambiguous and
  carried up to 17.4 codes of L1 max per scene; the arithmetic moved scene averages by 0.6 to 3.3.
  On flat fields `spec-float` equals the current decode, so section 6 holds unchanged. Which
  convention devices use is a WS6 question; a `code / 1024` decision would be a refit (new
  composer, new RPU).

Where: this file; `fit_hlg_composer report --spec-fixed`

### 2026-10-05: P8, 4:2:0 chroma of the HLG decode

The P8 step "4:2:0 chroma siting of the analyzer's decode against a renderer on non-flat
patterns" is done; it opens a decode change. Planning found that the reference is the spec
composer, not a renderer: ETSI GS CCM 001 v1.1.1 §5.4.2.3.3 down-samples the BL luma to the chroma
positions and runs the MMR at chroma resolution, in integer arithmetic with 10-bit inputs as
`code / 1024`. The analyzer replicates each chroma sample over its quad and reshapes it with each
pixel's own luma (`code / 1023`, f32); libplacebo upsamples chroma first and reshapes per pixel
(`code / 1023`). Tables and method: [`HLG_COMPOSER.md`](HLG_COMPOSER.md) §9.

- New `fit_hlg_composer chroma-siting` (analyzer, spec in the analyzer's arithmetic, spec in its
  fixed point, renderer; nearest or bilinear for chroma location left and top-left) and
  `scripts/validate_hlg_chroma_siting.sh` (synthetic edges, lines, 1×1 to 3×3 highlights, colour
  ramp, flat controls; optional real cuts). Anchors on every run: the tool reproduces the
  analyzer's per-frame maximum exactly and average within 0.5 code; its renderer variant matches
  libplacebo (`upscaler=bilinear`) within 0.32 codes per pixel (`bt2100`) and 3.84 (preset),
  including every pixel around the test highlights.
- Acceptance limit, set in plan mode: per-scene |analyzer − spec| ≤ 4 codes for L1 max and avg on
  every scene of the five HLG cuts, both composers. Measured (`bt2100` on every frame, preset on
  every 4th): L1 max up to 18.3 (`bt2100`) and 18.2 (preset), on Wimbledon; average up to 4.9 and
  3.5, on Glastonbury. Exceeded, so P8 gets the open step "spec 4:2:0 decode in the analyzer".
- The structure (MMR at chroma resolution) accounts for up to 17.4 codes of L1 max and below
  1 code of average; the arithmetic (`code / 1024`) for most of the average (−4.0 codes on flat
  red through `bt2100`, on every pixel). The display's upsampler of the composed chroma is not
  specified: replicated instead of bilinear gives 6.5 codes on Wimbledon instead of 18.3.
- Two of the five cuts (Glastonbury, Champions League) reach the 3079 clamp in every scene, so
  the L1 max evidence comes from three cuts. 18 codes of L1 max lies below the smallest WS7 point
  (+75 codes).
- Hypothesis, not tested: the PQ path replicates chroma over the quad too (`frame.rs`, the non-HLG
  branch). Its matrix is linear, so only colour edges differ, but saturated small highlights may
  contribute to the clean-source over-read on the MEL cuts (WS1).

Where: this file; [`HLG_COMPOSER.md`](HLG_COMPOSER.md) §9

### 2026-10-05: WS8 item 5 done; findings for P9, WS1, E7

Two deliveries (2026-10-04 and 2026-10-05) filled the per-format gaps. All cuts are stream copies
of about 60 s, checked on arrival with MediaInfo, `dovi_tool` and `hdr10plus_tool`, and remuxed
with mkvmerge (the containers carried the statistics tags of the full source). Each new PQ title
also gave two holdout cuts; 29 cuts are now held out, unanalyzed.

| Format | Title | What it adds |
|---|---|---|
| HDR10+ | Deadloch S01E01 (Amazon WEB-DL) | Profile B, 1000-nit master, two cuts (day, night) |
| HDR10+ | The Shining (disc) | DV MEL + HDR10+ Profile B, 4000-nit master; retail L1 |
| HDR10+ | Alien (disc) | Profile A with MaxSCL 0 on every frame |
| HDR10+ | Alita: Battle Angel (disc) | DV MEL + HDR10+ Profile A on one disc: the same-master pair; retail L1 |
| HLG | Wimbledon 2024 (iPlayer) | live sport, 50p, HLG only as the alternative transfer |
| HLG | Glastonbury 2025 (iPlayer) | concert, stage strobes |
| HLG | The Green Planet (iPlayer) | nature, saturated highlights |
| HLG | Champions League (broadcast capture) | 50p, starts at an open-GOP CRA (3036 pictures, 3033 decoded) |
| HDR10 / MEL | Joker: Folie à Deux (disc) | MEL, 1000-nit master; starts at a CRA with 2 RASL pictures |
| HDR10 / MEL | Skyfall (disc) | MEL with per-shot L1 (32 and 11 distinct values), no MaxCLL |
| HDR10 | Exodus: Gods and Kings (disc) | 1100-nit master, no MaxCLL |
| HDR10 | Star Trek (2009, disc) | 1000-nit master, no MaxCLL/MaxFALL |
| DV 8.1 | Inside Out 2 (Disney+) | animation; RPU shot list kept, static L1 not used |
| HDR10 | The Revenant ("Open Matte" hybrid) | bright snow; injected RPU and HDR10+ not used |

Not usable as references: The Revenant's RPU states a 0.0001/1000-nit master against 0.005/4000 in
its own L6 and the HDR10 base, and marks one shot in 60 s. Inside Out 2's L1 max is 2467 to 2471 on
all 21 shots. The Shining's two formats are not proven to come from one master; Alita's are.

First scores with the default estimator (per-shot peak against retail L1, bias in 12-bit codes):
Alita +162.3 (day) and +147.6 (night), Skyfall +88.4 and +161.8, The Shining +25.6 (night) and
−106.3 (day). Alita is digital and largely computer-generated, so the over-read on clean MEL
sources is not a grain effect (WS1).

HDR10+ Profile A (Alien, Alita) carries MaxSCL 0 on every frame, with the percentile distribution
and the average present (P9).

Before sidecar v5 (#26) every analyzer cut on the Joker cut sat exactly 2 frames before the
authored one (10 of 10) and the L1 comparison stopped on 1446 against 1448 frames; it is to be
rescored with a v5 build (E7).

Scene detection against the RPU and HDR10+ shot lists: Deadloch 13/13 and 20/21, Alita 15/17 and
17/17, Skyfall 27/34 and 10/10, Inside Out 2 20/20, Alien 12/18, The Shining 2/7 (day). Shot lists
made by the acquisition agent (HLG, plain HDR10) are unchecked; two of them (the football cut, the
Red Sea cut) list 2 cuts where the analyzer finds 8 and 25.

Where: this file

### 2026-10-05: WS8, P9, P8, coverage of real material

The development tier was reviewed per input format. Results:
- HDR10: 8 cuts plus 9 held out, from two titles, all mastered at 0.005 / 4000 nits, with shot
  lists but no authored L1.
- HDR10+: one cut (11 scenes).
- HLG: one drama cut, with no shot list or reference.
- Synthetic and open-content pairs: 7 entries.
- The only real-content L1 reference comes from the Dolby Vision titles (Profile 7 FEL and MEL),
  whose base layers are HDR10.

P9's gate (scored on the development tier) cannot be met with one HDR10+ cut, so HDR10+ material
now comes before P9. HLG material goes with the P8 playback test. The acquisition order is WS8
item 5.

Where: this file

### 2026-10-04: E7, open-GOP cuts

A dev-tier HDR10 cut failed `--verify` with 1443 measured frames against 1445 in the video.

**Cause.** The cut starts at a CRA picture (NAL type 21), followed by a complete RASL_R (9) and a
RASL_N (8) picture. FFmpeg skips both ("Skipping invalid undecodable NALU"). `dovi_tool` 2.3.4
`inject-rpu` assigns `rpus[presentation_number]` (`rpu_injector.rs`). `hevc_parser` sorts each GOP
by POC, so the two RASL pictures come first, and missing entries repeat the last RPU.

**Measurement.**
- Took 60 frames of the cut and an RPU with a distinct L1 max per frame.
- Matched the RPU bytes of each access unit in decode order, and mapped access units to displayed
  frames through the decoder's packet positions.
- Result: CRA→RPU 2, RASL_R→0, RASL_N→1. Displayed frame *k* carried RPU *k* + 2, so every scene's
  L1 was shown two frames early.

**Fix.**
- The analyzer counts the RASL pictures of the first IRAP from the packets' NAL types. It requires
  `decoded + leading == pictures` (packets that carry a slice; an end-of-sequence packet is not
  one) and errors on any other loss.
- Sidecar v5 records both numbers.
- `mkvdovi` shifts the shots and makes the RPU one entry per picture. `--verify` compares in stream
  frames.
- A measured RPU that `inject-rpu` reports as mismatched is refused.

**Results.**
- The cut now passes `--verify`. It reports 2 leading pictures and 1443 measured frames, and its
  MKV statistics tags are stale: MediaInfo reports the full film's 235152 frames.
- A new integration test cuts a synthetic open-GOP clip with `mkvmerge --split`, which leaves 3
  RASL pictures. It checks the displayed-frame-to-L1 association without `dovi_tool`'s parser. A
  second test checks that a stream starting after its CRA is refused and that its source is kept.

Where: [#26](https://github.com/tinof/hdr-analyze/pull/26)

### 2026-10-04: P10, E10

P10 steps 1 and 2. Profile 8.1 `extra.json` passes `source_min_pq` / `source_max_pq` as the PQ codes
of the mastering minimum and peak (`dolby_vision::utils::nits_to_pq_12_bit`, the crate's conversion
for a Dolby CM XML). Checked on `dovi_tool` 2.3.4: with the keys, a 0.05 / 600-nit master is
written as 189 / 2851; without them the lookup writes 0 / 3079. Standard masters (min 0.0001 or
0.005 nits; peak 1000, 2000, 4000 or 10000 nits) and the defaults give the same values as the
lookup, which a unit test checks against the crate's `source_meta_from_l6`. Profile 8.4 keeps
62 / 3079. `--verify` parses a generated RPU in-process. It checks the source range on every frame,
and checks that every frame of every sidecar scene carries the measured L1 after the generator's
clamp (`ExtMetadataBlockLevel1::from_stats_cm_version` with CM v2.9). It reports per field how many
scenes were clamped and the largest change, and counts scenes whose L1 max lies above
`source_max_pq`. E10: the generator converts with ×4095 like the analyzer; the retail-RPU check
against the ×4096 of ETSI GS CCM 001 is still open.

Where: [#25](https://github.com/tinof/hdr-analyze/pull/25)

### 2026-10-03: FEL (replaces F1, F2, R1)

Profile 7 FEL compositor and its re-encode removed; FEL inputs are refused with a pointer to the
plan. Measured against the reconstruction in ETSI GS CCM 001 on real base-layer pixels: seven of
eight Profile 7 FEL test cuts use order-3 MMR chroma on every frame, and there the composed chroma
missed the specified prediction by a mean of 69 to 534 10-bit codes (mostly clipped to 0 or 1023),
while the specified prediction stays within 0.2 to 2.8 codes of the base layer. Pivots were used as
absolute values although they are delta-coded, the polynomial constant and the MMR constant and
cross terms were scaled wrongly, and the unit tests encoded the same convention. One cut with
identity mapping was unaffected.

Where: [`docs/FEL_PLAN.md`](FEL_PLAN.md)

### 2026-10-03: P8–P11, E8–E10, WS7, WS8; P0, P1, P7, WS1, WS2, WS4, E4

Review of all conversion paths against public specifications, the generator's code and output, and
the retail RPUs of the test cuts. Priorities reordered: defects that make delivered output wrong for
every file of an input type first, then evaluation in displayed-picture units, then measured
improvements, then research. New items: HLG composer tint, HDR10+ L1 average, generator clamps and
source range, histogram percentile reader, `--mdfix` and authored trims, display-mapping simulator,
test-material fixes. Statements that the measured L1 minimum is delivered were corrected.

Where: this roadmap; [`docs/CM_ANALYZE_PARITY.md`](CM_ANALYZE_PARITY.md)

### 2026-10-02: WS1, WS2

Robust peak estimator replaced by a rule on the shape of the histogram top (centre and width of the
top population, grain variance removed from the width, correction limited to 5.2 sigma; flat tops
and detached groups kept; aware of the 10-bit code grid). Same kernel statistics, CPU and CUDA
identical, default unchanged. Flat highlights and flashes far above the picture: 0 codes lost on 12
synthetic segments (old rule: 14 to 246). Three clean/grain pairs against the clean raw maximum:
bias +102.7 → +23.4, +41.0 → +17.0, +83.3 → +50.5; mean absolute error 108 → 60, 41 → 19, 85 → 71.
Grainy retail cut against embedded L1: +76.1 → +54.4. Not met: one to three pixel highlights near
the grain are lowered by up to 5.2 sigma, up to 27% of grainy frames read below the clean twin,
clean content moves 3 to 21 codes per frame, in-shot variation is 10% above `max`, three cuts with
embedded L1 move further below it. Shot aggregation unchanged. Stays opt-in.

Where: [#22](https://github.com/tinof/hdr-analyze/pull/22)

### 2026-10-02: WS1

`--peak-estimator robust` runs in the CUDA kernel with output identical to the CPU (kernel fills the
cross-quad difference histogram; CPU max-RGB mix and histogram binning moved to the kernel's f32
arithmetic). No algorithm change. A real-content run now takes the same time as the default
estimator, so estimator candidates can be scored in minutes. First scores of the unchanged
estimator: clean/grain pair bias +102.7 → −1.1 codes per frame, but the largest error grows (251 →
333) and shot-to-shot spikes get larger; one grainy retail cut +76.1 → +64.0 against its embedded
L1.

Where: [#21](https://github.com/tinof/hdr-analyze/pull/21)

### 2026-10-02: WS2, E1

Scene detection rebuilt: cuts chosen after analysis from a flash-tolerant score and the local
frame-to-frame level, strongest candidate first. Authored shot lists of ten real-content cuts: 142
of 168 cuts matched with 13 extra (before: 91 matched, 228 extra). The fixed threshold was saturated
by grain on about half of all frames; picture type played no role. Open: pictures that change
completely on every frame. The CI clip now has unequal shot lengths and a constructed scene
reference.

Where: [#20](https://github.com/tinof/hdr-analyze/pull/20)

### 2026-10-01: WS1, WS2

Measurement study for the grain-robust peak started: how isolated the raw peak is on the available
clips, spatial-support and temporal candidates, CUDA feasibility. Blocker for promotion: the two
grainy titles and their `cm_analyze` v2 output from [VALIDATION.md §7](VALIDATION.md) are not on the
development host.

Where: in progress

### 2026-10-01: WS2

Scene averages from unfiltered frame means (sidecar version 4). `mkvdovi` regenerates a stale RPU on
resume.

Where: [#18](https://github.com/tinof/hdr-analyze/pull/18)

### 2026-10-01: P0, WS6

Final RPU checked against the sidecar on real HDR10 and HLG clips; 0.5.1 final-RPU baselines
captured for five clips.

Where: [#17](https://github.com/tinof/hdr-analyze/pull/17)

### 2026-10-01: E1, WS0

`l1_diff` limits, L1 regression gate in CI, CPU/CUDA parity check, final-RPU baseline script. The
gate's first CI run exposed a lossy test-clip encode on newer FFmpeg; fixed.

Where: [#17](https://github.com/tinof/hdr-analyze/pull/17)

## Item history moved from ROADMAP.md on 2026-10-29

Closed history that sat inside `ROADMAP.md` until roadmap step E18.1 (hot-file budget): the old
Status lines, notes on finished steps, dated measurements and orders whose steps are closed.
Verbatim, apart from relative links rewritten for this directory. Each item's new Status in
`ROADMAP.md` points here.

### Priorities (from ROADMAP.md)

- **Landed before this order:** dependable HDR10/HDR10+ → Profile 8.1 with source-derived L1, L5,
  provenance, completeness checks and the input contract (P0, P1, P3, P7, E6, E7; code 2026-09-15;
  source retention reviewed and the default, delete a non-DV source after success, **kept by
  decision**); regression gates (E1, WS0; 2026-10-01); unbiased scene averages (WS2; 2026-10-01).
  The final-RPU regression corpus and the written Shield/TV playback procedure (WS6) grow alongside
  items 1 to 3, which each need a playback check.

- Priority openings as they read until 2026-10-29 (priority 1 then named the closed WS6.5 and
  WS6.6; priority 11 had no E18). Priority 1, first line:
1. **HLG → Profile 8.4 composer** (P8, WS6.9, WS6.5, WS6.6). `--hlg-composer bt2100` and the input refusal landed

  Priority 11:
11. **Engineering backlog** (E13, E14, E12, E1, E2, E3, E6, E11, E17, P3, P5, WS0, WS6): the
    remaining E13 PRs (E13.6–E13.21), the E14 test audit, CUDA release binaries (E12.1, after
    v0.7.0), CI and corpus gates (E1, WS0, WS6.1), crop and L5 per scene (E2, P3), debug options,
    provenance, open-GOP scoring and the HDR10+ extract fallback (E3, E6, E11, P5), `--verify`
    checks for authored CM v4.0 blocks (E17). Ranked below 1–10; added
    2026-10-18 so `/ship:roadmap-next` can reach them (owner). Gate: per item.

### P1: CPU analysis default (from ROADMAP.md)

- **Status:** Partial. Source-honest generation is the default. CUDA is the main pipeline;
  `accurate` (downscale 1, sample-rate 1) is the only parity-checked measurement. On CUDA,
  `balanced`/`fast` sample with a stride and the CPU resizes, so those presets differ by backend and
  no gate covers them ([`docs/CUDA_PIPELINE.md`](CUDA_PIPELINE.md)). They also save no time on
  CUDA, where the run is NVDEC-bound, and they lose the measured MaxCLL (`fast` also MaxFALL).

### P3: L5 from the crop (from ROADMAP.md)

- **Status:** Core complete. L5 offsets come from the committed full-resolution crop (sidecar v2);
  sampled source L5 keeps precedence for Dolby Vision inputs.

### P8: HLG → Profile 8.4 composer (from ROADMAP.md)

- **Status:** Core complete; default bt2100; playback confirmed on an LG OLED C9 (2026-10-13).
- `--hlg-composer bt2100` (2026-10-03) is fitted to the BT.2100 / BT.2408 1000-nit HLG-to-PQ
  conversion and keeps neutrals neutral; the HLG colour input contract is enforced. Design and
  measurements: [`docs/HLG_COMPOSER.md`](HLG_COMPOSER.md). The preset it replaces tints
  neutrals (75% grey ΔE_ITP 6.8, up to 10.3 near black) and decodes nominal white to about
  1150 nits.

### P9: HDR10+ → Profile 8.1 L1 (from ROADMAP.md)

- **Status:** Partial; step 0 (frame alignment) done 2026-10-08; scene list chosen 2026-10-13; the
  hybrid flag done 2026-10-14 (opt-in); peak parity done 2026-10-14.
- `dovi_tool generate --hdr10plus-json` takes L1 from the first frame of each HDR10+ scene
  (minimum 0, average rounded to whole nits, no measured crop). On the HDR10+ test cut the average
  reads 156 to 505 codes above the analyzer's mean; the cause is not separated.
- **Prerequisite met (2026-10-05):** four HDR10+ titles from two studios and one streamer
  (WS8 item 5). Alita: Battle Angel carries Dolby Vision MEL and HDR10+ on the same disc, so its
  retail L1 can score an HDR10+-derived L1 frame by frame.

### P10: measured against delivered (from ROADMAP.md)

- **Status:** Partial. Steps 1 and 2 landed 2026-10-04.

### P12: `--mdfix` ignores the source RPU's L6 (from ROADMAP.md)

- **Status:** Open; defect found 2026-10-15, not fixed.

### WS0: L1 scoring tools (from ROADMAP.md)

- **Status:** Core complete. `tools/l1_diff`, synthetic ground-truth tests, embedded-L1
  comparison and licensed `cm_analyze` scoring have shipped. `l1_diff` takes limits
  (`--max-peak-bias`, `--max-peak-error` and the same for minimum and max-RGB average) and exits
  nonzero on a breach; CI runs it on a generated six-shot PQ and HLG clip against committed
  references.

### WS2: shots and scene averages (from ROADMAP.md)

- **Status:** Partial. The shot maximum is the maximum of its frame peaks. Scene averages are
  unbiased since sidecar v4 ([VALIDATION.md §9](VALIDATION.md)); the `cm_analyze` v2 average
  comparison has not been repeated for v4. Cuts are chosen after analysis (2026-10-02). Retail RPUs
  change L1 only at shot starts, so shot-constant L1 is kept.

### WS5: CM metadata XML export (from ROADMAP.md)

- **Status:** Deferred; conditional. Worthwhile only if the external Profile 5 authoring route (R2)
  is chosen.

### WS6: final-RPU corpus and playback procedure (from ROADMAP.md)

- **Status:** Open. WS6.3 and WS6.4 closed (PR #66), WS6.8 closed (PR #78), WS6.5 and WS6.6 closed
  (PR #80; the `code / 1023` question is answered for the C9). The slope ladder was
  built and handed off on 2026-10-18 (8 files; the [log](ROADMAP_LOG.md) has the design and
  checks). Viewed on the C9 on 2026-10-21 (WS6.4):
  outcome "anything else" in the pre-registered table, so WS6.5 as written cannot start. The C9
  follows the curve's shape at every slope up to 20.46, but the neutral moves to higher BL
  chroma codes as the slope grows (about +2.5 codes at gain 2.5, +5–6.5 at gain 5, more than
  +12 at gain 10), independent of the constant and the term size. A follow-up probe that
  measures this shift comes before any `code / 1023` canary ([log](ROADMAP_LOG.md)
  2026-10-21). The probe is WS6.8 (build) and WS6.9 (viewing or measurement), owner 2026-10-22.
  WS6.8 was built and handed off on 2026-10-27 ([log](ROADMAP_LOG.md)): metered with the
  owner's i1Display Pro (owner decision), 13 files (gain-0 reference; arm A centre 912 at gains 0.25–10; arm B centre 160 at
  gains 0.25–2.5; control), 15 runs in about 40 minutes, picture mode Dolby Vision Cinema. The
  pre-registered analysis passed nine synthetic devices and the libplacebo check (outcome H1023:
  every Cb and Cr crossing within 0.03 code of `code / 1023` against a 0.1-code gate; both arm
  intercepts within 0.003 code). Design and checks: `~/mkvdovi-work/ws6-neutral-probe-2026-10-27/`
  (`PREREG.md`, `run.sh`, `render/analysis.txt`); method in [`docs/PLAYBACK.md`](PLAYBACK.md)
  "Metered tests". The owner then chose to read WS6.9 by eye (2026-10-29); every metered test
  moved to a separate later list. Viewed by eye on 2026-10-29 in two passes (Plex on the
  Shield, Dolby Vision Cinema, badge on every file): no large shift at codes 912 and 160 (pass 1,
  without hints: at gain 10 grey within about ±2 codes of the centre, where the WS6.4 law
  predicts +12), so the WS6.4 shift belongs to the codes near 944 (at 944, gain 5 puts grey at
  ≥ 950.5 and gain 10 shows no grey in 928–960). Pre-registered decision on pass 1 alone:
  undecided by eye, and WS6.9's gate (±0.5 code per gain) is not met (pass 1 reaches ±0.5 only at
  gains 1 and 2.5 on arm A, ±2 elsewhere). The pass-2 fine readings repeat the same patch
  positions across gains and arms after a guide hint, so they carry no sub-code conclusion.
  WS6.9 stays open as the metered run. Evidence:
  `~/mkvdovi-work/ws6-neutral-eye-2026-10-29/` (`RESULT.md`, `PREREG.md`, `answers_pass*.json`).
  WS6.5 and WS6.6 (2026-10-29): a blind forced-choice canary at code 912 (v2; an Opus skeptic
  replaced v1 before any viewing), seven pages at gains 5, 10 and −10 with real-RPU-range
  coefficients, mirrored pages, a gain-0 page and a texture catch. Every decisive answer (24 of
  24) fell on the `code / 1023` side and matched libplacebo, with no side or tint bias.
  Pre-registered decision: `code / 1023` holds, the bt2100 fit stays; gate (3) is met for the C9.
  Evidence: `~/mkvdovi-work/ws6-canary-2026-10-29/` (`RESULT.md`, `PREREG.md`, `answers.json`,
  `score.txt`, `render/check.txt`).
- The `code / 1023` question below: the first canary gave no verdict (2026-10-17, below); next, a
  canary within the coefficient range real RPUs use, preceded by a slope ladder that shows how
  steep a chroma curve the C9 still follows. Claude builds each, checks it in libplacebo and hands
  the files to the owner (owner decision 2026-10-17); the canary is designed from what the ladder
  showed, so build and viewing alternate (WS6.3 to WS6.6). The ladder showed a gain-dependent
  neutral shift, so the neutral-finder probe (WS6.8, WS6.9) comes before the canary.
- **First canary (2026-10-17): no verdict.** On the C9 (Plex on the Shield, Dolby Vision Cinema
  Home, DV badge on all three files) both steep canaries (chroma coefficients 3.73 on u, u², u³
  and a constant of about −9.3) showed the same uniform green on all seven patches, where either
  normalization predicts a green-to-magenta row with one grey patch; the bt2100 control showed
  the predicted purple. So the TV applies these composers but does not follow the steep curve:
  its composed chroma comes out flat and low, as if clamped. This is the guide's "anything else"
  row. Not recorded: TV firmware, room light. Reread 2026-10-21 (WS6.4): not a slope limit; the
  C9 follows steep curves, but its neutral moves with the gain.
- **P8 result (2026-10-13):** an LG OLED C9 applies the 8.4 composer in the RPU (canary method in
  [log](ROADMAP_LOG.md)). Not recorded for that test: firmware, picture mode, HDMI path. The
  preset's grey tint looked yellow on the C9, where the libplacebo SDR render predicted lavender
  (mid greys) and olive (dark greys).

### WS7: evaluation in displayed-picture units (from ROADMAP.md)

- **Status:** Open.

### FEL: Profile 7 FEL (from ROADMAP.md)

- **Status:** Deferred; placeholder, on hold by the owner (2026-10-13). The BL+EL compositor and
  its re-encode were removed on 2026-10-03 because the composed picture did not match ETSI GS CCM
  001. `mkvdovi` refuses Profile 7 FEL inputs, keeps the source and continues with the next file.
- FEL_PLAN gate 3 (does the player apply a non-identity 8.1 mapping) is partly answered on the
  local F2 track: clip 1 (2026-10-07, Plex on the Shield → C9) showed a non-identity 8.1 luma
  mapping applied. The chroma MMR and L1 clips (F2 clips 3 and 4) stay held with the rest of FEL.
  A fixed-pixel 8.1 chroma canary like the P8 one needs new code: `tools/fit_hlg_composer
  canary-rpu` accepts only `bt2100` Profile 8.4 RPUs.

### E1: regression gates (from ROADMAP.md)

- **Status:** Core complete. `tools/l1_diff` runs in CI (`scripts/ci/l1-regression-gate.sh`,
  references in `tools/l1_diff/corpus`); `scripts/cuda-parity.sh` checks CPU against CUDA on a GPU
  host; `scripts/rpu-baseline.sh` captures and compares final RPUs. Synthetic accuracy runs in
  workspace CI. Since 2026-10-06 the CI lint job runs clippy on the analyzer with `--features cuda`
  (no GPU or toolkit; first hosted run passed on #31).
- Step 1, expand the corpus (feeds WS6), is the same work with the same gate as WS0.1 and is
  tracked there.

### E2: crop detection (from ROADMAP.md)

- **Status:** Partial. Seven-position crop probing, low-signal rejection, modal voting and
  variable-AR union shipped in PR [#4](https://github.com/tinof/hdr-analyze/pull/4), closing issue
  [#3](https://github.com/tinof/hdr-analyze/issues/3).

### E3: `mkvdovi` debug options (from ROADMAP.md)

- **Status:** Open.

### E4: second scene signal (from ROADMAP.md)

- **Status:** Open. `--scene-metric hybrid` is a histogram-only placeholder. Missed cuts on
  whole-picture motion (WS2) need a signal that explains motion; both candidates are sensitive to
  flashes, exposure changes and low texture. Full optical-flow fusion stays deferred.

### E5: hardware decode contexts (from ROADMAP.md)

- **Status:** Deferred. Add proper VAAPI and VideoToolbox decode device contexts and hardware-frame
  transfer.

### E7: completeness (from ROADMAP.md)

- **Status:** Partial. `--verify` fails when the RPU frame count differs from the muxed video track
  or the L1 sidecar, and warns when output and input frame counts differ.
- Since 2026-10-04 the analyzer accounts for every picture of the stream, so measured L1 lines up
  with the picture. It counts the undecodable RASL pictures at the start of an open-GOP cut, and
  any other loss is an error. A measured RPU whose length differs from the video is refused at
  inject.

### E8: histogram percentile reader (from ROADMAP.md)

- **Status:** Open. The 256-bin luma histogram is split into 64 bins below PQ(100 nits) and 192
  above, but `compute_histogram_percentile_pq` and `find_highlight_knee_nits`
  (`hdr_analyzer_mvp/src/analysis/histogram.rs`) convert a bin index back with bin/255, so a
  1000-nit value reads as about 305 nits. It reaches the output only with
  `--peak-source histogram99/histogram999` or `--peak-domain luma` with a non-conservative
  optimizer profile.

### E9: `--pre-denoise` values (from ROADMAP.md)

- **Status:** Open. `--pre-denoise` accepts any string and its help text names `nlmeans`, but only
  `median3` is implemented; every other value silently does nothing and is still written into the
  sidecar provenance.

### E11: `l1_diff` and open-GOP cuts (from ROADMAP.md)

- **Status:** Core complete 2026-10-06 (`CHANGELOG.md` `[0.6.0]`). `tools/l1_diff` shifts our
  frames by `source.leading_skipped_frames` (sidecar v5) when the reference covers the stream
  frames, re-bases `--per-shot` and `--scenes`, and labels `--export-reference` rows with stream
  frames. Any other count difference is still refused; no measurements are invented for the skipped
  pictures. Evidence: [`docs/ROADMAP_LOG.md`](ROADMAP_LOG.md) (2026-10-06).

### E12: CUDA release binaries (from ROADMAP.md)

- **Status:** Open. `release.yml` builds without `--features cuda`, so release users never get GPU
  analysis, and `auto` resolves to CPU `balanced` even on an NVIDIA host.

### E13: module size refactor (from ROADMAP.md)

- **Status:** Partial; step 2 (safety net) done 2026-10-07; step 3 (mkvdovi split) done 2026-10-07,
  evidence in the PR body and `~/mkvdovi-work/rpu-baseline/e13-step3-281b927/proof/`; step 4
  (`convert_file` by phase) done 2026-10-13, evidence in
  `~/mkvdovi-work/rpu-baseline/e13-step4-d7c080d/`; step 5 (analyzer split) done 2026-10-17
  (#59, efbb948), evidence in `~/mkvdovi-work/rpu-baseline/e13-step5/`. No
  production file is over the 800-line limit in CLAUDE.md ("Module size") any more; E13.11 done
  2026-10-29 (PR #79); E13.6–E13.10 and E13.12–E13.21 open. Since E13.11 the limits are in `.claude/workflow.md` "## Size limits" (ship plugin 1.5.0):
  `clippy::too_many_lines` at 150 fails the gate, and each function over it carries an
  `#[expect(…, reason = "E13.<n>: split by phase")]` naming its step below.
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
- Step 6 (split the long functions by phase, each a behaviour-neutral PR, not a move) is split
  into one step per PR: E13.6 keeps the number; the chore PR, the function that waits for E8.1 and
  the other two functions took the next free numbers (2026-10-17, step format).
- **Found by the step 2 tests:** two L6 defects under P6 (priority 4), fixed 2026-10-11.
- E13 Gate, first line as it read until 2026-10-29 (steps 3-5 are done; the rest of the Gate is
  unchanged in `ROADMAP.md`):
- **Gate:** per move (acceptance gate for steps 3-5): fmt, clippy (also `--features cuda`), tests;

### E14: test-suite audit (from ROADMAP.md)

- **Status:** Partial. Rules: [`docs/TESTING.md`](TESTING.md). Step 1 done (#54, CI run
  38753171952, only allowed skips). Step 2 done: 3 of the pilot's tests red on main, nothing new
  found, pipeline wiring missed; the owner dropped a separate test author (2026-10-17): in-session
  tests plus the E14.6 mutation gate stay the rule. Steps 3 and 4 done on main@c74effb for `verify.rs` and the riskiest
  modules only (owner choice): 336 survivors, 0 actual bugs; missing tests in 19 high-risk groups
  (57 mutants), 17 medium (115) and 14 low (43); 121 print-only or equivalent. Step 5: high-risk
  groups done (#55, 0 missed after); medium and low groups open. Step 3 not yet run on 15 mkvdovi
  files (878 mutants) and the analyzer contract files (273; counts from `cargo mutants --list` at
  main@6f92306).
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
     and the cost. Done; dropped as a rule by the owner (2026-10-17).

- E14 order, entry 1, as it read until 2026-10-29 (v0.7.0 shipped 2026-10-18 and E13 step 5
  merged in #59):
  1. After the v0.7.0 release PR (due about 2026-10-21). Steps 3 and 4 (mutation runs, triage)
     are local, in a detached worktree, and need no branch, so they can run at any time. Steps 5
     and 6 are PRs: they open only when no E13 move PR is in flight (the E13 sequencing rule).
     E13 step 5 keeps the first branch-free slot, so E14 PRs go between its move PRs or after it.

### E15: abort-safe RPU parsing (from ROADMAP.md)

- **Status:** Open. `mkvdovi` parses source RPUs in-process with the `dolby_vision` crate in
  `rpu_check::classify_rpu_format` (format detection, before the FEL refusal: the Profile 7 MEL/FEL
  split, and the RPU sniff for inputs MediaInfo does not report as Dolby Vision; Profile 8 is
  classified from the MediaInfo text without a parse; also `inspect`), `analyze_rpu_file`
  (`inspect`) and `sample_rpu_windows` (step 1 of every Dolby Vision conversion, after the FEL
  refusal: L5 offsets, the unreliable-metadata warning and the `--mdfix` notice). The crate
  (3.4.0) calls `unimplemented!` on a polynomial piece with `poly_order_minus1 == 0` and
  `linear_interp_flag` set (`src/rpu/rpu_data_mapping.rs:415`). Under the release profile's
  `panic = "abort"` that ends the process: no `Result` reaches the caller, a multi-file run stops,
  and a Profile 7 FEL input that carries such a piece is never refused cleanly. No such RPU was found on the FEL test cuts; a
  corrupt or unusual stream can carry one ([`docs/FEL_PLAN.md`](FEL_PLAN.md) section 6).
  `verify.rs` parses only RPUs that mkvdovi generated or rewrote with the same crate, whose writer
  panics on that piece, so it is outside this step.

### E18: hot-file budgets (from ROADMAP.md)

- **Status:** Open. `ROADMAP.md` is 72 KB, read on every `/ship:roadmap-next` and `/ship` run;
  `CLAUDE.md` is sent on every turn.

### E17: `--verify` checks for authored CM v4.0 blocks (from ROADMAP.md)

- **Status:** Open. `--verify` checks that the generated levels exist (L6, L9, L11, L254;
  `verify.rs` `assert_rpu_invariants`), not whether CM v4.0 blocks are consistent. RPUs that
  mkvdovi generates do not trigger these checks. Authored L8 and L10 blocks carried over from Dolby
  Vision inputs can. Prior art: the metadata rules of the CB1 engine of
  CroqueMr/libreelec-x86-DV (`CB1/docs/ALGORITHMS.md:68-72`, `CB1/src/dvbridge_creative.c:177,
  184` at commit 5e63e16; GPL-3.0, the rules only, no code copied).

## Item history moved from ROADMAP.md on 2026-10-26

Closed history that sat inside `ROADMAP.md` until the ship 1.4.0 hot-file rule (the roadmap holds
open work only): the one-line progress-log table, the Done and Fixed notes of each item, and
"Checked and kept". Verbatim, apart from relative links rewritten for this directory and "this file"
in three table rows, which now links `ROADMAP.md`.

### Progress-log table (from ROADMAP.md)

Newest first. One line per step that changed the state of a roadmap item. The full text is in
this log; details are in the pull request and in
[`CHANGELOG.md`](../CHANGELOG.md).

| Date | Step | Items | Where |
|------|------|-------|-------|
| 2026-10-23 | E16.2: `RpuBuilder::new` refuses every value a later write would (L2 count, L5, L6, L9, L11, 8.1 source range; plus an empty or inverted range) as `Error::OutOfRange`, with a backstop write; `Error::code`, `Level6::clamped`; docs on `panic = "abort"`, L3/L254 and HDR10+ seeks (NuvioTV fork review); RPU bytes unchanged; E16 closed. | E16 | [#68](https://github.com/tinof/hdr-analyze/pull/68) |
| 2026-10-22 | E16.1: `dovi_rpu_builder`, in-process RPU builders for the NuvioTV fork (DV2.1): 8.1 and 8.4 (preset, bt2100, canary), HDR10+ → L1, static 8.4 (avg 1283, owner); `mkvdovi` RPUs identical to 0.7.0 on all nine baselines; builders = `dovi_tool` 2.3.4 on every fixture and 64/64 HDR10+ corpus runs; E16 closed. | E16 | [#67](https://github.com/tinof/hdr-analyze/pull/67) |
| 2026-10-22 | WS6.3 slope ladder built and handed off (2026-10-18); WS6.4 viewed on the C9 (2026-10-21): shape followed up to slope 20.46, neutral moves with gain, outcome "anything else". Neutral-finder probe WS6.8/WS6.9 added; WS6.9 may answer the `code / 1023` question (owner). | WS6 | [#66](https://github.com/tinof/hdr-analyze/pull/66) |
| 2026-10-20 | AetherEngine's FEL playback read against FEL_PLAN: FEL_PLAN gains prior art and an EL siting table; E15 (abort-safe RPU parsing) added. | FEL, E15 | [#65](https://github.com/tinof/hdr-analyze/pull/65) |
| 2026-10-18 | v0.7.0 released (#63): P7, P9, P11, P13; no rc. CUDA parity PASS and all nine RPUs byte-identical to 0.6.0 before the tag (only the P11 notice lines differ in the `--mdfix` logs). P12.1 moves to the next patch; P9.3 and E12.1 can start. | P7, P9, P11, P13 | [v0.7.0](https://github.com/tinof/hdr-analyze/releases/tag/v0.7.0) |
| 2026-10-17 | C9 viewings: P10 step 4 closed (blind A/B, no difference, `ms_weight` 2048 kept); first WS6 `code / 1023` canary no verdict (uniform green, slope ladder next); WS6 (2) playback procedure written ([`docs/PLAYBACK.md`](PLAYBACK.md)) and corrected. | P10, WS6 | `docs/owner-decisions-2026-10-17` |
| 2026-10-17 | P9 holdout scoring: the pre-registered rule passes on the four holdout cuts (average clamped MAE 276.6 → 33.8 above the floor, 315.9 → 25.2 over all frames; max equal; min 5.98 → 4.91); the owner flips the default after v0.7.0. P6 check list drafted, five picked; WS6 canary and P10 `ms_weight` A/B built and handed off. | P9, P6, WS6, P10 | `docs/owner-decisions-2026-10-17` |
| 2026-10-17 | Owner decisions: P9 holdout rule pre-registered, P13 keeps v29 without L2 (closed), E14 step 2 dropped, no second P8 device, E12 bundles NVRTC, P10 waits for WS7, gates stated for every `TODO(owner)` step. | P3–P6, P8–P10, P13, WS0–WS8, FEL, E1–E7, E10–E12, E14 | `docs/owner-decisions-2026-10-17` |
| 2026-10-17 | E13 step 5: analyzer split, `frame.rs`, `gpu.rs` and `pipeline.rs` into module directories by pure move; 12 optimizer-enabled CPU and CUDA runs byte-identical to main@941489d, CUDA parity PASS. | E13 | [#59](https://github.com/tinof/hdr-analyze/pull/59) |
| 2026-10-15 | E14 step 5, first PR: tests for the 19 high-risk survivor groups (composer, `--verify`, sidecar/RPU/HLG probes, pipeline delivery steps); 0 of the targeted mutants missed. Steps 1, 2 and 3–4 (partial) recorded. | E14 | `test/e14-high-risk` |
| 2026-10-15 | P13: `--cm-version v29` writes a CM v2.9 RPU (L1, L5, L6) and `--verify` checks the requested CM version; CM v4.0 RPUs identical to 0.6.0. | P13 | `fix-p13-cm-v29` |
| 2026-10-15 | P11 notice: `--mdfix` names the authored levels it replaces or drops, from the sampled source RPU windows; r4/r5 `--mdfix` RPUs identical to 0.6.0; two defects found (P12, P13). | P11, P12, P13 | `feat/p11-mdfix-notice` |
| 2026-10-14 | P9 peak parity: under `histogram99`, `max-scl` and `max-scl-luminance` the hybrid max equals `dovi_tool`'s on every non-fallback frame of six cuts (0 mismatches), fallbacks carry the analyzer scene's max, `--boost` equals `histogram99`. | P9 | `docs/p9-peak-parity` |
| 2026-10-14 | P9 hybrid flag: opt-in `--hdr10plus-l1 hybrid` (union pieces, HDR10+ peak, measured average and minimum); default RPUs byte-identical to 0.6.0, Alita pooled clamped MAE 37.58 / 18.91 as pre-registered. | P9 | `feat/p9-hdr10plus-hybrid` |
| 2026-10-14 | P7: mkvdovi refuses PQ input tagged full range before any temp work (HDR10, HDR10+, `--mdfix`), including a full-range stream the container overrides as limited; 9/9 RPU baselines byte-identical to 0.6.0. | P7 | `fix-p7-full-range-pq` |
| 2026-10-13 | E13 step 4: `convert_file` split by phase into `pipeline/{temp_dir,measure,config,deliver}.rs`, `mod.rs` 1037 to 409 lines, 27 new unit tests; all 9 RPU baselines byte-identical to 0.6.0. | E13 | [log](ROADMAP_LOG.md) |
| 2026-10-13 | P9 scene list for the pixel average: pixel average 19 codes from retail L1 (Alita, pooled, clamped) against 338 for today's HDR10+ average; HDR10+ scenes, analyzer cuts and their union within about 1.5 codes; the union carries it by the pre-registered rule. | P9 | [log](ROADMAP_LOG.md) |
| 2026-10-13 | v0.6.0 released (#46): #41 and #43 on top of rc.1; CUDA parity and all ten RPU baselines identical before the tag. | P6, E7, P8 | [v0.6.0](https://github.com/tinof/hdr-analyze/releases/tag/v0.6.0) |
| 2026-10-13 | P8 playback test: on an LG OLED C9 (Plex on the TV and on a Shield) a green canary composer renders green and the preset's grey tint is visible, bt2100 neutral; bt2100 stays the default. | P8, WS6 | [log](ROADMAP_LOG.md) |
| 2026-10-11 | L6 fixes: MaxCLL/MaxFALL read from the stream behind a container 0, `Details.txt` thousands separators, `--verify` warns on L6 0 in a passed-through MEL RPU; r4 MEL default baseline captured. | P6, E7, E13 | `fix-l6-zero-light-levels` |
| 2026-10-08 | P9 step 0: HDR10+ scene starts line up with the picture (offset 0 on six cuts, v5 runs, mkvdovi extraction path); 24% of frames in HDR10+ scenes holding several shots. | P9 | [log](ROADMAP_LOG.md) |
| 2026-10-07 | E13 step 3: pure-move split of mkvdovi `metadata.rs` and `pipeline.rs` into submodules, tests in `tests.rs`; all 8 RPU baselines identical. | E13 | `feat/e13-mkvdovi-split` |
| 2026-10-07 | E13 safety net: 34 characterization tests for mkvdovi (metadata, pipeline, HLG source deletion), CUDA gate on module dirs, final-RPU baselines at main@e6d63ed; no production code changed. | E13 | this branch |
| 2026-10-06 | CUDA as main pipeline: warn on `balanced`/`fast` with GPU, re-analyze coarser sidecars under `accurate`, warn on CPU-analyzed CUDA run; CI lints `cuda`. | P1, E1 | [#31](https://github.com/tinof/hdr-analyze/pull/31) |
| 2026-10-06 | Review with CUDA as the main pipeline: `accurate` is the only parity-checked preset; release binaries and hosted CI never build the `cuda` feature; P1, E1, E2 amended, E12 opened. | P1, E1, E2, E12 | [`ROADMAP.md`](../ROADMAP.md) |
| 2026-10-06 | `l1_diff` lines references up with open-GOP cuts by `leading_skipped_frames`; exports are labelled in stream frames. | E11 | [#30](https://github.com/tinof/hdr-analyze/pull/30) |
| 2026-10-06 | Analyzer measures HLG max-RGB through the spec 4:2:0 decode (`spec-float`); gate passed at 0.000 codes, CPU = CUDA; new sidecar names, older sidecars re-analyzed. | P8 | [#29](https://github.com/tinof/hdr-analyze/pull/29) |
| 2026-10-06 | Spec fixed-point arithmetic measured: `bt2100` is not neutral at `code / 1024` (spread 5.9 codes); the decode change targets the spec structure in `code / 1023` (`spec-float`). | P8, WS6 | [log](ROADMAP_LOG.md) |
| 2026-10-05 | 4:2:0 chroma of the HLG decode measured against the spec composer and libplacebo: the analyzer differs from the spec by up to 18 codes of L1 max per scene; spec decode step opened. | P8 | [log](ROADMAP_LOG.md) |
| 2026-10-05 | Real material per format added: four HDR10+ titles (one same-master DV MEL + HDR10+ pair), four HLG cuts, six HDR10/MEL titles; 29 holdout cuts. | WS8, P9, P8, WS1 | [log](ROADMAP_LOG.md) |
| 2026-10-05 | Coverage of real material per format reviewed; HDR10+ and HLG material comes before P9 and P8 work. | WS8, P9, P8 | [`ROADMAP.md`](../ROADMAP.md) |
| 2026-10-04 | L1 lines up with the picture in open-GOP cuts (RASL leading pictures); sidecar v5. | E7 | [#26](https://github.com/tinof/hdr-analyze/pull/26) |
| 2026-10-04 | 8.1 source range from the mastering display; `--verify` compares delivered with measured L1. | P10, E10 | [#25](https://github.com/tinof/hdr-analyze/pull/25) |
| 2026-10-03 | FEL compositor and re-encode removed; FEL inputs refused. | FEL | [FEL_PLAN](FEL_PLAN.md) |
| 2026-10-03 | Review of all conversion paths; priorities reordered; new items. | P8–P11, E8–E10, WS7, WS8 | [`ROADMAP.md`](../ROADMAP.md) |
| 2026-10-02 | Robust peak estimator reads the shape of the histogram top. | WS1, WS2 | [#22](https://github.com/tinof/hdr-analyze/pull/22) |
| 2026-10-02 | `--peak-estimator robust` in the CUDA kernel, identical to CPU. | WS1 | [#21](https://github.com/tinof/hdr-analyze/pull/21) |
| 2026-10-02 | Scene cuts chosen after analysis; 142 of 168 authored cuts matched. | WS2, E1 | [#20](https://github.com/tinof/hdr-analyze/pull/20) |
| 2026-10-01 | Grain-robust peak measurement study started. | WS1, WS2 | in progress |
| 2026-10-01 | Unfiltered scene averages (sidecar v4); stale RPU regenerated on resume. | WS2 | [#18](https://github.com/tinof/hdr-analyze/pull/18) |
| 2026-10-01 | Final RPU checked against the sidecar; 0.5.1 RPU baselines captured. | P0, WS6 | [#17](https://github.com/tinof/hdr-analyze/pull/17) |
| 2026-10-01 | `l1_diff` limits, L1 gate in CI, CUDA parity check, RPU baseline script. | E1, WS0 | [#17](https://github.com/tinof/hdr-analyze/pull/17) |

### P1: CPU analysis default

- **Done 2026-10-06** (former open items 2–4; evidence: unit tests `coarser_sampling`,
  `rejects_coarser_sidecar`, `analysis_quality_notice`, `gpu_analysis_missing` in
  `mkvdovi/src/pipeline/tests.rs`, e2e test `accurate_reanalyzes_coarser_measurements` in
  `mkvdovi/tests/hlg_profile84.rs`): mkvdovi warns on an explicit `balanced`/`fast` with GPU
  analysis; under `accurate` it re-analyzes a coarser sidecar instead of reusing it; it warns when a
  run that expected GPU analysis comes back with `analysis.gpu: false`.

### P6: source consistency warnings

- Step 1 (the check list) done 2026-10-17 (list in [log](ROADMAP_LOG.md) 2026-10-17).
- Fixed 2026-10-11: a container MaxCLL/MaxFALL of 0 no longer hides the stream's value
  (`track_light_level` reads `MaxCLL_Original`/`MaxFALL_Original`; a non-zero container value
  still wins), and `Details.txt` thousands separators are read (`parse_details_number`).

### P8: HLG → Profile 8.4 composer

- **Done 2026-10-13: playback test.** An LG OLED C9 (Plex on the TV, and Plex on an Nvidia Shield TV Pro) shows a
  canary composer (bt2100 with both chroma MMR constants shifted, `fit_hlg_composer canary-rpu`)
  fully green, with the Dolby Vision badge, on a byte-identical HLG base layer: the display applies
  the composer in the RPU. The preset's tint is visible on greys (the owner saw yellow), bt2100 is
  neutral. bt2100 stays the default. Not covered: devices other than the C9, and the WS6
  `code / 1023` question. Evidence: [log](ROADMAP_LOG.md) 2026-10-13.
- **Done 2026-10-06: spec 4:2:0 decode in the analyzer** (both composers;
  [#29](https://github.com/tinof/hdr-analyze/pull/29)). Max-RGB runs the chroma MMR at chroma resolution on down-sampled
  luma in `code / 1023` f32 (`spec-float`; `code / 1024` leaves `bt2100` 5.9 codes off neutral) and
  upsamples the composed chroma bilinearly at the stream's chroma location; CPU and CUDA identical.
  New sidecar names `dovi84-v3` / `dovi84-bt2100-v1-spec420`; older sidecars are re-analyzed, and
  mkvdovi refuses an analyzer whose `--help` does not name the mapping. Gate
  (`scripts/validate_hlg_chroma_siting.sh --gate`): 0.000 codes on the synthetic patterns and the
  five cuts. Scene L1 max moves by up to 19 codes on the two cuts compared. Evidence: [log](ROADMAP_LOG.md) 2026-10-06.

### P9: HDR10+ → Profile 8.1 L1

- **Done 2026-10-17: step 1, holdout scoring.** The pre-registered rule passes on every clause
  (four holdout cuts, 0 fallbacks, all eight runs `--verify` passed). The minimum passes only
  pooled: hybrid is worse on Alita (retail 0) and better on The Shining (retail 12). Evidence:
  `~/mkvdovi-work/p9-holdout-2026-10-17/` ([log](ROADMAP_LOG.md) 2026-10-17).
- **Done 2026-10-08: step 0, HDR10+ scene boundaries against the picture.** HDR10+ frame n is
  picture frame n: offset 0 on all six multi-scene HDR10+ cuts (five by the scan, one by
  corroboration), no confirmed off-by-one (Alien 298 ambiguous), v5 runs and mkvdovi's extraction path
  ([log](ROADMAP_LOG.md) 2026-10-08). The scene list is not a shot
  list: 24% of the frames sit in HDR10+ scenes holding more than one shot (Alita, The Shining), and
  some HDR10+ starts split a shot (Alien).
- **Done 2026-10-13: which scene list carries the pixel average.** The union of HDR10+ starts and
  the analyzer's cuts, by the pre-registered rule; the HDR10+ scenes, the analyzer's cuts and the
  union score within about 1.5 codes of each other (pooled clamped MAE above the floor 38.3 / 36.8 /
  37.6), and the analyzer's cuts are the numerical best. The gain is the measured average itself:
  pooled clamped MAE over all frames 18.9 codes against 338.4 for today's HDR10+ path, which writes
  1229 where retail sits at the 819 floor. The 18.9 holds after the generator's 819 floor; raw, the
  measured average is 147.6 codes off over all frames (about 259 below on floor frames). Two cuts from one title; peak and minimum not
  evaluated ([log](ROADMAP_LOG.md) 2026-10-13).
- **Done 2026-10-14: the hybrid flag.** `--hdr10plus-l1 hybrid` (opt-in, default unchanged):
  union pieces, HDR10+ peak (`--peak-source`, whole nits like `dovi_tool`) with a reported fallback
  to the analyzer scene peak, measured average and minimum. Default path 9/9 RPUs byte-identical to
  0.6.0; the four HDR10+-only cuts (h1, h5) convert with `--verify` passing and 0 fallbacks. On
  DV-free remuxes of the Alita and The Shining cuts (`accurate`): 0 L1 mismatches against the
  pre-registered rule, max equal to `dovi_tool`'s HDR10+ path on every frame, Alita pooled clamped
  MAE of the average 37.58 above floor / 18.91 all frames (pre-registered 37.6 / 18.9). Evidence:
  `~/mkvdovi-work/p9-hybrid-2026-10-14/` ([log](ROADMAP_LOG.md) 2026-10-14).
- **Done 2026-10-14: peak parity for the other `--peak-source` values.** `histogram99`, `max-scl`
  and `max-scl-luminance`: the hybrid max equals `dovi_tool`'s on every non-fallback frame of six
  cuts; fallbacks (all Profile A pieces under `max-scl*`) carry the analyzer scene's max; `--boost`
  equals `histogram99`. Evidence: `~/mkvdovi-work/p9-peak-parity-2026-10-14/`
  ([log](ROADMAP_LOG.md) 2026-10-14). The unit test `hdr10plus_peak_sources_follow_dovi_tool`
  pins the mirror, not parity: re-run that evidence's `run.sh` after a `dovi_tool` bump.

### P10: measured against delivered

- **Done:**
  - (1) Profile 8.1 RPUs carry `source_min_pq` / `source_max_pq` converted from the mastering
    display, instead of the generator's L6 lookup. Implausible mastering values (peak outside
    100–10000 nits, minimum above 1 nit) keep the lookup, with a warning.
  - (2) `--verify` checks the source range and the clamped measured L1 on every frame of a
    generated RPU, and reports per field how many scenes the generator's limits changed.
- **Done 2026-10-17: step 4, `ms_weight` A/B.** Blind on the C9 (Plex on the Shield, Dolby
  Vision Cinema Home, DV badge on all four files): no difference on either Fury Road cut, so
  generated L2 keeps `ms_weight` 2048. Retail RPUs in the development corpus carry 512, 0 and 2048
  (three titles each). Evidence: `~/mkvdovi-work/p10-ms-weight-2026-10-17/`
  ([log](ROADMAP_LOG.md) 2026-10-17).

### WS6: final-RPU corpus and playback procedure

- **Done 2026-10-17: (2) the written playback procedure,** [`docs/PLAYBACK.md`](PLAYBACK.md),
  used for the WS6 and P10 viewings and corrected afterwards with what they showed (owner asked
  Claude to make the corrections).

### WS8: test material

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
  Measurements and what was rejected as a reference: [`docs/ROADMAP_LOG.md`](ROADMAP_LOG.md).

### E7: completeness

- Fixed 2026-10-11: `--verify` warns, not fails, on L6 MaxCLL/MaxFALL 0 in a passed-through
  Profile 7 MEL RPU (`delivery` is `None`); generated RPUs, missing or malformed fields and MaxDML 0
  still fail.

### E13: module size refactor

- **Done 2026-10-17: step 5, analyzer split** (pure move, 7 commits). `analysis/frame.rs` →
  `frame/{mod,peak}.rs` (peak estimators and grain statistics in `peak.rs`), `analysis/gpu.rs` →
  `gpu/{mod,backend}.rs` (the `cuda`-gated backend; `include_str!("../kernels.cu")`), `pipeline.rs`
  → `pipeline/{mod,analyze,crop_space,scenes}.rs`; test modules in files under their old names.
  Gates: `verify_move.py` PURE MOVE OK per commit, the fmt commit reproduced, 125 analyzer test names
  identical, fmt/clippy (also `cuda`)/tests pass (only allowed skips), the L1 gate without
  `--update`, CUDA parity PASS, and 12 optimizer-enabled runs (synthetic PQ/HLG, d1, g3 bt2100 and
  preset, r1; CPU and CUDA, `gpu: true`) byte-identical `.bin` and sidecar to main@941489d
  ([log](ROADMAP_LOG.md) 2026-10-17).

### Checked and kept (2026-10-03)


The review re-examined these decisions and found no evidence to change them:

- **Max-RGB as the L1 domain** and **the L1 average as the mean of PQ max-RGB**, unfiltered.
- **The 819 average floor** (CM v2.9): retail averages sit at exactly 819 on 24 of 32 and 25 of 35
  shots of two retail cuts.
- **Shot-constant L1**: all nine retail RPUs change L1 only at shot starts.
- **Neutral L2 for analysis output.**
- **Nearest-neighbour chroma** for max-RGB: within about 10 codes of native 4:2:0 ingest
  ([VALIDATION.md §7](VALIDATION.md)). A correctly sited reconstruction has not been compared
  on saturated edges; reopen only with such a measurement.
- **Stream copy for MEL and HLG.** With the FEL compositor gone, no path encodes video.

## Item detail as of 2026-10-03

The full roadmap rows before they were shortened on 2026-10-04, verbatim.

### Priority table

- **1** (P8, P7)
  - Work: HLG → Profile 8.4 (`--hlg-composer bt2100` and the input refusal landed 2026-10-03; bt2100
    made the default by owner decision before the playback test, which the owner runs with it):
    composer that keeps neutrals neutral, fitted to the BT.2100 / BT.2408 1000-nit HLG-to-PQ
    conversion. Refuse full-range or non-BT.2020 HLG instead of warning.
  - Evidence: The fixed preset decodes 75% grey to R′G′B′ 2384/2387/2439 in 12-bit PQ codes (blue 52
    codes high, ΔE_ITP 6.8) and reaches ΔE_ITP 10.3 near black. Its `ycc_to_rgb` and `rgb_to_lms`
    matrices are plain BT.2020, so nothing compensates. Nominal peak white decodes to about 1150
    nits and L1 clamps it to 1000. Every HLG output is affected on a display that applies the
    composer.
  - Gate: Default since 2026-10-03 by owner decision. Whether devices accept a mapping that is not
    the preset is unverified; the WS6 playback test decides whether it stays (`--hlg-composer
    preset` is the fallback).
- **2** (P9)
  - Work: HDR10+ → Profile 8.1: hybrid mode with the scene list and peak from HDR10+ and the
    average, minimum and crop from pixels.
  - Evidence: `dovi_tool` takes L1 from the first frame of each HDR10+ scene: average = PQ of a
    linear-light mean rounded to whole nits, minimum 0, no measured L5. On the HDR10+ test cut it
    reads 156 to 505 codes (median 264) above the analyzer's mean of PQ max-RGB in all 11 scenes.
    The cause is not isolated: PQ concavity and the producer's own measurement are not separated.
  - Gate: Opt-in; scored on the development tier before any default change.
- **3** (P10)
  - Work: Deliver what was measured: pass `source_min_pq` / `source_max_pq` explicitly, record
    measured against delivered values, decide whether to write L1 in-process to avoid the
    generator's clamp.
  - Evidence: `dovi_tool generate` writes an L1 minimum above 12 codes as 12 (135 of 435 test
    scenes), a maximum below 2081 as 2081 (24 of 435) and an average below 819 as 819. It maps a
    mastering peak other than 1000/2000/4000/10000 nits to 1000 and a mastering minimum other than ≤
    0.001 or exactly 0.005 nits to 0. Retail RPUs carry L1 minima above 12 codes (up to 251).
  - Gate: The source-range fix has no open question. Writing unclamped L1 needs WS7 or a playback
    test to show it is at least as good.
- **4** (E8, E9, P1, E10)
  - Work: Code defects with narrow reach: histogram percentile reader; `--pre-denoise` values;
    half-resolution CPU default; L1 scaling note.
  - Evidence: The percentile reader returns bin/255 on a histogram split into 64 + 192 bins, so a
    1000-nit value reads as about 305 nits; it reaches the RPU only with `--peak-source
    histogram99/999` or `--peak-domain luma` with a non-conservative profile. `--pre-denoise`
    advertises `nlmeans`, but any value other than `median3` does nothing. CPU-only hosts default to
    half-resolution `FAST_BILINEAR` analysis, which averages small highlights; the effect is
    unmeasured.
  - Gate: Reader and option fixes: unit tests. CPU default: measure first.
- **5** (P11)
  - Work: `--mdfix` replaces the whole RPU: decide between a targeted repair that keeps unaffected
    levels and full regeneration.
  - Evidence: Authored L2 and L4 are replaced by neutral L2 and no L4. Authored trims are far from
    neutral: up to 928 codes on slope and 1189 on power at the 100-nit target on one MEL title; L4
    is on every frame of all nine retail RPUs. Against a targeted repair: trims were authored
    against the authored L1 and shot list.
  - Gate: Open decision.
- **6** (WS7)
  - Work: Evaluation in displayed-picture units: a histogram-domain display-mapping simulator, then
    ColorVideoVDP on a few cuts.
  - Evidence: Recomputed with open curves for a 1000-nit shot on a 1000-nit panel: L1 max +75 / +140
    codes costs 14.6% / 24.4% at the peak with libplacebo's spline and 2.5% / 4.5% with the ITU-R
    BT.2390 EETF, and nothing when L1 max stays below the panel peak. Reading low clips highlights.
  - Gate: Simulator output reproduces these numbers. It does not model a Dolby display (limits under
    WS7).
- **7** (WS8)
  - Work: Test material: grain on the synthetic highlights and unaligned highlight positions, a
    grainy twin from the same master, transitions.
  - Evidence: The synthetic highlight pair writes its highlights after the grain and aligned to the
    chroma quad; the grainy twin comes from a different master (about ±50 codes); no test cut
    contains a dissolve or a fade.
  - Gate: Needed before 9 can be scored.
- **8** (WS2, E4)
  - Work: Scene detection: second signal for whole-picture motion; strong cuts closer than 12
    frames.
  - Evidence: Missed cuts dominate on two whole-picture-motion cuts: 7 detected against 32 authored
    shots, 10 against 36. Eight authored shots are shorter than 12 frames.
  - Gate: Precision and recall against authored shot lists, no loss on the other cuts.
- **9** (WS1, WS2, WS4)
  - Work: Research, default unchanged: grain-robust peak; per-frame L1 inside detected transitions;
    L4; automatic trims.
  - Evidence: No spatial statistic passes on the clean/grainy twins (WS1). All nine retail RPUs are
    shot-constant in L1 (WS2). Nothing can validate L4 or trims in the open (WS7).
  - Gate: Each stays opt-in or unbuilt until its gate in the tables below is met.
- **10** (FEL)
  - Work: Profile 7 FEL: **placeholder.** The compositor was removed on 2026-10-03 and FEL inputs
    are refused. A design that delivers FEL information without re-encoding is tracked in
    [`docs/FEL_PLAN.md`](FEL_PLAN.md).
  - Evidence: See the progress log.
  - Gate: Set in the plan.
- **Conditional** (R2, WS5)
  - Work: Profile 5 through an established encoder, only if matched playback tests justify it

### P0 (Core complete)

Measured per-scene L1 (minimum, max-RGB mean, maximum) reaches the RPU as explicit `dovi_tool
generate` shots and bypasses optimizer targets. A missing, invalid, or mismatched sidecar re-runs
analysis. The old `--madvr-file --use-custom-targets` generation, where L1 max follows optimizer
`target_pq` and L1 avg is a placeholder, is reachable only through `--legacy-madvr-l1`. Checked
2026-10-01 on the final RPU of one real HDR10 clip (33 scenes), three real HLG clips (96 scenes) and
the synthetic HDR10 clip: every scene's L1 equals the sidecar after the generator's limits (minimum
at most 12, maximum at least 2081, average at least 819). These limits are rules of `dovi_tool` and
the `dolby_vision` crate, not of ETSI GS CCM 001; what they do to the measured values is P10.

### P1 (Partial)

Source-honest generation is the default. Open: decide whether full-resolution every-frame analysis
becomes the CPU default. Today `auto` resolves to `accurate` only with CUDA analysis and to
`balanced` otherwise; measure the CPU cost first. `balanced` downsizes with `FAST_BILINEAR` at half
resolution, which averages small highlights, where CUDA samples full-resolution pixels with a
stride; the size of the effect on L1 max is unmeasured (2026-10-03) and should be measured before
any more elaborate peak estimator. `fast` also copies each measurement into two skipped frames, so
cuts land on a 3-frame grid and peaks on skipped frames are missed.

### P8 (Landed; default bt2100; playback test open)

HLG → Profile 8.4 composer. 2026-10-03: `--hlg-composer bt2100` implemented as designed in
[`docs/HLG_COMPOSER.md`](HLG_COMPOSER.md) (neutral spread 0.005 of a 12-bit code, nominal white 1000
nits, colour-patch ΔE_ITP 11.8 against 26.1 in libplacebo's render; HLG colour input contract
enforced). Default switched to `bt2100` the same day by owner decision; the WS6 playback test still
has to show that devices apply it (`--hlg-composer preset` is the fallback). Background before the
change: `mkvdovi` wrote the preset in every HLG output. 4:2:0 chroma siting measured 2026-10-05
against the spec composer: up to 18 codes of L1 max per scene, so the analyzer decode is to move to
the spec's (progress log, 2026-10-05). Background: the fixed,
phone-derived preset of the `dolby_vision` crate. Its chroma MMR tints neutrals; recomputed from the
preset coefficients through the 8.4 decode (12-bit PQ codes R′/G′/B′, ΔE_ITP against a neutral of
the same luminance): code 200 → 879/868/793, 10.3; code 502 → 1819/1813/1851, 4.5; code 721 (75%) →
2384/2387/2439, 6.8; code 940 → 3134/3143/3155, about 1150 nits luminance. The preset's `ycc_to_rgb`
and `rgb_to_lms` are plain BT.2020, so the tint is not compensated later in the chain. In luma the
preset is close to the reference (75% HLG decodes to 208 nits against 203 in BT.2408). Plan: fit the
same mapping syntax (8-piece luma polynomial, order-3 chroma MMR) to the public BT.2100 / BT.2408
1000-nit HLG-to-PQ conversion with neutrals preserved and nominal white at 1000 nits; weight the low
end, where the tint is largest; fit colour behaviour, not only the grey ramp. The emitted RPU and
the analyzer's decode must change together, with a new `luminance_mapping` value in the sidecar.
Opt-in first. Unverified: whether playback devices apply a composer that is not the preset, and how
visible the tint is on a TV (nothing was measured on a display); both need the WS6 playback test.

### P9 (Open)

HDR10+ → Profile 8.1 L1. `mkvdovi` never analyzes HDR10+ inputs; `dovi_tool generate
--hdr10plus-json` takes L1 from the first frame of each HDR10+ scene, with minimum 0, average = PQ
of the linear-light mean of max-RGB rounded to whole nits, and no measured crop. On the HDR10+ test
cut (11 scenes, no letterbox) this average is 156 to 505 codes (median 264) above the analyzer's
mean of PQ max-RGB. The rounding alone moves a 0.7-nit scene by 71 codes. How much of the gap is PQ
concavity and how much the producer's own measurement is not separated; the sidecar has no
linear-light mean to test it. Plan: opt-in hybrid mode (HDR10+ scene list and peak; average, minimum
and L5 from pixels), scored on the development tier before any default change; a pixel fallback for
missing or implausible HDR10+ statistics. The panel peak is still not passed as a trim target, and
suspicious scene peaks still only warn.

### P10 (Open)

Measured against delivered. `dovi_tool generate` (2.3.4, checked on its output) clamps each L1: a
minimum above 12 codes (0.00026 nits) becomes 12, a maximum below 2081 (100 nits) becomes 2081, an
average below 819 (2.43 nits) becomes 819; `mkvdovi` requests the CM v2.9 average floor, otherwise
it would be 1229. Of 435 test scenes, 135 have a measured minimum above 12 and 24 a maximum below
2081. Retail RPUs carry minima above 12 (up to 251 codes), so the minimum cap is the generator's,
not a rule every deliverable obeys. For Profile 8.1 the generator derives `source_min_pq` /
`source_max_pq` from L6 through a lookup: a mastering peak other than 1000/2000/4000/10000 nits
gives 3079 (1000 nits), a mastering minimum other than ≤ 0.001 or exactly 0.005 nits gives 0;
`mkvdovi` does not pass the two fields. Plan: (1) pass `source_min_pq` / `source_max_pq` explicitly
from the mastering metadata; (2) record measured and delivered L1 per scene and report the
difference in `--verify`; (3) decide whether to write L1 in-process (the `dolby_vision` crate is
already a dependency) so the measured minimum and sub-100-nit maxima reach the RPU. Step 3 needs
evidence from WS7 or playback; raising the average floor to 1229 is not planned. Also open:
generated L2 carries `ms_weight` 2048 where the retail RPUs carry 512 or 0; the effect is
unverified.

### P11 (Open; decision)

`--mdfix` on Profile 7 MEL and Profile 8 regenerates the whole RPU from a clean base layer, so
authored L2 and L4 are replaced by neutral L2 and no L4, authored L5 is reduced to one sampled value
and L6 comes from the container. Authored trims in the retail RPUs are far from neutral: at the
100-nit target slope 1120 to 1384 and power 859 to 1149 against the neutral 2048 on one MEL title,
typically 100 to 400 codes on the others; L4 is present on every frame of all nine. The open
decision: a targeted repair that replaces only the defective level and keeps the others (`dovi_tool`
can carry the display metadata over unchanged), against the finding that trims were authored for the
authored L1 and shot list (the analyzer's average read 24 to 169 codes lower on one cut and its
segmentation differed by up to 4.6× in shot count), so new L1 under old trims applies a correction
to a curve it was not made for. Until decided, `--mdfix` should state in its output which authored
levels it drops.

### WS1 (Partial)

Measurement core, per-scene minimum passed to the generator (which then clamps it, P10), and the
max-RGB-mean average domain have shipped. Open: a grain-robust peak. Raw max-RGB reads +92.6 / +74.4
codes hot against `cm_analyze` v2; the first opt-in robust estimator reached +80.4 / +66.4 and
missed its promotion gate ([VALIDATION.md §7](VALIDATION.md)). It also lowered flat highlights and
was replaced on 2026-10-02 by a rule on the shape of the histogram top ([TECHNICAL_REFERENCE.md
§2.4](TECHNICAL_REFERENCE.md)), which keeps flat highlights far above the picture and removes 39 to
77% of the per-frame bias of three clean/grain pairs (17 to 55% of the mean absolute error), but
lowers small highlights near the grain by up to 5.2 sigma, lowers clean content by 3 to 21 codes per
frame, raises in-shot variation by 10% and leaves a grainy retail cut at +54 against its embedded
L1; it has not been scored against `cm_analyze`. Three histogram-only designs (tail shape, sigma
estimate, shot aggregation) were tried on one development corpus, and none separated grain from
clean detail or an isolated grain pixel from a small specular. Spatial statistics were measured on
2026-10-03 on the clean/grainy twins (median and range of the error against the clean raw maximum,
12 frames): raw +112 (−58 to +216); 3x3 sliding minimum −22 (−338 to +90), losing up to 314 codes on
clean frames; `[1 3 3 1]/8` prefilter +22.5 (−198 to +117), losing up to 157; quad-aligned 2x2
minimum +23 (−155 to +148), losing up to 99. Compared in the same statistic, they remove 55 to 70%
of the grain excess. The 3x3 minimum and the prefilter read a 2x2 highlight more than 1000 codes
low; the 2x2 minimum keeps it only because the test highlights are aligned to the chroma quad (WS8).
On grain correlated over two pixels a 3x3 minimum still leaves about 3 sigma in simulation. None
passes, and the 3x3 window and the prefilter should not be built as estimators on these numbers.
Public prior art for spatial smoothing before L1 is WO2021247670A1; it does not validate any of
these. The 5.2-sigma reach of the current rule is set for white noise; the simulated max-RGB excess
at 8 megapixels is 5.8 sigma. Research track: next is better test material (WS8) and the
display-unit scale (WS7), which shows that an over-read costs nothing while L1 max stays below the
panel peak; do not enable the robust estimator by default. The new estimator must run on CPU and
CUDA with identical output (integer counts, fixed-point sums, order-independent reductions; see
[`docs/CUDA_PIPELINE.md`](CUDA_PIPELINE.md)), as the current `robust` estimator does since
2026-10-02 (`median3` is still CPU-only). Its frozen synthetic gate must include what temporal
persistence can wrongly remove: a small specular, a one-frame flash, a highlight on the first or
last frame of a shot, a 2–3 frame specular and a fade. HLG max-RGB shipped in v0.5.0. Also open:
true target-gamut peaks.

### WS2 (Partial)

Initial shot aggregation shipped: the shot maximum is the maximum of its frame peaks, so one
retained grain spike can set a whole shot. A pooled two-level shot rule and a 4-pixel support floor
were prototyped on 2026-10-02 and not adopted: the floor erases 1 to 3 pixel highlights and moves
clean shot peaks by 7 to 58 codes, and the pooled rule lowers highlights within about two sigma of
the shot maximum. Scene averages are unbiased since sidecar version 4 (2026-10-01): they are the
mean of unfiltered frame means; before, a forward-only EMA made a 24-frame fade read 893 codes
instead of 1569 ([VALIDATION.md §9](VALIDATION.md)). The `cm_analyze` v2 average comparison has not
been repeated for version 4. Scene boundaries (2026-10-02): the detector chooses cuts after analysis
from a flash-tolerant score against the local frame-to-frame level; 142 of 168 authored cuts matched
with 13 extra on ten real-content cuts. Checked 2026-10-03 against the nine retail RPUs: L1 changes
only at shot starts (0 changes elsewhere), so shot-constant L1 is kept. L4 is present on every
frame; its anchor correlates with the analyzer's per-frame average at r = 0.94 to 0.99 in eight RPUs
and 0.71 in one, equally well with the luma average, so it does not decide the average domain. In 9
of 18 test cuts the analyzer's scenes leave fewer frames more than 100 codes from their segment mean
than the authored shots do; missed cuts dominate only on two whole-picture-motion cuts (7 scenes
against 32 authored shots, 10 against 36). Open, in this order: (1) a second scene signal for those
pictures (E4) and strong cuts closer than `--min-scene-length` 12 (eight authored shots in three
cuts are shorter), by a confidence exception, not a lower global minimum; (2) research, opt-in:
per-frame L1 inside detected dissolves and fades through the generator's per-frame edits, which
needs a per-frame peak in the sidecar (a version bump) and transition test material (WS8), and can
itself cause pumping; (3) L4 emission, deferred until something can validate it (no open renderer
reads L4); (4) robust aggregation (investigated with WS1).

### WS6 (Open)

Final-RPU regression corpus covering grain, saturated highlights, raised blacks, fades, flashes,
rapid cuts, and changing aspect ratios. Checks run on the RPU extracted from the muxed file. Add a
Shield/TV playback procedure comparing matched material from the same master, recording player,
firmware, TV picture mode, and HDMI path. The procedure is the gate for P8 (does the device apply a
fitted 8.4 composer) and for the default changes of P9 and P10.

### WS7 (Open)

Evaluation in displayed-picture units. (1) A histogram-domain display-mapping simulator in a tool
crate outside the workspace: per-frame PQ histograms and an L1 stream go through libplacebo's
spline, the SMPTE ST 2094-10 curve and the ITU-R BT.2390 EETF for 100-, 600- and 1000-nit panels,
and candidate metadata is compared with reference metadata as change in displayed luminance, clipped
highlight share and frame-to-frame steps. It needs a per-frame histogram dump from the analyzer
(integer counts, identical on CPU and CUDA). (2) ColorVideoVDP on rendered output of a few cuts.
Recomputed 2026-10-03 for a 1000-nit shot on a 1000-nit panel (change at 10 / 100 / 200 nits /
peak): spline, L1 max +75 codes −3.5 / −5.9 / −7.8 / −14.6%, +140 codes −6.5 / −10.4 / −13.6 /
−24.4%; BT.2390 EETF −2.5% / −4.5% at the peak and nothing below 200 nits; a shot whose L1 max stays
below the panel peak is unchanged; L1 max 100 codes low on a 600-nit panel brightens mid-tones by 5
to 11% and clips everything above it. The average sets mid-tone placement in both open curves, so an
average error is of the same order as a peak error. Limits to state with every result: open curves
are not a Dolby display; libplacebo reads only L1 max and avg and the source range (and treats L1
max as luminance), so trims, L1 min, L4 and L11 cannot be evaluated in the open; libplacebo's tone
mapping changed on 2026-09-30, so the version must be pinned; the development host's libplacebo runs
on software Vulkan (about 6 fps at 4K) and ffmpeg's filter has no target-peak option, so a 600-nit
render needs another front end; released ColorVideoVDP (0.5.7) runs, but the cross-display metric
adaptation needs an unreleased version and its repository declares no licence.

### WS8 (Open)

Item 5, real material per format, was done on 2026-10-05 (progress log). Test material the
2026-10-03 review found wanting. (1) The synthetic highlight pair writes its
highlights after the grain, so they carry none and the clean and grainy twins are identical on them;
all 2x2 highlights sit on even coordinates and fill exactly one chroma quad. Add grain on the
highlight and odd offsets. (2) The grainy twin of the clean/grainy pair comes from a different
master and conversion (median level up to 71 codes higher, raw maximum below the clean one on one
frame), so "clean is truth" holds only to about ±50 codes. Build a twin from the same master. (3) No
test cut contains a dissolve or a fade. Add synthetic cross-dissolves and fades built from real
shots (blended in PQ code and in linear light) with their intervals as truth, and retail cuts with
authored transitions. (4) Keep a held-out split: synthetic references and open implementations for
development, licensed reference output for scoring only ([`docs/PROVENANCE.md`](PROVENANCE.md)).

### FEL (Placeholder)

Profile 7 FEL. The BL+EL compositor and its re-encode were removed on 2026-10-03 because the
composed picture did not match the reconstruction in ETSI GS CCM 001 (progress log). `mkvdovi`
refuses Profile 7 FEL inputs, keeps the source, and continues with the next file. The only FEL work
of interest is what can be delivered without re-encoding: the base layer stays bit-exact and FEL
information is carried, as far as that is possible, in the Profile 8.1 RPU. Design, open questions
and what was learned are in [`docs/FEL_PLAN.md`](FEL_PLAN.md). This row replaces F1 (compositing),
F2 (FEL-discard mode) and R1 (metadata fit). Profile 7 MEL conversion is unchanged: the enhancement
layer is dropped and the video is copied (see P11 for `--mdfix`).

### R2 (Conditional)

Profile 5. Genuine Profile 5 uses the IPT signal representation, so HDR10 BL → Profile 5 requires
decoding, color transformation, and re-encoding. Feasible through an established encoder (Resolve
Studio, Dolby Encoding Engine); an open-source Profile 5 encoder in this project is substantial
separate work; changing profile flags on untouched HEVC is not a target. Precondition: a matched
HDR10 / P8.1 / correctly encoded P5 playback comparison from the same master with the playback chain
recorded.

### E8 (Open)

Histogram percentile reader. The 256-bin luma histogram is split into 64 bins below PQ(100 nits) and
192 above, but `compute_histogram_percentile_pq` and `find_highlight_knee_nits`
(`hdr_analyzer_mvp/src/analysis/histogram.rs`) convert a bin index back with bin/255: the lower edge
of bin 64 (100 nits) reads as 5.2 nits and a 1000-nit value as about 305 nits. It reaches the
`.bin`, the sidecar and the RPU only with `--peak-source histogram99/histogram999`, or with
`--peak-domain luma` and a non-conservative optimizer profile, where `histogram99` is the default.
`mkvdovi` never selects either, and `--peak-estimator percentile` uses the uniform 4096-bin
histogram, so default output is unaffected. Fix: one shared bin-edge function for writer and
readers, with a round-trip test.
