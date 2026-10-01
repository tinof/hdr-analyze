# Performance

This page records analysis throughput for `hdr_analyzer_mvp`, the measurement stage that decodes the
source and produces per-frame and per-scene L1 statistics. How the GPU path works, and how it was made
faster, is explained in [CUDA_PIPELINE.md](CUDA_PIPELINE.md).

Conversion wall time in `mkvdovi` (extracting the video stream, generating and injecting the RPU,
muxing) is a separate cost. One observation, not a benchmark: a 4K HLG episode (8.25 GB, 86,275
frames) on the machine below, from a local ext4 disk, took 2 min 53 s to analyze (crop probe
included), 1 s to generate the RPU, 48 s to extract the base layer, 1 min 25 s to inject the RPU and
49 s to mux.

## Recorded result: CUDA analysis, September 2026

End-to-end throughput is frames ÷ wall-clock time of the whole analyzer run, measured with
`--no-crop` so the fixed crop-probe cost (reported separately) does not dominate a short clip. Each
figure is the second of two runs, from a warm file cache.

| Build | 4K HDR10 clip | 4K HLG clip | Analyzer CPU use |
|---|---:|---:|---:|
| 0.5.0 (`95ea9a5`), before the changes below | 132 fps | 125 fps | 107–117% of one core |
| + crop probe on all cores | — | — | — |
| + grid-stride kernel with warp reductions | 167 fps | 172 fps | 107% |
| + NVDEC frames analyzed in place | 307 fps | 470 fps | 43–64% |
| + no frame downloads without crop monitoring (final) | **325 fps** | **493 fps** | 43–66% |

Per-frame GPU time from Nsight Systems (`nsys stats --report cuda_gpu_kern_sum,cuda_gpu_mem_time_sum`):

| Per 4K frame | Before | After |
|---|---:|---:|
| `analyze_frame` kernel, HDR10 / HLG | 2.43 / 2.55 ms | 0.16–0.18 / 0.33 ms |
| Download of the decoded frame to host memory | ~2.2–2.5 ms | none with `--no-crop`; with crop monitoring, only scene-cut frames (13 of 1,668 HDR10, 33 of 2,976 HLG) |
| Upload of the same frame back to the GPU | ~2.0–2.3 ms | none (the only uploads left are 3 LUTs at startup) |
| Result download (18 KB) | 2 copies | 1 copy |

FFmpeg's own device-to-device copy of each NVDEC surface (~12.5 MB, ~0.04 ms) is unchanged.

The HLG kernel costs more than the HDR10 one because every pixel goes through the Profile 8.4 chroma
MMR decode for max-RGB. The HLG clip also decodes faster: its bitrate is a third of the HDR10 clip's.

The crop-probe change does not affect `--no-crop` runs, so it has no row values. Whole run with the
crop probe on, before → after all three changes: HDR10 76.4 s → 16.2 s, HLG
39.0 s → 9.7 s. The probe decodes 7 short runs, each from the preceding keyframe; it used to decode on
one core. The probe change alone took these runs to 25.1 s and 26.3 s.

L1 output of every build in the table is identical: the `.l1.json` crop, scenes and frames and the
`.bin` file compare equal, on both clips, and also with `--downscale 2`, with `--sample-rate 3`, with
the host-download path forced (`HDR_ANALYZER_CUDA_HOST_FRAMES=1`), and on an 8-bit source (which the
kernel cannot read, so it falls back to CPU analysis as before).

| Field | Value |
|---|---|
| CPU | 13th Gen Intel Core i5-13400F |
| GPU and driver | NVIDIA GeForce RTX 4070, driver 616.92 |
| NVRTC | 12.x (`libnvrtc.so.12`, loaded first) |
| FFmpeg libraries | 6.1.1 (Ubuntu `libavcodec60` 7:6.1.1-3ubuntu5) |
| Operating system | Ubuntu 24.04.4 LTS on WSL2 (kernel 6.6.87.2) |
| HDR10 clip | HEVC Main 10, PQ, 3840×2160, 63.9 Mb/s, 66.8 s, 1,668 frames analyzed |
| HLG clip | HEVC Main 10, HLG, 3840×2160, 19.1 Mb/s, 119.8 s, 2,976 frames analyzed |
| Decode path | NVDEC through FFmpeg `AVHWDeviceContext` |
| `--downscale` / `--sample-rate` | 1 / 1 |
| Crop | Off for the throughput runs; on for the crop-probe times and the parity runs |
| Peak estimator | `max` |
| Source storage | Local ext4 disk |

## Earlier result: July 2026

| Path | Throughput | Hardware | Source | L1 output |
|---|---:|---|---|---|
| CPU analysis | 17 fps | Not recorded | 4K | Reference |
| CUDA analysis (`--features cuda`, `--hwaccel cuda`) | 213 fps | NVIDIA RTX 4070 | 4K | Bit-identical to the CPU path |

The 213 fps figure is the "Analysis" stage rate printed by `--profile-performance`, which excludes
decoding and the host copies. Frames ÷ wall time on the same kind of source was about 125–135 fps with
that build. The measurement was taken with the CUDA backend merged in commit `51ebd6c` (crate version
0.3.0, committed after the `v0.3.0` tag). The figure was first written down in commit `ed7d061`. The
exact commit that was timed was not recorded.

Bit-identical L1 output shows that the CPU and CUDA implementations agree with each other. It does not
show that either one is accurate. Accuracy against synthetic truth and reference analyzers is covered
in [VALIDATION.md](VALIDATION.md).

### Not recorded for the July run

The July run was not documented well enough to reproduce. These fields were not captured:

| Field | Value |
|---|---|
| CPU model | Not recorded |
| GPU driver version | Not recorded |
| NVRTC / CUDA version | Not recorded |
| FFmpeg version | Not recorded |
| Operating system | Not recorded |
| Source codec | Not recorded |
| Source bitrate | Not recorded |
| Source duration | Not recorded |
| Source frame count | Not recorded |
| Decode path (NVDEC or software) | Not recorded |
| `--downscale` / `--sample-rate` | Not recorded |
| Crop | Not recorded |
| Peak estimator | Not recorded |
| Preparation time | Not recorded |
| Total conversion time | Not recorded |
| Temporary storage used | Not recorded |

Treat the 17 and 213 fps figures as one observation on one machine.

## How to reproduce

### Build

```bash
cargo build --release -p hdr_analyzer_mvp --features cuda
./target/release/hdr_analyzer_mvp --version   # must contain +cuda
cargo build --release --manifest-path tools/l1_diff/Cargo.toml
```

One `--features cuda` binary can run both paths, so the CPU and CUDA runs use the same build.

### Run both paths with identical settings

The commands below are for bash on Linux, where the CUDA path runs. `/usr/bin/time -v` is GNU time.
macOS has only the CPU path; there, install GNU time with `brew install gnu-time` and replace
`/usr/bin/time -v` with `gtime -v`, or time the run with hyperfine. `set -o pipefail` makes a
failed analyzer run fail the whole pipeline, which `tee` would otherwise hide.

```bash
set -euo pipefail
A=./target/release/hdr_analyzer_mvp
SRC=source.mkv
COMMON=(--downscale 1 --sample-rate 1 --peak-estimator max --profile-performance)

/usr/bin/time -v "$A" -i "$SRC" -o cpu.bin  --hwaccel none "${COMMON[@]}" 2>&1 | tee cpu.log
/usr/bin/time -v "$A" -i "$SRC" -o cuda.bin --hwaccel cuda "${COMMON[@]}" 2>&1 | tee cuda.log
```

Notes on these commands:

- The September 2026 throughput table was measured with `--no-crop` added to `COMMON`, so the crop
  probe (a fixed cost per run) does not skew short clips. Keep the crop on when checking that the two
  paths agree.
- The analyzer has no dedicated `none` value. `--hwaccel none` is treated as an unknown type and
  decodes in software, which is the CPU path. Omitting `--hwaccel` gives the same result.
- `--pre-denoise median3` and `--peak-estimator robust` are CPU-only and turn off the CUDA kernel.
  Keep them out of a CPU/CUDA comparison.
- `--downscale` means a resize on the CPU path and a sampling stride on the CUDA path. Leave it at 1
  unless both runs are meant to measure that trade-off.
- Run each command at least twice and keep the later timing, so both paths read the source from a
  warm file cache. With [hyperfine](https://github.com/sharkdp/hyperfine) installed,
  `hyperfine --warmup 1 --runs 3 '<cpu command>' '<cuda command>'` does this for you.

Throughput is frame count divided by the `Elapsed (wall clock) time` line from `/usr/bin/time -v`.
`--profile-performance` also prints per-stage throughput at the end of the run.

### What to record

| Field | How to capture it |
|---|---|
| CPU model | `lscpu \| grep 'Model name'` |
| GPU and driver version | `nvidia-smi --query-gpu=name,driver_version --format=csv` |
| NVRTC / CUDA version | `ldconfig -p \| grep nvrtc` (the library the analyzer loads at runtime) |
| FFmpeg version | `ffmpeg -version \| head -1` |
| Operating system | `uname -a` and the distribution release |
| Source codec, bitrate, duration, frame count | `ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,width,height,bit_rate,nb_frames:format=duration,bit_rate "$SRC"` |
| Decode path | The analyzer log: `CUDA NVDEC active through FFmpeg AVHWDeviceContext`, `CUDA decode active through hevc_cuvid fallback`, or a software decoder message |
| Downscale, sample rate, GPU use | `jq .analysis cpu.bin.l1.json cuda.bin.l1.json` |
| Crop | `jq .crop cpu.bin.l1.json` |
| Peak estimator | `jq .peak_estimator cpu.bin.l1.json` |
| Preparation time | Wall time of any step before analysis (for example extracting a sample). Report it separately |
| Total conversion time | Wall time of `mkvdovi` on the same source, reported separately from analyzer time |
| Temporary storage | Peak size of the `mkvdovi_temp_*` directory, if a conversion is timed |
| Wall time and peak memory | `Elapsed (wall clock) time` and `Maximum resident set size` from `/usr/bin/time -v` |

### Check that the outputs agree

`tools/l1_diff` compares an analyzer run against a per-frame reference CSV
(`frame,min_pq,max_pq,avg_pq`, 12-bit PQ codes). Turn the CPU run into that CSV, then score the CUDA
run against it:

```bash
L1_DIFF=tools/l1_diff/target/release/l1_diff

# 1. Dump the CPU run per frame (l1_diff needs a reference, so feed it zeros).
n=$(jq '.frames.min_pq_12bit | length' cpu.bin.l1.json)
{ echo frame,min_pq,max_pq,avg_pq; seq 0 $((n - 1)) | sed 's/$/,0,0,0/'; } > zeros.csv
$L1_DIFF --ours cpu.bin --reference zeros.csv --csv cpu_frames.csv > /dev/null

# 2. Keep min, max and max-RGB average as the reference.
awk -F, 'NR == 1 { print "frame,min_pq,max_pq,avg_pq"; next }
         { printf "%s,%s,%d,%s\n", $1, $3, $5 + 0.5, $8 }' cpu_frames.csv > cpu_l1.csv

# 3. Score the CUDA run.
$L1_DIFF --ours cuda.bin --reference cpu_l1.csv
```

When the two paths agree, the minimum and max-RGB average rows report 0 error, and the peak row
reports about half a code at most, because `l1_diff` writes the CPU peak with one decimal and step 2
rounds it to a whole code. The Y-luma average row
compares a different quantity against the max-RGB reference and is expected to differ. For a direct
check of the per-scene values, compare the sidecars:

```bash
diff <(jq -S '{crop, scenes, frames}' cpu.bin.l1.json) \
     <(jq -S '{crop, scenes, frames}' cuda.bin.l1.json) && echo "sidecar L1 identical"
```

## Scope

This page does not compare speed with Dolby's `cm_analyze`. A comparison would be added once both
workflows have been run on the same source and hardware under documented conditions. It would report
preparation time, such as building an intermediate file for `cm_analyze`, separately from analyzer
time. This analyzer reads supported source files directly, without preparing a ProRes intermediate,
so the two workflows do not start from the same point.
