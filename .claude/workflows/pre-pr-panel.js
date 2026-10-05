export const meta = {
  name: 'pre-pr-panel',
  description: 'Pre-PR review of a roadmap change: three review lenses, each finding checked by an independent skeptic, plus the gates the touched paths require',
  whenToUse: 'Internal to the pre-pr-review skill; do not run it directly. Started by that skill (standalone or from /codex-ship step 1b). args: {base, files, item, acceptanceGate, scopeFile}. Do not start it without args.',
  phases: [
    { title: 'Review', detail: 'correctness, repo contracts, tests and gates; one Opus reviewer each' },
    { title: 'Verify', detail: 'one skeptic per finding, with a different lens than the finder' },
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
const workspaceRust = files.some(f => !f.startsWith('tools/') && /\.rs$|(^|\/)Cargo\.(toml|lock)$|^\.cargo\/|^rust-toolchain\.toml$|^clippy\.toml$/.test(f))
if (workspaceRust) { gates.add('fmt'); gates.add('clippy'); gates.add('test') }
// CPU/GPU analysis path: the kernel, its host side, the decode path, the orchestration that hands
// frames to it, the composer coefficients both backends use, and build inputs that change them.
// Build inputs that can change measured output without touching Cargo.lock: the workspace and crate
// manifests (features, dependency versions, profiles), compiler flags and the toolchain.
const BUILD = /^Cargo\.(toml|lock)$|^hdr_analyzer_mvp\/Cargo\.toml$|^dovi84_composer\/Cargo\.toml$|^\.cargo\/config\.toml$|^rust-toolchain\.toml$/
const cuda = any(BUILD) || any(/^hdr_analyzer_mvp\/src\/(analysis\/|ffmpeg_io\.rs$|pipeline\.rs$|cli\.rs$|crop\.rs$)|^dovi84_composer\/src\/|^scripts\/cuda-parity\.sh$|^hdr_analyzer_mvp\/tests\/cuda_parity\.rs$/)
if (cuda) { gates.add('clippy-cuda'); gates.add('cuda-parity') }
// Measurement change: anything that can move the analyzer's L1 output or how it is scored.
const measurement = any(BUILD) || any(/^hdr_analyzer_mvp\/src\/|^dovi84_composer\/src\/|^tools\/l1_diff\/|^scripts\/ci\/l1-regression-gate\.sh$/)
if (measurement) gates.add('l1-regression')
for (const f of files) {
  const m = /^tools\/([^/]+)\/(.+)$/.exec(f)
  if (m && /\.rs$|Cargo\.(toml|lock)$/.test(m[2])) gates.add('tool:' + m[1])
}
const requiredGates = [...gates]
log('required gates: ' + (requiredGates.length ? requiredGates.join(', ') : 'none (no Rust or analysis paths touched)'))

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

const LENSES = [
  {
    key: 'correctness',
    prompt: 'Lens: correctness. Logic errors, off-by-one, wrong units (PQ codes 10-bit vs 12-bit, nits, normalized), integer overflow, error paths that lose data or delete a source, resume/temp-dir state, concurrency in the external-tool runners.',
  },
  {
    key: 'contracts',
    prompt: 'Lens: repo contracts from CLAUDE.md. The L1 sidecar schema and version on both sides (hdr_analyzer_mvp/src/l1_sidecar.rs, mkvdovi metadata::L1_SIDECAR_MAX_VERSION, tools/l1_diff), the +cuda version probe, luminance_mapping names, --help option probes (external::analyzer_lists_option), resume_settings for artifact-affecting flags, CPU/CUDA bit-identical arithmetic and buffer layouts (kernels.cu vs gpu.rs), mkvdovi never re-encodes video, no silent clamp, HLG colour contract.',
  },
  {
    key: 'tests-and-gates',
    prompt: `Lens: tests and gates. Does the change carry tests for what it changes? Would a test skip silently on this host (missing tool, missing sibling analyzer)? If L1 moves, are the tools/l1_diff/corpus references updated on purpose with the reason stated? The gates computed for this diff are: ${requiredGates.join(', ') || 'none'}; name a gate that is missing for this change. Is there evidence for the item's acceptance gate, or is the step being marked done without it?`,
  },
]

const ORDER = { P0: 0, P1: 1, P2: 2, P3: 3 }
const MAX_VERIFY = 6
const verifyLens = { correctness: 'contracts', contracts: 'correctness', 'tests-and-gates': 'correctness' }

const results = await pipeline(
  LENSES,
  l => agent(`${l.prompt}\n\n${SCOPE}`, {
    label: `review:${l.key}`, phase: 'Review', schema: FINDINGS, model: 'opus', effort: 'low',
  }).then(r => (r ? { lens: l.key, findings: r.findings.map(f => ({ ...f, lens: l.key })) } : { lens: l.key, failed: true })),
)
// A lens that returned nothing (skipped, API error) is missing coverage, not a clean review.
const failedLenses = LENSES.map((l, i) => (results[i] && !results[i].failed ? null : l.key)).filter(Boolean)
if (failedLenses.length) log(`review incomplete: lens(es) ${failedLenses.join(', ')} returned no result`)

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
    { label: `verify:${f.file}:${f.line}`, phase: 'Verify', schema: VERDICT, model: 'opus', effort: 'low' },
  ).then(v => ({ ...f, verdict: v ? (v.refuted ? 'refuted' : 'confirmed') : 'unverified', verdictReason: v ? v.reason : 'verifier failed' }))
))

// A verifier that threw leaves null in its slot: keep that finding, marked unverified.
const checked = toVerify.map((f, i) => verified[i] || { ...f, verdict: 'unverified', verdictReason: 'verifier failed' })
return {
  started: true,
  complete: failedLenses.length === 0,
  failedLenses,
  requiredGates,
  findings: checked.filter(f => f.verdict === 'confirmed'),
  refuted: checked.filter(f => f.verdict === 'refuted'),
  unverified: [
    ...checked.filter(f => f.verdict === 'unverified'),
    ...unverified.map(f => ({ ...f, verdict: 'unverified', verdictReason: `over the cap of ${MAX_VERIFY}` })),
  ],
}
