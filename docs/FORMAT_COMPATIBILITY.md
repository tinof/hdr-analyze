# Format compatibility and conversion guide

How `mkvdovi` converts HDR10, HDR10+, and HLG sources, maps brightness metadata, generates CM v4.0
metadata, and verifies the result. For every flag, see [CLI_REFERENCE.md](CLI_REFERENCE.md). For
analyzer accuracy and remaining technical gaps, see [CM_ANALYZE_PARITY.md](CM_ANALYZE_PARITY.md).

## Conversion paths

| Input | Output | Picture re-encoded | Maturity |
|-------|--------|--------------------|----------|
| HDR10 | Profile 8.1, CM v4.0, L1 from analyzer measurements | No | Measurement comparisons published; playback unvalidated |
| HDR10+ | Profile 8.1, L1 from source HDR10+ metadata | No | Measurement comparisons published; playback unvalidated |
| HLG | Profile 8.1 after HLG to PQ conversion | Yes: libx265, `hevc_videotoolbox` or `hevc_nvenc` | Works, less validated |
| Dolby Vision Profile 7 MEL | Profile 8.1, metadata-only enhancement layer discard | No | Works |
| Dolby Vision Profile 7 FEL | Profile 8.1 from a BL+EL composite | Yes: composite, then re-encode | Experimental, compositor accuracy unverified |
| Dolby Vision Profile 8 or MEL with `--mdfix` | Profile 8.1 with rebuilt metadata, source kept | No | Works, not a guaranteed improvement over authored metadata |

Not supported: Profile 5 output, lossless FEL to Profile 8 conversion, and XML metadata export.

- HDR10: the base layer is copied unchanged. When no compatible measurements exist, `mkvdovi`
  runs `hdr_analyzer_mvp` and supplies its measurements to `dovi_tool generate`.
- HDR10+: the base layer is copied unchanged. Source HDR10+ metadata is extracted with
  `hdr10plus_tool` and supplied directly to `dovi_tool generate`. The file falls back to HDR10
  analysis only when extraction runs but yields no metadata (the stream carries no dynamic metadata,
  or the extraction step exits with an error or writes an empty file). `hdr10plus_tool` is not
  checked at startup, and if it cannot be started the file fails instead of falling back.
- HLG: the analyzer first measures the original HLG stream, mapping it to PQ with
  `--hlg-peak-nits`. The pipeline then converts the video from HLG to PQ with `zscale` and encodes
  it with libx265 or `hevc_videotoolbox` (`--encoder`), or with `hevc_nvenc` under `--hwaccel cuda`
  when FFmpeg has it. The encoded PQ picture is not measured again, so L1 describes the analyzer's
  PQ mapping of the source rather than the encoder output.
- Profile 7 FEL: the base and enhancement layers are composited in software and re-encoded, then a
  new Profile 8.1 RPU is generated. See [experimental/README.md](experimental/README.md).

HDR10, HDR10+, Profile 7 MEL and `--mdfix` picture data is never filtered or re-encoded. Conversion
quality for those paths depends on metadata accuracy and the display's mapping. HLG and Profile 7 FEL
change pixels.

### Analyzer input contract

`hdr_analyzer_mvp` measures PQ (SMPTE ST 2084) and HLG (ARIB STD-B67) signals. Any other tagged
transfer is refused, including the BT.2020 10/12-bit tags, which share the BT.709 SDR curve. An
untagged transfer is analyzed as PQ with a notice. Samples are interpreted as limited range with
BT.2020 non-constant-luminance coefficients; streams tagged full range or with another matrix produce
a warning.

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
`target_pq` and L1 avg is a placeholder; use it only to reproduce old output.

Existing measurements next to the input are reused only when their sidecar validates: scenes start at
frame 0, are contiguous, keep min ≤ avg ≤ max, cover the input's video frame count, and (version 2)
name the same file and size. Otherwise `mkvdovi` warns and re-runs the analyzer. Reused measurements
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

Y-luma and max-RGB mean series use identical EMA/temporal smoothing settings and scene resets before
serialization. Robust minima remain raw per-frame spatial-percentile measurements.

`mkvdovi` embeds the per-scene values as explicit `dovi_tool generate` shots, so the measured minimum,
max-RGB mean, and maximum reach the RPU. Sidecar version 2 (current) adds `analyzer_version`,
`source` (file name, size, dimensions, transfer), `analysis` (downscale, sample rate, GPU use, crop
disabled), and stores `crop` in full-resolution coordinates (`crop_space: "full"`). `mkvdovi` accepts
versions 1 and 2. Version 1 carries no identity or full-resolution crop, so only structure and frame
count are checked and no L5 is derived from it.

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
| **L1** | HDR10/HLG: per-scene minimum, max-RGB mean, and maximum from the analyzer's L1 sidecar. HDR10+: derived from source scenes |
| **L2** | Neutral compatibility trims for 100/600/1000-nit targets |
| **L5** | HDR10/HLG: offsets from the committed crop. Dolby Vision inputs: sampled source L5. Full-frame content: `dovi_tool` zero default |
| **L6** | Static mastering-display metadata and MaxCLL/MaxFALL, read with MediaInfo; warned defaults when MediaInfo is absent or a field is missing |
| **L9** | Mastering-display primaries, preferring MediaInfo mastering metadata over container primaries; warned BT.2020 fallback (also used when MediaInfo is absent) and CLI override |
| **L11** | Content type and reference mode (`movies` / `false` by default) |
| **L254** | Default CM v4.0 algorithm metadata added by `dovi_tool` |

`mkvdovi` does not synthesize L3 offsets or creative L8 trims. L2 values are neutral (`2048`), and
experimental non-neutral trim derivation remains opt-in roadmap work. Note that while `dovi_tool` 2.3.4
parses Level 253 extension metadata blocks, the `dolby_vision` crate (3.4.0) used in-process (for inspect
sampling and FEL NLQ parsing) does not yet support L253 blocks; support will be updated when the crate releases it.

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
sane L6 metadata, and required L9/L11/L254 blocks for CM v4.0. It fails when the RPU frame count
from `dovi_tool info --summary` differs from the muxed video track's frame count or from the L1
sidecar, and warns when the output and input video frame counts differ. Missing source L6 fields or
L9 primaries are reported when warned fallbacks are used.

## Playback troubleshooting

- Start with a television's Dolby Vision Cinema/reference picture mode; brighter home modes can raise
  blacks and are a poor diagnostic baseline.
- Ensure the playback device and HDMI input are configured for Dolby Vision and the required enhanced
  color/deep-color mode.
- Preserve the source with `--keep-source` while comparing changes; source deletion remains the
  successful-conversion default.
