# AGENTS.md

AI-agent guidance for this repo lives in **[CLAUDE.md](./CLAUDE.md)** — the single source of truth.
This file is intentionally a pointer to avoid the two drifting apart. The one exception is the
section below: every code reviewer reads it (Codex's GitHub review natively, `/ship` and
`/pre-pr-review` by pasting it into their prompts), and CLAUDE.md does not repeat it.

## Code Review Rules

- **Cross-binary contracts change on both sides in the same PR.** These are the L1 sidecar
  (`hdr_analyzer_mvp/src/l1_sidecar.rs` version and fields, read by mkvdovi through
  `metadata::L1_SIDECAR_MAX_VERSION` and by `tools/l1_diff`), the analyzer's `--version` `+cuda`
  probe, the `Composer::luminance_mapping` names, and the analyzer `--help` options that mkvdovi
  probes (`external::analyzer_lists_option`). A change on one side only is P1. Safe path: bump
  the sidecar version on the writer and accept it on the readers in the same PR.
- **CPU and CUDA analysis stay bit-identical, and resumed runs never reuse stale output.**
  `hdr_analyzer_mvp/src/analysis/kernels.cu` and its Rust mirror keep the same f32 operation
  order, the `_rn` intrinsics and the buffer layouts (`SUMS_WORD`, the `DOVI_*` offsets) on both
  sides. A new mkvdovi flag that changes any artifact goes into `pipeline::resume_settings`.
  Either miss is P1.
- **mkvdovi copies the video stream bit-exactly and never re-encodes; out-of-range metadata gets
  a visible warning or a refusal, never a silent clamp.** A change that moves L1 on purpose ships
  with `scripts/ci/l1-regression-gate.sh --update` and the reference diff in the PR description.
  Missing either is P1.
- CI already checks formatting, clippy, unexpected test skips and the L1 regression gate. Do not
  report those.
