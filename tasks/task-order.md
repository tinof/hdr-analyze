Here’s the order I’d tackle things to maximize visible output quality (and keep risk low). Think “fix the inputs → stabilize the signal → shape the curve → cut precisely,” with a tiny harness to prove we didn’t lie to ourselves.

0) Baseline & Harness (do this first, ~half-day)
	•	Lock a tiny baseline pack (3× short HDR10 clips: scope letterbox, TV doc, bright demo).
	•	Freeze current outputs (.bin, verifier logs) and compute simple deltas (scene count, MaxCLL/FALL, per-frame target_nits 95th-pct delta).
	•	Use the existing Rust harness at `tools/compare_baseline` to compare new runs to the frozen baseline and print the key metrics (scene count deltas, MaxCLL/FALL deltas, target_nits P95 delta).

Why first: every change below needs a ruler. This is it.

1) PQ Noise Robustness — Robust PQ Histograms (Roadmap §10, Phase C) ✓ COMPLETE

	Status: Implementation complete (see commit 9cbed178).

	Delivered:
	- ✓ `--peak-source {max|histogram99|histogram999}` with histogram99 default for v6 balanced/aggressive profiles.
	- ✓ Per-bin EMA smoothing (`--hist-bin-ema-beta`, default 0.1) with scene-cut resets and renormalization.
	- ✓ Optional temporal median smoothing (`--hist-temporal-median N`, default 0/off; N=3 recommended).
	- ✓ Optional Y-plane `median3` pre-denoise (`--pre-denoise {median3|off}`, default off).
	- ✓ MaxCLL/FALL validation against baseline harness with ≥30% APL σ reduction on grainy clips.

	Next: Roll into broader regression pack once more clips are annotated.

2) Future-aware Target-Nits Smoothing (Roadmap §10, Phase C) ✓ COMPLETE

	•	Bidirectional EMA smoothing implemented with per-scene resets and delta caps; exposed via `--target-smoother ema` (default) and `--smoother-*` flags.
	•	Defaults flipped on (“balanced” profile and CLI default) with opt-out through `--target-smoother off`.
	•	Unit tests cover delta reduction and scene boundary resets; needs visual validation on baseline pack to confirm ≥25% 95th-pct delta drop.

3) Dynamic Clipping Heuristics Calibration (Roadmap §10, Phase A)

Then. Now that peaks and smoothing are stable, set the curve.
	•	Define knee from P99/P99.9 + APL category; scene-aware reset; delta caps (you already have scaffolding).
	•	Finalize --optimizer-profile {conservative,balanced,aggressive} numeric tables in docs; make “balanced=P99” with modest knee slope.
	•	Add `--header-peak-source {max|histogram99|histogram999}` to select header MaxCLL from robust percentiles and reduce outlier-driven spikes.

Exit check
	•	dovi_tool generate --madvr-file … accepts our bins; plot shows monotone, smooth intra-scene trajectories; no banding in troublesome highlights.

4) Scene Detection: Hybrid Metric (Roadmap §10, Phase C, CPU-first)

After the above. Cuts gate smoothing resets; but hist/peaks must be robust first or you’ll tune to noise.
	•	Add flow magnitude histograms (Farnebäck default, TV-L1 optional) on downscaled luma; fuse with your histogram delta (--scene-metric hybrid).
	•	Keep ML (TransNetV2 via ONNX) as optional. Land it, don’t default to it yet (offline users can flip it on).

Exit check
	•	On the eval subset: F1 ↑ ≥3% vs histogram-only; runtime overhead ≤15% at --downscale 4.
	•	Don’t change defaults until that F1 threshold is met.

5) Native HLG Support Pipeline (New Feature) — In Progress

	•	Detection wired via FFmpeg transfer metadata; ARIB STD-B67 streams now convert to PQ histograms in-memory (default 1000 nit peak via `--hlg-peak-nits`).
	•	New `analysis::hlg` module implements inverse EOTF + PQ mapping; CLI/documentation updated.
	•	Completed: `mkvdolby` now calls `hdr_analyzer_mvp` directly on HLG content for measurements (native HLG path) and retains a single HLG→PQ encode for the DV base layer.
	•	Next: validate native HLG outputs against legacy zscale re-encode workflow; add integration tests + dovi_tool smoke to satisfy exit criteria.

6) Benchmark Corpus & Protocol (Roadmap §10, Phase C)

Do in parallel with #4/#5 (light lift).
	•	Publish the annotation format (JCut-style JSON) and the harness doc (docs/benchmark.md).
	•	Include a CI-legal tiny subset (<60s total) for SBD + smoothing KPIs.

Exit check
	•	CI job emits SBD F1/P/R and target-nits stability stats; artifacts include verifier logs.

7) v6 Gamut Peaks: Full RGB Conversion (existing “Not yet implemented”)

Later, correctness polish.
	•	Replace luminance approximations with proper RGB transforms for P3/709 peaks.
	•	Good for spec correctness and parity checks; smaller visible win than #1–#3.

8) Dolby XML path & validators (Phase B skeleton)

Later, but wire a smoke test early.
	•	Even before full XML export, keep a dovi_tool smoke test in CI to catch regressions.

⸻

TL;DR queue (pin this in an issue)
	1.	Robust PQ histograms → default P99, bin EMA (on), temporal median (off), optional NL-Means (off).
	2.	Two-pass EMA smoothing with scene resets + delta caps → make default in “balanced”.
	3.	Dynamic clipping calibration → knee from P99/P99.9 + APL, finalize profile tables.
	4.	Hybrid scene detector (flow + hist) → make default only if F1 ↑ ≥3% within +15% time.
	5.	Native HLG Support Pipeline.
	6.	Benchmark corpus + harness (tiny CI subset + docs).
	7.	Full RGB gamut peaks for v6.
	8.	DV XML/validator smoke tests (scaffold).
	9.	Post-mux verification in mkvdolby (`--verify`): IMPLEMENTED — runs `verifier` on .bin, extracts RPU + `dovi_tool info --summary`, cross-checks frame counts via mediainfo, returns non-zero on mismatches.

⸻

“Don’t trip the build” guardrails
	•	Land each step behind flags, run the harness, then flip the profile defaults only after passing AC.
	•	Keep a golden outputs folder for the baseline pack and auto-diff it per PR.
	•	For ARM boxes, prefer downscale=4 for flow; if users opt into ML, recommend ONNX Runtime with intra-op threads = cores.
