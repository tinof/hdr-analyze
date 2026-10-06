# Format compatibility and conversion guide

How `mkvdovi` converts HDR10, HDR10+, and HLG sources, maps brightness metadata, generates CM v4.0
metadata, and verifies the result. For every flag, see [CLI_REFERENCE.md](CLI_REFERENCE.md). For
analyzer accuracy and remaining technical gaps, see [CM_ANALYZE_PARITY.md](CM_ANALYZE_PARITY.md).

## Conversion paths

| Input | Output | Picture re-encoded | Maturity |
|-------|--------|--------------------|----------|
| HDR10 | Profile 8.1, CM v4.0, L1 from analyzer measurements | No | Measurement comparisons published; playback unvalidated |
| HDR10+ | Profile 8.1, L1 from source HDR10+ metadata | No | Measurement comparisons published; playback unvalidated |
| HLG | Profile 8.4, HLG base layer kept, L1 through the 8.4 decode (luma and chroma curves) | No | Decode validated against libplacebo; playback unvalidated |
| Dolby Vision Profile 7 MEL | Profile 8.1, metadata-only enhancement layer discard | No | Works |
| Dolby Vision Profile 7 FEL | None: the file is refused | – | Not supported ([plan](FEL_PLAN.md)) |
| Dolby Vision Profile 8 or MEL with `--mdfix` | Profile 8.1 with rebuilt metadata, source kept | No | Works, not a guaranteed improvement over authored metadata |

Not supported: Profile 7 FEL input, Profile 5 output, and XML metadata export.

- HDR10: the base layer is copied unchanged. When no compatible measurements exist, `mkvdovi`
  runs `hdr_analyzer_mvp` and supplies its measurements to `dovi_tool generate`.
- HDR10+: the base layer is copied unchanged. Source HDR10+ metadata is extracted with
  `hdr10plus_tool` and supplied directly to `dovi_tool generate`. The file falls back to HDR10
  analysis only when extraction runs but yields no metadata (the stream carries no dynamic metadata,
  or the extraction step exits with an error or writes an empty file). `hdr10plus_tool` is not
  checked at startup, and if it cannot be started the file fails instead of falling back.
- HLG: the base layer is copied unchanged and the output is Dolby Vision Profile 8.4 (HLG
  backward-compatible). A Profile 8.4 RPU carries a reshaping curve set, the composer, that a Dolby
  Vision decoder uses to turn the HLG signal into PQ. `--hlg-composer` selects it. `preset` is
  the `dolby_vision` crate's `Profile84` preset, which `dovi_tool generate` embeds; with it the RPU
  is kept exactly as `dovi_tool` writes it. `bt2100`, the default since 2026-10-03, is a composer
  fitted to the BT.2100 / BT.2408 1000-nit HLG-to-PQ conversion that keeps neutrals neutral
  ([HLG_COMPOSER.md](HLG_COMPOSER.md)). `mkvdovi` writes it into every RPU frame after
  `dovi_tool generate`. Whether playback devices apply a composer other than the preset is
  unverified; that needs a playback test. `hdr_analyzer_mvp` measures the HLG stream through the
  selected composer's decode: the luma curve for luma statistics, and for max-RGB peaks and means
  the full reconstruction (luma curve, the two chroma MMR curves and the RPU's YCbCr-to-RGB matrix).
  Both are clamped to the RPU's declared source range (PQ codes 62–3079, about 0–1000 nits), so L1
  describes what the decoder reconstructs. The sidecar names the composer in
  `analysis.luminance_mapping`: `"dovi84-v3"` (preset) or `"dovi84-bt2100-v1-spec420"` (bt2100), the
  decode with the chroma curves at chroma resolution (ETSI GS CCM 001 §5.4.2.3.3) and bilinear
  upsampling at the stream's chroma location. `mkvdovi` reuses HLG measurements only when that value
  names the selected composer; a sidecar of the other composer, the pre-spec 4:2:0 names
  `"dovi84-v2"` and `"dovi84-bt2100-v1"`, and older luma-only `"dovi84-v1"` sidecars from
  pre-release builds are re-analyzed. An analyzer whose `--help` does not name the selected
  composer's mapping is refused before any analysis, for every composer. HLG
  input tagged full range, with a matrix other than BT.2020 non-constant luminance, or with primaries
  other than BT.2020 is refused before any work, because the RPU cannot describe it (see
  [CLI_REFERENCE.md](CLI_REFERENCE.md#hlg-input)). GPU analysis
  (`--hwaccel cuda`) works for HLG exactly as for HDR10. Broadcast HLG that signals BT.2020 in the
  VUI and HLG in the alternative transfer characteristics SEI (BBC iPlayer style) is handled: the
  analyzer reads the transfer from decoded frames, and the output still gets Dolby Vision
  compatibility ID 4 (HLG).
- Profile 7 MEL: the enhancement layer carries no picture data. It is discarded and the RPU is
  converted to Profile 8.1 with `dovi_tool`; the authored metadata is kept unless `--mdfix` is given.
- Profile 7 FEL: refused, with or without `--mdfix`. The file fails with an error before any
  temporary work, the source is kept, and a multi-file run continues with the next file. The BL+EL
  compositor and its re-encode were removed: the compositor did not match the Dolby Vision
  reconstruction specification (ETSI GS CCM 001) on real discs. A design that keeps the base layer
  bit-exact is planned in [FEL_PLAN.md](FEL_PLAN.md). `mkvdovi inspect` still reports FEL.

`mkvdovi` never encodes video. On every path the picture data is copied unchanged, so conversion
quality depends on metadata accuracy and the display's mapping.

### Analyzer input contract

`hdr_analyzer_mvp` measures PQ (SMPTE ST 2084) and HLG (ARIB STD-B67) signals. Any other tagged
transfer is refused, including the BT.2020 10/12-bit tags, which share the BT.709 SDR curve. An
untagged transfer is analyzed as PQ with a notice. Samples are interpreted as limited range with
BT.2020 non-constant-luminance coefficients; streams tagged full range or with another matrix
(BT.2020 constant luminance included) produce a warning. `mkvdovi` refuses such HLG input instead.

### Analysis quality and optimizer behavior

For HDR10/HLG analysis, `--analysis-quality` selects:

| Preset | Resolution | Frames analyzed |
|--------|------------|-----------------|
| `fast` | half | every third frame |
| `balanced` | half | every frame |
| `accurate` | full | every frame |

The default `auto` resolves to `accurate` when CUDA analysis is available and to `balanced` otherwise.

The analyzer still computes its dynamic `target_nits` optimizer for the madVR `.bin`, but `mkvdovi`
does not use it for L1. HDR10/HLG RPUs take per-scene L1 minimum, max-RGB mean, and maximum from the
analyzer's `.l1.json` sidecar. `--legacy-madvr-l1` restores the old
`dovi_tool generate --madvr-file --use-custom-targets` path, where L1 max follows optimizer
`target_pq` and L1 avg is a placeholder; use it only to reproduce old output. It is HDR10-only: HLG
input with `--legacy-madvr-l1` is refused, because Profile 8.4 needs L1 measured through the 8.4
decode.

Existing measurements next to the input are reused only when their sidecar validates: scenes start at
frame 0, are contiguous, cover the input's video frame count within a small tolerance (MediaInfo
estimates the count from the duration for some MKVs), and (version 2 and later) name the same file
and size. A scene outside min ≤ avg ≤ max only produces a warning, because percentile and robust peak
estimators and `--peak-domain luma` can legitimately produce one. Otherwise `mkvdovi` warns and re-runs the analyzer. Reused measurements
print their provenance, with a warning when they were analyzed more coarsely than the resolved
`--analysis-quality`.

`--target-peak-nits` belongs to `hdr_analyzer_mvp` v6 header output and does not configure a display
target in `mkvdovi`. The planned opt-in `mkvdovi --target-nits` workflow does not exist yet.

### Active-area detection

The analyzer probes seven positions across the middle 70% of a seekable input by default, rejects
black/low-signal candidates, clusters crops within a two-pixel edge tolerance, and commits a stable
crop before analysis. If multiple aspect-ratio modes are observed, their union is used so full-frame
picture is not cut. Scene cuts provide reporting-only stability telemetry; the crop does not yet
change per scene.

Use `--crop-probes 0` for the hardened in-stream fallback or `--no-crop` for full-frame diagnostics.
The committed crop is recorded in full-resolution coordinates and emitted as L5 active-area offsets.
Sampled source L5 keeps precedence for Dolby Vision inputs. Because the crop is one conservative
stream-level union, changing aspect ratios are not described per scene.

### L1 measurement sidecar

Every `hdr_analyzer_mvp` run writes `<output>.l1.json` next to the madVR `.bin`. The versioned JSON
records the minimum percentile, denoise mode, committed crop, per-frame robust minimum and Y/max-RGB
means, and per-scene min/avg/max values as 12-bit PQ codes. Scene minimum is the minimum of the
noise-rejected per-frame minima. Use `--min-percentile <percent>` to change the default P0.1 lower
percentile; `--min-percentile 0` requests the absolute minimum.

The Y-luma and max-RGB means in the sidecar are unfiltered: each frame value is that frame's mean
and each scene value is the mean of its frames (version 4). The histogram EMA and temporal median
apply only to the histograms and the frame average in the madVR `.bin`. Robust minima are raw
per-frame spatial-percentile measurements.

`mkvdovi` passes the per-scene values to `dovi_tool generate` as explicit shots. The generator
clamps them before it writes the RPU: a minimum above 12 codes is written as 12, a maximum below
2081 (100 nits) as 2081, and an average below 819 as 819. Values inside those limits reach the RPU
unchanged. Sidecar version 2 added `analyzer_version`, `source` (file
name, size, dimensions, transfer), `analysis` (downscale, sample rate, GPU use, crop disabled), and
stores `crop` in full-resolution coordinates (`crop_space: "full"`). Version 3 adds
`analysis.luminance_mapping`: `pq`, or for HLG the composer whose full Profile 8.4 decode was
measured (`dovi84-v2` for the preset; `dovi84-bt2100-v1`, added later without a version change, for
the bt2100 composer; both replaced in version 5 by `dovi84-v3` and `dovi84-bt2100-v1-spec420` for
the spec 4:2:0 decode, again without a version change). Version 4 has the same layout and stores unfiltered averages. Max-RGB runs also
write `light_level` (`max_cll_nits`, `max_fall_nits`): the content light levels of CTA-861.3 over
the active image area, with the frame average taken in linear light. The block is optional, so a
version 4 sidecar written before it existed stays valid.

Version 5 (current) adds `source.stream_frames` and `source.leading_skipped_frames`. A stream cut
at an open-GOP CRA picture starts with RASL pictures that no decoder outputs. They still come first
in the RPU's presentation order, so measured frame `i` is stream frame `i + leading_skipped_frames`.
`mkvdovi` moves the scenes back by that count, and `--verify` compares in stream frames. The
analyzer refuses a stream that loses any other picture, for example one that does not start at a
random access picture.

`mkvdovi` accepts versions 1–5. It reuses a sidecar below version 5 only when its frame count
matches the input exactly, and it reuses a sidecar below version 4 with a warning that its averages were smoothed
over time (delete the measurements to re-analyze). HLG input requires version 3 or later with the selected composer's name, and a `dovi84`
sidecar is rejected for a non-HLG input. Version 1 carries no identity or full-resolution crop, so
only structure and frame count are checked and no L5 is derived from it.

## HDR10+ peak mapping

For HDR10+ input, `mkvdovi` forwards the selected peak source to
`dovi_tool generate --hdr10plus-peak-source`:

- `histogram`: default and recommended balanced baseline.
- `histogram99`: last HDR10+ histogram percentile (usually 99.98%). `--boost` selects it when you
  want a brighter alternative on purpose.
- `max-scl`: largest RGB MaxSCL component. More sensitive to channel highlights and outliers.
- `max-scl-luminance`: luminance calculated from MaxSCL components. Can look dimmer.

Neutral L2 compatibility targets remain `100,600,1000`. They are not panel-calibration controls and
should not be replaced with a television's measured peak brightness. A Dolby Vision-capable display
applies its own display mapping.

For initial A/B testing, preserve the source and verify the output:

```bash
mkvdovi --keep-source --verify "input.mkv"
```

Use `--boost` only as an intentional alternative after comparing the default output.

### Outlier handling

When the selected HDR10+ source produces scene L1 peaks above three times the mastering-display peak,
`mkvdovi` warns and preserves the source metadata. Real sources can contain valid outliers, so peaks
are never silently clamped.

`hdr10plus_tool extract --skip-reorder` is not currently retried when ordinary extraction fails; that
fallback remains on the roadmap.

## CM v4.0 metadata

`mkvdovi` generates Content Mapping v4.0 metadata by default. HDR10+ inputs derive L1 from source
scenes; HDR10/HLG inputs derive it from analyzer measurements.

```bash
# Default: CM v4.0 with auto-detected settings
mkvdovi "input.mkv"

# Set L11 content type
mkvdovi "input.mkv" --content-type movies
mkvdovi "input.mkv" --content-type sport

# Legacy CM v2.9
mkvdovi "input.mkv" --cm-version v29

# Override L9 source primaries (0=P3-D65, 1=BT.709, 2=BT.2020)
mkvdovi "input.mkv" --source-primaries 0
```

### Metadata levels generated

| Level | Current output |
|-------|----------------|
| **L1** | HDR10/HLG: per-scene minimum, max-RGB mean, and maximum from the analyzer's L1 sidecar, after the `dovi_tool generate` clamps (minimum at most 12 codes, maximum at least 2081, average at least 819). HDR10+: derived from source scenes |
| **L2** | Neutral compatibility trims for 100/600/1000-nit targets |
| **L5** | HDR10/HLG: offsets from the committed crop. Dolby Vision inputs: sampled source L5. Full-frame content: `dovi_tool` zero default |
| **L6** | Static mastering-display metadata and MaxCLL/MaxFALL, read with MediaInfo. HDR10/HLG: a MaxCLL or MaxFALL the source does not state (or states as 0) is taken from the analyzer's measurement (see below). Otherwise warned defaults when MediaInfo is absent or a field is missing |
| **L9** | Mastering-display primaries, preferring MediaInfo mastering metadata over container primaries; warned BT.2020 fallback (also used when MediaInfo is absent) and CLI override |
| **L11** | Content type and reference mode (`movies` / `false` by default) |
| **L254** | Default CM v4.0 algorithm metadata added by `dovi_tool` |

Measured MaxCLL/MaxFALL: source-stated values always win, field by field. The sidecar's
`light_level` fills a missing field only when every frame was analyzed (`--analysis-quality fast`
skips frames and keeps the defaults). MaxCLL also needs full-resolution analysis
(`--analysis-quality accurate`, the default with GPU analysis); at half resolution only MaxFALL is
taken, because a sampling stride can miss a small highlight. A filled value is adjusted so that
MaxFALL does not exceed MaxCLL; a source-stated value is never changed. A sidecar without the block
(older analyzer, `--peak-domain luma`, or `--pre-denoise median3`) keeps the defaults: delete the measurements to re-analyze.

`mkvdovi` does not synthesize L3 offsets or creative L8 trims. L2 values are neutral (`2048`), and
experimental non-neutral trim derivation remains opt-in roadmap work. Note that while `dovi_tool` 2.3.4
parses Level 253 extension metadata blocks, the `dolby_vision` crate (3.4.0) used in-process (for inspect
sampling and MEL/FEL classification) does not yet support L253 blocks; support will be updated when the crate releases it.

### HLG caveats

- Profile 8.4 playback support is narrower than 8.1. Devices without 8.4 support play the HLG base
  layer.
- HLG peaks default to max-RGB of the full 8.4 decode, like PQ; `--peak-domain luma` selects the
  luma curve alone. With the preset composer, neutral content reads about 2% higher in max-RGB than
  in luma, because the preset's chroma curves tint neutrals slightly blue (grey code 721: luma 2389,
  max-RGB 2439). The bt2100 composer decodes grey to equal R′G′B′ (code 721: 2378.6 on all three
  channels), so luma and max-RGB agree.
- The bt2100 composer is the default. Its decode is checked against libplacebo, but no Dolby Vision
  display has been tested with it yet; `--hlg-composer preset` is the fallback.
- The decode was checked against libplacebo's Dolby Vision renderer on lossless test patterns: for
  the preset, the luma curve within 3.2 twelve-bit PQ codes on a grey ramp over HLG codes 64–1008,
  and max-RGB within 0.59 codes on 52 flat colour patches; for bt2100, within 1.77 and 0.49 codes.
  CPU and CUDA give identical results (see
  [VALIDATION.md §8](VALIDATION.md#8-hlg-dolby-vision-84-decode-vs-libplacebo-2026-09-30)).
- With the preset, the brightest HLG codes decode above PQ 3079 in the 8.4 model: grey at nominal
  peak (10-bit 940) decodes to PQ 3155, and superwhite codes from 943 up reach PQ 4095. The bt2100
  composer decodes grey 940 to PQ 3078.7 and holds neutral superwhite at that level (1000 nits). The analyzer clamps luma and
  max-RGB to the RPU's declared range on purpose, so L1 never exceeds what the RPU declares.
  libplacebo's apparent plateau near 1000 nits for these codes is its display tone mapping (it clips
  to the L1 max_pq, or to source_max_pq 3079 when L1 is absent), not Dolby Vision decoder behaviour.
- HLG sources rarely carry mastering metadata, so the L6 mastering display usually falls back to
  1000 / 0.005 nits, which matches the 8.4 source range. MaxCLL and MaxFALL come from the
  analyzer's measurement of the 8.4 decode, capped at 1000 nits (the top of the range, PQ code
  3079, is 1000.9 nits and would round to 1001); only when that is
  unavailable do they fall back to 1000 / 400.
- Legacy temp directories from the removed HLG→PQ re-encode path (they contain `HLG_to_PQ.mkv`) are
  discarded, not resumed.

## Post-mux verification

Pass `--verify` to validate the generated file before cleanup:

```bash
mkvdovi --keep-source --verify "input.mkv"
```

For HDR10/HLG inputs with measurements, `mkvdovi` resolves `verifier` from `PATH`. It also extracts
the final RPU and validates structured `dovi_tool info --frame 0` JSON: Profile 8, ordered L1 values,
sane L6 metadata, and required L9/L11/L254 blocks for CM v4.0. It fails when the RPU frame count
differs from the muxed video track's frame count or from the L1 sidecar, and warns when the output
and input video frame counts differ. For an RPU that `mkvdovi` generated (every path except the
Profile 7 MEL passthrough), the RPU is parsed in-process, which also gives the frame count. Every
frame must then carry the expected source range: for 8.1 the mastering display range (not checked
when the mastering values are implausible and `dovi_tool` derives the range from L6, which the
conversion warns about), for 8.4 62/3079. Every frame of every measured scene must carry the measured L1 after the
generator's limits (minimum at most 12 codes, maximum at least 2081, average at least 819 and below
the maximum); any other difference fails. The output reports how many scenes those limits changed
per field and by how much (one line per scene with `--verbose`). It also notes scenes whose L1 max
lies above `source_max_pq`. HDR10+ output and `--legacy-madvr-l1` output are not compared with the
measurements, because their L1 does not come from them. For HLG output it also
fails when no measurements are available or the L1 sidecar does not load, and it checks that every
RPU frame carries the composer the sidecar names (`dovi84-v3` = preset,
`dovi84-bt2100-v1-spec420` = bt2100), so a measurement is never paired with an RPU of another composer. Missing source L6 fields or
L9 primaries are reported when warned fallbacks are used.

## Playback troubleshooting

- Start with a television's Dolby Vision Cinema/reference picture mode; brighter home modes can raise
  blacks and are a poor diagnostic baseline.
- Ensure the playback device and HDMI input are configured for Dolby Vision and the required enhanced
  color/deep-color mode.
- Preserve the source with `--keep-source` while comparing changes; source deletion remains the
  successful-conversion default.
