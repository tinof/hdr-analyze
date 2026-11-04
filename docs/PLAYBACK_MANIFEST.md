# Playback manifest (P15)

Status: design written 2026-11-04. The AU table is implemented (P15.1); the manifest, the matcher
and `mkvdovi manifest` are not yet (P15.2 to P15.4). Steps: ROADMAP P15.

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
  video packets with the same pts.
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
  digest ‖ tail digest). A plain HTTP folder can serve `<lookup_key>.dvmanifest.json`. The player
  computes the key from one size lookup and two 1 MiB range reads.
- `track`: as in the AU table.
- `aus`: as in the AU table.
- `analysis`: `luminance_mapping` (`pq` or the HLG composer name), `downscale`, `sample_rate`,
  `gpu`, `no_crop`, `crop`, the peak settings and `scene`.
- `rpu`: everything `RpuParams` needs. That is the profile, the composer, the CM version, L9 and
  L11, the trim targets, L6, L5, `l1_avg_pq_cm_version` and the 8.1 source range. The values are
  written and read by one symmetric pair in `dovi_rpu_builder`.
- `l1_policy`: `"measured"`, or `"rpu-bin"` for P15.4.
- `shots`: parallel arrays `start[]` (presentation index in stream frames, starting at 0),
  `min[]`, `avg[]` and `max[]` (12-bit PQ, unclamped, exactly as mkvdovi's `extra.json` gets
  them).
- `coverage`: `[{start, end, class}]`, sorted, not overlapping, within `[0, stream pictures)`.
  - `full` means sample rate 1, downscale 1, the global scene detector at mkvdovi's defaults,
    mkvdovi's default peak and denoise settings, and a passed order check.
  - `approximate` is anything else.
  - Version 1 writes one range over the whole stream. The field exists for the fork's later
    server and on-device providers.

**Parity.** For every AU, the builder's `shot(l1).write(scene_refresh)` from the manifest equals
the RPU NAL that mkvdovi writes for the same file and settings. P15.3 tests this.

## Matching rule (`dovi_rpu_builder::manifest`)

The schema, the strict parser and the matcher live in `dovi_rpu_builder`. That crate is pure
Rust, builds for Android and is already linked by the fork, so the writer and the player share
one definition. The parser never panics. It refuses:

- an unknown format or version;
- array lengths that disagree;
- a pts that is not unique;
- shots that are unsorted or do not start at 0;
- bad coverage;
- a composer whose luminance mapping differs from `analysis.luminance_mapping`.

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
     carry the player's fallback RPU (Media3 normally drops them). The first IRAP is validated in
     full, and continuity applies from there on.
4. **Shot.** The presentation index p is the pts rank. The shot is the last one with
   `start ≤ p`, and `scene_refresh` is set only when `p == start` (mkvdovi parity).
5. **Mismatch.** Any mismatch rejects the manifest for the rest of the playback, with a reason
   code. How the player moves to its fallback inside a shot is its own policy (fork DESIGN 2.2).

## Not decided here

- How the player fetches the manifest and caches it (fork DV4.1).
- How the player hands over to its fallback in the middle of a shot. With B-frames this is a
  presentation-order window (fork DESIGN 2.2).
- HDR10 fallback RPUs in the player before its static 8.1 provider exists (fork DV3.3).
