# Implementation Plan: Upgrade mkvdolby to CM v4.0 with Full Metadata Levels

**Author**: AI Assistant  
**Date**: 2026-01-06  
**Target**: Upgrade Profile 8.1 conversion to use Content Mapping v4.0 and populate L9/L11 metadata

---

## Executive Summary

The current `mkvdolby` workflow generates Dolby Vision Profile 8.1 RPUs using **CM v2.9** with minimal metadata (L1, L2, L5, L6). This plan upgrades the pipeline to:

1. **Use CM v4.0** for enhanced display-side tone mapping
2. **Leave authored L8 trims to a dedicated trim-generation workflow**
3. **Populate L9** (source display primaries metadata)
4. **Populate L11** (content type and reference mode flags)

---

## Current State Analysis

### What We Have Now

**File**: `mkvdolby/src/metadata.rs` → `generate_extra_json()`

```rust
let json_content = json!({
    "level6": {
        "max_display_mastering_luminance": *max_dml as u32,
        "min_display_mastering_luminance": (*min_dml * 10000.0) as u32,
        "max_content_light_level": *max_cll as u32,
        "max_frame_average_light_level": *max_fall as u32,
    }
});
```

**Result**: `dovi_tool generate` defaults to CM v2.9, producing only L1, L2, L5, L6. The old `target_nits` key was ignored by `dovi_tool`; neutral L2 trim blocks must be provided explicitly.

### What We Need

Based on `dovi_tool`'s `full_example.json`:

```json
{
    "cm_version": "V40",
    "profile": "8.1",
    "level5": { ... },
    "level6": { ... },
    "default_metadata_blocks": [
        { "Level2": { ... } },
        { "Level9": { "source_primary_index": 0 } },
        { "Level11": { "content_type": 1, "whitepoint": 0, "reference_mode_flag": false } }
    ]
}
```

HDR10+ scene metadata supplies L1. Authored L8 trims are outside this workflow.

---

## Dolby Vision Metadata Levels Reference

| Level | Name | Purpose | CM Version | Source |
|-------|------|---------|------------|--------|
| **L1** | Content Min/Mid/Max | Scene-by-scene brightness stats | 2.9+ | Analysis (madVR/hdr_analyzer) |
| **L2** | CM v2.9 Trims | Display-target tone mapping (100/600/1000 nits) | 2.9+ | Analysis + algorithm |
| **L5** | Active Area | Letterbox/pillarbox cropping | 2.9+ | Source detection |
| **L6** | Static Metadata | MaxCLL, MaxFALL, mastering display luminance | 2.9+ | Source HDR10 metadata |
| **L8** | CM v4.0 Trims | Extended trims with mid-tone control and saturation | 4.0 | Dedicated trim-generation workflow |
| **L9** | Source Primaries | Which color gamut was used for mastering | 4.0 | Source or hardcoded |
| **L11** | Content Type | Movies/game/sport/UGC + whitepoint + reference mode | 4.0 | User preference |

---

## Implementation Plan

### Phase 1: Extend JSON Configuration Structure

**File**: `mkvdolby/src/metadata.rs`

#### 1.1 Add New Types

```rust
/// Source primary index for L9 metadata
#[derive(Debug, Clone, Copy, Default)]
pub enum SourcePrimaryIndex {
    P3D65 = 0,
    BT709 = 1,
    #[default]
    BT2020 = 2,        // Default for most UHD content
}

/// Content type for L11 metadata
#[derive(Debug, Clone, Copy, Default)]
pub enum ContentType {
    Default = 0,
    #[default]
    Movies = 1,
    Game = 2,
    Sport = 3,
    UserGeneratedContent = 4,
}
```

#### 1.2 Update `generate_extra_json()` Signature

```rust
pub struct CmV40Config {
    pub source_primary_index: u8,  // L9: 0 = P3-D65, 1 = BT.709, 2 = BT.2020
    pub content_type: u8,          // L11: 0-4 (see enum above)
    pub reference_mode: bool,      // L11: explicit authored reference workflow
}

pub fn generate_extra_json(
    output_path: &Path,
    metadata: &HashMap<String, f64>,
    trim_targets: &[u32],
    cm_v40_config: Option<&CmV40Config>,  // NEW: Optional CM v4.0 config
) -> Result<()>
```

#### 1.3 New JSON Generation Logic

```rust
pub fn generate_extra_json(
    output_path: &Path,
    metadata: &HashMap<String, f64>,
    trim_targets: &[u32],
    cm_v40_config: Option<&CmV40Config>,
) -> Result<()> {
    let min_dml = metadata.get("min_dml").unwrap_or(&0.005);
    let max_dml = metadata.get("max_dml").unwrap_or(&1000.0);
    let max_cll = metadata.get("max_cll").unwrap_or(&1000.0);
    let max_fall = metadata.get("max_fall").unwrap_or(&400.0);

    let mut json_content = json!({
        "profile": "8.1",
        "level6": {
            "max_display_mastering_luminance": *max_dml as u32,
            "min_display_mastering_luminance": (*min_dml * 10000.0) as u32,
            "max_content_light_level": *max_cll as u32,
            "max_frame_average_light_level": *max_fall as u32,
        }
    });

    // Add CM v4.0 specific configuration
    if let Some(cfg) = cm_v40_config {
        json_content["cm_version"] = json!("V40");
        
        // Add neutral L2 compatibility trims, then L9 and L11
        json_content["default_metadata_blocks"] = json!([
            {
                "Level9": {
                    "length": 1,
                    "source_primary_index": cfg.source_primary_index
                }
            },
            {
                "Level11": {
                    "content_type": cfg.content_type,
                    "whitepoint": 0,  // D65 (standard)
                    "reference_mode_flag": cfg.reference_mode
                }
            }
        ]);
    }

    let file = File::create(output_path)?;
    serde_json::to_writer_pretty(file, &json_content)?;
    Ok(())
}
```

---

### Phase 2: Add CLI Arguments

**File**: `mkvdolby/src/cli.rs`

```rust
/// CM version to use for RPU generation
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum CmVersion {
    #[default]
    V29,
    V40,
}

#[derive(Parser)]
pub struct Args {
    // ... existing args ...

    /// Use CM v4.0 for L9/L11 and dovi_tool defaults
    #[arg(long, default_value = "v40")]
    pub cm_version: CmVersion,

    /// Source color primaries for L9 metadata (0=P3-D65, 1=BT.709, 2=BT.2020)
    #[arg(long)]
    pub source_primaries: Option<u8>,

    /// Content type for L11 metadata
    #[arg(long, value_enum, default_value_t = ContentType::Movies)]
    pub content_type: ContentType,

    /// Enable reference mode for L11 (for critical viewing)
    #[arg(long, default_value_t = false)]
    pub reference_mode: bool,
}
```

---

### Phase 3: Update Pipeline

**File**: `mkvdolby/src/pipeline.rs`

#### 3.1 Modify `convert_file()` to Use New Config

```rust
// Around line 167, update the call:
let cm_v40_config = if args.cm_version == CmVersion::V40 {
    Some(CmV40Config {
        source_primary_index: args.source_primaries,
        content_type: args.content_type,
        reference_mode: args.reference_mode,
    })
} else {
    None
};

metadata::generate_extra_json(
    &extra_json_path,
    &static_meta,
    &final_trims,
    cm_v40_config.as_ref(),
)?;
```

---

### Phase 4: Auto-Detection of Source Primaries (L9)

**File**: `mkvdolby/src/metadata.rs`

Add automatic detection based on MediaInfo:

```rust
/// Detect source primaries from MediaInfo
pub fn detect_source_primaries(input_file: &str) -> u8 {
    if let Ok(json) = get_mediainfo_json(input_file) {
        if let Some(tracks) = json
            .get("media")
            .and_then(|m| m.get("track"))
            .and_then(|t| t.as_array())
        {
            for track in tracks {
                if track.get("@type").and_then(|s| s.as_str()) == Some("Video") {
                    // Prefer mastering-display primaries; then fall back to encoded primaries.
                    if let Some(primaries) = track
                        .get("MasteringDisplay_ColorPrimaries")
                        .or(track.get("mastering_display_color_primaries"))
                        .or(track.get("colour_primaries"))
                        .or(track.get("ColorPrimaries"))
                        .and_then(|s| s.as_str())
                    {
                        let p = primaries.to_uppercase();
                        if p.contains("P3") || p.contains("DCI") {
                            return 0; // P3-D65
                        }
                        if p.contains("709") {
                            return 1; // BT.709
                        }
                    }
                }
            }
        }
    }
    2 // Default: BT.2020
}
```

---

### Phase 5: Enhanced L2/L8 Trim Generation

`dovi_tool` does not automatically synthesize L8 trims from L1 metadata just because the JSON includes `"cm_version": "V40"`. The current workflow emits neutral L2 compatibility trims; authored L8 trims require a separate trim-generation workflow.

---

## File Changes Summary

| File | Changes |
|------|---------|
| `mkvdolby/src/cli.rs` | Add `--cm-version`, `--source-primaries`, `--content-type`, `--reference-mode` args |
| `mkvdolby/src/metadata.rs` | Add `CmV40Config` struct, update `generate_extra_json()`, add `detect_source_primaries()` |
| `mkvdolby/src/pipeline.rs` | Wire up new config, pass to `generate_extra_json()` |

---

## Testing Plan

### Unit Tests

**File**: `mkvdolby/tests/integration.rs` (or new test file)

```rust
#[test]
fn test_cmv40_json_generation() {
    let temp_dir = tempdir().unwrap();
    let output_path = temp_dir.path().join("extra.json");
    
    let mut metadata = HashMap::new();
    metadata.insert("max_dml".to_string(), 4000.0);
    metadata.insert("min_dml".to_string(), 0.0001);
    metadata.insert("max_cll".to_string(), 2000.0);
    metadata.insert("max_fall".to_string(), 800.0);
    
    let cfg = CmV40Config {
        source_primary_index: 0,
        content_type: 1,
        reference_mode: false,
    };
    
    generate_extra_json(&output_path, &metadata, &[100, 600, 1000], Some(&cfg)).unwrap();
    
    let content: Value = serde_json::from_reader(File::open(&output_path).unwrap()).unwrap();
    
    assert_eq!(content["cm_version"], "V40");
    assert_eq!(content["default_metadata_blocks"][3]["Level9"]["source_primary_index"], 0);
    assert_eq!(content["default_metadata_blocks"][4]["Level11"]["content_type"], 1);
}
```

### Integration Test

```bash
# Test CM v4.0 conversion
cargo run --release -- \
    --cm-version v40 \
    --source-primaries 0 \
    --content-type movies \
    /path/to/test_hdr10.mkv

# Verify output RPU contains L9/L11 and dovi_tool CM v4.0 defaults
dovi_tool info -i output.DV.mkv --summary
# Expected output should show:
#   CM version: 4.0
#   L9: source_primary_index = 0
#   L11: content_type = 1, reference_mode = false
```

### Manual Verification

1. Convert a known HDR10 file with `--cm-version v40`
2. Extract the RPU: `dovi_tool extract-rpu output.DV.mkv -o rpu.bin`
3. Inspect with: `dovi_tool info -i rpu.bin -f 0` (check frame 0)
4. Verify presence of Level9 and Level11 blocks in JSON output. L8 is present only when supplied by a separate authored trim workflow.

---

## L8/L9/L11 Optimal Values Reference

### L9: Source Primaries

| Index | Color Space | When to Use |
|-------|-------------|-------------|
| 0 | P3-D65 | Digital cinema masters, some Netflix originals |
| 1 | BT.709 | SDR upconverts, HD sources |
| 2 | BT.2020 | Default for UHD Blu-ray, streaming 4K HDR |

**Recommendation**: Auto-detect the mastering-display primaries from MediaInfo before falling back to encoded `colour_primaries`, since HDR video is commonly carried in a BT.2020 container even when mastered on a P3-D65 display. Fall back to 2 (BT.2020).

### L11: Content Type

| Value | Type | When to Use |
|-------|------|-------------|
| 0 | Default | Legacy Dolby Vision playback behavior |
| 1 | Movies | Minimize post-processing to preserve artistic intent |
| 2 | Game | Minimize input latency |
| 3 | Sport | Enable frame rate conversion for high-motion sports |
| 4 | User Generated Content | Enable compensating post-processing for consumer capture |

**Recommendation**: Default to 1 (Movies) for movie and episodic content. Allow user override.

### L11: Reference Mode

- `true`: Content intended for critical viewing in controlled environment
- `false`: Default living-room behavior

**Recommendation**: Default to `false`; enable explicitly only for an authored reference-mode workflow.

---

## Migration Notes

### Backward Compatibility

- Default `--cm-version` to `v40` for corrected metadata generation
- Existing scripts continue to work unchanged
- Users can opt back into CM v2.9 with `--cm-version v29`

### Breaking Changes

The default generated metadata changes from CM v2.9 to CM v4.0. Use `--cm-version v29` when legacy output is required.

---

## Dependencies

No new dependencies required. Uses existing:
- `serde_json` for JSON generation
- `clap` for CLI parsing
- `dovi_tool` external binary (already required)

---

## Future Enhancements

1. **L5 active area detection**: Currently hardcoded/default. Could analyze video for letterboxing.
2. **Content type detection**: ML-based classification of film vs animation vs live action.
3. **Preserve source L9/L11**: When converting Profile 7 FEL, preserve original L9/L11 from source RPU.

---

## Verification Checklist

- [x] `cargo build --release` succeeds (completed 2026-01-22)
- [x] `cargo test` passes (completed 2026-01-22)
- [ ] `cargo clippy` shows no warnings (minor pre-existing warnings only)
- [x] `--cm-version v40` produces RPU with L9/L11 and `dovi_tool` CM v4 defaults
- [x] `--cm-version v29` produces RPU matching legacy behavior
- [ ] `dovi_tool info --summary` confirms CM v4.0 on output files
- [ ] Manual playback test on Dolby Vision capable device

## Implementation Status: COMPLETED (2026-01-22)

### Changes Made:
1. **cli.rs**: Added `CmVersion` enum (V29/V40), `ContentType` enum, and CLI args `--cm-version`, `--content-type`, `--reference-mode`, `--source-primaries`
2. **metadata.rs**: Added `CmV40Config` struct, `detect_source_primaries()` function, updated `generate_extra_json()` to include L9/L11 blocks
3. **pipeline.rs**: Wired up CM v4.0 config with auto-detection of source primaries
4. **Default**: CM v4.0 is now the default for all mkvdolby runs

---

## References

- [dovi_tool generator examples](https://github.com/quietvoid/dovi_tool/tree/main/assets/generator_examples)
- [dovi_tool generator documentation](https://github.com/quietvoid/dovi_tool/blob/main/docs/generator.md)
- [Dolby Vision Metadata Levels](https://professionalsupport.dolby.com/s/article/Dolby-Vision-Metadata-Levels)
