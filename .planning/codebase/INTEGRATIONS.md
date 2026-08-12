# External Integrations

**Analysis Date:** 2026-08-12

## External CLI Tools

**Required (checked at runtime):**

- **FFmpeg** - Video decoding and encoding
  - Used by: `hdr_analyzer_mvp` (frame decoding), `mkvdovi` (encode re-encode FEL/HLG)
  - Integration: `ffmpeg_next` crate wraps FFmpeg C library
  - Checked via: `ffmpeg::format::input()` and encoder enumeration in `external.rs`
  - Location: Must be in system `$PATH`
  - Dependency check: `find_tool("ffmpeg")` in `mkvdovi/src/external.rs:check_dependencies()`

- **mkvmerge** - Matroska container manipulation
  - Used by: `mkvdovi` pipeline for muxing final DV metadata into MKV
  - Integration: Direct command invocation via `std::process::Command`
  - Checked via: `find_tool("mkvmerge")` in `external.rs`
  - Location: Must be in system `$PATH` or MKVToolNix installation
  - Invoked by: `pipeline.rs` during metadata injection phase

- **dovi_tool** 2.3.2+ - Dolby Vision RPU metadata operations
  - Used by: `mkvdovi` for Profile 7/8 metadata generation, extraction, injection
  - Integration: Direct CLI invocation via `std::process::Command`
  - Checked via: `find_tool("dovi_tool")` in `external.rs`
  - Location: Must be in system `$PATH`
  - Dependency check: Hard failure if not found (`check_dependencies()` bails)
  - Notes: Version 2.3.2+ required for RPU padding fixes in `inject-rpu`

- **mediainfo** OR **ffprobe** (one required)
  - Used by: `mkvdovi` for media metadata extraction (codecs, HDR10+ markers, color space)
  - Integration: Direct CLI invocation returning JSON output (`mediainfo --Output=JSON`)
  - Checked via: `find_tool("mediainfo")` or `find_tool("ffprobe")` in `external.rs`
  - Location: Must be in system `$PATH`
  - Fallback logic: Prefers `mediainfo`; uses `ffprobe` if `mediainfo` absent
  - Used in: `metadata.rs` for source primaries detection, HDR10+ marker detection

**Optional (failure handled gracefully):**

- **hdr10plus_tool** - HDR10+ metadata extraction
  - Used by: `mkvdovi` when input is HDR10+ (not required for HDR10/HLG)
  - Integration: Direct CLI invocation
  - Checked via: `find_tool("hdr10plus_tool")` in pipeline
  - Location: Must be in system `$PATH` when processing HDR10+ inputs
  - Behavior: If not found and input is HDR10+, pipeline falls back to HDR10 analysis via `hdr_analyzer_mvp`

- **nvidia-smi** - NVIDIA GPU detection
  - Used by: `mkvdovi` for auto-detection of NVIDIA GPU when `--hwaccel auto`
  - Integration: Query device list via `nvidia-smi -L`
  - Checked via: `detect_nvidia_gpu()` in `external.rs`
  - Special handling: WSL2 may have binary at `/usr/lib/wsl/lib/nvidia-smi` instead of PATH
  - Behavior: If not found or no GPU detected, falls back to CPU analysis

- **hdr_analyzer_mvp** (self-invocation)
  - Used by: `mkvdovi` to analyze HDR10 base layers and generate L1 measurements
  - Integration: Direct subprocess invocation
  - Discovery: Prefers `target/release/hdr_analyzer_mvp` relative to cwd; falls back to PATH
  - CUDA forwarding: If `mkvdovi --hwaccel cuda` is set, passes `--hwaccel cuda` to analyzer
  - Location: `pipeline::analyzer_executable()` in `mkvdovi/src/pipeline.rs`

## Media Processing Libraries

**FFmpeg Integration** (`ffmpeg-next` crate):
- **Bindings**: C FFmpeg library via `ffmpeg-sys-next`
- **Decode paths**:
  - Software decode: All platforms via libavcodec
  - Hardware decode (optional): NVIDIA NVDEC when CUDA feature enabled
    - Configured via `AVHWDeviceContext` in `hdr_analyzer_mvp/src/ffmpeg_io.rs`
- **Data access**:
  - Direct YUV420P10LE frame buffers for 10-bit analysis
  - Transfer function metadata extraction (PQ, HLG detection)
  - Codec/bitrate/resolution via format context

**Dolby Vision Metadata** (`dolby_vision` crate 3.3.2):
- **RPU parsing/generation**: Dolby Vision RPU frame structures
- **Used by**: `mkvdovi` for L1/L2/L5/L6/L9/L11/L254 metadata levels
- **CM v4.0**: Default metadata version generation
- **Profile support**: Profile 7 (MEL+FEL), Profile 8.1 (P8)

**MadVR Measurements** (`madvr_parse` crate 1.0.3):
- **Format**: `.bin` binary measurement file format
- **Used by**: `hdr_analyzer_mvp` (writer), `verifier` (reader/validator)
- **Location**: `hdr_analyzer_mvp/src/writer.rs` writes measurements
- **Schema**: Per-frame MaxCLL/APL, per-scene statistics, histogram bins (256-bin v5/v6 compatible)

## GPU Acceleration (Optional)

**NVIDIA CUDA** (optional `cuda` Cargo feature):
- **Runtime**: CUDA Toolkit 12.0+
- **Components**:
  - NVRTC - NVIDIA Runtime Compiler for kernel compilation at runtime
  - CUDA driver - GPU execution
  - NVDEC - Hardware video decode (via FFmpeg AVHWDeviceContext)
- **Integration**: `cudarc` crate for bindings + `libloading` for dynamic linking
- **Kernel code**: `hdr_analyzer_mvp/src/analysis/kernels.cu` (CUDA C)
- **Activation**:
  - Runtime detection: `detect_nvidia_gpu()` in `external.rs`
  - Feature flag: Build with `--features cuda`
  - Command-line: `--hwaccel cuda` or auto-detection with `--hwaccel auto`
- **Analysis kernel**: Single-launch GPU kernel computing:
  - v5 histogram (256 bins)
  - Hue histogram
  - 4096-bin peak-domain PQ histogram
  - Max-RGB peaks per-frame
  - Exact per-pixel Y means
- **Result buffer**: Layout kept in sync between `kernels.cu` and `gpu.rs` buffer constants
- **Fallback**: Automatic CPU fallback at every stage if GPU unavailable or error occurs

## Data Files & Formats

**Input Video Formats:**
- Supported codecs: H.264, HEVC, VP9, AV1 (via FFmpeg)
- Bit depths: 8-bit (SDR), 10-bit (HDR10/HLG), 12-bit support
- HDR metadata: HDR10 static, HDR10+ dynamic, HLG ARIB STD-B67

**Output Formats:**
- `.bin` - MadVR measurement file (binary, structured)
  - Written by: `hdr_analyzer_mvp`
  - Read by: `verifier`, `mkvdovi`, `dovi_tool`
- `.l1.json` - L1 measurement sidecar (JSON, per-scene/per-frame luminance)
  - Written by: `hdr_analyzer_mvp` (optional, default off)
  - Read by: `mkvdovi` for source-honest L1 metadata generation
  - Schema: Versioned (version: 1); schema change requires coordinated version bump
- `.mkv` - Final output container (Matroska)
  - Contains: Base layer video + Dolby Vision RPU track
  - Metadata: L1/L2/L5/L6/L9/L11/L254 levels (CM v4.0)

**Temporary Files:**
- `mkvdovi_temp_*` - Resume-checkpoint sentinels (`.done` files mark completed steps)
  - Extracted BL/EL streams, intermediate encodes
  - Cleaned up on successful completion
  - Reused on re-run for interrupt recovery

## External System Dependencies

**At Runtime:**
- System libc (GLIBC on Linux, system frameworks on macOS/Windows)
- FFmpeg shared libraries: libavformat, libavcodec, libavutil, libavfilter, libswscale
- MKVToolNix binary (mkvmerge)
- Dolby Vision CLI tool (dovi_tool)
- Media introspection (mediainfo or ffprobe)

**Build-Time Only:**
- LLVM/Clang 12+ (for bindgen code generation)
- pkg-config (library discovery)
- C compiler (cc crate)

## Command Invocation Patterns

**Location:** `mkvdovi/src/external.rs`

**Key functions:**
- `find_tool(tool_name)` - Cross-platform tool discovery (uses `which` on Unix, `where` on Windows)
- `check_dependencies()` - Validates all required tools at startup
- `run_command_with_progress()` - Long-running tool execution with byte-progress bar and stall detection
- `run_command_with_spinner()` - Short operations with spinner feedback
- `run_command_live()` - Stream tool output to terminal and log file
- `get_command_output()` - Capture tool stdout as string (used for `mediainfo --Output=JSON`, `ffmpeg -encoders`)
- `detect_nvidia_gpu()` - Query `nvidia-smi -L` for GPU availability
- `ffmpeg_has_encoder(name)` - Check if FFmpeg supports a specific encoder
- `analyzer_has_cuda_feature(exe)` - Check if `hdr_analyzer_mvp` binary was built with CUDA support

**Error Handling:**
- Missing required tools → Fatal error, bail with diagnostic message
- Tool execution failure → Logged to file, spinner shows error, pipeline continues or fails based on context
- Stall timeout (default 300s) → Warning shown, but doesn't halt process (watchdog only)

## Workflow Integration Points

**`hdr_analyzer_mvp` → `mkvdovi`:**
- Analyzer writes `.bin` measurements
- Optional: Analyzer writes `.l1.json` L1 sidecar (schema versioned)
- mkvdovi reads both files via `metadata::load_l1_sidecar()` for source-honest per-scene L1
- Contract: L1 schema version mismatch → silently falls back to measurements-only L1

**`mkvdovi` → External tools:**
1. FFmpeg decode → extract BL/EL frames → store in temp directory
2. hdr_analyzer_mvp (if needed) → compute measurements → read `.bin` and `.l1.json`
3. dovi_tool inject → apply Dolby Vision metadata to BL stream
4. ffmpeg encode (optional FEL re-encode for Profile 7) → apply NLQ processing
5. mkvmerge → package BL + RPU into final MKV

**Resume Checkpoints:**
- Each major step creates a `.done` sentinel file
- Re-run auto-detects completed steps and skips them
- Use `--no-resume` to force clean run

## Network & Remote Integration

**Not applicable:** This codebase does not use:
- HTTP/HTTPS APIs
- Remote database connections
- Cloud storage integrations
- Webhook callbacks
- Web services

All operations are local filesystem I/O and subprocess-based tool execution.

## Authentication & Secrets

**Not applicable:** This is a CLI tool with no authentication mechanism.
- No API keys, tokens, or credentials required
- No configuration of remote services
- Environment variables used only for testing (not secrets)

## Logging & Observability

**Approach:**
- CLI progress: `indicatif` progress bars and spinners (TTY auto-detect)
- Tool output: Logged to files per operation in temp directory
- Verbose mode: `-v/--verbose` flag streams all output to terminal
- No structured logging library; plain stdout/stderr

**Log Locations** (mkvdovi):
- `mkvdovi_temp_<uuid>/*.log` - Per-step operation logs (FFmpeg, dovi_tool, mkvmerge output)
- Stderr: Progress bars and errors
- Stdout: Summary messages and diagnostics (controlled by `-q/--quiet`, `-v/--verbose`)

---

*Integration audit: 2026-08-12*
