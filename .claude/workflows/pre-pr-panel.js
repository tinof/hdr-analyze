export const meta = {
  name: 'pre-pr-panel',
  description: 'Pre-PR review of a roadmap change: three Opus lenses at high effort plus Fable on high-risk diffs, each finding checked by an independent Opus skeptic, plus the gates the touched paths require',
  whenToUse: 'Internal to the pre-pr-review skill; do not run it directly. Started by that skill (standalone or from /codex-ship step 1b). args: {base, files, item, acceptanceGate, scopeFile, kind?, moveProof?, moveFiles?}; kind "pure-move" with a moveProof path and moveFiles covering every non-Markdown file runs one move-integrity lens instead of the panel. Do not start it without args. With {base, files, gatesOnly: true} it only returns requiredGates and starts no agents; pre-pr-review step 5 and /codex-ship steps 4 and 6 call it that way to recompute gates after fixes.',
  phases: [
    { title: 'Review', detail: 'correctness, repo contracts, tests and gates (Opus, high); whole diff (Fable, medium) on high-risk paths; or one move-integrity lens (Opus, high) on a pure move' },
    { title: 'Verify', detail: 'one Opus skeptic per finding (medium), with a different lens than the finder' },
  ],
}

let a = args
if (typeof a === 'string') {
  try { a = JSON.parse(a) } catch (e) { a = null }
}
if (!a || !Array.isArray(a.files) || a.files.length === 0 || !a.base) {
  log('no scope handed over: nothing reviewed')
  return { started: false, reason: 'pre-pr-panel needs args {base, files[], item, acceptanceGate, scopeFile}' }
}

const files = a.files.map(f => String(f).replace(/^\.\//, ''))
const any = re => files.some(f => re.test(f))

// Required gates, computed from the touched paths (not left to judgment).
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
// The Gemini pass and its fake-agy harness (no network, no model).
if (any(/^\.claude\/workflows\/(pre-pr-gemini\.|tests\/)|^\.agents\/agents\/pre-pr-reviewer\.md$/)) gates.add('gemini-harness')
const requiredGates = [...gates]
log('required gates: ' + (requiredGates.length ? requiredGates.join(', ') : 'none (no Rust, analysis or Gemini-pass paths touched)'))
// Gate list only, no reviewers: lets the skills recompute gates for a file list that grew after
// fixes, without a second copy of the path rules.
if (a.gatesOnly) return { gatesOnly: true, requiredGates }

// Pure move (a module-size split with a passing line-exact move proof): one move-integrity lens
// replaces the three lenses and Fable. The skill selects it only after rerunning the proof on HEAD;
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

// High-risk diffs add the Fable reviewer: the paths that require CUDA parity or the L1 regression
// gate, plus the cross-binary contract modules named in CLAUDE.md. A module counts as a file or as
// a directory of submodules (metadata.rs or metadata/), so a split never drops the reviewer.
const highRisk = !pureMove && (cuda || measurement ||
  any(/^hdr_analyzer_mvp\/src\/l1_sidecar(\.rs$|\/)|^mkvdovi\/src\/(metadata|pipeline|external|resume)(\.rs$|\/)/))

const SCOPE = [
  `Scope: run \`git diff ${a.base}...HEAD\` for the committed part, and read the uncommitted files `,
  `named in ${a.scopeFile || 'the file list below'} (\`git diff HEAD -- <file>\` or Read for untracked ones). `,
  `Files in scope: ${files.join(', ')}. Roadmap item and step: ${a.item || 'unknown'}. `,
  `Its acceptance gate: ${a.acceptanceGate || 'not stated'}. `,
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
    prompt: 'Lens: repo contracts from CLAUDE.md. The L1 sidecar schema and version on both sides (hdr_analyzer_mvp/src/l1_sidecar.rs, mkvdovi metadata::L1_SIDECAR_MAX_VERSION, tools/l1_diff), the +cuda version probe, luminance_mapping names, --help option probes (external::analyzer_lists_option), resume_settings for artifact-affecting flags, CPU/CUDA bit-identical arithmetic and buffer layouts (kernels.cu vs gpu.rs), mkvdovi never re-encodes video, no silent clamp, HLG colour contract.',
  },
  {
    key: 'tests-and-gates', model: 'opus', effort: 'high',
    prompt: `Lens: tests and gates. Does the change carry tests for what it changes? Would a test skip silently on this host (missing tool, missing sibling analyzer)? If L1 moves, are the tools/l1_diff/corpus references updated on purpose with the reason stated? The gates computed for this diff are: ${requiredGates.join(', ') || 'none'}; name a gate that is missing for this change. Is there evidence for the item's acceptance gate, or is the step being marked done without it?`,
  },
]
if (highRisk) {
  LENSES.push({
    key: 'fable', model: 'fable', effort: 'medium',
    prompt: 'Whole-diff review, no fixed lens. Three narrow reviewers already cover correctness, the repo contracts in CLAUDE.md, and tests and gates. Look for what narrow lenses miss: a design or specification error, an interaction between two changed files, an assumption that holds in the tests but not on real media, a contract changed on one side only. Report only real defects.',
  })
} else {
  log(pureMove ? 'pure move: one move-integrity lens, no Fable' : 'fable reviewer skipped: no high-risk path in this diff')
}


const ORDER = { P0: 0, P1: 1, P2: 2, P3: 3 }
const MAX_VERIFY = 8
// The skeptic always has a different lens than the finder; Fable findings are checked by Opus.
// Gemini runs outside this workflow (pre-pr-gemini.sh, started by the skill), so the panel never
// waits for it; the skill checks its findings itself, like Codex's.
const verifyLens = { correctness: 'contracts', contracts: 'correctness', 'tests-and-gates': 'correctness', fable: 'contracts', 'move-integrity': 'contracts' }

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
  reviewers.push({ key: 'fable', model: 'fable', effort: 'medium', ran: false, reason: 'pure move' })
} else if (!highRisk) reviewers.push({ key: 'fable', model: 'fable', effort: 'medium', ran: false, reason: 'no high-risk path' })

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
    `Open the code, trace the scenario, and decide whether it really fails. Default to refuted=true ` +
    `if the scenario cannot happen or the code already handles it. ${SCOPE}`,
    { label: `verify:${f.file}:${f.line}`, phase: 'Verify', schema: VERDICT, model: 'opus', effort: 'medium' },
  ).then(v => ({ ...f, verdict: v ? (v.refuted ? 'refuted' : 'confirmed') : 'unverified', verdictReason: v ? v.reason : 'verifier failed' }))
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
