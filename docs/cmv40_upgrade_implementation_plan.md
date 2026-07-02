# CM v4.0 Upgrade: Implementation Status

**Updated**: 2026-05-31
**Status**: Shipped in the native `mkvdovi` workflow

## Summary

`mkvdovi` generates Dolby Vision Profile 8.1 RPUs with Content Mapping v4.0 by
default. The default workflow prioritizes honest source metadata, accurate L1
generation, and display-side tone mapping:

- CM v4.0 adds L9 source primaries and L11 content-type metadata.
- L2 blocks remain neutral compatibility trims for `100,600,1000` nit targets.
- Authored L8 creative trims remain outside the default workflow.
- Statistical oddities are reported as warnings; source peaks are never clamped
  silently.

Use `--cm-version v29` only when legacy CM v2.9 output is required.

## Generated Metadata

| Level | Behavior | Source |
|-------|----------|--------|
| L1 | Scene min/mid/max luminance | HDR10+ dynamic metadata, or `hdr_analyzer_mvp` measurements for HDR10/HLG |
| L2 | Neutral compatibility trims | `--trim-targets`, default `100,600,1000` |
| L5 | Active-area offsets | `dovi_tool` defaults |
| L6 | Static mastering and light-level metadata | MediaInfo, with explicit warned fallbacks |
| L9 | Mastering-display primaries | MediaInfo auto-detection or `--source-primaries` |
| L11 | Content type and reference-mode flag | CLI settings, default `movies` and `false` |
| L254 | CM v4.0 algorithm metadata | Added by `dovi_tool` |

The default workflow does **not** synthesize L3 offsets, authored L8 trims, or
panel-specific calibration. A Dolby Vision-capable display performs its own
display mapping. Do not replace neutral L2 targets with a TV's measured peak
brightness.

## CLI Surface

```bash
# Default: CM v4.0, auto-detected source primaries, neutral L2 trims
mkvdovi --keep-source --verify "input.mkv"

# Legacy CM v2.9 output
mkvdovi --cm-version v29 "input.mkv"

# Explicit L9/L11 settings
mkvdovi \
  --source-primaries 0 \
  --content-type movies \
  --reference-mode false \
  "input.mkv"
```

L9 source-primary indices:

| Index | Primaries |
|-------|-----------|
| `0` | P3-D65 |
| `1` | BT.709 |
| `2` | BT.2020 |

For HDR10 and HLG inputs that require analysis, `--analysis-quality` selects the
`hdr_analyzer_mvp` sampling preset:

| Preset | `--downscale` | `--sample-rate` | Purpose |
|--------|---------------|-----------------|---------|
| `fast` | `2` | `3` | Previous fast behavior |
| `balanced` | `2` | `1` | Default: every frame at half resolution |
| `accurate` | `1` | `1` | Full-resolution, every-frame analysis |

For HDR10+ sources, `--peak-source` is forwarded to
`dovi_tool generate --hdr10plus-peak-source`. The default is `histogram`;
`histogram99`, `max-scl`, and `max-scl-luminance` remain explicit alternatives.

## Guardrails

### Source Metadata

`get_static_metadata` warns before applying fallbacks for missing L6 values:

| Field | Warned fallback |
|-------|-----------------|
| Max mastering-display luminance | `1000.0` nits |
| Min mastering-display luminance | `0.005` nits |
| MaxCLL | `1000.0` nits |
| MaxFALL | `400.0` nits |

L9 auto-detection prefers mastering-display primaries, then encoded container
primaries. If neither is available, `mkvdovi` warns before falling back to
BT.2020. `generate_extra_json` rejects missing static metadata rather than
silently applying a second fallback layer.

### HDR10+ Scene Peaks

After HDR10+ extraction, `mkvdovi` evaluates the selected peak-source values
for scene first frames. It warns when a selected L1 peak exceeds three times the
mastering-display peak and reports the highest selected peak. The warning is
advisory because legitimate source metadata can contain outliers.

### Post-Mux Verification

`mkvdovi --verify` resolves the installed `verifier` from `PATH` for available
measurement files. It extracts the final RPU and parses structured
`dovi_tool info --frame 0` JSON, hard-failing when:

- Dolby Vision Profile 8 is missing.
- L1 does not satisfy `min_pq <= avg_pq <= max_pq`.
- Required L6 fields are missing or invalid.
- CM v4.0 output lacks L9, L11, or L254 with `dm_version_index = 2`.
- Input and output durations differ by more than one second.

## Validation

Completed locally on 2026-05-31:

```bash
cargo metadata --no-deps --format-version 1
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --verbose
cargo build --release --workspace
```

Focused regression tests cover:

- CM v4.0 JSON generation and custom trim-target PQ conversion.
- Missing static metadata rejection.
- Source-primary detection precedence.
- Analysis-quality preset mappings.
- HDR10+ selected-peak outlier detection and MaxSCL luminance weighting.
- Structured RPU JSON parsing and hard invariant failures.

## Deferred Work

- Add a curated external HDR10+ fixture that runs through `dovi_tool generate`.
- Add post-mux frame-count comparison and end-padding diagnostics.
- Validate `--source-primaries` and `--trim-targets` with typed CLI values.
- Consider an experimental, opt-in peak clamp only after fixture collection and
  playback A/B testing.
- Consider experimental L2/L8 derivation only after reference files and
  display-side A/B validation are available.

## References

- [dovi_tool generator documentation](https://github.com/quietvoid/dovi_tool/blob/main/docs/generator.md)
- [dovi_tool releases](https://github.com/quietvoid/dovi_tool/releases)
- [Dolby Vision metadata levels](https://professionalsupport.dolby.com/s/article/Dolby-Vision-Metadata-Levels)
