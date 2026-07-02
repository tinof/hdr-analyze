# Provenance & Clean-Room Statement

This project generates dynamic HDR metadata (Dolby Vision Profile 8.1, CM v4.0) using only
public standards, public documentation, and open-source tools. This document states exactly
where every piece of domain knowledge comes from, and where the honest limits of that
statement are.

## What this project does NOT contain or use

- **No Dolby Laboratories proprietary code, SDKs, lookup tables, tone curves, or binary
  blobs** — enforced by review; the repository is auditable, and `cargo deny` gates dependency
  licenses in CI.
- **No leaked or informally circulated Dolby tools.** In particular, this project does not
  use, invoke, or compare against the leaked `cm_analyze` binary or any other Dolby
  professional tooling, and it never will. Validation targets are the RPUs of *retail Dolby
  Vision releases* (observable, shipped metadata) and synthetic test patterns — never the
  output of unlicensed Dolby software.
- **No reverse engineering of Dolby binaries.** Behavioral reference is limited to public
  specs and the observable input/output of open-source tools.

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
pixels and emits generic measurement data plus a configuration JSON; the actual Dolby Vision
RPU bitstream is authored entirely by `dovi_tool`, which the user installs independently.

## Honest legal framing

- **Copyright / trade secrets:** clean. Nothing proprietary is copied, embedded, or
  misappropriated; all format knowledge derives from published standards.
- **Patents:** no one can honestly certify an implementation of any modern video technology
  as free of all patent claims, and this project does not. Dolby holds patents in the HDR
  metadata space, including on the generation side. What we can state is the method: a
  clean-room implementation of public standards, in the same posture as `dovi_tool` and
  `hdr10plus_tool`, which have existed publicly since 2020 and 2019 respectively.
- **Trademarks:** Dolby, Dolby Vision, and the double-D symbol are trademarks of Dolby
  Laboratories Licensing Corporation. HDR10+ is a trademark of HDR10+ Technologies, LLC.
  This project is not affiliated with, endorsed, or sponsored by either. References are
  nominative — describing compatibility and interoperability only. For this reason the
  converter binary is named `mkvdovi` (community convention, cf. `dovi_tool`), not after
  the trademark.

Because of the patent reality above, this project never claims "no IP infringement" as an
absolute. The accurate claim, which we stand behind and which this document exists to make
auditable, is: **no Dolby code, no Dolby secrets, no leaked tools — public standards only.**
