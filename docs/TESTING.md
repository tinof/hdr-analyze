# Testing

How tests are written, reviewed and gated in this repository. The rules are written for agents and humans alike. Test commands, skips and gates per path are in `CLAUDE.md` ("Testing quirks", "CI behavior that affects edits"). The audit that produced this file is roadmap item E14.

## 1. Principles

1. **Coverage measures execution, not verification.** A line that ran was not necessarily checked. Use coverage (`cargo llvm-cov`, local only) to find code no test touches. Never treat it as a quality score.
2. **Every test must be able to fail.** Before you write or accept a test, name the production change that would make it fail. If you cannot name one, the test is suspect. `cargo-mutants` makes those changes for you (section 3).
3. **A skipped test is not a passing test.** Many tests here print `Skipping` and return when a tool, a sibling analyzer or media is missing. Under `cargo test` such a test passes. A test run counts only when `scripts/ci/check-test-skips.sh` passes on its log. A mutation run counts only when its baseline log passes the same check.
4. **Tests encode requirements, not accidents.** A test that pins whatever the code does now may pin a bug. When the intended behaviour is unclear, ask the owner.
5. **The author does not grade its own work.** The pre-PR panel (`.claude/workflows/pre-pr-panel.js`) reviews tests in a fresh context. A separate test author is being piloted on P13 (E14).
6. **CI is the oracle.** Local hooks can be edited. A required GitHub check cannot be bypassed from a coding session. PR CI and `release.yml` run the same tools and the same skip check (`.github/actions/setup-test-tools`).

## 2. Writing tests

- Assert on values the code under test produces. Never compare a literal with itself. Never let "does not panic" or `is_ok()` be the only assertion: when a function returns advisories or an error reason, check them.
- **Test both sides of every decision.** A function that refuses, accepts, classifies or chooses a path gets two kinds of test: one where it must trigger and one where it must not. Add the boundary cases: the tolerance edge, the empty input, the untagged field.
- Take the expected value from an independent source: a hand-written table, a spec value, an external tool such as `dovi_tool info`, or a reference implementation in the test. Never compute it with the function under test or a copy of its formula.
- Prefer a structured value over the text of a log line, when both exist.
- Mock only at external boundaries. Here those are external tools on `PATH` (a PATH sandbox, as in `mkvdovi/tests/pq_input_contract.rs`), the filesystem and the clock. Do not fake a Dolby Vision or HEVC input with a mock. Build a small fixture with `ffmpeg`/`dovi_tool`, or mark the test env-gated.
- Never delete, skip or loosen an existing test to make a change pass. If a test seems wrong, say so and change it in a separate, labelled commit.
- **A bug fix starts with a failing test.** The test must fail on the parent commit and pass after the fix. State that in the PR body.
- Use property tests (`proptest`) where an invariant holds for all inputs: parsers, serialisation round trips, encode and decode. Commit `proptest-regressions/`.
- Analyzer numerics (`analysis/`, `kernels.cu`) are guarded by `scripts/cuda-parity.sh` (CPU = CUDA bit for bit) and the L1 regression gate (`scripts/ci/l1-regression-gate.sh`), plus synthetic ground truth. A spec-blind test author has nothing to assert there.

## 3. Mutation testing

`cargo-mutants` changes the code one mutation at a time (flip a condition, return a default, delete a statement) and runs the tests. A mutant that no test catches is a **survivor**. Read the survivors, not the score.

```bash
cargo install --locked cargo-mutants
# Before any run: the suite passes with every skip accounted for on this host.
cargo test --workspace -- --nocapture --skip cuda_output_matches_cpu 2>&1 | tee baseline.log \
  && scripts/ci/check-test-skips.sh baseline.log 'built without HDR10_PLUS|does not accept dhdr10-info'
# One module; mkvdovi and dovi84_composer need --test-workspace=true, so that the analyzer
# binary is built next to the mkvdovi tests in the mutant copy and mkvdovi's tests run.
cargo mutants -f 'mkvdovi/src/verify.rs' --test-workspace=true -j 3 --timeout-multiplier 3 \
  -o <dir>/verify -- -- --nocapture --skip cuda_output_matches_cpu
# Changes on a branch only:
cargo mutants --in-diff <(git diff main...HEAD) --test-workspace=true -j 3
```

- Run modules one after another. One run at `-j 3` with `CARGO_BUILD_JOBS=4` fits a 16-core, 10 GB host.
- Check the run's own baseline log (`<dir>/mutants.out/log/baseline.log`) with `check-test-skips.sh`. A skip there means the run is invalid, and every survivor it reports is suspect.

### Triage: every survivor goes into exactly one bucket

| Bucket | Meaning | Action |
|---|---|---|
| Missing test | Real behaviour that no test checks | Write a targeted test. Re-run the mutant and confirm it is caught. |
| Equivalent | The change cannot alter behaviour | Record it and move on. |
| Actual bug | The mutated code is more correct, or the survivor exposes wrong logic | Stop. Report to the owner before you write any test. |
| Dead code | Unreachable or unused | Propose removal. |
| Characterization | The test pins current behaviour on purpose (for example the E13 safety-net tests) | Keep it until the refactor that needs it is done. Then turn it into a requirement test or delete it. |

Several survivors in one function are a strong sign of a bug. One operator survivor in a utility function is usually low priority. Stop after three rounds of mutation-driven test writing per module. Hand the remaining survivors to the owner.

## 4. Test-smell review

A reviewer that did not write the tests looks for:

- assertions that would pass if the implementation were deleted, or that only check "no panic";
- expected values computed by the code under test;
- decisions tested on one side only;
- tests that skip on a host where they should run;
- tolerances too loose to catch a plausible bug;
- duplicate tests that pin the same fact;
- text assertions where a structured value exists.

## 5. Definition of done

- [ ] The tests pass, and the log passes `check-test-skips.sh`. State the command and its result.
- [ ] Every new or changed test has a named production change that makes it fail.
- [ ] Every new decision is tested on both sides.
- [ ] A bug fix has a test that fails on the parent commit.
- [ ] No test was deleted, skipped or loosened without the owner's approval.
- [ ] For changed logic in mkvdovi, verifier or dovi84_composer: a `cargo mutants --in-diff` run, with every survivor triaged.

Deferred: a PreToolUse hook that blocks test edits during implementation. It matters only if test authoring moves to a separate session; revisit after the P13 pilot (E14).
