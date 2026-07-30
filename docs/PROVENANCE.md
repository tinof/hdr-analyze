# Implementation Provenance and Source-Material Boundary

This project generates dynamic HDR metadata (Profile 8.1, CM v4.0, compatible with the Dolby
Vision® format) from the public standards and open-source dependencies listed below. This
document records where the domain knowledge in this repository comes from, so that the claim
is auditable rather than asserted.

The repository implementation is intended to be independently derived from the public standards
and identified open-source dependencies listed below. To the maintainer's knowledge, no Dolby
proprietary source code, SDK, confidential documentation, leaked tooling, or material obtained
in breach of a confidentiality obligation was copied into or used to derive the implementation.
A licensed copy of the Dolby Vision Professional Tools has been used only for comparative output
evaluation as described below; its code and documentation are not included or redistributed here.

**This is a factual provenance statement, not a legal conclusion concerning copyright, trade
secrets, patents, contractual rights, or freedom to operate.**

## What this project does NOT contain or use

- **No Dolby Laboratories proprietary code, SDKs, lookup tables, tone curves, or binary blobs
  are included in this repository.** The repository is auditable in full, and `cargo deny` gates
  dependency licenses in CI. (`cargo deny` checks dependency-license policy; it does not and
  cannot verify implementation provenance — that rests on the review record and this document.)
- **No leaked or unlicensed Dolby tools.** To the maintainer's knowledge this project does not
  use, invoke, or compare against leaked or informally circulated copies of Dolby professional
  tooling (such as the `cm_analyze` binary circulating in forums). Validation targets are the
  RPUs of *retail releases* (observable, shipped metadata), synthetic test patterns with
  mathematically known values, and — strictly for scoring output accuracy — a **licensed** copy
  of the Dolby Vision Professional Tools (see the validation boundary below).
- **No reverse engineering of Dolby binaries.** Behavioral reference is limited to public specs
  and the observable input/output of open-source tools.

## Validation boundary for licensed Dolby tools

The maintainer holds a licensed copy of the Dolby Vision Professional Tools. Its role in this
project is bounded and one-directional: its output may be used to *score* this project's
measurements (published as accuracy numbers in `docs/VALIDATION.md`), and for nothing else.
No Dolby tool output is used to derive, fit, or tune any algorithm or constant in this
codebase; the implementation remains built from the public standards listed below, and every
implementation decision must be traceable to them. Licensed Dolby software and its
documentation are never redistributed through this repository.

This use remains subject to the applicable end-user license agreement; a provenance statement
cannot override contractual restrictions.

**License review (v5.6.4 EULA, reviewed 2026-07-31).** The end-user license agreement presented by
the v5.6.4 installer was read in full. It contains **no benchmark or publication restriction** — no
clause limiting the publication of test results, comparative measurements, or performance data. Its
restrictions cover copying, derivative works, reverse engineering, and commercial hosting, none of
which describe measuring our own content and comparing the resulting statistics. Two terms do bound
what may be published here, and this project observes both:

- **Documentation is licensee-confidential and may not be republished** (EULA §1.2). No Dolby
  manual, release note, or bundled license document is quoted, excerpted, or committed to this
  repository, and none ever should be.
- **Confidential Information is defined broadly**, extending to the software's documentation and
  proprietary information. Accordingly, published validation material is limited to **derived
  statistics computed by this project** — deltas, error distributions, and summary accuracy figures
  over our own test content — and never to the tool's documentation, its raw output files, or
  verbatim text from either.

Re-check this section on any major version upgrade of the licensed tool; the terms reviewed above
are those of v5.6.4 and are not guaranteed to carry forward.

## Public sources everything is built from

| Area | Source |
|------|--------|
| DV bitstream / RPU / composer metadata | ETSI GS CCM 001 V1.1.1 (public ETSI spec, "Compound Content Management") |
| Dynamic metadata semantics (L1 etc.) | SMPTE ST 2094-10 (published SMPTE standard) |
| PQ transfer function | SMPTE ST 2084 / ITU-R BT.2100 |
| HLG transfer function | ARIB STD-B67 / ITU-R BT.2100 |
| Tone-mapping reference (planned trims) | ITU-R BT.2390 (EETF) |
| Static HDR metadata (MaxCLL/MaxFALL) | CTA-861 |
| madVR measurement file format | MIT-licensed [`madvr_parse`](https://crates.io/crates/madvr_parse) by quietvoid |
| RPU authoring & injection | MIT-licensed [`dovi_tool`](https://github.com/quietvoid/dovi_tool) by quietvoid (external, user-installed) |
| HDR10+ metadata extraction | MIT-licensed [`hdr10plus_tool`](https://github.com/quietvoid/hdr10plus_tool) by quietvoid (external, user-installed) |

The boundary is strict: this project computes per-frame luminance statistics from decoded
pixels and emits generic measurement data plus a configuration JSON; the RPU bitstream itself
is authored entirely by `dovi_tool`, which the user installs independently.

## Legal framing and its limits

- **Copyright / trade secrets:** to the maintainer's knowledge, nothing proprietary is copied
  or embedded, and all format knowledge in this repository derives from the published standards
  listed above. Under US federal law, independent derivation and lawful reverse engineering are
  excluded from "improper means"; EU law similarly permits independent creation and the study or
  testing of lawfully obtained products absent a valid duty to the contrary. Those provisions
  describe the method used here; they are not an adjudication of any claim, and they do not
  displace obligations arising under a license agreement.
- **Patents:** no patent search, claim analysis, licensing determination, or freedom-to-operate
  review has been performed. Independent development, public documentation, and open-source
  licensing do **not** establish non-infringement of patents — independent creation is generally
  not a defense to direct patent infringement. Dolby holds patents in the HDR metadata space,
  including on the generation side. This project makes no representation that making, using,
  distributing, or operating the software is licensed under or free from third-party patent
  rights.
- **Trademarks:** Dolby, Dolby Vision, and the double-D symbol are trademarks of Dolby
  Laboratories Licensing Corporation. HDR10+ is a trademark of HDR10+ Technologies, LLC.
  This project is not affiliated with, endorsed by, sponsored by, certified by, approved by, or
  licensed by either. References are nominative — describing compatibility and interoperability
  only. The converter binary is deliberately named `mkvdovi` rather than after the mark.

This project therefore never claims "no IP infringement" as an absolute, and does not present
this document as a legal opinion. What it does record, and what the sources above make
auditable, is the method: an independent implementation of public standards, with no Dolby
code, confidential material, or leaked tooling used to derive it.
