<!-- refreshed: 2026-08-12 -->
# Architecture

**Analysis Date:** 2026-08-12

## System Overview

This is a Rust workspace containing three shipped HDR metadata analysis and processing binaries. The system orchestrates video frame analysis, scene detection, and Dolby Vision metadata generation/injection workflows.

```text
┌─────────────────────────────────────────────────────────────────┐
│                      CLI Entry Points                            │
├──────────────────┬──────────────────┬───────────────────────────┤
│ hdr_analyzer_mvp │     mkvdovi      │       verifier            │
│ (video analysis) │ (DV conversion)  │  (measurement validate)   │
└────────┬─────────┴────────┬─────────┴──────────────┬────────────┘
         │                  │                        │
         ▼                  ▼                        ▼
┌──────────────────────────────────────────────────────────────────┐
│                     Orchestration Layer                           │
│  pipeline::run (hdr_analyzer_mvp)  /  pipeline::convert_file      │
│  mkvdovi/src/pipeline.rs (mkvdovi) / verifier main.rs            │
└──────────────────────────────────────────────────────────────────┘
         │                  │                        │
         ▼                  ▼                        ▼
┌──────────────────────────────────────────────────────────────────┐
│                   Analysis / Processing Layer                     │
│  hdr_analyzer_mvp/src/analysis/*  mkvdovi/src/{metadata,         │
│  + FFmpeg I/O + frame crop/optimize   rpu_check, fel_composite}  │
└──────────────────────────────────────────────────────────────────┘
         │                  │                        │
         ▼                  ▼                        ▼
┌──────────────────────────────────────────────────────────────────┐
│                 External Integrations / Output                    │
│  FFmpeg (decoding) / mkvmerge / dovi_tool / mediainfo            │
│  Binary output (.bin) / JSON sidecars / MKV files               │
└──────────────────────────────────────────────────────────────────┘
```

## Component Responsibilities

| Component | Responsibility | File |
|-----------|----------------|------|
| `hdr_analyzer_mvp` | HDR10 video frame analysis → PQ histograms + scene peaks | `hdr_analyzer_mvp/src/main.rs` |
| `pipeline::run` (hdr_analyzer_mvp) | Orchestrate analysis workflow: FFmpeg I/O → frame analysis → optimization → output | `hdr_analyzer_mvp/src/pipeline.rs` |
| `analysis/frame.rs` | Per-frame luminance/color histogram computation + peak estimation (max, percentile, robust) | `hdr_analyzer_mvp/src/analysis/frame.rs` |
| `analysis/scene.rs` | Scene cut detection + histogram-based scene metrics | `hdr_analyzer_mvp/src/analysis/scene.rs` |
| `analysis/gpu.rs` | Optional CUDA-accelerated frame analysis (full-resolution sampling) | `hdr_analyzer_mvp/src/analysis/gpu.rs` |
| `analysis/histogram.rs` | PQ-domain histogram bin logic + frame-to-frame smoothing | `hdr_analyzer_mvp/src/analysis/histogram.rs` |
| `analysis/hlg.rs` | HLG transfer function conversion to PQ for analysis | `hdr_analyzer_mvp/src/analysis/hlg.rs` |
| `ffmpeg_io` | FFmpeg context setup, decoding, format detection, crop probing | `hdr_analyzer_mvp/src/ffmpeg_io.rs` |
| `writer.rs` | Binary measurement file generation (madVR format, v5/v6) | `hdr_analyzer_mvp/src/writer.rs` |
| `l1_sidecar.rs` | JSON L1 per-scene/per-frame metadata export | `hdr_analyzer_mvp/src/l1_sidecar.rs` |
| `optimizer.rs` | Post-analysis nits curve smoothing + tone-mapping profile selection | `hdr_analyzer_mvp/src/optimizer.rs` |
| `crop.rs` | Active video area (letterbox/pillarbox) detection and validation | `hdr_analyzer_mvp/src/crop.rs` |
| `mkvdovi` main | File discovery, sorting, subprocess orchestration | `mkvdovi/src/main.rs` |
| `mkvdovi::pipeline::convert_file` | Per-file: detect format → extract/analyze → generate metadata → inject/mux | `mkvdovi/src/pipeline.rs` |
| `metadata.rs` (mkvdovi) | Dolby Vision L1–L11 metadata generation (CM v4.0, trims, source primaries) | `mkvdovi/src/metadata.rs` |
| `rpu_check.rs` | DV Profile classification, RPU sampling, L1 reliability diagnostics | `mkvdovi/src/rpu_check.rs` |
| `fel_composite.rs` | Profile 7 FEL re-encoding + metadata rebuild | `mkvdovi/src/fel_composite.rs` |
| `external.rs` | Tool availability checks + subprocess invocation (ffmpeg, mkvmerge, dovi_tool, etc.) | `mkvdovi/src/external.rs` |
| `resume.rs` | Resumable checkpoint tracking for multi-step conversions | `mkvdovi/src/resume.rs` |
| `verifier` | Read + validate madVR measurement files | `verifier/src/main.rs` |

## Pattern Overview

**Overall:** Staged analysis pipeline with optional acceleration and post-processing optimization.

**Key Characteristics:**
- **Three independent binaries:** hdr_analyzer_mvp (analysis engine), mkvdovi (metadata injection orchestrator), verifier (validation utility)
- **Layered separation:** CLI → orchestration → analysis → FFmpeg/external tools
- **Resumable workflows:** mkvdovi uses checkpointed steps to survive interruption
- **Optional GPU acceleration:** CUDA backend for hdr_analyzer_mvp via feature flag
- **Sidecar metadata:** L1 JSON export alongside binary measurement files for per-scene source truth

## Layers

**CLI & Validation (`cli.rs` in each crate):**
- Purpose: Parse and validate user input arguments
- Location: `hdr_analyzer_mvp/src/cli.rs`, `mkvdovi/src/cli.rs`, `verifier/src/main.rs`
- Contains: Clap derive structs (Args, Cli), validation logic, help text
- Depends on: Clap, anyhow
- Used by: main.rs in each crate

**Orchestration (`pipeline.rs` in each crate):**
- Purpose: Control overall workflow: setup → analysis → optimization → output
- Location: `hdr_analyzer_mvp/src/pipeline.rs`, `mkvdovi/src/pipeline.rs`
- Contains: Workflow state machines, step coordination, progress reporting
- Depends on: Analysis modules, external tools, I/O
- Used by: main.rs, subcommand handlers

**FFmpeg I/O Layer (`ffmpeg_io.rs` in hdr_analyzer_mvp):**
- Purpose: Handle video decoding, format detection, hardware acceleration
- Location: `hdr_analyzer_mvp/src/ffmpeg_io.rs`
- Contains: AVFormat context setup, NVDEC/software decode fallback, transfer function detection
- Depends on: ffmpeg-next (Rust wrapper), system FFmpeg
- Used by: pipeline, frame analysis

**Analysis Layer (`analysis/` subdirectories):**
- Purpose: Compute per-frame histograms, scene metrics, peak values
- Location: `hdr_analyzer_mvp/src/analysis/{frame,histogram,scene,hlg,gpu,mod}.rs`
- Contains: PQ histogram bin logic, peak estimators (max, percentile, robust), scene cut detection
- Depends on: ffmpeg-next (frame access), rayon (parallelization), optional cudarc (GPU)
- Used by: pipeline for frame-by-frame processing

**Metadata Generation (`metadata.rs` in mkvdovi):**
- Purpose: Generate Dolby Vision L1–L11 metadata structures
- Location: `mkvdovi/src/metadata.rs`
- Contains: DV trims, source primaries, content type, reference mode configs
- Depends on: dolby_vision crate, hdr_analyzer output (L1)
- Used by: pipeline after format detection

**Output & Serialization (`writer.rs`, `l1_sidecar.rs`):**
- Purpose: Write madVR binary files + JSON L1 sidecars
- Location: `hdr_analyzer_mvp/src/{writer,l1_sidecar}.rs`
- Contains: MadVRHeader/Scene/Frame serialization, JSON L1 schema (versioned)
- Depends on: madvr_parse, serde/serde_json
- Used by: pipeline after analysis

**Tool Orchestration (`external.rs` in mkvdovi):**
- Purpose: Check for / invoke external binaries
- Location: `mkvdovi/src/external.rs`
- Contains: Dependency checks, subprocess spawning, streaming progress
- Depends on: std::process, indicatif
- Used by: pipeline for FFmpeg, mkvmerge, dovi_tool calls

## Data Flow

### Primary Request Path: hdr_analyzer_mvp

1. **CLI Parse & Validation** (`main.rs`, line ~21)
   - Parse arguments (input path, analysis settings, output format)
   - Validate thresholds (percentiles, thread counts, peak nits)
   - Initialize Rayon thread pool if `--analysis-threads` specified

2. **Video Probing** (`ffmpeg_io.rs`, called from `main.rs` line ~69)
   - Open video with FFmpeg, detect resolution, frame count, transfer function
   - Return VideoInfo struct with codec/format metadata

3. **Crop Probing** (`pipeline.rs` line ~334)
   - Sample frames at `--crop-probes` positions across the file
   - Detect active video area (letterbox/pillarbox removal)
   - Commit consensus crop rect or use in-stream fallback

4. **Frame Analysis Loop** (`pipeline.rs` → `pipeline::run_native_analysis_pipeline`)
   - Decode frames via FFmpeg (hardware or software)
   - For each frame:
     - Scale to YUV420P10LE (crop + downscale if needed)
     - Compute 4096-bin PQ histogram (`analysis/frame.rs` → `analyze_native_frame_cropped`)
     - Estimate frame peak (max/percentile/robust estimator)
     - Track min/avg/max PQ for L1 sidecar
   - Detect scene cuts via histogram difference threshold

5. **Histogram Smoothing** (`pipeline.rs` line ~391)
   - Optional EMA + temporal median on per-frame histograms
   - Reset smoothing on scene boundaries

6. **Optimization Pass** (`pipeline.rs` line ~407, `optimizer.rs`)
   - Select tone-mapping profile (SDR, cinema, game, etc.)
   - Smooth `target_nits` curve with EMA or per-frame deltas

7. **Output Generation**
   - Write binary measurement file (`.bin`) → madVR format v5/v6 (`writer.rs`)
   - Write L1 sidecar (`.bin.l1.json`) → JSON with per-frame/per-scene peaks (`l1_sidecar.rs`)
   - Optionally dump frame-by-frame stats CSV (`--dump-frame-stats`)

### Secondary Flow: mkvdovi

1. **File Discovery** (`main.rs` lines ~218–242)
   - Accept explicit file list or walk cwd for `.mkv` files
   - Filter already-converted files (ending `.DV.mkv`)
   - Sort naturally by episode number if regex matches

2. **Per-File Pipeline** (`pipeline.rs` → `convert_file`)

   a. **Format Detection** (line ~80)
      - Probe input: HDR10, HDR10+, Dolby Vision (Profile), or SDR
      - Sample RPU windows if DV input to assess metadata reliability
      - Decision: repair, enhance, or pass-through

   b. **Analyze HDR** (line ~200+)
      - Run `hdr_analyzer_mvp` if HDR10/no measurements available
      - Locate or generate L1 sidecar from analyzer output

   c. **Generate Metadata** (line ~300+)
      - Read L1 sidecar (per-scene measurements)
      - Call `metadata::generate_dv_metadata()` → Dolby Vision L1–L11
      - Include trims (100/600/1000 nits), source primaries, content type

   d. **Extract/Composite** (line ~350+)
      - If Profile 7 FEL: call `fel_composite::run_composite_pipe` for re-encode
      - Otherwise: extract BL, EL (if Profile 7 MEL/8) via FFmpeg

   e. **Inject/Mux** (line ~400+)
      - Call `dovi_tool inject-rpu` to add metadata
      - Call `mkvmerge` to rebuild container with DV codec

   f. **Cleanup**
      - On success: delete source (unless `--keep-source`)
      - On failure: preserve temp dir for resume

3. **Checkpointed Resumption** (`resume.rs`)
   - Each step leaves a `.done` sentinel in `mkvdovi_temp_*`
   - Re-run detects sentinels and skips completed steps
   - Supports long conversions that survive interruption (SIGHUP, Ctrl+C)

### State Management

- **Analysis Pipeline State:** Accumulated scene list, per-frame histograms, detected peaks (held in memory during frame loop)
- **Workflow Checkpoints:** Filesystem sentinels (`<step>.done`) in temp directories (mkvdovi only)
- **Global Rayon Pool:** Thread pool configuration set once in `main()`, reused across all parallel analysis
- **Optional GPU Context:** CUDA device context initialized once if `--hwaccel cuda` (hdr_analyzer_mvp)

## Key Abstractions

**MadVRFrame/Scene/Header (from `madvr_parse` crate):**
- Purpose: Unified frame/scene data struct aligned with madVR tool expectations
- Examples: `hdr_analyzer_mvp/src/writer.rs` (serialization), `verifier/src/main.rs` (deserialization)
- Pattern: Serde-compatible structs with binary I/O

**FrameAnalysisOptions:**
- Purpose: Bundle analysis configuration for frame processor
- Examples: transfer function, peak estimator, crop rect
- Pattern: Passed immutably to frame workers

**CropRect:**
- Purpose: Represent active video area as {x, y, width, height}
- Locations: `hdr_analyzer_mvp/src/crop.rs`, used in pipeline and L1 sidecar
- Pattern: Struct with helper methods for matching/scaling

**L1Sidecar (JSON schema, versioned):**
- Purpose: Export per-scene/per-frame luminance truth for downstream metadata generation
- Examples: `hdr_analyzer_mvp/src/l1_sidecar.rs` (write), `mkvdovi/src/metadata.rs` (read)
- Pattern: Versioned JSON struct with scene/frame arrays

**OptimizerProfile:**
- Purpose: Tone-mapping curve parameters (nits targets, smoothing deltas)
- Examples: SDR, cinema, game profiles in `hdr_analyzer_mvp/src/optimizer.rs`
- Pattern: Enum-backed struct with per-profile constants

**TransferFunction (enum):**
- Purpose: Classify video as PQ, HLG, or unknown
- Examples: `hdr_analyzer_mvp/src/ffmpeg_io.rs` (detection), `analysis/frame.rs` (peak domain selection)
- Pattern: Used to branch analysis/conversion logic

**HdrFormat (mkvdovi):**
- Purpose: Input type (HDR10, HDR10+, DV Profile N, SDR)
- Examples: `mkvdovi/src/metadata.rs` (format classification)
- Pattern: Enum driving pipeline branches (analyze, extract, composite, etc.)

## Entry Points

**`hdr_analyzer_mvp`:**
- Location: `hdr_analyzer_mvp/src/main.rs` (lines 20–82)
- Triggers: User invocation with input video + analysis options
- Responsibilities: Parse CLI, probe video, call `pipeline::run()`, report progress

**`mkvdovi`:**
- Location: `mkvdovi/src/main.rs` (lines 133–290+)
- Triggers: User invocation with input MKV + conversion options, or directory walk
- Responsibilities: Collect files, dispatch subcommands (inspect/composite-pipe) or call `pipeline::convert_file()` per file

**`verifier`:**
- Location: `verifier/src/main.rs` (line 38+)
- Triggers: User invocation with measurement file path
- Responsibilities: Read madVR .bin file, validate structure, display contents

**`SubCmd::Inspect` (mkvdovi):**
- Location: `mkvdovi/src/main.rs` (lines 139–140)
- Triggers: `mkvdovi inspect <file>`
- Calls: `rpu_check::inspect_file()` (samples RPU metadata)

**`SubCmd::CompositePipe` (mkvdovi):**
- Location: `mkvdovi/src/main.rs` (lines 142–143)
- Triggers: `mkvdovi composite-pipe --input <file>`
- Calls: `fel_composite::run_composite_pipe()` (pipes raw frames for Profile 7 FEL compositing)

## Architectural Constraints

- **Threading:** Rayon thread pool for frame-level parallelism in analysis. Single-threaded orchestration/I/O. GPU kernel launch is thread-safe via cudarc.
- **Global State:** Rayon pool configured once in `main()`, immutable thereafter. Optional CUDA device context (hdr_analyzer_mvp) initialized once if GPU enabled.
- **Circular Imports:** None observed; tree imports follow std → third-party → crate:: (absolute paths internally).
- **Memory Buffering:** Scene list and per-frame histogram arrays held in memory during analysis pipeline (no streaming to disk mid-analysis).
- **FFmpeg Context Lifetime:** Must remain open until all frame decoding complete; scoped to `pipeline::run()` duration.
- **Checkpoint Atomicity:** Temp directory / sentinel files are not atomic; interrupted writes mid-mux can leave partial output (resumption re-runs incomplete steps).
- **GPU Memory Pinning:** Full-resolution CUDA analysis (`--downscale 1`) may require significant VRAM on 4K+ sources; no streaming buffer management.

## Anti-Patterns

### Storing Raw Decoder Context Outside Orchestration
**What happens:** Code path tries to hold FFmpeg context beyond function scope.
**Why it's wrong:** Context lifetime is tied to input file handle; use-after-free occurs.
**Do this instead:** Keep `format::context::Input` scoped within `pipeline::run()` (`hdr_analyzer_mvp/src/pipeline.rs` line 268); pass VideoInfo + data to worker functions.

### Ignoring Transfer Function in Peak Estimation
**What happens:** Analysis assumes all input is PQ even when `--hlg-peak-nits` is specified.
**Why it's wrong:** HLG→PQ conversion changes absolute nits values; wrong trim targets result.
**Do this instead:** Check `VideoInfo::transfer_function` in `pipeline::run()` (line ~273) and route to HLG conversion in `analysis/hlg.rs`.

### Unversioned JSON Sidecars
**What happens:** L1 schema changes break downstream metadata generation without signaling incompatibility.
**Why it's wrong:** mkvdovi silently falls back to partial metadata; user doesn't notice corrupted trims.
**Do this instead:** Include `version` field in L1Sidecar struct (`l1_sidecar.rs` line ~12); bump version on schema changes; reject unknown versions in reader.

### Blocking Subprocess Calls Without Progress Feedback
**What happens:** Long FFmpeg/mkvmerge commands block without user feedback.
**Why it's wrong:** User assumes the tool hung.
**Do this instead:** Use `external::run_command_with_progress()` (mkvdovi/src/external.rs) which streams bytes and updates a progress bar.

## Error Handling

**Strategy:** `anyhow::Result` + `.context()` at application level for user-facing errors; `thiserror` for library error types (mkvdovi only uses anyhow).

**Patterns:**
- Validation errors (CLI, input file format) → early return with `anyhow::bail!()` in `main()` or orchestration layer
- I/O errors (file not found, write failed) → propagate with `.context()` to add operation description
- FFmpeg errors (decode failure, unsupported codec) → `ffmpeg_next` crate returns error, wrap with context
- GPU errors (CUDA out of memory, kernel launch failure) → from `cudarc`, convert to anyhow with context
- Tool invocation errors (dovi_tool not in PATH) → check in `external::check_dependencies()` before any file processing
- Resumption errors (sentinel file corruption) → treat as "not resuming" and start clean (safety-first)

**No panic in production code** except within tests or as an abort on "impossible" logic error (e.g., array bounds checked at compile time).

## Cross-Cutting Concerns

**Logging:**
- Framework: `println!()` / `eprintln!()`
- Pattern: Info → stdout, warnings/errors → stderr, progress bars via `indicatif`
- Verbosity: `--verbose` enables analysis tracing (mkvdovi/src/progress.rs)
- Quiet mode: `--quiet` suppresses non-critical output

**Validation:**
- Input file existence + readability checked early in `pipeline::run()` / `convert_file()`
- CLI args validated in `main()` before any heavy work (Rayon pool init, file walks)
- madVR file format validated in `verifier` via struct deserialization + checksums

**Authentication:**
- Not applicable; no service integration

**Interruption Handling:**
- hdr_analyzer_mvp: No graceful handling; long analysis cannot be resumed
- mkvdovi: `ctrlc::set_handler()` prints resume hint and exits cleanly, preserving temp artifacts
- Signals (SIGHUP, SIGTERM) trigger interrupt handler and preserve checkpoints

---

*Architecture analysis: 2026-08-12*
