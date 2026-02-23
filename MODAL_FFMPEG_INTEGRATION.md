# Integrating modal-ffmpeg for Accelerated Encoding

This document explains how to use the `modal-ffmpeg` pipeline at `/home/ubuntu/modal-ffmpeg/` to offload FFmpeg HEVC encoding to Modal.com cloud instances from the hdr-analyze workflow.

## What modal-ffmpeg provides

A Modal.com-based remote FFmpeg encoding service with three workflows:

| Mode | Encoder | Runs on | Speed | Quality |
|------|---------|---------|-------|---------|
| `composite` | `hevc_nvenc` | L4 GPU | Fast (93s encode for 2h FEL) | NVENC p5, compositing + encode in one pass |
| `hevc` | `hevc_nvenc` | L4 GPU | Fast (~5s for 1080p 11s clip) | Hardware encoder, good quality |
| `x265` | `libx265` | 8-core CPU (default) | Slower (~18s same clip) | Software encoder, excellent quality |

All modes use FFmpeg 8.x (BtbN master builds). Files stream via a Modal Volume (`ffmpeg-working`) — no file size limit, low memory usage. Each job is isolated by UUID and cleaned up after completion.

## When to use this

The hdr-analyze pipeline **analyzes** video and generates Dolby Vision metadata. It does NOT encode. The encoding step — where you produce the final HEVC file with DV metadata applied — is where modal-ffmpeg fits in.

**Use modal-ffmpeg when:**
- **Profile 7 FEL conversion** (`--mode composite`): Uploads BL+EL+RPU, runs compositing via cross-compiled mkvdolby binary, pipes to NVENC — the primary use case
- Re-encoding a source file to HEVC after analysis/metadata generation
- The Oracle instance CPU is too slow for x265 software encoding of large files
- You need GPU-accelerated HEVC encoding and no local GPU is available

**Do NOT use modal-ffmpeg for:**
- The frame decoding/analysis step (hdr-analyze uses native ffmpeg-next library bindings — this is I/O bound and requires sequential frame access)
- Small files where Volume upload/download overhead exceeds encoding time savings

## CLI interface

The entry point is at `/home/ubuntu/modal-ffmpeg/src/modal_ffmpeg.py`. It is invoked via `modal run`:

```bash
# Composite mode (Profile 7 FEL → HEVC, primary use case)
modal run /home/ubuntu/modal-ffmpeg/src/modal_ffmpeg.py \
    --mode composite \
    --bl /path/to/FEL_BL.hevc \
    --el /path/to/FEL_EL.hevc \
    --rpu /path/to/FEL_RPU.bin \
    --output /path/to/output.mkv \
    --width 3840 --height 2160 \
    --fps-num 24000 --fps-den 1001 \
    --preset p5 --qp 18 \
    --master-display "G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,50)" \
    --max-cll "1000,400"

# GPU HEVC encoding (hardware, fast)
modal run /home/ubuntu/modal-ffmpeg/src/modal_ffmpeg.py \
    --mode hevc \
    --input /path/to/input.mkv \
    --output /path/to/output.mkv \
    --preset p5 \
    --qp 18

# CPU x265 encoding (software, high quality)
modal run /home/ubuntu/modal-ffmpeg/src/modal_ffmpeg.py \
    --mode x265 \
    --input /path/to/input.mkv \
    --output /path/to/output.mkv \
    --preset slow \
    --crf 18
```

### Composite mode parameters

| Flag | Default | Description |
|------|---------|-------------|
| `--bl` | required | Path to base layer HEVC file |
| `--el` | required | Path to enhancement layer HEVC file |
| `--rpu` | required | Path to RPU binary file |
| `--output` | required | Output MKV path |
| `--width` / `--height` | required | Video dimensions |
| `--fps-num` / `--fps-den` | `24000`/`1001` | Frame rate as fraction |
| `--preset` | `p5` | NVENC preset: p1 (fastest) to p7 (best quality) |
| `--qp` | `18` | Constant QP. Lower = better quality, larger file |
| `--master-display` | none | HDR10 mastering display metadata string |
| `--max-cll` | none | MaxCLL,MaxFALL string (e.g. `1000,400`) |

### HEVC (GPU) parameters

| Flag | Default | Description |
|------|---------|-------------|
| `--preset` | `p5` | NVENC preset: p1 (fastest) to p7 (best quality) |
| `--qp` | `18` | Constant QP. Lower = better quality, larger file |
| `--scale` | none | Output resolution, e.g. `3840x2160` |

### x265 (CPU) parameters

| Flag | Default | Description |
|------|---------|-------------|
| `--preset` | `medium` | x265 preset: ultrafast/superfast/veryfast/faster/fast/medium/slow/slower/veryslow |
| `--crf` | `18` | Constant rate factor. Lower = better quality, larger file |
| `--scale` | none | Output resolution, e.g. `1920x1080` |

### Output naming

If `--output` is omitted, output is written to the current directory as `{input_stem}_{mode}.mkv`.

## How mkvdolby uses modal-ffmpeg (composite mode)

The `--fel-encoder modal` flag in mkvdolby automatically invokes modal-ffmpeg's composite mode. The flow:

1. mkvdolby demuxes the FEL source into BL + EL + RPU locally
2. mkvdolby calls `modal run modal_ffmpeg.py --mode composite` with BL/EL/RPU paths
3. Modal uploads all three files to a Volume
4. On the L4 GPU instance, Modal runs a cross-compiled x86_64 `mkvdolby composite-pipe` binary
5. `composite-pipe` reads BL+EL+RPU, applies NLQ compositing, writes raw YUV to stdout
6. stdout is piped directly to `ffmpeg hevc_nvenc` — no intermediate files on Modal
7. The encoded MKV is downloaded back to the local machine

This is implemented in `fel_composite.rs::encode_via_modal()`. HDR10 metadata (mastering display, MaxCLL/MaxFALL) is passed as CLI flags and applied as VUI color tags (no BSF required).

## Typical workflow integration

### Profile 7 FEL → Profile 8.1 (primary use case)

```
1. Source video (Profile 7 FEL dual-layer .mkv)
         │
         ▼
2. mkvdolby --fel-encoder modal source.mkv
   Internally:
   a. Extract HEVC → demux BL + EL + RPU (local)
   b. Upload BL+EL+RPU to Modal         ◄── modal-ffmpeg composite mode
   c. Composite + NVENC encode on L4 GPU
   d. Download encoded MKV
   e. Generate Profile 8.1 RPU (local)
   f. Inject RPU + mux final MKV (local)
         │
         ▼
3. Output: Profile 8.1 Dolby Vision .mkv
```

### HDR10/HDR10+ → Profile 8.1 (standard workflow)

```
1. Source video (HDR10/HDR10+ .mkv)
         │
         ▼
2. mkvdolby source.mkv
   (All processing local — modal-ffmpeg not used)
         │
         ▼
3. Output: Profile 8.1 Dolby Vision .mkv
```

## Performance and overhead

Wall times include fixed overhead for Modal app init + Volume upload/download.
Upload and download are streamed, so overhead scales with file size but remains
small relative to encode time for large files.

### Composite mode (Profile 7 FEL, 4K ~2 hour film)

| Phase | Time | Notes |
|-------|------|-------|
| Upload (BL+EL+RPU) | ~75s | ~700 MB total for 2h film |
| Composite + NVENC encode | ~93s | L4 GPU, p5 preset, qp=18 |
| Download (encoded MKV) | ~8s | |
| **Total** | **~185s** | vs hours locally on ARM |

### Local vs Modal (x265 slow preset, legacy modes)

| Where | 1080p wall time | 4K wall time |
|-------|----------------|-------------|
| Local (4 ARM cores) | 58.5s | 246.3s |
| Modal 8 CPU (x265) | 24.2s (2.4x faster) | ~83s (3.0x faster) |
| Modal L4 GPU (hevc_nvenc) | ~5s | ~20s |

## File size handling

Files stream through a Modal Volume — no in-memory size limit. Production REMUX files (tens of GB) are supported.

- Upload/download are streamed (chunked), so memory usage stays low regardless of file size
- GPU instances have sufficient memory for 4K compositing + encoding
- Timeouts are sized for large files: GPU 3600s (1hr), CPU 7200s (2hr)

## Cross-compiled binary for composite mode

The composite mode requires a cross-compiled x86_64 `mkvdolby` binary on the Modal Volume, since Modal runs on x86_64 Linux while the dev host is ARM64:

```bash
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-gnu
cargo zigbuild --release -p mkvdolby --target x86_64-unknown-linux-gnu
# Upload to Modal volume (done by modal_ffmpeg.py automatically)
```

The `.cargo/config.toml` has empty `rustflags` for the x86_64 target to override ARM-specific flags.

## HDR10 metadata handling

For HEVC and composite modes, HDR10 metadata is applied via **VUI color tags only** (no BSF):
- `color_primaries=bt2020`, `color_trc=smpte2084`, `colorspace=bt2020nc`, `color_range=tv`
- The `hevc_metadata` BSF (`master_display`/`max_cll`) is NOT used — Modal's FFmpeg build may not support it
- This is acceptable because the Dolby Vision RPU controls display mapping; HDR10 SEI in the base layer is non-critical

## Prerequisites

- Modal CLI installed and authenticated: `pip install modal && modal token set`
- Python 3.12+ available (for `modal run`)
- Network access to Modal.com from this Oracle instance
- For composite mode: cross-compiled x86_64 mkvdolby binary on the Modal Volume

## Cost

Modal charges per-second for actual compute usage (no idle charges). Modal Volumes are free — no storage cost for transient job files.

| Resource | Rate |
|----------|------|
| CPU | $0.0000131/core/sec ($0.047/core/hr) |
| Memory | $0.00000222/GiB/sec ($0.008/GiB/hr) |
| L4 GPU | ~$0.000222/sec (~$0.80/hr) |

### Estimated cost per encode

| Mode | Example | Approx cost |
|------|---------|-------------|
| Composite (L4 GPU) | 4K 2h FEL film, 93s encode | ~$0.02 |
| HEVC (L4 GPU) | 4K 11s clip | ~$0.005 |
| x265 (8 CPU) | 4K 11s clip | ~$0.009 |

Costs scale with encode duration. A 2-hour 4K FEL composite costs roughly $0.02 in GPU time.
