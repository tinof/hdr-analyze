# Dolby Vision Conversion Guide

How `mkvdovi` converts HDR10, HDR10+, and HLG sources, maps brightness metadata, generates CM v4.0
metadata, and verifies the result. For every flag, see [CLI_REFERENCE.md](CLI_REFERENCE.md). For
analyzer accuracy and remaining technical gaps, see [CM_ANALYZE_PARITY.md](CM_ANALYZE_PARITY.md).

## Conversion paths

- **HDR10:** the base layer is copied unchanged. When no compatible measurements exist, `mkvdovi`
  runs `hdr_analyzer_mvp` and supplies its madVR v5 file to `dovi_tool generate`.
- **HDR10+:** the base layer is copied unchanged. Source HDR10+ metadata is extracted and supplied
  directly to `dovi_tool generate`; the analyzer is used only if extraction fails and processing falls
  back to HDR10.
- **HLG:** pixels are converted from HLG to PQ with `zscale` and encoded with x265 or VideoToolbox,
  then analyzed for RPU generation.

HDR10 and HDR10+ picture data is therefore never filtered or re-encoded. Conversion quality for those
paths is determined by metadata accuracy and the display's mapping. HLG is the only path that changes
pixels.

### Analysis quality and optimizer behavior

For HDR10/HLG analysis, `--analysis-quality` selects:

| Preset | Resolution | Frames analyzed |
|--------|------------|-----------------|
| `fast` | half | every third frame |
| `balanced` (default) | half | every frame |
| `accurate` | full | every frame |

The analyzer currently enables its dynamic `target_nits` optimizer by default. For HDR10/HLG,
`mkvdovi` passes the resulting v5 measurements to `dovi_tool generate --use-custom-targets`. The
exact RPU-level effect of that switch has not yet been characterized; it is tracked as P0 in the
[roadmap](../ROADMAP.md#conversion-quality). A source-honest default without custom optimizer targets
is planned but has **not** shipped, so do not read that design decision as current behavior.

`--target-peak-nits` belongs to `hdr_analyzer_mvp` v6 header output and does not configure a display
target in `mkvdovi`. The planned opt-in `mkvdovi --target-nits` workflow does not exist yet.

### Active-area detection

The analyzer probes seven positions across the middle 70% of a seekable input by default, rejects
black/low-signal candidates, clusters crops within a two-pixel edge tolerance, and commits a stable
crop before analysis. If multiple aspect-ratio modes are observed, their union is used so full-frame
picture is not cut. Scene cuts provide reporting-only stability telemetry; the crop does not yet
change per scene.

Use `--crop-probes 0` for the hardened in-stream fallback or `--no-crop` for full-frame diagnostics.
The committed crop affects analysis, but L5 active-area metadata is not yet emitted.

### L1 measurement sidecar

Every `hdr_analyzer_mvp` run writes `<output>.l1.json` next to the madVR `.bin`. The versioned JSON
records the minimum percentile, denoise mode, committed crop, per-frame robust minimum and Y/max-RGB
means, and per-scene min/avg/max values as 12-bit PQ codes. Scene minimum is the minimum of the
noise-rejected per-frame minima. Use `--min-percentile <percent>` to change the default P0.1 lower
percentile; `--min-percentile 0` requests the absolute minimum.

Y-luma and max-RGB mean series use identical EMA/temporal smoothing settings and scene resets before
serialization. Robust minima remain raw per-frame spatial-percentile measurements.

This sidecar is for measurement and validation. `dovi_tool`'s madVR input path hardcodes L1 minimum to
zero, so the sidecar minimum is not inserted into the RPU yet. That wiring is deferred until it can be
made compatible with `--use-custom-targets` frame edits.

## HDR10+ peak mapping

For HDR10+ input, `mkvdovi` forwards the selected peak source to
`dovi_tool generate --hdr10plus-peak-source`:

- **`histogram`** — default and recommended balanced baseline.
- **`histogram99`** — last HDR10+ histogram percentile (usually 99.98%); selected by `--boost` when a
  deliberately brighter alternative is desired.
- **`max-scl`** — largest RGB MaxSCL component; more sensitive to channel highlights and outliers.
- **`max-scl-luminance`** — luminance calculated from MaxSCL components; can look dimmer.

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
| **L1** | Scene mid/max from analyzer measurements; `dovi_tool` currently hardcodes min to zero for madVR input. The analyzer's robust min is retained in its JSON sidecar |
| **L2** | Neutral compatibility trims for 100/600/1000-nit targets |
| **L5** | `dovi_tool` defaults; detected crop is not emitted yet |
| **L6** | Static mastering-display metadata and MaxCLL/MaxFALL |
| **L9** | Mastering-display primaries, preferring MediaInfo mastering metadata over container primaries; warned BT.2020 fallback and CLI override |
| **L11** | Content type and reference mode (`movies` / `false` by default) |
| **L254** | Default CM v4.0 algorithm metadata added by `dovi_tool` |

`mkvdovi` does not synthesize L3 offsets or creative L8 trims. L2 values are neutral (`2048`), and
experimental non-neutral trim derivation remains opt-in roadmap work.

### HLG caveat

HLG→PQ output is tagged as BT.2020, but its x265 `master-display` coordinates are currently hardcoded
to P3. Correct BT.2020 mastering coordinates are tracked under P5 in the roadmap.

## Post-mux verification

Pass `--verify` to validate the generated file before cleanup:

```bash
mkvdovi --keep-source --verify "input.mkv"
```

For HDR10/HLG inputs with measurements, `mkvdovi` resolves `verifier` from `PATH`. It also extracts
the final RPU and validates structured `dovi_tool info --frame 0` JSON: Profile 8, ordered L1 values,
sane L6 metadata, and required L9/L11/L254 blocks for CM v4.0. Missing source L6 fields or L9
primaries are reported when warned fallbacks are used.

## Playback troubleshooting

- Start with a television's Dolby Vision Cinema/reference picture mode; brighter home modes can raise
  blacks and are a poor diagnostic baseline.
- Ensure the playback device and HDMI input are configured for Dolby Vision and the required enhanced
  color/deep-color mode.
- Preserve the source with `--keep-source` while comparing changes; source deletion remains the
  successful-conversion default.
