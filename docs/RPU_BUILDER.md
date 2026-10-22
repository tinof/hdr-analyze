# In-process RPU builders (`dovi_rpu_builder`)

`dovi_rpu_builder` builds Dolby Vision Profile 8.1 and 8.4 RPUs in-process, one complete RPU NAL
unit per access unit (AU). Its output is byte-identical to `dovi_tool generate` 2.3.4 for the same
configuration. It was made for the NuvioTV fork (`tinof/NuvioTV-Fork`, `ROADMAP.md` Appendix A,
steps DV2.1 and DV2.2). The fork adds an RPU to each AU during playback on a Shield TV Pro and
wraps this crate in a JNI staticlib; JNI, Gradle and APK code stay in the fork. mkvdovi uses the
same crate for the one definition of its `extra.json` and still runs `dovi_tool generate`
(switching it to in-process generation is an owner decision, tied to ROADMAP P10.3). Roadmap
step: E16.1.

## Constraints

- Pure Rust: no FFmpeg, no CUDA, no process calls and no file I/O in the API. It builds for
  `aarch64-linux-android` (CI job `rpu-builder`: `cargo check --target aarch64-linux-android -p
  dovi_rpu_builder`; a check needs the target's std only, not the NDK).
- Nothing panics on caller input. The workspace release profile has `panic = "abort"`, so a panic
  would end the player. Every refusal is an `Error`. That includes the inputs on which the
  `dolby_vision` 3.4 crate itself panics:
  - a composer the crate's writer cannot write. `dovi84_composer::check_mapping_writable` checks
    every index the writer touches, and the polynomial piece with `poly_order_minus1 == 0` and
    `linear_interp_flag` set, which the writer answers with `unimplemented!`
    (`rpu_data_mapping.rs:229`).
  - a metadata block of a level the RPU's CM version cannot carry (the crate's `unreachable!` in
    `VdrDmData::replace_metadata_block`).
  - an HDR10+ payload without the selected peak (`dovi_tool` unwraps it).
- The builders never parse an RPU from outside. A later entry point that rewrites a source RPU
  (the fork's DV7.3, `scene_refresh_flag` cleared on a borrowed RPU) needs the E15.1 pre-scan
  first and belongs next to the builder, not inside it.
- `rust-version` 1.88 (the `dolby_vision` 3.4 minimum); `hdr10plus` is pinned to `=2.1.5`, the
  version in dovi_tool 2.3.4's lock, without its `json` feature.

## API

| Item | What it is |
|---|---|
| `RpuParams` | Everything but L1: `Profile::P81 { source_range }` or `Profile::P84 { composer }`, `CmMetadata` (`V29`, or `V40` with the L9/L11 config and the trim targets), `Level6`, optional `Level5`, `l1_avg_pq_cm_version` |
| `RpuParams::extra_json(shots)` | The `extra.json` mkvdovi writes (one definition with the builder) |
| `RpuBuilder::new(&params)` | The base RPU, once per params (`DoviRpu::profile81_config` / `profile84_config`, then the composer for 8.4) |
| `RpuBuilder::shot(L1)` → `ShotRpu` | The base with one shot's L1, clamped as `dovi_tool` clamps it, once per shot |
| `ShotRpu::write(scene_refresh)` | One NAL unit, once per AU |
| `build(&params, L1, scene_refresh)` | All three in one call |
| `L1 { min_pq, avg_pq, max_pq }` | 12-bit PQ codes; `L1::clamped(CmVersion)` is the generator's clamp |
| `HlgComposer::{Preset, Bt2100, Canary { offset }}` | 8.4 composers; `HlgComposer::canary()` is the P8 canary at its default offset |
| `Hdr10PlusFrame::parse(payload)`, `.l1(peak, cm)`, `.scene_key()` | One HDR10+ payload → L1, and the key `dovi_tool`'s scene rule compares |
| `Hdr10PlusHold` | That rule over frames in presentation order: `(L1, scene_refresh)` per frame |
| `hdr10plus_l1(payload, peak, cm)` | `parse` then `l1` |
| `STATIC_84_L1`, `STATIC_84_L1_AVG_CM_VERSION`, `static_84_params(composer)` | A static HLG → 8.4 RPU |
| `source_range_81`, `source_range_84`, `nits_to_pq_code`, `Level6::from_nits`, `DEFAULT_*` | The pieces of mkvdovi's configuration, shared |

### Byte contract

`ShotRpu::write` returns the HEVC NAL unit header `0x7C 0x01` (`UNSPEC62`, `RPU_NAL_HEADER`)
followed by the RPU payload with emulation prevention applied: no start code and no length
prefix. The caller adds the framing of its container (an Annex B start code, or a length
prefix). The CRC and emulation prevention are done by the `dolby_vision` crate on every write, so
a changed field is always a newly written RPU, never a patch of escaped bytes. `dovi_tool`'s
`RPU.bin` stores each RPU as `00 00 00 01` followed by the same NAL without its first two bytes.

### Per-AU use

```rust
let builder = RpuBuilder::new(&params)?;   // once per title (or per params change)
let mut shot = builder.shot(l1)?;          // once per shot
let first = shot.write(true)?;             // the shot's first AU: scene refresh
let next = shot.write(false)?;             // every other AU of the shot
```

`write` sets `scene_refresh_flag` explicitly on every call. `dovi_tool` sets it on the first frame of
each shot only. Not supported: `long_play_mode` (scene refresh on every frame: pass `true` instead)
and per-frame edits inside a shot (build another shot). Which AU starts a shot, and holding the
shot's L1 for the rest of it, is the caller's job.

### Defaults

`RpuParams` carries the values mkvdovi writes for the same source; mkvdovi builds its
`extra.json` from it (`mkvdovi/src/metadata/rpu_config.rs`, `generate_extra_json`).

| Field | mkvdovi default |
|---|---|
| CM version | V40 (`CmMetadata::mkvdovi_default`); V29 under `--cm-version v29` (no default blocks) |
| L2 | One neutral trim (all 2048) per `--trim-targets` value, default 100, 600, 1000 nits |
| L9 / L11 | `CmV40Config::default()`: BT.2020 primaries (2), content type movies (1), reference mode off |
| L6 | From the source; when it states none, 1000 / 0.005 nits mastering, MaxCLL 1000, MaxFALL 400 (`Level6::mkvdovi_default`) |
| 8.1 source range | `source_range_81` of the mastering display; `None` (the generator's L6 lookup) when implausible |
| 8.4 source range | Always the preset's 62..3079 |
| `l1_avg_pq_cm_version` | V29 with measured L1 (an analyzer sidecar, 8.1 and 8.4), the CM version otherwise (HDR10+) |

`extra_json` writes `l1_avg_pq_cm_version` with shots (as mkvdovi always has) or when it differs
from the CM version. That keeps the HDR10+ `extra.json`, which never stated it, byte-identical.

## HDR10+ → L1

The arithmetic is ported from dovi_tool 2.3.4 (`src/dovi/generator.rs`,
`parse_hdr10plus_for_l1`). min is 0. avg is the average maxRGB in nits, max is the selected peak in
nits, and both are rounded to whole nits before the PQ conversion. Then comes
`ExtMetadataBlockLevel1::from_stats_cm_version` with the RPU's L1 average floor. Peak sources:
`histogram` (the highest percentile value), `histogram99` (the last percentile), `max-scl`,
`max-scl-luminance`; the same four mkvdovi offers.

**Input bytes.** `payload` is the payload of one `user_data_registered_itu_t_t35` SEI message as an
RBSP (emulation prevention bytes removed), starting at the country code `0xB5`. These are the bytes
`hdr10plus_tool extract` reads. A payload without the HDR10+ T.35 identity (country `0xB5`,
provider `0x003C`, provider-oriented code 1, application 4 version 1) is `Error::NotHdr10Plus`:
`hdr10plus_tool` skips such messages. The others must pass `Hdr10PlusMetadata::validate`, which
`hdr10plus_tool` runs by default and which accepts one processing window only.

**Scenes.** `dovi_tool` takes L1 from the first frame of each scene `hdr10plus_tool` found, and
`hdr10plus_tool` starts a scene when a frame's metadata differs from the previous frame's in
presentation order (`compute_scene_information`). `SceneKey` holds exactly the compared fields:
- the average maxRGB, the percentile distribution and MaxSCL
- the number of windows
- the targeted system display maximum luminance
- for profile B, the Bezier curve

`fraction_bright_pixels` is not compared. To reproduce `dovi_tool`:
- When the key changes, take that frame's L1 and write it with `scene_refresh: true`.
- Otherwise hold the L1 and write `false` (`Hdr10PlusHold`).
- A scene can start with the same L1 as the previous one: a Bezier-only change, or values that
  round or clamp to equal codes. The scene refresh still goes out.
- `hdr10plus_tool` compares the Bezier curve only when every frame of the file is profile B. This
  key compares it whenever the frame is, so the two differ only for a stream that mixes profiles.

**Order and gaps.** The rule runs in presentation order, so a player that sees AUs in decode
order reorders first. An AU without a payload gets no L1 from this crate. `hdr10plus_tool` (like
FFmpeg) gives it the previous payload in decode order, before reordering, and that is what
`dovi_tool` sees. Not covered: `--hdr10plus-l1 hybrid` (the analyzer's cuts and averages), HDR10+
together with a reused measurement sidecar, and multi-window payloads.

## Static Profile 8.4

A new policy, not a port: mkvdovi refuses HLG without L1 measured through the selected composer
(`metadata::dv_profile_for`), and that rule is unchanged. The library's static L1 (owner decision,
2026-10-22):

- **max 3079** (about 1000 nits): the 8.4 source-range ceiling, the peak of a 1000-nit HLG system,
  not a measured value. The 65 measured scenes of the four HLG development cuts peak at 2293 to
  3079, 36 of them at 3079.
- **avg 1283** (about 11.8 nits): the frame-weighted mean of the measured scene averages of the four
  HLG development cuts. The runs used `--hlg-composer bt2100` (`dovi84-bt2100-v1-spec420`), accurate
  sampling and CUDA, over 9160 frames:

  | Cut | Scene average |
  |---|---|
  | Wimbledon 2024 final rally | 874.9 |
  | Green Planet tropical canopy | 1377.1 |
  | Glastonbury 2025 Supergrass stage | 1511.9 |
  | UCL Bayern–PSG match action | 1528.2 |

  The pooled mean is 1282.67, rounded to 1283. Evidence is in
  `~/mkvdovi-work/e16-1-2026-10-22/static-avg/`. Four cuts, mostly sport, so this is a conservative
  middle value, not a model of HLG content.
- **min 0**.
- **`l1_avg_pq_cm_version` V29**, as for every measured 8.4 RPU mkvdovi delivers. The average is
  above both floors (819 and 1229), so the choice changes no value.

`static_84_params(composer)` gives the rest: CM v4.0 with mkvdovi's default blocks, the default L6
an HLG source gets, and no L5. The fork's DV3.2 gate (the canary turns green through the app's own
conversion) uses `static_84_params(HlgComposer::canary())` with `STATIC_84_L1`.

## Composers

`HlgComposer::Preset` and `Bt2100` are `dovi84_composer::Composer::Preset` and `Bt2100V1`. `Canary`
is the P8 playback canary: bt2100 with both chroma MMR constants shifted by `round(offset * 2^23)`,
default −1/16 (green). `dovi84_composer::canary_mapping` is the one place the shift lives;
`tools/fit_hlg_composer canary-rpu` uses it too. The canary is not a `Composer` variant:
mkvdovi's `--hlg-composer` does not offer it, no analyzer measures through it, and no mkvdovi
output changes. The library's canary bytes equal `fit_hlg_composer canary-rpu` applied to its own
bt2100 output (fixtures `p84_canary_shots` and `static_84_canary`).

## Verification

- **Fixtures** (`dovi_rpu_builder/tests/fixtures`, written by `make_fixtures.sh`):
  - every expected RPU comes from `dovi_tool` 2.3.4, and for the bt2100 and canary composers also
    from `fit_hlg_composer`
  - the cases: 8.1 with shots (V40 with the V29 floor, V40 with the V40 floor, V29), 8.4 preset,
    bt2100 and canary, static 8.1, static 8.4 bt2100 and canary, and two synthetic HDR10+
    sequences for every peak source under V29 and V40
  - the HDR10+ sequences cover both sides of the scene rule: a held frame, a
    `fraction_bright_pixels`-only change, a luminance change, a change that rounds to the same L1,
    two frames that clamp to the same L1, and a Bezier-only change
  - `tests/dovi_tool_fixtures.rs` compares the builders with them without running a tool
- **Live** (`tests/dovi_tool_live.rs`): writes today's `extra.json`, runs `dovi_tool generate`,
  and compares. It prints `Skipping` without dovi_tool 2.3.4, which the test gate counts as a
  failure; CI installs the pinned version.
- **Equivalence** (`tools/rpu_builder_check`, outside the workspace): `extra_json` read the way
  `dovi_tool` reads a config (serde, `fixup_l1`, `generate_rpu_list`), over 96 parameter
  combinations. It lives outside the workspace because `hdr10plus/json` and `dolby_vision/serde`
  turn on `serde_json/preserve_order`, which would reorder mkvdovi's `extra.json` keys in every
  workspace test build.
- **Real HDR10+ content** (local evidence, `~/mkvdovi-work/e16-1-2026-10-22/hdr10plus/`): the
  eight HDR10+ development cuts:
  - `rpu_builder_check payloads` (the stream's SEI payloads, ordered as `hdr10plus_tool` orders
    them) against `dovi_tool generate --hdr10plus-json`, per peak source and CM version
  - the parsed values against the JSON
  - the scene starts against `SceneInfoSummary`
- **mkvdovi unchanged**: the byte-exact `extra.json` tests in `mkvdovi/src/metadata/tests.rs`. The
  HDR10+ and CM v2.9 goldens were captured from the code before the move. On the development
  corpus, `scripts/rpu-baseline.sh compare --require-identical-l1` and `sha256sum` of `RPU.bin`
  ran against the nine 0.7.0 baselines.

Regenerate the fixtures after a deliberate change with `dovi_rpu_builder/tests/fixtures/make_fixtures.sh`
(it checks the dovi_tool version), and explain the reason in the PR.
