# Dolby Vision: HDR10+ Peak Mapping & CM v4.0 Metadata

How `mkvdovi` maps source brightness into Dolby Vision metadata, what it generates, and how to
verify the result. For the full flag list, see [CLI_REFERENCE.md](CLI_REFERENCE.md). For analysis
internals, see [TECHNICAL_REFERENCE.md](TECHNICAL_REFERENCE.md).

---

## HDR10+ peak mapping

For HDR10+ input, `mkvdovi` forwards the selected peak source to
`dovi_tool generate --hdr10plus-peak-source`:

- **`histogram`** — default and recommended balanced baseline.
- **`histogram99`** — uses the last HDR10+ histogram percentile (usually 99.98%); available
  explicitly with `--boost` when a brighter mapping is desired.
- **`max-scl`** — largest RGB MaxSCL component; more sensitive to channel highlights and outliers.
- **`max-scl-luminance`** — luminance calculated from MaxSCL components; can look dimmer.

The standard neutral L2 compatibility trim targets remain `100,600,1000`. They are **not** a
panel-calibration control — do not replace them with your TV's measured peak brightness. A
Dolby Vision-capable display applies its own display mapping.

For movie or episodic HDR10+ content on an LG OLED C9-class display, start with the defaults and
preserve the source for A/B testing:

```bash
mkvdovi --keep-source --verify "input.mkv"
```

Use `--boost` only as an intentional brighter alternative after comparing the default output.

### Outlier handling (no silent clamping)

When the selected HDR10+ peak source produces scene L1 peaks **above three times** the mastering
display peak, `mkvdovi` warns and leaves the source metadata unchanged. This is advisory because
real sources can contain valid outliers; `mkvdovi` never clamps peaks silently.

### Playback troubleshooting (Shield / LG OLED)

- Start with the TV's Dolby Vision **Cinema** picture mode and enable **Ultra HD Deep Color** for
  the Shield HDMI input.
- Avoid **Cinema Home** as the diagnostic baseline: it is brighter but can raise blacks in dark scenes.
- On Shield, enable Dolby Vision under **Settings → Device Preferences → Display & Sound → Dolby
  Vision**; if necessary, choose a custom display mode labelled **Dolby Vision Ready**.

---

## CM v4.0 metadata

`mkvdovi` generates Content Mapping v4.0 metadata by default. HDR10+ inputs derive L1 brightness
metadata from the source HDR10+ scenes; CM v4.0 adds L9/L11 metadata and `dovi_tool` CM v4 defaults
alongside the CM v2.9-compatible L2/L6 blocks.

```bash
# Default: CM v4.0 with auto-detected settings
mkvdovi "input.mkv"

# Specify L11 content type
mkvdovi "input.mkv" --content-type movies   # preserve movie artistic intent (default)
mkvdovi "input.mkv" --content-type sport    # high-motion content

# Legacy CM v2.9
mkvdovi "input.mkv" --cm-version v29

# Override auto-detected source primaries (0=P3-D65, 1=BT.709, 2=BT.2020)
mkvdovi "input.mkv" --source-primaries 0
```

### Metadata levels generated

| Level | Contents |
|-------|----------|
| **L1** | Scene min/mid/max luminance (from HDR10+ or `hdr_analyzer_mvp`) |
| **L2** | Neutral compatibility trim parameters for 100/600/1000-nit displays |
| **L6** | Static mastering display metadata (MaxCLL/MaxFALL) |
| **L9** | Mastering-display color primaries (auto-detected; `0=P3-D65, 1=BT.709, 2=BT.2020`) |
| **L11** | Content type and reference mode (default content type `movies`, reference mode `false`) |
| **L254** | Default CM v4.0 algorithm metadata (added by `dovi_tool`) |

`mkvdovi` does **not** synthesize L3 offsets or creative L8 trims. With `dovi_tool` 2.3.2 the
generator adds its default L254 block; producing authored offsets or trims requires a separate
workflow.

---

## Post-mux verification

Pass `--verify` to validate the generated file before cleanup:

```bash
mkvdovi --keep-source --verify "input.mkv"
```

For HDR10/HLG sources with a measurements file, `mkvdovi` resolves the installed `verifier` from
`PATH`. It then extracts the final RPU and validates structured `dovi_tool info --frame 0` JSON:
Profile 8, ordered L1 values, sane L6 metadata, and the required L9/L11/L254 blocks for CM v4.0.
Missing source L6 fields or L9 primaries are reported when warned fallbacks are used.
