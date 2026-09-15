# Performance

This page records analysis throughput for `hdr_analyzer_mvp`, the measurement stage that decodes the
source and produces per-frame and per-scene L1 statistics. Conversion wall time in `mkvdovi`
(extracting the video stream, generating and injecting the RPU, muxing) is a separate cost and has not
been benchmarked yet.

## Recorded result

| Path | Throughput | Hardware | Source | L1 output |
|---|---:|---|---|---|
| CPU analysis | 17 fps | Not recorded | 4K | Reference |
| CUDA analysis (`--features cuda`, `--hwaccel cuda`) | 213 fps | NVIDIA RTX 4070 | 4K | Bit-identical to the CPU path |

The CUDA path ran at approximately 12× the throughput of the CPU path on this configuration. The
measurement was taken in July 2026 with the CUDA backend merged in commit `51ebd6c` (crate version
0.3.0, committed after the `v0.3.0` tag). The figure was first written down in commit `ed7d061`. The
exact commit that was timed was not recorded.

Bit-identical L1 output shows that the CPU and CUDA implementations agree with each other. It does not
show that either one is accurate. Accuracy against synthetic truth and reference analyzers is covered
in [VALIDATION.md](VALIDATION.md).

## Not recorded

The run above was not documented well enough to reproduce. These fields were not captured:

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

Treat the 17 and 213 fps figures as one observation on one machine until a run with these fields
filled in replaces them.

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
reports less than 0.5 code, because step 2 rounds the CPU peak to a whole code. The Y-luma average row
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
