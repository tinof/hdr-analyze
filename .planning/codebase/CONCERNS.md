# Codebase Concerns

<!-- refreshed: 2026-08-12 -->

**Analysis Date:** 2026-08-12

## Tech Debt

**Security: Known vulnerability in transitive dependency**
- Issue: `RUSTSEC-2025-0119` in `indicatif` (transitive via progress bar rendering)
- Files: `deny.toml` (line 60), `mkvdovi/src/progress.rs` (indirect)
- Impact: Security advisory with no patch available yet; risk is time-limited until upstream fixes
- Fix approach: Monitor `RUSTSEC-2025-0119` status; update `indicatif` when safe upgrade available. Currently acceptable risk since ignored explicitly.

**FFmpeg build system fragility**
- Issue: `ffmpeg-next` requires C toolchain (clang/libclang), dev libraries, and `BINDGEN_EXTRA_CLANG_ARGS` env setup. CI hardcodes paths (`.github/workflows/ci.yml` lines 20-24, 49-53, 69-73).
- Files: `hdr_analyzer_mvp/Cargo.toml` (line 31), `.cargo/config.toml`, `.github/workflows/ci.yml`
- Impact: Complex cross-platform build; Linux ARM64 builds fail without custom CI setup (not automated)
- Fix approach: Document required dev deps per OS; consider vendoring FFmpeg bindings or using system-provided bindings

**L1 sidecar schema versioning is implicit and fragile** — *Resolved 2026-09-15: versioned loader with explicit errors and re-analysis; v2 schema tests on both sides.*
- Issue: Schema version in `hdr_analyzer_mvp/src/l1_sidecar.rs` (line 12, `L1_SIDECAR_VERSION = 1`) must stay in sync with what `mkvdovi/src/metadata.rs` expects. Version mismatch causes **silent fallback** to measurements-only L1 (per CLAUDE.md lines 53).
- Files: `hdr_analyzer_mvp/src/l1_sidecar.rs` (write), `mkvdovi/src/metadata.rs::load_l1_sidecar` (read)
- Impact: A future schema bump (e.g., adding new fields) breaks the contract; downstream mkvdovi silently ignores new sidecars without warning
- Fix approach: Add a versioned loader in mkvdovi with explicit warnings on version mismatch; pin schema version in unit tests for both sides

**CUDA feature is not tested in CI**
- Issue: `--features cuda` flag in hdr_analyzer_mvp builds locally but CI workflow (`.github/workflows/ci.yml`) only runs clippy on default feature set
- Files: `.github/workflows/ci.yml` (lines 41-42: `cargo clippy --workspace --all-targets`), `hdr_analyzer_mvp/src/analysis/gpu.rs`, `hdr_analyzer_mvp/src/analysis/kernels.cu`
- Impact: CUDA code path can rot without CI catching compile/clippy errors; GPU analysis correctness is validated locally but not in CI
- Fix approach: Add conditional CI step `cargo clippy -p hdr_analyzer_mvp --all-targets --features cuda -- -D warnings` (only on GPU-capable runners or optional check). Existing CLAUDE.md documents this locally but CI gap remains.

**Temp directory resumption doesn't validate input file freshness** — *Resolved 2026-09-15: `resume.json` fingerprint (input name/size/mtime, version, settings).*
- Issue: `mkvdovi/src/pipeline.rs` (lines 32-46) resumes from `mkvdovi_temp_*` on re-invocation without checking if the source input was modified since the temp dir was created
- Files: `mkvdovi/src/pipeline.rs::convert_file`
- Impact: If source file changes mid-workflow (e.g., re-download), resumption may use stale partial outputs
- Fix approach: Store source file mtime in a `.resumption.json` in temp dir; validate on resume

---

## Known Bugs

**HDR10+ metadata extraction silent fallback**
- Symptoms: HDR10+ file is detected but dynamic metadata extraction fails; tool silently falls back to HDR10Unsupported analysis
- Files: `mkvdovi/src/pipeline.rs` (lines 150-160)
- Trigger: `extract_hdr10plus_metadata()` returns `Ok(None)` (metadata extraction successful but empty); or `Err(_)` causes immediate `Ok(false)` exit
- Workaround: User won't know why HDR10+ wasn't processed until they run with `--verbose` and check logs

**Process exit handler bypasses cleanup**
- Symptoms: Ctrl+C or SIGHUP mid-conversion leaves temp dir in place with partial artifacts; exit code 130 kills the process immediately
- Files: `mkvdovi/src/main.rs` (lines 156-162)
- Trigger: Any interrupt signal during file processing
- Workaround: Documented in CLAUDE.md (line 51) — user must re-run to resume. Cleanup of temp dir only happens on successful completion (line 194).

---

## Security Considerations

**No runtime bounds checking on external tool output parsing**
- Risk: External tools (dovi_tool, mkvmerge, ffmpeg) output JSON/binary that is parsed without comprehensive validation
- Files: `mkvdovi/src/rpu_check.rs`, `mkvdovi/src/external.rs`, `hdr_analyzer_mvp/src/ffmpeg_io.rs`
- Current mitigation: serde deserialization for JSON; regex for structured parsing; some validation in `rpu_check::analyze_rpus()`
- Recommendations: Add fuzzing tests for malformed tool outputs; add validation pass after tool execution before trust

**Dependency check occurs late in pipeline**
- Risk: `mkvdovi` runs `external::check_dependencies()` only after CLI parsing (line 165 in main.rs). Early subcommands `inspect` and `composite-pipe` dispatch before this check (lines 138-145).
- Files: `mkvdovi/src/main.rs` (lines 138-168)
- Current mitigation: `inspect` and `composite-pipe` only use subset of tools; documented as "before dependency checks"
- Recommendations: Move dependency check to entry point, or document which subcommands have minimal requirements

---

## Performance Bottlenecks

**Full-resolution GPU crop probing on large files**
- Problem: `hdr_analyzer_mvp/src/analysis/gpu.rs` analyzes full-resolution frames with CUDA when GPU is available; crop probing on high-bitrate files (4K, 10-bit) creates memory pressure
- Files: `hdr_analyzer_mvp/src/analysis/gpu.rs`, `hdr_analyzer_mvp/src/crop.rs`, `hdr_analyzer_mvp/src/pipeline.rs::scale_rect`
- Cause: Full-res analysis improves accuracy but trades memory/throughput; no adaptive downscaling for GPU-constrained systems
- Improvement path: Add `--gpu-memory-limit` flag to fall back to half-res crop probing if available VRAM is low

**FFmpeg re-initialization on every convert_file call**
- Problem: `hdr_analyzer_mvp/src/ffmpeg_io.rs` initializes FFmpeg context per input file; mkvdovi spawns analyzer for each file independently (pipeline.rs line 273-284)
- Files: `hdr_analyzer_mvp/src/ffmpeg_io.rs`, `mkvdovi/src/pipeline.rs::run_hdr_analyzer`
- Cause: Context created fresh; no pooling across invocations
- Improvement path: For batch processing (multiple files), spawn analyzer once with multiple inputs; or use a named pipe queue

**Extract/inject/mux operations are CPU-bound on single file**
- Problem: Large MKV operations (mkvmerge extract/inject, ffmpeg re-encode) are single-threaded per file; no parallelism across multiple files
- Files: `mkvdovi/src/external.rs::run_command_with_progress`
- Cause: Each file's pipeline runs sequentially; external tools don't benefit from multi-file queuing
- Improvement path: Implement file-level pipeline parallelism (e.g., extract file A while analyzing file B)

---

## Fragile Areas

**Metadata L1-L11 schema synchronization**
- Files: `mkvdovi/src/metadata.rs` (CM v4.0 metadata generation), `hdr_analyzer_mvp/src/l1_sidecar.rs` (L1 output)
- Why fragile: Multiple metadata levels (L1=frame luminance, L2=trims, L6=static, L9=primaries, L11=content-type) generated by mkvdovi with defaults that must match analyzer output. A new metadata level added to spec requires coordination across binaries.
- Safe modification: Add new level to both analyzer (sidecar schema) and mkvdovi (metadata.rs) simultaneously; bump L1_SIDECAR_VERSION; add unit tests for round-trip serialization
- Test coverage: `hdr_analyzer_mvp/tests/synthetic_accuracy.rs` validates frame-level accuracy; no explicit test for full L1-L11 round-trip with mkvdovi

**GPU kernel result-buffer layout sync**
- Files: `hdr_analyzer_mvp/src/analysis/kernels.cu`, `hdr_analyzer_mvp/src/analysis/gpu.rs` (lines 488, result buffer constants)
- Why fragile: CUDA kernel writes peak stats to a buffer in a specific order; C++ and Rust code must agree on layout. Comments warn of this (CLAUDE.md line 41), but no compile-time check enforces it.
- Safe modification: Add a Rust struct that mirrors kernel layout; use `#[repr(C)]` and write unsafe tests to validate alignment
- Test coverage: Validated bit-identical L1 output (per CLAUDE.md line 41) but no unit test for buffer layout

**Resume logic relies on `.done` sentinels**
- Files: `mkvdovi/src/resume.rs`, `mkvdovi/src/pipeline.rs` (lines 32-46, 70-76)
- Why fragile: Temp directory presence + `.done` files gate which steps are re-run. A corrupted sentinel (e.g., partial file or permission issue) causes wrong steps to re-run or skip.
- Safe modification: Add checksum validation of `.done` files; explicitly log which steps are being skipped on resume
- Test coverage: No integration test for resume behavior; only documented in CLAUDE.md

---

## Scaling Limits

**Single-threaded FFmpeg demuxing for crop probes**
- Current capacity: Crop probing works on files up to ~100 GB (tested), but seeks are linear; large 4K files (2-3 hrs, 50+ GB) can take minutes to probe
- Limit: O(n) seek time per frame; no sampling optimization for very large files
- Scaling path: Implement adaptive probe sampling (fewer frames on very long files); use FFmpeg's keyframe seek hints

**Temp directory accumulation on repeated interrupts**
- Current capacity: Each interrupted run leaves `mkvdovi_temp_<stem>` directory; no cleanup of orphaned dirs
- Limit: Disk fills if user repeatedly interrupts batch processing on high-volume storage
- Scaling path: Add `--cleanup-temp` option to wipe orphaned temp dirs; implement periodic cleanup in batch mode

**Memory growth during full-resolution GPU analysis**
- Current capacity: GPU memory for full-res YUV10 frame + histogram + peak buffers ~1-2 GB per frame on 4K
- Limit: Systems with <8 GB VRAM will OOM on high-bitrate 4K content
- Scaling path: Implement streaming analysis (process tile-by-tile) or add explicit `--max-gpu-memory` with adaptive fallback

---

## Dependencies at Risk

**dovi_tool 2.3.2+ dependency**
- Risk: Undocumented changes in tool versions; mkvdovi assumes specific behavior of `inject-rpu` padding fix (per CLAUDE.md line 48)
- Impact: Upgrade to mismatched dovi_tool version produces incorrect RPU frame offsets
- Migration plan: Version-gate critical operations; add runtime version detection + warn if mismatched. Currently assumes user maintains dovi_tool ≥2.3.2

**ffmpeg-next 8.0 dependency**
- Risk: FFmpeg upstream breaking changes; transitive dependency through ffmpeg-sys-next on libavformat/libavcodec ABI
- Impact: New FFmpeg releases may break AV_NOPTS_VALUE constants or API signatures without warning
- Migration plan: Pin ffmpeg-next to 8.0 explicitly; add CI step to detect ABI breakage via cargo audit

---

## Missing Critical Features

**No dry-run mode for mkvdovi**
- Problem: `mkvdovi` immediately deletes source on success (--keep-source flag reverses this). No way to preview what would happen without side effects.
- Blocks: Users cannot batch-test configuration changes safely
- Impact: High risk of accidental data loss on misconfiguration

**No progress persistence across resume**
- Problem: Resume logic re-runs completed steps silently; no indication to user which steps were skipped
- Blocks: User confusion on large files — no ETA, unclear if still working
- Impact: Users may cancel mid-resume assuming the tool hung

---

## Test Coverage Gaps

**mkvdovi integration tests are minimal**
- What's not tested: Profile 7 MEL/FEL conversion, HDR10+ extraction, metadata generation, resume behavior, interrupt recovery
- Files: `mkvdovi/tests/integration.rs` (only 43 lines, 2 tests)
- Risk: Metadata corruption, incomplete conversions, or resume failures only surface in production
- Priority: High — add parameterized tests for each input profile (MEL, FEL, P8, HDR10+)

**CUDA feature path not tested in CI**
- What's not tested: `--features cuda` code path for GPU analysis
- Files: `hdr_analyzer_mvp/src/analysis/gpu.rs`, `.github/workflows/ci.yml` (no CUDA job)
- Risk: GPU code path can accumulate bugs without CI visibility
- Priority: Medium — add optional CI job (manual trigger) or skip on non-GPU runners

**No fuzzing for external tool output parsing**
- What's not tested: Malformed JSON from dovi_tool, mkvmerge, mediainfo; buffer overflows from binary parsing
- Files: `mkvdovi/src/rpu_check.rs`, `mkvdovi/src/external.rs`, `hdr_analyzer_mvp/src/ffmpeg_io.rs`
- Risk: Malicious or corrupted tool output could panic the tool
- Priority: Medium — add property-based tests for JSON parsing; fuzz binary format handling

**Analyzer synthetic_accuracy test is CPU-only**
- What's not tested: GPU (`--hwaccel cuda`) analysis produces bit-identical output to CPU baseline
- Files: `hdr_analyzer_mvp/tests/synthetic_accuracy.rs`, `hdr_analyzer_mvp/src/analysis/gpu.rs`
- Risk: GPU optimizations may drift from CPU baseline without detection
- Priority: Medium — add conditional test that runs on NVIDIA runners; compare GPU vs CPU measurements

**No tests for L1 sidecar round-trip**
- What's not tested: Full cycle: analyzer writes sidecar → mkvdovi reads sidecar → metadata uses L1 values
- Files: `hdr_analyzer_mvp/src/l1_sidecar.rs`, `mkvdovi/src/metadata.rs::load_l1_sidecar`
- Risk: Serialization bugs, version mismatches, or schema drifts only surface during real workflow
- Priority: High — add integration test that runs both tools end-to-end

---

## Architectural Concerns

**Early dispatch of subcommands bypasses standard setup**
- Pattern: `mkvdovi` dispatches `inspect` and `composite-pipe` before dependency checks (main.rs lines 138-145)
- Risk: These paths have different error handling, logging setup, and feature availability
- Recommendation: Consolidate early dispatch after progress setup (line 149) but before dependency checks; document which dependencies each subcommand requires

**Silent fallbacks on metadata/format detection**
- Pattern: HDR10+ detection → no metadata → falls back to HDR10Unsupported (pipeline.rs line 157); no logged reason
- Risk: User doesn't know why their file was processed differently than expected
- Recommendation: Log "fallback reason" at info level; add `--strict` mode that errors instead of falling back

---

## Environmental/Operational Issues

**Linux ARM64 binaries not built automatically**
- Files: `.github/workflows/ci.yml`, `release.yml`
- Issue: CI builds for ubuntu-24.04 (x86_64), macOS (Intel+ARM), Windows (x64) only; Linux ARM64 requires manual build
- Impact: Developers on ARM64 Linux hosts must build locally; no automated release artifacts
- Recommendation: Document cross-compilation setup; consider adding ARM64 runner (cost/complexity tradeoff)

**Stale FFmpeg binding generation**
- Files: `hdr_analyzer_mvp/Cargo.toml` (ffmpeg-next 8.0), `.cargo/config.toml`, `BINDGEN_EXTRA_CLANG_ARGS`
- Issue: `BINDGEN_EXTRA_CLANG_ARGS` is hardcoded in CI; local builds may fail if clang paths differ
- Impact: Developers with non-standard toolchain locations get cryptic build errors
- Recommendation: Add fallback clang path detection; document clang version requirements

---

## Summary of Priority Items

| Category | Item | Priority | Effort |
|----------|------|----------|--------|
| Security | Update indicatif when safe patch available | Medium | Low |
| Testing | Add mkvdovi integration tests for all profiles | High | Medium |
| Testing | Add L1 sidecar round-trip integration test | High | Low |
| Tech Debt | Add schema version mismatch warning (L1 sidecar) | Medium | Low |
| Tech Debt | Implement dry-run mode for mkvdovi | High | Medium |
| Testing | Add CUDA feature to CI (conditional) | Medium | Medium |
| Fragility | Validate input file freshness on resume | Medium | Low |

---

*Concerns audit: 2026-08-12*
