# Codebase Structure

**Analysis Date:** 2026-08-12

## Directory Layout

```
hdr-analyze/
├── .github/                           # GitHub configuration
│   ├── workflows/                     # CI/CD workflows
│   │   └── ci.yml                     # Lint, test, cross-platform build
│   └── ISSUE_TEMPLATE/                # GitHub issue templates
├── .claude/                           # Claude Code configuration
│   ├── skills/                        # GSD skills (custom tooling)
│   ├── hooks/                         # Auto-execution hooks (LSP, shell)
│   ├── agents/                        # Agent tool definitions
│   ├── commands/                      # Custom slash commands
│   └── gsd-core/                      # GSD system integration
├── .planning/                         # Planning and codebase docs
│   └── codebase/                      # Generated architecture/structure docs
├── .cargo/                            # Cargo configuration (build flags)
├── coordination/                      # Project coordination / memory
│   ├── memory_bank/                   # Persistent learning artifacts
│   ├── orchestration/                 # Current phase orchestration
│   └── subtasks/                      # Task tracking
├── docs/                              # Project documentation
│   ├── README.md                      # Main project documentation
│   ├── VALIDATION.md                  # Test results and validation
│   └── [API/tech docs]                # Framework/library-specific docs
├── hdr_analyzer_mvp/                  # HDR10 analysis binary (Rust crate)
│   ├── src/
│   │   ├── main.rs                    # CLI entry point, validation
│   │   ├── cli.rs                     # Clap argument parser
│   │   ├── pipeline.rs                # Orchestration: workflow state machine
│   │   ├── ffmpeg_io.rs               # FFmpeg context, decode, format detection
│   │   ├── crop.rs                    # Active video area detection
│   │   ├── optimizer.rs               # Nits curve smoothing, profile selection
│   │   ├── writer.rs                  # Binary .bin file generation (madVR format)
│   │   ├── l1_sidecar.rs              # JSON L1 per-scene/frame export
│   │   ├── analysis/                  # Frame/scene analysis modules
│   │   │   ├── mod.rs                 # Module definition
│   │   │   ├── frame.rs               # Per-frame histogram + peak estimation
│   │   │   ├── histogram.rs           # PQ bin logic, smoothing, percentiles
│   │   │   ├── scene.rs               # Scene cut detection, metrics
│   │   │   ├── gpu.rs                 # CUDA-accelerated analysis (optional)
│   │   │   ├── hlg.rs                 # HLG→PQ transfer function
│   │   │   └── kernels.cu             # CUDA kernels (compiled by nvrtc)
│   │   └── ...
│   ├── Cargo.toml                     # Package manifest, dependencies
│   └── tests/                         # Integration tests
├── mkvdovi/                           # DV metadata injection binary (Rust crate)
│   ├── src/
│   │   ├── main.rs                    # CLI entry point, file discovery, subcommand dispatch
│   │   ├── cli.rs                     # Clap argument parser, SubCmd enum
│   │   ├── pipeline.rs                # convert_file(): per-file orchestration + checkpointing
│   │   ├── metadata.rs                # DV L1–L11 generation from L1 sidecar
│   │   ├── rpu_check.rs               # RPU sampling, Profile classification, diagnostics
│   │   ├── fel_composite.rs           # Profile 7 FEL re-encoding pipeline
│   │   ├── external.rs                # Tool availability checks, subprocess invocation
│   │   ├── resume.rs                  # Checkpoint sentinel management
│   │   ├── verify.rs                  # RPU/metadata validation logic
│   │   ├── progress.rs                # Verbosity control, progress output formatting
│   │   └── ...
│   ├── Cargo.toml                     # Package manifest, dependencies
│   ├── tests/                         # Integration tests
│   └── build/                         # Build artifacts (generated)
├── verifier/                          # Measurement validation binary (Rust crate)
│   ├── src/
│   │   ├── main.rs                    # Read + validate .bin files, display contents
│   │   └── ...
│   ├── Cargo.toml                     # Package manifest, dependencies
│   └── tests/                         # Integration tests
├── tools/                             # Utility crates (excluded from workspace)
│   ├── l1_diff/                       # L1 sidecar comparison tool
│   │   ├── src/main.rs                # Compare L1 JSON files
│   │   └── Cargo.toml
│   └── compare_baseline/              # Measurement file baseline comparison
│       ├── src/main.rs
│       └── Cargo.toml
├── scripts/                           # Shell scripts for build/setup
│   ├── dev-refresh.sh                 # Rebuild workspace + dev install
│   └── mkvdovi_hifi_workflow.sh       # Specialist DV comparison helper
├── tmp/                               # Temporary files (git-ignored)
│   ├── dv_probe/                      # Temporary DV probing results
│   ├── fel_test/                      # FEL composite test artifacts
│   └── validation/                    # Validation test outputs
├── target/                            # Build outputs (git-ignored)
│   ├── release/                       # Release binary artifacts
│   ├── debug/                         # Debug builds
│   └── ...
├── Cargo.toml                         # Workspace manifest (resolver v2)
├── Cargo.lock                         # Dependency lock file
├── CLAUDE.md                          # AI agent instructions (checked in)
├── AGENTS.md                          # Subagent routing (links to CLAUDE.md)
├── CONTRIBUTING.md                    # Contribution guidelines
├── CHANGELOG.md                       # Release notes + version history
├── README.md                          # Main project README
├── ROADMAP.md                         # Planned features
├── clippy.toml                        # Clippy lint configuration
├── deny.toml                          # Dependency auditing rules (cargo deny)
├── .pre-commit-config.yaml            # Pre-commit hooks (fmt check, clippy)
├── .gitignore                         # Git exclusion rules
├── CITATION.cff                       # Citation metadata
└── dev-handoff.md                     # Development handoff notes
```

## Directory Purposes

**`hdr_analyzer_mvp/src/`:**
- Purpose: HDR10 video analysis engine — frame histogram computation + scene detection
- Contains: Frame/scene analysis modules, FFmpeg I/O, optimization, output serialization
- Key files: `main.rs` (entry), `pipeline.rs` (orchestration), `analysis/frame.rs` (core logic)

**`hdr_analyzer_mvp/src/analysis/`:**
- Purpose: Encapsulate HDR luminance + color analysis
- Contains: PQ histogram bins, peak estimators, HLG conversion, GPU pipeline
- Key files: `frame.rs` (per-frame peaks), `histogram.rs` (bin logic), `scene.rs` (cuts)

**`mkvdovi/src/`:**
- Purpose: Orchestrate Dolby Vision file conversion and metadata injection
- Contains: Format detection, metadata generation, subprocess calls, checkpoint management
- Key files: `main.rs` (discovery + dispatch), `pipeline.rs` (per-file workflow), `metadata.rs` (L1–L11 generation)

**`verifier/src/`:**
- Purpose: Standalone validation tool for madVR measurement files
- Contains: Binary file reading, structure validation, nits conversion
- Key files: `main.rs` (only file; read + display)

**`tools/l1_diff/`:**
- Purpose: Compare L1 sidecar JSON files (per-scene/frame differences)
- Built/run independently: `cargo run --manifest-path tools/l1_diff/Cargo.toml -- <file1> <file2>`

**`tools/compare_baseline/`:**
- Purpose: Compare measurement .bin files against baseline
- Built/run independently: `cargo run --manifest-path tools/compare_baseline/Cargo.toml -- <baseline> <current>`

**`scripts/`:**
- Purpose: Automation for development (build refresh, test workflows)
- Key files: `dev-refresh.sh` (rebuild + install), `mkvdovi_hifi_workflow.sh` (DV A/B testing)

**`docs/`:**
- Purpose: User-facing and technical documentation
- Contains: Implementation notes, validation results, API references
- Key files: `VALIDATION.md` (test methodology + results)

**`.planning/codebase/`:**
- Purpose: Generated codebase analysis documents (ARCHITECTURE.md, STRUCTURE.md, etc.)
- Maintained by: `/gsd-map-codebase` agent, read by `/gsd-plan-phase` and `/gsd-execute-phase`

**`.claude/`:**
- Purpose: Claude Code project configuration and agent tooling
- Contains: Project skills, auto-execution hooks, custom commands, GSD integration
- Key files: `CLAUDE.md` (global instructions), skills subdirectory (project-specific tooling)

## Key File Locations

**Entry Points:**
- `hdr_analyzer_mvp/src/main.rs`: HDR analysis CLI entry point
- `mkvdovi/src/main.rs`: DV metadata injection CLI entry point
- `verifier/src/main.rs`: Measurement validation tool

**Configuration:**
- `Cargo.toml`: Workspace manifest (resolver v2, release profile tuning)
- `hdr_analyzer_mvp/Cargo.toml`: Defines `cuda` feature flag
- `mkvdovi/Cargo.toml`: Lists dolby_vision crate dependency
- `.cargo/config.toml`: `-C target-cpu=native`, clang/lld selection for ARM
- `clippy.toml`: Lint baseline, complexity thresholds
- `deny.toml`: cargo-deny rules (advisories, licenses, dependency sources)
- `.pre-commit-config.yaml`: fmt check, clippy deny-warnings on commit; `cargo test` on push

**Core Logic:**
- `hdr_analyzer_mvp/src/pipeline.rs`: Main analysis workflow (FFmpeg → frame loop → optimizer → output)
- `hdr_analyzer_mvp/src/analysis/frame.rs`: Per-frame PQ histogram + peak estimation
- `hdr_analyzer_mvp/src/analysis/scene.rs`: Scene cut detection via histogram difference
- `hdr_analyzer_mvp/src/ffmpeg_io.rs`: FFmpeg context, decoding, format detection, crop probing
- `hdr_analyzer_mvp/src/optimizer.rs`: Post-analysis nits curve smoothing
- `mkvdovi/src/pipeline.rs`: Per-file DV conversion orchestration (detect → analyze → generate metadata → mux)
- `mkvdovi/src/metadata.rs`: DV L1–L11 generation from L1 sidecar + cli args
- `mkvdovi/src/rpu_check.rs`: RPU sampling, Profile classification, reliability diagnostics

**Testing:**
- `hdr_analyzer_mvp/tests/`: Integration tests for analysis pipeline
- `mkvdovi/tests/`: Integration tests for DV conversion
- `verifier/tests/`: Measurement validation tests

**Documentation:**
- `README.md`: Project overview, quick start, features
- `CLAUDE.md`: AI agent instructions (build commands, module walkthrough, patterns)
- `CONTRIBUTING.md`: Contribution guidelines
- `CHANGELOG.md`: Version history
- `ROADMAP.md`: Planned roadmap

## Naming Conventions

**Files:**
- `main.rs`: Binary entry point for each crate
- `cli.rs`: Clap argument parser definitions
- `pipeline.rs`: Orchestration (workflow state machine)
- `mod.rs`: Module definition (directory becomes a module)
- `*.rs`: Rust source (snake_case for file names)

**Functions:**
- `run()`: Primary orchestration function (pipeline entry)
- `analyze_*()`: Frame/scene analysis functions
- `write_*()`: Output serialization
- `check_*()`: Validation/probing functions
- `convert_*()`: Type conversion or pipeline stages
- Snake_case throughout (Rust convention)

**Variables:**
- `output_path`, `input_file`: File path strings/Path objects
- `frames`, `scenes`: Collections of analysis results
- `measurements`: Per-frame data from hdr_analyzer_mvp
- `temp_dir`: Temporary working directory
- Snake_case throughout

**Types/Structs:**
- `MadVRFrame`, `MadVRScene`: From `madvr_parse` crate (capitalized)
- `FramePeakStats`, `FrameL1Measurement`: Analysis results (PascalCase)
- `HdrFormat`, `TransferFunction`: Enums (PascalCase)
- `CropRect`: Geometry helper (PascalCase)
- `Cli`, `Args`: CLI argument structs (PascalCase)

**Constants:**
- `PQ_HIST_BINS`, `DIFF_VALUE_BANDS`: Analysis parameters (SCREAMING_SNAKE_CASE)
- `L1_SIDECAR_VERSION`: Versioning constant
- `ST2084_M1`, `ST2084_M2`: Physics constants (SCREAMING_SNAKE_CASE)

## Where to Add New Code

**New Frame Analysis Feature (e.g., grain detection):**
- Primary location: `hdr_analyzer_mvp/src/analysis/frame.rs`
- Integration: Call new function from `analyze_native_frame_cropped()` (line ~150)
- Output: Add field to `FramePeakStats` or create new result struct
- Tests: Add test in `hdr_analyzer_mvp/tests/` or inline unit test
- CLI flag (if user-configurable): Add to `hdr_analyzer_mvp/src/cli.rs`

**New Scene Metric (e.g., motion detection):**
- Primary location: `hdr_analyzer_mvp/src/analysis/scene.rs` or new `analysis/motion.rs`
- Integration: Call from `pipeline::run_native_analysis_pipeline()` after scene detection
- Output: Add field to `MadVRScene` (if persisted) or internal tracking struct
- Tests: Scene-level integration test in `hdr_analyzer_mvp/tests/`

**New GPU Analysis Kernel:**
- Kernel code: `hdr_analyzer_mvp/src/analysis/kernels.cu` (CUDA code)
- Rust wrapper: `hdr_analyzer_mvp/src/analysis/gpu.rs` (kernel invocation + host↔device transfer)
- Feature gating: Wrap calls in `#[cfg(feature = "cuda")]`
- Version contract: Update `cuda` feature detection in `cli.rs` (VERSION const)

**New DV Metadata Level (e.g., L12):**
- Metadata generation: `mkvdovi/src/metadata.rs` → extend `generate_dv_metadata()`
- Schema validation: Update `rpu_check.rs` if validation needed
- CLI flag: Add to `mkvdovi/src/cli.rs` if user-configurable
- Tests: `mkvdovi/tests/` with sample DV file

**New Output Format (e.g., CSV export):**
- New module: `hdr_analyzer_mvp/src/export_csv.rs`
- Integration: Call from `pipeline::run()` if `--format csv` (add to CLI)
- Writer implementation: Follow pattern of `writer.rs` (take scenes/frames, serialize)
- Tests: Unit test in new module, integration test in `hdr_analyzer_mvp/tests/`

**New External Tool Integration (e.g., new encoder):**
- Tool check: Add to `mkvdovi/src/external.rs` → `check_dependencies()`
- Invocation: Add function like `run_command_with_progress()` for the tool
- Integration: Call from `pipeline::convert_file()` at appropriate stage
- Tests: `mkvdovi/tests/` with sample file, or mock tool exit codes

**New Resumable Workflow Step (mkvdovi):**
- Step implementation: Add function in relevant module (e.g., `pipeline.rs`)
- Checkpoint: Create `.done` sentinel via `resume::mark_done()` on success
- Resume detection: Check `resume::is_done()` at step start
- Ordering: Update step sequence in `convert_file()` to maintain atomicity

## Special Directories

**`target/`:**
- Purpose: Cargo build outputs (debug + release binaries, intermediate objects)
- Generated: Yes (by `cargo build`, `cargo test`)
- Committed: No (in `.gitignore`)

**`tmp/`:**
- Purpose: Temporary files during development/validation
- Generated: Yes (by test workflows, manual experiments)
- Committed: No (in `.gitignore`)

**`.planning/codebase/`:**
- Purpose: Generated codebase analysis documents
- Generated: Yes (by `/gsd-map-codebase` agent)
- Committed: Yes (consumed by `/gsd-plan-phase` and `/gsd-execute-phase`)

**`coordination/`:**
- Purpose: Project memory, orchestration state, task tracking
- Generated: Yes (by GSD agents and manual updates)
- Committed: Yes (persistent project knowledge)

**`.claude/`:**
- Purpose: Claude Code configuration, skills, hooks, agents
- Generated: Partially (skills auto-loaded, but custom rules checked in)
- Committed: Yes (project-specific tooling)

---

*Structure analysis: 2026-08-12*
