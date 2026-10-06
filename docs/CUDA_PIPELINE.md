# CUDA analysis pipeline

This page explains how `hdr_analyzer_mvp` analyzes video on an NVIDIA GPU, and how the September 2026
optimization work raised end-to-end throughput from about 130 fps to about 325 fps (4K HDR10)
and 490 fps (4K HLG) on an RTX 4070 without changing a single bit of the output. The measurements and
test conditions are in [PERFORMANCE.md](PERFORMANCE.md).

The CUDA path needs a build with `--features cuda` and is selected with `--hwaccel cuda` (`mkvdovi`
passes it when a GPU is detected). No CUDA toolkit is needed to build: the driver and NVRTC are
loaded at runtime, and the kernel is compiled from `hdr_analyzer_mvp/src/analysis/kernels.cu` when
the analyzer starts.

## How a frame flows

```
            crop probe (7 samples, CPU decode, all cores)
                         │ committed crop rectangle
                         ▼
 packets ─► NVDEC (FFmpeg AVHWDeviceContext) ─► P010 surface in GPU memory
                                                     │ device pointers
                                                     ▼
                                    analyze_frame kernel (one launch)
                                                     │ one 18 KB result buffer
                                                     ▼
                         host: histograms → scene detection → L1 per frame
                                                     │ at an accepted scene cut only
                                                     ▼
                              download that frame → crop-stability sample
```

1. **Crop probe.** `ffmpeg_io::probe_crop` seeks to 7 points in the file, decodes on the CPU until
   it has a usable frame at each, and votes on the active picture area (letterbox bars). The winner
   becomes the crop rectangle for the whole run.
2. **Decode.** `setup_hardware_decoder` opens the stream's decoder with a CUDA `AVHWDeviceContext`.
   NVDEC writes each decoded picture to a surface in GPU memory (`AV_PIX_FMT_CUDA`, P010 for 10-bit
   sources). If that fails, it tries `hevc_cuvid`, then software decoding.
3. **Analysis.** `GpuAnalyzer::analyze_device` hands the surface's plane pointers straight to the
   `analyze_frame` kernel. The kernel computes, for the pixels inside the crop rectangle:
   - the 256-bin v5 luminance histogram and the 31-bin hue histogram;
   - a 4,096-bin PQ histogram in the selected peak domain (max-RGB by default);
   - the per-pixel luma and max-RGB sums (for exact averages) and both maxima.

   For HLG, every pixel goes through the Dolby Vision Profile 8.4 decode first: the luma reshaping
   curve, the two order-3 chroma MMR curves, and the RPU's YCbCr-to-RGB matrix. The host fills the
   luma LUT and the `dovi_params` buffer from the composer `--hlg-composer` selects (`preset` or
   `bt2100`), the same decoder the CPU path uses; both composers have the same shape, so the kernel
   and the buffer layout do not depend on the choice.
4. **Host side.** The analyzer downloads one result buffer, turns it into the frame's histograms and
   L1 values, and runs scene detection on the luminance histogram.
5. **Crop monitoring.** At each accepted scene cut the pipeline downloads that one frame and checks
   whether the picture area still matches the committed crop. That is the only per-frame pixel data
   that leaves the GPU (about 1.5% of frames on a typical episode).

## What was slow, and why

A profile of a 1,670-frame 4K clip with Nsight Systems (`nsys profile --trace=cuda`) showed that the
GPU was mostly waiting for copies. Per frame, one after another on a single thread:

| Step | Time |
|---|---:|
| Download of the decoded NVDEC surface to host memory (`av_hwframe_transfer_data`, ~12.5 MB) | ~2.2–2.5 ms |
| Upload of the same pixels back to the GPU (`GpuAnalyzer::analyze`, pageable memory) | ~2.0–2.3 ms |
| `analyze_frame` kernel | 2.43 ms (HDR10), 2.55 ms (HLG) |

The frame was already on the GPU when it was decoded. It went to host memory only because FFmpeg's
decoder and the analyzer used separate CUDA contexts, so the analyzer could not read the decoder's
memory. The analyzer's main thread showed 100% CPU, but it was spinning in synchronous copies, not
computing.

The kernel was slow for a different reason. It launched one 256-thread block per 256 pixels,
up to 32,400 blocks per 4K frame (about 26,000 for a 2.2:1 letterboxed picture). Every block cleared and then flushed 4,383 shared-memory histogram
bins, so the bookkeeping cost more than the pixels. Every pixel also did five shared atomic
operations on the same few words (two sums, a count, two maxima), which serialize, and converted
two values through `double`, which runs at 1/64 rate on consumer GPUs.

Separately, the crop probe decoded on one core: `probe_crop` opened its decoder without setting a
thread count, and libavcodec defaults to one thread. On a source with a keyframe every 10 seconds,
each probe decodes up to ~250 4K frames. This is a fixed cost per run of up to a minute, and it is
why timing a short clip end to end once suggested a speed of 16 fps.

## The three changes

Each change was a separate commit, measured on its own, and checked for bit-identical output
before the next one started.

### 1. Crop probe on all cores

`probe_crop` now calls the same `set_automatic_thread_count` helper the main decoders use. Crop-on
runs went from 76.4 s to 25.1 s (HDR10 clip) and from 39.0 s to 26.3 s (HLG clip).

A threaded decoder holds its last frames back until it is drained, so a probe that reaches the end
of the file now sends EOF and drains the decoder before giving up (the review found this). That
only matters on clips shorter than about 30 seconds, because each probe starts at 15–85% of the
duration and stops after 120 frames. The drain also recovers the frames a single-threaded decoder
held back for B-frame reordering, which the old probe missed at the end of a file.

### 2. A cheaper kernel

- **Grid-stride loop.** The kernel launches at most SMs × 8 blocks (368 on an RTX 4070; fewer for a
  small crop) and each thread walks over many pixels. The shared histograms are cleared and flushed a few hundred
  times per frame instead of tens of thousands of times.
- **Per-thread partials and warp reductions.** Each thread keeps its own sums, count and maxima in
  registers. After the loop, a warp combines them with `__shfl_xor_sync`, and one lane per warp
  does one shared atomic per value (five in all) instead of every pixel doing five. `__reduce_*_sync` was not used because it needs sm_80 or newer.
- **Fixed point in f32.** The per-pixel sums use `__float2ull_rz(__fmul_rn(x, 2^32))` instead of
  `(unsigned long long)((double)x * 2^32)`. For a finite x in [0, 1], multiplying by a power of two
  only changes the exponent, so the f32 product is exact and truncates to the same integer. A unit
  test (`f32_fixed_point_matches_the_former_f64_conversion`) checks this for every value in both
  transfer lookup tables.
- **One result buffer.** Counts and the u64 sums share one buffer (sums start at the 8-byte-aligned
  word `SUMS_WORD`), so each frame needs one memset and one download instead of two of each.

The per-pixel code (sample addressing, the PQ and HLG decodes, histogram binning) did not change.
Kernel time went from 2.43 to 0.18 ms (HDR10) and from 2.55 to 0.33 ms (HLG). End to end: 132 → 167
fps (HDR10) and 125 → 172 fps (HLG). The copies now dominated.

### 3. Analyzing NVDEC frames where they are

- **One shared context.** Before cudarc opens its context, `gpu::prepare_primary_context` sets
  device 0's primary context to `CU_CTX_SCHED_BLOCKING_SYNC`. The decoder is then created with
  `AV_CUDA_USE_PRIMARY_CONTEXT` (`ffmpeg_io::open_cuda_hwdevice_decoder`). FFmpeg accepts a shared
  primary context only with exactly that flag, and CUDA allows the flag to be set only while the
  context is inactive, so `GpuAnalyzer::new` must run before the decoder is opened. It does
  (`pipeline::run_native_analysis_pipeline`). A side effect: waits now block instead of spinning,
  which is part of why the analyzer's CPU use dropped.
- **Identity gate.** `GpuAnalyzer::device_planes` accepts a frame only if its `AVHWFramesContext`
  holds P010 surfaces and its `AVCUDADeviceContext` is the analyzer's own context. Frames from
  `hevc_cuvid` or another context, and 8-bit or 12-bit surfaces, go through the old download path,
  so an ineligible source behaves exactly as before.
- **Ordering.** FFmpeg copies each decoded picture into its surface pool on its CUDA stream, which is
  the legacy default stream unless an application sets one. The kernel launches on the same legacy
  stream, so CUDA runs it after that copy. A non-default producer stream is synchronized
  explicitly. The analyzer synchronizes after downloading the results, so the surface is no longer
  in use when the decoder reuses it.
- **Lazy downloads.** `pipeline::host_view` downloads a frame only when host code needs its pixels:
  crop-stability sampling at a scene cut (only when crop monitoring is on, so never with
  `--no-crop`), fallback crop detection when the probe committed nothing, the host-upload GPU path
  (ineligible frames, `HDR_ANALYZER_CUDA_HOST_FRAMES`) and CPU analysis. Host code must never
  receive an `AV_PIX_FMT_CUDA` frame, because its data pointers are GPU addresses.
- **Failure handling.** A frame rejected before any CUDA work is downloaded and its analysis is
  retried from host memory (an empty crop fails that check too and ends in CPU analysis), and the
  in-place path stays off for the rest of the run. A failed CUDA call is different: `GpuAnalyzer` records it (`context_faulted`), and because the
  decoder shares the context, the run stops with an error. It does not try to recover by
  downloading, because FFmpeg's CUDA download can report success after a failed copy, so a
  successful download proves nothing about the context. Rerun with `--hwaccel none` to analyze on the
  CPU. With software-decoded frames, a CUDA failure still falls back to CPU analysis as before.

End to end: 167 → 307 fps (HDR10) and 172 → 470 fps (HLG); after the review fixes (no downloads
without crop monitoring), about 325 and 490 fps. The analyzer's CPU use fell from about
one full core to 43–64% of one. The remaining limit is NVDEC itself; the HLG clip reaches a higher
rate because its bitrate is a third of the HDR10 clip's.

On a full 86,275-frame 4K HLG episode, analysis inside `mkvdovi --hwaccel cuda` took 2 min 53 s,
crop probe included. Only the 1,262 scene-cut frames were downloaded, for crop monitoring.

## Review

Before release, the work was reviewed by Codex (GPT-6 Astra) and the docs were audited against the
code. The review confirmed the stream ordering, the frame lifetimes, the `AVCUDADeviceContext`
mirror, the warp reductions and the fixed-point argument, and found three problems, all fixed:

- recovery after a CUDA failure relied on a download that can falsely succeed (now: stop the run);
- the multithreaded crop probe did not drain the decoder at the end of the file;
- scene cuts downloaded frames under `--no-crop`, where nothing reads them.

Not covered by tests yet: fault injection on the CUDA path, a Windows `--features cuda` build, and
kernel parity on unusual geometry (tiny crops, odd pitches). The negative-zero case of the
bit-pattern maxima is safe only because no input path produces −0.0; the old kernel had the same
property.

## How "bit-identical" was checked

Each commit was compared against the previous build with identical arguments, and all of these
compared equal: the `.l1.json` sidecar's `crop`, `scenes` and `frames`, and the `.bin` measurement
file byte for byte (`cmp`).

- 4K HDR10 clip and 4K letterboxed HLG clip, crop probe on;
- `--downscale 2` (the kernel samples with a stride);
- `--sample-rate 3` (mkvdovi's `fast` quality);
- the download path forced with `HDR_ANALYZER_CUDA_HOST_FRAMES=1`;
- an 8-bit HEVC source, which the kernel cannot read, so it falls back to CPU analysis as before.

This works because only order-independent values are combined across threads: integer counts, u64
fixed-point sums, and maxima of non-negative f32 values compared as bit patterns. Changing the
launch shape changes the order of those combinations, never their result. Keep it that way: a
floating-point sum across threads would make the output depend on scheduling.

## HLG chroma pass (P8, 2026-10-06)

HLG max-RGB follows the 4:2:0 structure of ETSI GS CCM 001 §5.4.2.3.3 (design:
[HLG_COMPOSER.md](HLG_COMPOSER.md) §9). On CUDA a second kernel, `compose_chroma`, runs before
`analyze_frame`: it reshapes each chroma sample once, on luma down-sampled with `[1 2 1]` across
two rows, and writes the composed Cb/Cr as interleaved f32 into a device buffer that covers the
crop's chroma rectangle plus one row above and below and one column to the right (the bilinear
taps' reach), clamped to the frame. `analyze_frame` then
upsamples it bilinearly at the stream's chroma location. With `HDR_ANALYZER_DUMP_MAX_RGB` set, a
dump buffer also receives every sample's max-RGB value (only at `--downscale 1`). PQ runs do not
launch the pass; their `.bin` and sidecar were byte-identical to the previous build on 16
configurations (CPU/CUDA × crop/no-crop × downscale 1/2 × max-RGB/luma) of a 134-frame 4K clip.

Throughput, frames ÷ wall time, `--hwaccel cuda --no-crop --disable-optimizer --transfer hlg`, best
of 2 interleaved runs, RTX 4070 under WSL2, previous build (main 71fd03d) against this one:

| Cut | Composer | downscale | before (fps) | after (fps) | change |
|-----|----------|-----------|--------------|-------------|--------|
| 3840×2160 50p, 3051 frames | bt2100 | 1 | 524.6 | 518.7 | −1.12% |
| same | bt2100 | 2 | 522.3 | 522.2 | −0.03% |
| same | preset | 1 | 520.8 | 519.1 | −0.32% |
| same | preset | 2 | 519.4 | 518.9 | −0.10% |
| 3840×2160 25p, 2976 frames | bt2100 | 1 | 494.8 | 490.8 | −0.82% |
| same | bt2100 | 2 | 494.3 | 490.7 | −0.73% |
| same | preset | 1 | 493.9 | 489.5 | −0.89% |
| same | preset | 2 | 494.8 | 490.8 | −0.81% |

The run is NVDEC-bound: downscale 1 and 2 run at the same speed. `--profile-performance` on the
25p cut (bt2100, downscale 1) reports analysis at 1855 fps before and 1888 fps after, and decode at
778 and 766 fps effective.

## Automated parity gate

`scripts/cuda-parity.sh` compares CPU analysis with CUDA analysis of the same build. Run it on the
GPU host before every PR that touches `hdr_analyzer_mvp/src/analysis/`, `kernels.cu` or
`ffmpeg_io.rs`. Hosted CI has no GPU, so nothing else runs this check.

The script encodes the synthetic clip from `tools/l1_diff/corpus/make_corpus.py` (640x360, 144
frames, six shots) as HEVC Main10 twice, once tagged PQ and once tagged HLG. It then runs
`hdr_analyzer_mvp/tests/cuda_parity.rs` against a debug build with the `cuda` feature. The HLG clip
is checked once per Profile 8.4 composer named in `HDR_ANALYZE_CUDA_PARITY_HLG_COMPOSERS`
(comma-separated `--hlg-composer` values; the script sets `preset,bt2100`, and unset means every
composer). For each clip and composer, with crop detection and with `--no-crop`, the test runs `--hwaccel none` and `--hwaccel cuda`
(both `--downscale 1 --disable-optimizer`) and requires:

- byte-identical `.bin` files;
- equal `crop`, `scenes` and `frames` in the two `.l1.json` sidecars;
- `analysis.gpu` false for the CPU run and true for the CUDA run, so a CPU fallback fails the test;
- `analysis.luminance_mapping` `pq` for the PQ clip, and for the HLG clip the composer's name:
  `dovi84-v3` (preset) or `dovi84-bt2100-v1-spec420` (bt2100).

The PQ clip is also analyzed with `--hlg-composer bt2100`, and its `.bin` must be byte-identical to
the default PQ run: the option must not touch PQ input.

The script fails when `nvidia-smi`, `python3` or an `ffmpeg` with `libx265` is missing, and the
test fails when the analyzer was built without the `cuda` feature. `--pq <file>` and `--hlg <file>`
replace a generated clip with a real HEVC 10-bit sample. Run with those options after a change to
frame geometry or crop handling. A plain `cargo test` skips the test, because the clip variables
are not set.

What it does not prove:

- The generated clips are small and synthetic, with no letterbox bars. They do not cover 4K
  frames, real crops, odd pitches, or 8-bit and 12-bit sources.
- It compares the CPU path with the GPU path of one build. It does not compare a build with the
  previous build, so a change that moves both paths the same way passes. The manual check above
  and `tools/l1_diff` cover that.
- Only `--downscale 1` is compared. With `--downscale 2` or `4` the GPU samples the full-resolution
  frame with a stride and the CPU resizes it, so the two outputs are not expected to match.
- `--sample-rate` above 1, the forced download path (`HDR_ANALYZER_CUDA_HOST_FRAMES`) and CUDA
  failures are not exercised.

## Rules for future changes

- Keep the result-buffer layout (`SUMS_WORD` and the counts before it) and the `dovi_params` layout
  (`DOVI_*`) identical in `kernels.cu` and `gpu.rs`.
- Keep `GpuAnalyzer::new` ahead of `setup_hardware_decoder`.
- Keep the HLG chroma pass identical to `frame.rs` operation for operation: the same pre-pass
  rectangle (crop chroma rect plus the taps' reach, clamped to the frame) and the same tap and bilinear
  arithmetic in f32 with `__fmul_rn`/`__fadd_rn`, no FMA.
- Never pass an `AV_PIX_FMT_CUDA` frame to host code; go through `host_view`.
- Benchmark with `--no-crop` (or subtract the crop probe) and report frames ÷ wall time.
  `--profile-performance` prints an "Analysis" rate that excludes decoding.
- Profile with `nsys profile --trace=cuda` and `nsys stats --report
  cuda_gpu_kern_sum,cuda_gpu_mem_time_sum`. With `--no-crop` on a 10-bit source, the only
  per-frame copies should be one 18 KB result download and FFmpeg's own device-to-device surface
  copy. Crop monitoring adds a download per scene cut; an unresolved crop, a forced or ineligible
  host path, and CPU fallback add one per frame.
- Check every change for identical output against the previous build (see above) before measuring
  speed.

## Possible next steps

- **Overlap decoding and analysis.** Analysis still waits for each frame before the next one is
  decoded. A small ring of in-flight frames with CUDA events would let NVDEC and the kernel run at
  the same time. The output would stay identical if frames are processed in order.
- **Crop detection on the GPU**, which would remove the remaining scene-cut downloads.
- **`--pre-denoise median3`** still switches the whole analysis to the CPU (NVDEC decoding can
  still be used). `mkvdovi` does not use it. `--peak-estimator robust` runs in the kernel: it
  adds the cross-quad difference histogram (16 x 64 counts after the u64 sums, `DIFF_WORD`),
  gathered only when that estimator is selected. Each counted pixel decodes its left neighbour
  a second time by position, so the count does not depend on thread order. Speed is unchanged
  within measurement noise (about 325 to 415 fps on 4K cuts).
- **CPU and kernel use the same f32 arithmetic** for the PQ max-RGB mix and for the histogram
  bin of a pixel (separate multiplies and adds; `__fmul_rn`/`__fadd_rn` in the kernel, no FMA).
  Before, the CPU used f64 there, and on real content an occasional pixel landed in a
  neighbouring bin: enough to change the robust estimator on 2 of 2,855 frames of one cut.
