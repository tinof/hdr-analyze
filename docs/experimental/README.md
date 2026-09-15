# Experimental material

Nothing in this directory is a release feature. These documents record research, prototypes and
open questions around Dolby Vision Profile 7 FEL and remote encoding.

The one FEL path that ships in `mkvdovi` composites the base and enhancement layers and re-encodes
the picture. The accuracy of that compositor has not been validated against an independent
reference decode. Profile 5 output and lossless FEL to Profile 8 conversion are not implemented.

## Documents

- [profile7_fel_to_profile81_preservation.md](profile7_fel_to_profile81_preservation.md): research
  plan that asks whether some FEL contribution can be approximated in a Profile 8.1 RPU without
  re-encoding the base layer. It frames the idea as a falsifiable experiment and explains why a
  lossless general conversion is not possible.
- [profile7_fel_developer_handoff.md](profile7_fel_developer_handoff.md): implementation handoff
  for testing that metadata-only hypothesis. It also describes the current pixel-baking path and the
  compositor's experimental status.
- [fel_to_rpu_research_brief.md](fel_to_rpu_research_brief.md): an earlier research brief on
  capturing FEL adjustments as modified RPU metadata. It is an idea, not a plan of record, and its
  preservation estimate is retracted.
- [MODAL_FFMPEG_INTEGRATION.md](MODAL_FFMPEG_INTEGRATION.md): notes on a prototype that offloads
  FFmpeg HEVC encoding, including FEL compositing, to Modal.com GPU or CPU instances. It depends on
  a separate local checkout and is not part of the release.

## See also

- [Format compatibility](../FORMAT_COMPATIBILITY.md) for supported conversion paths and maturity.
- [Roadmap](../../ROADMAP.md) for the FEL feasibility track.
