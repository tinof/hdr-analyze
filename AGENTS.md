# AGENTS.md

AI-agent guidance for this repo lives in **[CLAUDE.md](./CLAUDE.md)** — the single source of truth.
This file is intentionally a pointer to avoid the two drifting apart. The one exception is the
section below, for the Codex reviewers: Codex's GitHub review reads it natively, Codex `exec` runs
load this file on their own, and the `/pre-pr-review` Codex pass also gets the base branch's copy
in its prompt. CLAUDE.md does not repeat it.

## Code Review Rules

- **Cross-binary contracts change on both sides in the same PR.** These are the L1 sidecar
  (`hdr_analyzer_mvp/src/l1_sidecar.rs` version and fields, read by mkvdovi through
  `metadata::L1_SIDECAR_MAX_VERSION` and by `tools/l1_diff`), the analyzer's `--version` `+cuda`
  probe, the `Composer::luminance_mapping` names, and the analyzer `--help` options that mkvdovi
  probes (`external::analyzer_lists_option`). A change on one side only is P1. Safe path: change
  the writer and every reader in the same PR; for the sidecar, also bump its version.
- **CPU and CUDA analysis stay bit-identical, and resumed runs never reuse stale output.**
  `hdr_analyzer_mvp/src/analysis/kernels.cu` and its Rust mirror keep the same f32 operation
  order, the `_rn` intrinsics and the buffer layouts (`SUMS_WORD`, the `DOVI_*` offsets) on both
  sides. A new mkvdovi flag that changes any artifact goes into `pipeline::resume_settings`.
  Either miss is P1. Safe path: change both sides in the same commit and run
  `scripts/cuda-parity.sh` on the CUDA host (CI has no GPU).
- **mkvdovi copies the video stream bit-exactly and never re-encodes; out-of-range metadata gets
  a visible warning or a refusal, never a silent clamp.** Every ffmpeg call on the video stream
  keeps `-c:v copy`; unsupported inputs are refused before any temp work, as
  `metadata::check_pq_range_contract`, `metadata::check_hlg_colour_contract` and
  `FEL_UNSUPPORTED_MESSAGE` do. A change that moves L1 on purpose commits the references
  refreshed by `scripts/ci/l1-regression-gate.sh --update` in the same PR, and the PR says why L1
  moved. An encoder, a clamp without a warning, or an unexplained L1 move is P1. Safe path: refuse
  or warn, never alter the picture or the measurement silently.
- CI already checks formatting, clippy, unexpected test skips and the L1 regression gate. Do not
  report those.
