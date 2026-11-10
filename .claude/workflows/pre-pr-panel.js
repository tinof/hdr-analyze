export const meta = {
  name: 'pre-pr-panel',
  description: 'Pre-PR review of a roadmap change: three Opus lenses at high effort, a spec lens when the step text is handed over, a Fable whole-diff reviewer on hard high-risk diffs, each finding checked by an independent Opus skeptic, plus the gates the touched paths require',
  whenToUse: 'Internal to the pre-pr-review skill; do not run it directly. Started by that skill (standalone, or as the /ship step 3b self-check). args: {base, files, item, acceptanceGate, scopeFile, specFile?, hard?, kind?, moveProof?, moveFiles?}; specFile (the roadmap step text, or the /ship spec file) adds the spec lens; hard: true (300 or more changed logic lines, or a judgment item of the hard rubric, set by the skill) adds the Fable reviewer on a high-risk diff, as do the numeric and two-sided contract paths by themselves; kind "pure-move" with a moveProof path and moveFiles covering every non-Markdown file runs one move-integrity lens instead of the panel. Do not start it without args. With {base, files, gatesOnly: true} it only returns requiredGates, highRisk, hard and fable (the reviewer decision) and starts no agents; only pre-pr-review step 5 (standalone) calls it that way, to recompute gates after fixes. /ship no longer calls gatesOnly: it reads the same path rules from .claude/workflow.md.',
  phases: [
    { title: 'Review', detail: 'correctness, repo contracts, tests and gates, spec when a specFile is given (Opus, high); whole diff (Fable, high) on a hard high-risk diff; or one move-integrity lens (Opus, high) on a pure move' },
    { title: 'Verify', detail: 'one Opus skeptic per finding (medium), with a different lens than the finder; refutes only an impossible failure, re-ranks the rest' },
  ],
}

let a = args
if (typeof a === 'string') {
  try { a = JSON.parse(a) } catch (e) { a = null }
}
if (!a || !Array.isArray(a.files) || a.files.length === 0 || !a.base) {
  log('no scope handed over: nothing reviewed')
  return { started: false, reason: 'pre-pr-panel needs args {base, files[], item, acceptanceGate, scopeFile, specFile?}' }
}

const files = a.files.map(f => String(f).replace(/^\.\//, ''))
const any = re => files.some(f => re.test(f))

// Required gates, computed from the touched paths (not left to judgment).
// Keep in sync with .claude/workflow.md: its `## Gates` paths copy these rules and its
// `## Tier 2 paths` copy `highRisk` below. Change both in the same PR.
const gates = new Set()
const workspaceRust = files.some(f => !f.startsWith('tools/') && /\.rs$|(^|\/)Cargo\.(toml|lock)$|^\.cargo\/|^rust-toolchain\.toml$|^clippy\.toml$|^rustfmt\.toml$/.test(f))
if (workspaceRust) { gates.add('fmt'); gates.add('clippy'); gates.add('test') }
// CPU/GPU analysis path: the kernel, its host side, the decode path, the orchestration that hands
// frames to it, the composer coefficients both backends use, and build inputs that change them.
// A module counts as a file or as a directory of submodules (pipeline.rs or pipeline/), so a
// split never drops the gate.
// Build inputs that can change measured output without touching Cargo.lock: the workspace and crate
// manifests (features, dependency versions, profiles), compiler flags and the toolchain.
const BUILD = /^Cargo\.(toml|lock)$|^hdr_analyzer_mvp\/Cargo\.toml$|^dovi84_composer\/Cargo\.toml$|^\.cargo\/config\.toml$|^rust-toolchain\.toml$/
const cuda = any(BUILD) || any(/^hdr_analyzer_mvp\/src\/(analysis\/|(ffmpeg_io|pipeline|cli|crop)(\.rs$|\/))|^dovi84_composer\/src\/|^scripts\/cuda-parity\.sh$|^hdr_analyzer_mvp\/tests\/cuda_parity\.rs$/)
if (cuda) { gates.add('clippy-cuda'); gates.add('cuda-parity') }
// Measurement change: anything that can move the analyzer's L1 output or how it is scored.
const measurement = any(BUILD) || any(/^hdr_analyzer_mvp\/src\/|^dovi84_composer\/src\/|^tools\/l1_diff\/|^scripts\/ci\/l1-regression-gate\.sh$/)
if (measurement) gates.add('l1-regression')
for (const f of files) {
  const m = /^tools\/([^/]+)\/(.+)$/.exec(f)
  if (m && /\.rs$|Cargo\.(toml|lock)$/.test(m[2])) gates.add('tool:' + m[1])
}
// tools/rpu_builder_check builds dovi_rpu_builder by path (and includes its fixture cases), so a
// library change runs the tool's equivalence tests too.
if (any(/^dovi_rpu_builder\//)) gates.add('tool:rpu_builder_check')
// The Gemini pass and its fake-agy harness (no network, no model).
if (any(/^\.claude\/workflows\/(pre-pr-gemini\.|tests\/)|^\.agents\/agents\/pre-pr-reviewer\.md$/)) gates.add('gemini-harness')
// The record cleanup of /pre-pr-review step 1.2 and its test.
if (any(/^\.claude\/skills\/pre-pr-review\/(clear-record\.sh$|tests\/)/)) gates.add('pre-pr-record')
// The reviewer-yield observer of /pre-pr-review step 7 and its test.
if (any(/^\.claude\/skills\/pre-pr-review\/(reviewer-yield\.sh$|tests\/)/)) gates.add('reviewer-yield')
// The end-of-turn Rust check hook and its builder-hold test.
if (any(/^\.claude\/hooks\//)) gates.add('rust-check-hook')
// This script's reviewer decision and gate list, and their test.
if (any(/^\.claude\/workflows\/(pre-pr-panel\.js$|tests\/pre-pr-panel-trigger-test\.sh$)/)) gates.add('panel-trigger')
const requiredGates = [...gates]
log('required gates: ' + (requiredGates.length ? requiredGates.join(', ') : 'none (no Rust, analysis, Gemini-pass, record-cleanup, reviewer-yield, hook or panel paths touched)'))

// High-risk paths: the ones that require CUDA parity or the L1 regression gate, plus the
// cross-binary contract modules named in CLAUDE.md. A module counts as a file or as a directory of
// submodules (metadata.rs or metadata/), so a split never drops the rule.
const riskPaths = cuda || measurement ||
  any(/^hdr_analyzer_mvp\/src\/l1_sidecar(\.rs$|\/)|^mkvdovi\/src\/(metadata|pipeline|external|resume)(\.rs$|\/)|^dovi_rpu_builder\/src\//)
// Hard diffs (owner decision 2026-11-10: Fable reviews when the difficulty warrants it). Two of
// the repo's hard criteria (.claude/workflow.md "## Notes") show in the paths, so they never
// depend on who built the step or on a local log:
// - numeric work mirrored on CPU and CUDA: the analysis code with its kernel, and the composers;
// - two or more sides of a cross-binary contract (CLAUDE.md "Cross-binary contracts"): the
//   analyzer side (L1 sidecar, --version and --help, AU table), the mkvdovi side that reads them,
//   and tools/l1_diff.
// The skill adds `hard: true` for the rest of the rubric: 300 or more changed logic lines, or a
// judgment item (a design decision with a real trade-off, an unknown root cause).
const numeric = any(/^hdr_analyzer_mvp\/src\/analysis\/|^dovi84_composer\/src\//)
const contractSides = [
  /^hdr_analyzer_mvp\/src\/(l1_sidecar|cli|au_table)(\.rs$|\/)/,
  /^mkvdovi\/src\/(metadata|external)(\.rs$|\/)|^mkvdovi\/src\/pipeline\/(measure|manifest)(\.rs$|\/)/,
  /^tools\/l1_diff\//,
].filter(re => any(re)).length
const hardReason = a.hard === true ? 'hard: true from the skill' : numeric ? 'CPU/CUDA numeric path' :
  contractSides >= 2 ? `${contractSides} sides of a cross-binary contract` : ''
const hard = hardReason !== ''
// Gate list and reviewer decision only, no reviewers: lets the skills recompute gates for a file
// list that grew after fixes, and lets a test check the Fable trigger, without a second copy of
// the path rules. `fable` ignores the pure-move mode, which is decided below.
if (a.gatesOnly) return { gatesOnly: true, requiredGates, highRisk: riskPaths, hard, hardReason, fable: riskPaths && hard }

// Pure move (a module-size split with a passing line-exact move proof): one move-integrity lens
// replaces the three lenses, the spec lens and Fable. The skill selects it only after rerunning the proof on HEAD;
// without a proof path here the full panel runs. Gates stay as computed above.
// Every non-Markdown file in scope must be one the split touches (moveFiles), so an unrelated edit
// on the same branch never escapes the full panel.
const moveFiles = new Set((Array.isArray(a.moveFiles) ? a.moveFiles : []).map(f => String(f).replace(/^\.\//, '')))
const uncovered = files.filter(f => !/\.md$/.test(f) && !moveFiles.has(f))
const pureMove = a.kind === 'pure-move' && typeof a.moveProof === 'string' && a.moveProof.length > 0 &&
  moveFiles.size > 0 && uncovered.length === 0
if (a.kind === 'pure-move' && !pureMove) {
  log('kind pure-move rejected (' + (!a.moveProof ? 'no moveProof' : moveFiles.size === 0 ? 'no moveFiles' :
    'not in moveFiles: ' + uncovered.join(', ')) + '): running the full panel')
}

// A hard high-risk diff adds the Fable reviewer (riskPaths and hard are computed above).
const highRisk = !pureMove && riskPaths

const SCOPE = [
  `Scope: run \`git diff ${a.base}...HEAD\` for the committed part, and read the uncommitted files `,
  `named in ${a.scopeFile || 'the file list below'} (\`git diff HEAD -- <file>\` or Read for untracked ones). `,
  `Files in scope: ${files.join(', ')}. Roadmap item and step: ${a.item || 'unknown'}. `,
  `Its acceptance gate: ${a.acceptanceGate || 'not stated'}. `,
  a.specFile ? `The specification (the roadmap step text) is in ${a.specFile}; read it. ` : '',
  'Review only this scope; read other code only to judge it. Do not edit files. ',
  'Report only real defects you can point at: file:line, a concrete failure scenario, a fix. No style notes. ',
  'Return an empty list if you find nothing.',
].join('')

const FINDINGS = {
  type: 'object',
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          priority: { type: 'string', enum: ['P0', 'P1', 'P2', 'P3'] },
          file: { type: 'string' },
          line: { type: 'integer' },
          summary: { type: 'string' },
          scenario: { type: 'string' },
          fix: { type: 'string' },
        },
        required: ['priority', 'file', 'line', 'summary', 'scenario'],
      },
    },
  },
  required: ['findings'],
}

const VERDICT = {
  type: 'object',
  properties: {
    refuted: { type: 'boolean' },
    reason: { type: 'string' },
    // Only for a finding that stands: the priority the skeptic gives it after tracing the code.
    priority: { type: 'string', enum: ['P0', 'P1', 'P2', 'P3'] },
  },
  required: ['refuted', 'reason'],
}

const MOVE_LENS = {
  key: 'move-integrity', model: 'opus', effort: 'high',
  prompt: `Lens: integrity of a pure move (CLAUDE.md "Module size"). The move proof is in ${a.moveProof}: the split spec, the allowed-edit pairs and the proof output. ` +
    'Review every allowed-edit pair: each must be structural only (visibility widened to at most pub(super), a path prefix such as crate::, super:: or std::result::, ' +
    'a rustfmt re-indent or re-wrap with identical tokens, string and raw-string contents included). Then check what the proof does not cover: ' +
    'use/mod/pub use lines in the new headers (glob re-exports that shadow or change which item a name resolves to, a missing re-export for a path that code, docs or .claude/ name), ' +
    'every moved test module still declared (#[cfg(test)] mod tests;) under the right parent, ' +
    'include_str! paths (file-relative), #[cfg(feature = "cuda")] gating moved with its items, f32 operation order that mirrors kernels.cu, ' +
    'the resume_settings string, and references to moved paths or line numbers in ROADMAP.md, docs/ and .claude/. ' +
    'Any change that is not a move is a finding: say what it changes.',
}

// Claude reviewers. Each one counts toward `complete`: a reviewer that returns nothing is missing
// coverage, not a clean review.
const LENSES = pureMove ? [MOVE_LENS] : [
  {
    key: 'correctness', model: 'opus', effort: 'high',
    prompt: 'Lens: correctness. Logic errors, off-by-one, wrong units (PQ codes 10-bit vs 12-bit, nits, normalized), integer overflow, error paths that lose data or delete a source, resume/temp-dir state, concurrency in the external-tool runners.',
  },
  {
    key: 'contracts', model: 'opus', effort: 'high',
    prompt: 'Lens: repo contracts. Check the diff against the documented rules: CLAUDE.md sections "Cross-binary contracts", "Real entrypoints and boundaries" (with the CUDA rules), "mkvdovi operational gotchas", "Testing quirks", "Lint/style policy" and "Module size", and the "Code Review Rules" section of AGENTS.md. For a cross-binary contract, check every side the rule names (rg -w each side; LSP does not link them). Cite the rule in each finding: file, section and the rule in a few words. Skip what fmt and clippy already enforce.',
  },
  {
    key: 'tests-and-gates', model: 'opus', effort: 'high',
    prompt: `Lens: tests and gates. Does the change carry tests for what it changes? Would a test skip silently on this host (missing tool, missing sibling analyzer)? If L1 moves, are the tools/l1_diff/corpus references updated on purpose with the reason stated? The gates computed for this diff are: ${requiredGates.join(', ') || 'none'}; name a gate that is missing for this change. Is there evidence for the item's acceptance gate, or is the step being marked done without it?`,
  },
]
// Spec axis: does the diff do what the step asks, and only that? Runs only with the step text.
if (!pureMove && a.specFile) {
  LENSES.push({
    key: 'spec', model: 'opus', effort: 'high',
    prompt: 'Lens: specification. Read the specification file named below, then the diff. Report (a) a requirement or acceptance criterion of the step that the diff leaves out or covers only in part, ' +
      '(b) behaviour the step did not ask for that changes an output, a flag, a file format or a contract (scope creep), (c) a requirement implemented in a way that contradicts the step. ' +
      'Quote the spec line in the summary. Skip work the step itself defers to a later step.',
  })
}

// Model diversity (owner decision 2026-11-10): one Fable 5.2 whole-diff reviewer at high effort on
// a hard high-risk diff, on the assumption that it catches defects the Opus lenses miss. The Sonnet
// reviewer of the 2026-11-08 trial left the panel the same day (5 runs, two P3 findings beyond
// Opus). .claude/skills/pre-pr-review/reviewer-yield.sh checks the assumption on later records.
const fableSkip = pureMove ? 'pure move' : !highRisk ? 'no high-risk path' : !hard ? 'high-risk but not hard: under 300 logic lines, no numeric or two-sided contract path' : ''
if (!fableSkip) {
  log(`fable reviewer on: high-risk and hard (${hardReason})`)
  LENSES.push({
    key: 'fable', model: 'fable', effort: 'high',
    prompt: 'Whole-diff review, no fixed lens. Narrow reviewers already cover correctness, the repo contracts in CLAUDE.md, tests and gates, and (when the step text is handed over) the specification. Look for what narrow lenses miss: a design or specification error, an interaction between two changed files, an assumption that holds in the tests but not on real media, a contract changed on one side only. Report only real defects.',
  })
} else {
  log(pureMove ? 'pure move: one move-integrity lens, no Fable' : `fable reviewer skipped: ${fableSkip}`)
}


const ORDER = { P0: 0, P1: 1, P2: 2, P3: 3 }
const MAX_VERIFY = 8
// The skeptic always has a different lens than the finder; Fable findings are checked by Opus.
// Gemini runs outside this workflow (pre-pr-gemini.sh, started by the skill), so the panel never
// waits for it; the skill checks its findings itself, like Codex's.
const verifyLens = { correctness: 'contracts', contracts: 'correctness', 'tests-and-gates': 'correctness', spec: 'correctness', fable: 'contracts', 'move-integrity': 'contracts' }

const results = await pipeline(
  LENSES,
  l => agent(`${l.prompt}\n\n${SCOPE}`, {
    label: `review:${l.key}`, phase: 'Review', schema: FINDINGS, model: l.model, effort: l.effort,
  }).then(r => (r ? { lens: l.key, findings: r.findings.map(f => ({ ...f, lens: l.key })) } : { lens: l.key, failed: true })),
)
// A lens that returned nothing (skipped, API error) is missing coverage, not a clean review.
const failedLenses = LENSES.map((l, i) => (results[i] && !results[i].failed ? null : l.key)).filter(Boolean)
if (failedLenses.length) log(`review incomplete: lens(es) ${failedLenses.join(', ')} returned no result`)
// Who reviewed, for the report and the record: `ran` is false for a reviewer that returned nothing.
const reviewers = LENSES.map(l => (failedLenses.includes(l.key)
  ? { key: l.key, model: l.model, effort: l.effort, ran: false, reason: 'returned no result' }
  : { key: l.key, model: l.model, effort: l.effort, ran: true }))
if (pureMove) {
  for (const key of ['correctness', 'contracts', 'tests-and-gates']) reviewers.push({ key, model: 'opus', effort: 'high', ran: false, reason: 'pure move: replaced by move-integrity' })
  reviewers.push({ key: 'spec', model: 'opus', effort: 'high', ran: false, reason: 'pure move' })
  reviewers.push({ key: 'fable', model: 'fable', effort: 'high', ran: false, reason: 'pure move' })
} else {
  if (!a.specFile) reviewers.push({ key: 'spec', model: 'opus', effort: 'high', ran: false, reason: 'no specFile handed over' })
  if (fableSkip) reviewers.push({ key: 'fable', model: 'fable', effort: 'high', ran: false, reason: fableSkip })
}

// Barrier on purpose: cap the verification count, highest priority first. No dedupe here: two
// findings at one location may be two different defects, and dropping one costs more than
// verifying a duplicate twice. Claude merges true duplicates when it checks the list.
const all = results.filter(r => r && !r.failed).flatMap(r => r.findings)
  .sort((x, y) => ORDER[x.priority] - ORDER[y.priority])
const toVerify = all.slice(0, MAX_VERIFY)
const unverified = all.slice(MAX_VERIFY)
if (unverified.length) log(`${unverified.length} lower-priority findings returned unverified (cap ${MAX_VERIFY})`)

const verified = await parallel(toVerify.map(f => () =>
  agent(
    `Try to refute this review finding. Lens: ${verifyLens[f.lens] || 'correctness'}. ` +
    `Finding [${f.priority}] ${f.file}:${f.line}: ${f.summary}. Scenario: ${f.scenario}. ` +
    `Open the code, trace the scenario, and decide whether it really fails. Set refuted=true only when the failure ` +
    `cannot happen: the scenario is impossible, or the code already handles it. If it can happen, keep refuted=false ` +
    `even when it looks minor, intended or out of scope: say so in reason and set priority to the priority you would ` +
    `give it. Intent and severity are for the main session to weigh, not grounds to refute. ${SCOPE}`,
    { label: `verify:${f.file}:${f.line}`, phase: 'Verify', schema: VERDICT, model: 'opus', effort: 'medium' },
  ).then(v => ({
    ...f,
    // A confirmed finding takes the skeptic's priority; the finder's stays in finderPriority.
    priority: v && !v.refuted && v.priority ? v.priority : f.priority,
    finderPriority: f.priority,
    verdict: v ? (v.refuted ? 'refuted' : 'confirmed') : 'unverified',
    verdictReason: v ? v.reason : 'verifier failed',
  }))
))

// A verifier that threw leaves null in its slot: keep that finding, marked unverified.
const checked = toVerify.map((f, i) => verified[i] || { ...f, verdict: 'unverified', verdictReason: 'verifier failed' })
return {
  started: true,
  kind: pureMove ? 'pure-move' : 'full',
  complete: failedLenses.length === 0,
  failedLenses,
  reviewers,
  requiredGates,
  findings: checked.filter(f => f.verdict === 'confirmed'),
  refuted: checked.filter(f => f.verdict === 'refuted'),
  unverified: [
    ...checked.filter(f => f.verdict === 'unverified'),
    ...unverified.map(f => ({ ...f, verdict: 'unverified', verdictReason: `over the cap of ${MAX_VERIFY}` })),
  ],
}
