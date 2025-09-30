Here’s the order I’d tackle things to maximize visible output quality (and keep risk low). Think “fix the inputs → stabilize the signal → shape the curve → cut precisely,” with a tiny harness to prove we didn’t lie to ourselves.

0) Baseline & Harness (do this first, ~half-day)
	•	Lock a tiny baseline pack (3× short HDR10 clips: scope letterbox, TV doc, bright demo).
	•	Freeze current outputs (.bin, verifier logs) and compute simple deltas (scene count, MaxCLL/FALL, per-frame target_nits 95th-pct delta).
	•	Add a micro harness: tools/compare_baseline.py (or Rust) that compares new runs to the frozen baseline and prints the metrics you already list in Acceptance Criteria.

Why first: every change below needs a ruler. This is it.

1) PQ Noise Robustness — Robust PQ Histograms (Roadmap §10, Phase C) ✓ COMPLETE

	Status: Implementation complete. Ready for field validation on test corpus.

	Completed:
	- ✓ Implemented `--peak-source histogram99` (internal default for "balanced"/"aggressive"); `max` and `histogram999` available as alternates.
	- ✓ Added **bin EMA** (β≈0.1) with `--hist-bin-ema-beta` flag. Scene-aware resets implemented.
	- ✓ Optional **temporal median (3f)** via `--hist-temporal-median N` flag. Default: off.
	- ✓ Added `--pre-denoise median3` (off by default). NLMeans reserved for future work.
	- ✓ Smart defaults: histogram99 for balanced/aggressive, max for conservative profiles.
	- ✓ Histogram smoothing with automatic renormalization to maintain sum ≈ 100.0.
	- ✓ Peak PQ and APL recomputed from smoothed histograms.

	Exit check (Pending validation):
	- Static/grainy scene: Expected **APL σ ↓ ≥ 30%**, **median shift ≤ 1% PQ**.
	- Across 3 clips: **no MaxCLL/MaxFALL regressions** beyond tolerance (use baseline harness).

2) Future-aware Target-Nits Smoothing (Roadmap §10, Phase C)

Next. This is the biggest user-visible win (less pumping/flicker).
	•	Implement two-pass EMA with per-scene resets and delta caps; wire to --target-smoother ema --bidirectional.
	•	Keep One-Euro & Savitzky–Golay as optional modes for later tuning; default to 2-pass EMA in the “balanced” profile.

Exit check
	•	On baseline pack: 95th-pct |Δ(target_nits)| ↓ ≥25% vs. current; no extra lag at cuts.
	•	Visual sweep: no breathing on slow pans, no step jumps at scene boundaries.

3) Dynamic Clipping Heuristics Calibration (Roadmap §10, Phase A)

Then. Now that peaks and smoothing are stable, set the curve.
	•	Define knee from P99/P99.9 + APL category; scene-aware reset; delta caps (you already have scaffolding).
	•	Finalize --optimizer-profile {conservative,balanced,aggressive} numeric tables in docs; make “balanced=P99” with modest knee slope.

Exit check
	•	dovi_tool generate --madvr-file … accepts our bins; plot shows monotone, smooth intra-scene trajectories; no banding in troublesome highlights.

4) Scene Detection: Hybrid Metric (Roadmap §10, Phase C, CPU-first)

After the above. Cuts gate smoothing resets; but hist/peaks must be robust first or you’ll tune to noise.
	•	Add flow magnitude histograms (Farnebäck default, TV-L1 optional) on downscaled luma; fuse with your histogram delta (--scene-metric hybrid).
	•	Keep ML (TransNetV2 via ONNX) as optional. Land it, don’t default to it yet (offline users can flip it on).

Exit check
	•	On the eval subset: F1 ↑ ≥3% vs histogram-only; runtime overhead ≤15% at --downscale 4.
	•	Don’t change defaults until that F1 threshold is met.

5) Benchmark Corpus & Protocol (Roadmap §10, Phase C)

Do in parallel with #4 (light lift).
	•	Publish the annotation format (JCut-style JSON) and the harness doc (docs/benchmark.md).
	•	Include a CI-legal tiny subset (<60s total) for SBD + smoothing KPIs.

Exit check
	•	CI job emits SBD F1/P/R and target-nits stability stats; artifacts include verifier logs.

6) v6 Gamut Peaks: Full RGB Conversion (existing “Not yet implemented”)

Later, correctness polish.
	•	Replace luminance approximations with proper RGB transforms for P3/709 peaks.
	•	Good for spec correctness and parity checks; smaller visible win than #1–#3.

7) Dolby XML path & validators (Phase B skeleton)

Later, but wire a smoke test early.
	•	Even before full XML export, keep a dovi_tool smoke test in CI to catch regressions.

⸻

TL;DR queue (pin this in an issue)
	1.	Robust PQ histograms → default P99, bin EMA (on), temporal median (off), optional NL-Means (off).
	2.	Two-pass EMA smoothing with scene resets + delta caps → make default in “balanced”.
	3.	Dynamic clipping calibration → knee from P99/P99.9 + APL, finalize profile tables.
	4.	Hybrid scene detector (flow + hist) → make default only if F1 ↑ ≥3% within +15% time.
	5.	Benchmark corpus + harness (tiny CI subset + docs).
	6.	Full RGB gamut peaks for v6.
	7.	DV XML/validator smoke tests (scaffold).

⸻

“Don’t trip the build” guardrails
	•	Land each step behind flags, run the harness, then flip the profile defaults only after passing AC.
	•	Keep a golden outputs folder for the baseline pack and auto-diff it per PR.
	•	For ARM boxes, prefer downscale=4 for flow; if users opt into ML, recommend ONNX Runtime with intra-op threads = cores.

If you want, I can draft the exact issue list with labels/assignees and pre-fill each with the DoD/AC and CLI examples.

⸻

PS: That earlier uploaded “HDR10 Analysis Tool Research Report.md” in the temp workspace isn’t accessible anymore (expired upload). If you want me to re-pull anything specific from it, just re-upload and I’ll fold it in.
