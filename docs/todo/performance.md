# Performance Profiling Notes

The native analyzer now parallelizes histogram aggregation across frame rows with Rayon. Each worker owns a local histogram buffer and the results are reduced without locks, so scaling is near-linear on multi-core CPUs.

## Quick profiling workflow

1. Build the release binary:
   ```bash
   cargo build --release -p hdr_analyzer_mvp
   ```
2. Run a representative clip (ideally 4K HDR10) and print metrics:
   ```bash
   ./target/release/hdr_analyzer_mvp \
       --input sample_hdr10.mkv \
       --profile-performance \
       --analysis-threads 8
   ```
3. The tool prints:
   - Overall FPS (wall clock)
   - Analysis-only wall time and effective FPS
   - Decode & IO wall time
   - The active Rayon worker count

To compare against a single-thread baseline, run the same command with `--analysis-threads 1`. Capture both outputs to confirm the ≥1.7× speedup target on 8-core machines.

## Tips

- Combine with `--downscale 2` or `--downscale 4` when you need extra throughput for quick inspections.
- On constrained VMs (e.g., 4 vCPU Ampere), match `--analysis-threads` to the allocated cores to avoid oversubscription.
- When profiling multiple files, consider piping logs to a CSV by redirecting stdout and parsing the FPS lines.

## Future work

- Investigate SIMD (NEON/AVX2) within the per-row loops once profiling highlights a bottleneck that parallelism alone cannot hide.
- Add automated benchmarks in CI once large sample clips can be stored in an artifact bucket.
