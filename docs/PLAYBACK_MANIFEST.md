# Playback manifest (P15)

Status: design written 2026-11-04. The AU table (P15.1), the manifest schema, parser and matcher
in `dovi_rpu_builder::manifest` (P15.2) and `mkvdovi manifest` (P15.3) are implemented; the
`RPU.bin` import is not yet (P15.4). Steps: ROADMAP P15.

## Purpose

The NuvioTV fork (`tinof/NuvioTV-Fork`, `ROADMAP.md` Appendix A, DV4) converts HLG to Profile 8.4
and HDR10 to 8.1 during playback on a Shield TV Pro. It adds one RPU to each access unit (AU) with
[`dovi_rpu_builder`](RPU_BUILDER.md). Without measurements it can only write static L1. The
playback manifest carries this repository's measured per-shot L1 to the player. It is made ahead
of time, possibly over a read-only network mount. The player fetches it at play start and checks
it against the stream as it reads each AU.

The fork's requirements are in its `docs/dv-conversion/DESIGN.md` section 2.8: track identity
with incremental validation, a timeline mapping without `time × fps` arithmetic, provenance, and
coverage with a quality class. The rule for every design choice: an AU gets manifest L1 only when
the manifest provably describes that AU of that encode. Otherwise the player keeps its own
fallback.

## Artifacts

| File | Written by | Contents |
|---|---|---|
| `<measurements>.l1.json` | analyzer | the L1 sidecar, version 6, unchanged |
| `<measurements>.au.json` | analyzer, with `--au-table` | the AU table (version 1), bound to the sidecar by digest |
| `<stem>.dvmanifest.json` | `mkvdovi manifest` | the playback manifest (version 1) |

The sidecar schema does not change: the 1.0.0 release row requires it to stay stable for a minor
cycle. The AU table is a new cross-binary contract. Its writer is `hdr_analyzer_mvp`. Its reader
is mkvdovi, which probes the capability token `au-table-v1` in the analyzer's `--help` before it
starts any analysis. That is the same pattern as `l1-sidecar-frame-peak`.

## Scope of version 1

- Container Matroska, codec HEVC, exactly one video track. The analyzer picks FFmpeg's "best"
  video stream and mkvdovi uses `v:0`, so a file with more than one video track is refused rather
  than reconciled.
- HLG to 8.4 and HDR10 to 8.1, with measured L1.
- Refused before analysis: HDR10+ input (the fork has its own HDR10+ provider), Dolby Vision
  input, `--legacy-madvr-l1`, and non-Matroska input.
- Refused by the packet pass: laced video blocks, a video packet without a pts, and two
  video packets with the same pts. The analyzer then writes no AU table and `mkvdovi manifest`
  refuses the file; the measurements and a normal `mkvdovi` conversion are unaffected.
- One known source of a packet without a pts is an open-GOP cut made by an FFmpeg stream copy:
  its leading RASL pictures can carry negative Matroska timestamps, and FFmpeg's demuxer gives
  them no pts. An open-GOP file whose leading pictures carry valid timestamps (tested on an x265 cut
  muxed by mkvmerge) is accepted.
- Imported `RPU.bin` files (P15.4) are accepted only when they are reproducible from the manifest's
  parameters and shot list, byte for byte for every AU. Storing arbitrary per-AU RPU payloads is
  a question for a version 2.

## AU table (`.au.json`, version 1)

The analyzer records every packet of the video stream in decode (file) order while it measures.
It needs no second read of the file. The fields:

- `version`: 1.
- `sidecar_sha256`: SHA-256 of the sidecar bytes as written in the same run. The analyzer deletes
  a stale `.au.json` before analysis starts. It writes the sidecar first, then the AU table, each
  through a temp name and a rename. mkvdovi uses the pair only when the digest matches.
- `source`:
  - `file_name`, `size_bytes`;
  - `head_sha256` over bytes `[0, min(1 MiB, size))`;
  - `tail_sha256` over bytes `[max(0, size − 1 MiB), size)`.
- `track`:
  - `container` (`matroska`), `codec` (`hevc`), `width`, `height`;
  - `codec_private_sha256`;
  - `time_base` as `[num, den]`, which is FFmpeg's stream time base (TimestampScale / 10⁹).
  - No Matroska TrackNumber: FFmpeg does not expose it. Version 1 allows one video track, so the
    track is the file's only video track.
- `analyzer_version`.
- `aus`:
  - `count`, `pts_first`, `pts_delta[]` (`count − 1` entries, `pts[k+1] − pts[k]` in decode
    order, in time-base ticks), `size[]`;
  - `non_picture[]`: the decode indices of packets without a slice NAL, such as an
    end-of-sequence packet;
  - `irap_crc32`: `[[k, crc], …]`. The CRC is CRC-32/ISO-HDLC, the one `java.util.zip.CRC32`
    computes, taken over the whole packet. Only IRAP AUs get one.
  - Sizes and CRCs are over the stored frame bytes after Matroska header stripping is undone,
    before any NAL reframing.
- `leading_skipped`: the sidecar's `leading_skipped_frames`, the RASL pictures of the first IRAP.
- `scene`: detector, metric, threshold and minimum length as run. The sidecar does not record
  these, and the quality class needs them.

**Presentation index.** The presentation index of a picture is its pts rank among the pictures.
The analyzer proves this rather than assuming it: the pts of the frames the decoder outputs must
equal the sorted picture pts, minus the first `leading_skipped` entries. H.265 outputs RASL before
RADL, so the smallest pts are the skipped RASL pictures. That matches mkvdovi's shot mapping,
where scene 0 absorbs the leading pictures. If the check fails, the analyzer writes no AU table
and exits with status 1. The `.bin` and the sidecar are written either way.

`--au-table-only` writes the table without measuring. It still decodes every frame for the order
check, and `sidecar_sha256` is null.

**Lacing.** FFmpeg splits a laced block into several packets. Its timestamps for the laces need
not match the times Media3 synthesizes from the track's default duration. The analyzer refuses
when two video packets share one container position. The player stays the authority: its
extractor knows each block's lace count and rejects the manifest on any laced video sample.

## Manifest (`.dvmanifest.json`, version 1)

- `format`: `"hdr-analyze.playback-manifest"`, and `version`: 1.
- `producer`: mkvdovi version, analyzer version (with `+cuda`), sidecar version, AU table version,
  `dovi_rpu_builder` version, and `sidecar_sha256`.
- `source`: as in the AU table, plus `lookup_key` = SHA-256 of (size as a big-endian u64 ‖ head
  digest ‖ tail digest), where the digests are their 32 raw bytes, not the hex text: 72 bytes are
  hashed. A plain HTTP folder can serve `<lookup_key>.dvmanifest.json`. The player computes the
  key from one size lookup and two 1 MiB range reads.
- `track`: as in the AU table.
- `aus`: as in the AU table.
- `analysis`: `luminance_mapping` (`pq` or the HLG composer name), `downscale`, `sample_rate`,
  `gpu`, `no_crop`, `crop`, the peak and denoise settings (`peak_domain`, `peak_estimator`,
  `peak_source`, `peak_percentile`, `min_percentile`, `denoise_mode`) and `scene`, copied from the
  sidecar and the AU table; a field they do not state is left out.
- `rpu`: everything `RpuParams` needs, written and read by `RpuParams::{to,from}_manifest_json`:
  - `profile`: `"8.1"` or `"8.4"`;
  - `composer`: `"preset"` or `"bt2100"` for 8.4, null for 8.1 (the P8 canary is refused);
  - `source_range`: `[min, max]` (12-bit PQ) or null for 8.1, null for 8.4;
  - `cm`: `{"version": "V29"}`, or `{"version": "V40", "source_primary_index",
    "content_type", "reference_mode", "trim_targets_nits"}`;
  - `level6`: the four `extra.json` fields; `level5`: `{left, right, top, bottom}` or null;
  - `l1_avg_pq_cm_version`: `"V29"` or `"V40"`.
- `l1_policy`: `"measured"`, or `"rpu-bin"` for P15.4.
- `shots`: parallel arrays `start[]` (presentation index in stream frames, starting at 0),
  `min[]`, `avg[]` and `max[]` (12-bit PQ, unclamped, exactly as mkvdovi's `extra.json` gets
  them).
- `coverage`: `[{start, end, class}]`, sorted, not overlapping, within `[0, stream pictures)`.
  Every `start` is a shot start, and every `end` is a shot start or the picture count, so no shot
  is part covered. Pictures outside every range get no manifest L1 and are still checked.
  - `full` means sample rate 1, downscale 1, the global scene detector at mkvdovi's defaults,
    mkvdovi's default peak and denoise settings, and a passed order check.
  - `approximate` is anything else.
  - Version 1 writes one range over the whole stream. The field exists for the fork's later
    server and on-device providers.

**Parity.** For every AU, the builder's `shot(l1).write(scene_refresh)` from the manifest equals
the RPU NAL that mkvdovi writes for the same file and settings. P15.3 tests this
(`mkvdovi/tests/playback_manifest.rs`): the source's packets go through `AuMatcher` in decode
order and each built RPU is compared with the RPU of the same AU in the `--keep-source` output,
for 8.1, 8.4 bt2100, VFR, a non-zero start, an open-GOP cut, CM v2.9, other trims and L9/L11, and a
letterboxed source (L5).

## `mkvdovi manifest`

`mkvdovi manifest <input> [--out <file>] [--work-dir <dir>]` writes `<stem>.dvmanifest.json` into
the work directory (default: the input's directory). It also takes the conversion flags that
change L1 or the RPU, with the conversion's defaults: `--hlg-composer`, `--cm-version`,
`--content-type`, `--reference-mode`, `--source-primaries`, `--trim-targets`,
`--analysis-quality`, `--hwaccel`. Code: `mkvdovi/src/pipeline/manifest/`.

- It never creates a temp directory, converts, muxes or deletes. It writes only
  `<stem>_measurements.bin` with its `.l1.json` and `.au.json`, `<stem>.manifest-analyzer.log`
  and the manifest (through `<out>.tmp`, created for the run, and a rename; an existing
  `<out>.tmp` may be another run's, so it is refused, never removed). The analyzer writes the
  `.l1.json` and `.au.json` through `.tmp` files too. Any of these paths, temp files included,
  that names the input, also through a symlink or a hard link, is refused.
- Refused before any analysis: input that is not Matroska (EBML DocType `matroska`), HDR10+,
  Dolby Vision, an HLG or PQ stream that fails the conversion's colour contracts, an analyzer
  whose `--help` does not name `au-table-v1`, and a missing `mediainfo` or `dovi_tool` (the
  format detection needs both to tell Dolby Vision and HDR10+ from HDR10).
- The measurements are the conversion's (`--transfer hlg`, `--hlg-composer`, the sampling of
  `--analysis-quality`) plus `--au-table`. A pair already in the work directory is reused only
  when the sidecar is valid for the input and composer, was sampled exactly as asked, the table
  is version 1 and bound to these sidecar bytes, its structure passes the parser's AU rules, its
  source still matches the file, and both count the same pictures and leading pictures.
  Anything else is reported and analyzed again.
- The RPU parameters come from the conversion's configuration phase
  (`pipeline::config::settle_configuration`, `metadata::rpu_params`), so they equal `extra.json`'s,
  except that an 8.4 manifest names the composer: the conversion installs it after
  `dovi_tool generate`, and a player has no such step.
- Coverage is one range over the stream. It is `full` when the files state downscale 1, sample
  rate 1, `no_crop` false, scene `global`/`hist`/3.0/12, peak domain `max-rgb`, estimator and
  source `max`, percentile 99.99, minimum percentile 0.1 and denoise `off` (the analyzer defaults
  mkvdovi runs with); otherwise `approximate`.

## Matching rule (`dovi_rpu_builder::manifest`)

The schema, the strict parser and the matcher live in `dovi_rpu_builder`. That crate is pure
Rust, builds for Android and is already linked by the fork, so the writer and the player share
one definition. The parser never panics; every refusal is `Error::Manifest` (code 8).
`AuMatcher::new` applies the same checks (`validate()`) to a manifest built in code. Unknown keys
are ignored within version 1. It refuses:

- an unknown format or version, or a track other than Matroska HEVC;
- array lengths that disagree, or no AUs;
- `non_picture` or `irap_crc32` indices that are unsorted, repeated or out of range, or an IRAP
  entry on a non-picture AU;
- a pts that is not unique over all AUs;
- `leading_skipped` above the picture count, or a time base that is not positive;
- shots that are empty, do not start at 0, do not rise, start beyond the pictures, or have an L1
  code above 4095;
- coverage that is empty, unsorted, overlapping, beyond the pictures, or off shot boundaries;
- digests that are not 64 lowercase hex digits;
- a composer whose luminance mapping differs from `analysis.luminance_mapping` (8.1 needs `pq`);
- an `rpu` that `RpuBuilder::new` refuses, so a parsed manifest builds every shot.

`check_source`, `check_track` (CodecPrivate digest, width, height, time base) and
`check_composer` cover the candidate and track steps below. SHA-256 is not computed in the crate:
the producer and the player compute the digests and compare hex strings.

The player then follows these steps:

1. **Candidate.** Size, both chunk digests and `lookup_key` match.
2. **Track.** The file has one video track, and its CodecPrivate digest and time base match. The time base must agree
   with the player's TimestampScale. Times are compared as raw ticks (cluster timecode plus block
   timecode), never as rounded microseconds.
3. **Per AU.**
   - Find the AU by its pts. An unknown pts rejects the manifest.
   - The size must match, and an IRAP AU's CRC must match.
   - While reading is continuous, the decode index must be the previous one plus 1.
   - After a seek or reset the matcher waits for an IRAP. AUs before it get no manifest L1 and
     carry the player's fallback RPU (Media3 normally drops them); their pts and size are still
     checked. The first IRAP is validated in full, and continuity applies from there on.
4. **Shot.** The presentation index p is the pts rank. The shot is the last one with
   `start ≤ p`, and `scene_refresh` is set only when `p == start` (mkvdovi parity).
   A non-picture AU, and a picture outside the coverage, get the fallback
   (`AuVerdict::Fallback`); an injected AU also carries its range's quality class.
5. **Mismatch.** Any mismatch rejects the manifest for the rest of the playback, also across
   resets, with a stable reason code: 1 unknown pts, 2 size, 3 CRC, 4 order, 5 laced, 6 past the
   end. How the player moves to its fallback inside a shot is its own policy (fork DESIGN 2.2).

## Not decided here

- How the player fetches the manifest and caches it (fork DV4.1).
- How the player hands over to its fallback in the middle of a shot. With B-frames this is a
  presentation-order window (fork DESIGN 2.2).
- HDR10 fallback RPUs in the player before its static 8.1 provider exists (fork DV3.3).
