# FFmpeg 9 Dolby Vision features vs. dovi_tool

## 1. Status

- 2026-10-13: evaluation only, no code change. Question: can the Dolby Vision bitstream filters in
  FFmpeg 9 (`dovi_split`, new in 9.0; `dovi_rpu`, older) replace any `dovi_tool` step in `mkvdovi`,
  and would that be faster?
- Answer: FFmpeg 9 can replace only `dovi_tool remove`, and on the current main path it is slower
  and changes the output bytes. `convert --discard`, `generate`, `inject-rpu` and `extract-rpu` have
  no FFmpeg equivalent. `dovi_tool` stays a hard dependency; nothing is adopted.
- Re-check when: FFmpeg adds RPU profile conversion or RPU injection, `dovi_tool` drops its direct
  MKV input, or the analyzer's system FFmpeg reaches 9 (section 6).

Evidence: FFmpeg source at tags `n8.0`, `n8.1`, `n9.0` and `master` (GitHub mirror), local runs with
the FFmpeg 9.0.2 CLI (static aarch64 build, libavcodec 63.1.102) and `dovi_tool` 2.3.4 on the Oracle
ARM host, and an independent re-check of the hashes, NAL counts, source lines and timing log.

## 2. Step by step

| `mkvdovi` step | FFmpeg 9 candidate | Verdict |
|---|---|---|
| `dovi_tool remove` (clean base layer, `--mdfix`) | `-bsf:v dovi_split=mode=bl` | Same pixels, but slower than `dovi_tool`'s direct MKV read and different bytes (section 4) |
| `dovi_tool -m 2 convert --discard` (P7 MEL → 8.1) | none | `dovi_split=mode=bl_rpu` keeps the RPU byte for byte (still profile 7); `dovi_rpu` cannot change the profile |
| `dovi_tool generate` (RPU from the analyzer's L1) | none | FFmpeg writes RPUs only inside encoders (libx265, libsvtav1); `mkvdovi` never encodes |
| `dovi_tool inject-rpu` | none | No filter, muxer option or CLI flag puts an external RPU into a copied stream |
| `dovi_tool extract-rpu` (`rpu_check.rs`, `verify.rs`) | `filter_units=pass_types=62 -f hevc` | Not a drop-in: keeps the 2-byte `7C01` NAL header and writes RPUs in decode order (4 positions swapped on a clip with B-frames) |
| `hdr10plus_tool extract` | none found | ffprobe prints HDR10+ side data in its own layout, not the JSON `dovi_tool generate` reads (not run) |

## 3. What the filters do (source, `n9.0`)

`libavcodec/bsf/dovi_split.c` (new in 9.0; absent at `n8.0`; byte-identical at `n9.0` and `master`):

- Selects NAL units by type only, never by `nuh_layer_id`. Type 63 is the enhancement layer (EL); its
  2-byte outer header is stripped when the EL is kept. Type 62 is the RPU, copied unchanged in the
  `*_rpu` modes and never parsed. Every other NAL is base layer (BL).
- Modes: `bl`, `bl_rpu`, `el`, `el_rpu`.
- Output `AV_PKT_DATA_DOVI_CONF`: only the `bl_present_flag`, `el_present_flag` and
  `rpu_present_flag` bits are cleared; `dv_profile` is kept. Without a DV record it assumes profile 7.

`libavcodec/bsf/dovi_rpu.c`:

- `strip`: removes the DV record and, per access unit, deletes the **last** NAL if it is type 62 or
  63. An RPU followed by EOS is therefore missed, and on Profile 7 only one of RPU/EL is removed.
- `compression`: re-encodes each RPU (`ff_dovi_rpu_parse` → `ff_dovi_rpu_generate`); refused below
  profile 8. The encoder copies the input record and writes back the parsed profile, residual flag
  and NLQ, so there is no P7 → 8.1 conversion.

HEVC decoder (`hevcdec.c`): NAL 62/63 are skipped for decoding, so FFmpeg never composes an EL (MEL
or FEL; nothing towards ETSI GS CCM 001). The RPU is parsed and attached as
`AV_FRAME_DATA_DOVI_RPU_BUFFER` and `AV_FRAME_DATA_DOVI_METADATA`. In that side data
`disable_residual_flag` is 0 for both MEL and FEL, but `nlq_offset` differs (0 vs 512 on the
fixture RPUs). `mkvdovi` classifies with the `dolby_vision` crate (`rpu_check.rs`) and gains nothing
from this.

Changelog: 9.0 has "Bitstream filter to split Dolby Vision multi-layer HEVC" and "SMPTE 2094-50
metadata support and passthrough"; 8.0 and 8.1 have no DV, RPU or DV-record entries.

## 4. Measurements

Media: a synthetic 48-frame 1080p P8.1 clip (the `metadata/tests.rs` recipe); synthetic P7 MEL and
FEL clips built from the same base layer, a 960x540 EL and the `dovi_tool` fixture RPUs
(`assets/tests/{mel,fel}_rpu.bin`) with `dovi_tool mux`; the `dovi_tool` fixtures `regular.mkv`
(259 frames, P8, B-frames, 2 EOS NALs) and `regular_start_code_4_muxed_el.hevc`; FFmpeg FATE samples
`mkv/dovi-p7-hvce.mkv` (one real 4K P7 FEL frame) and `mov/dovi-p81.mp4`.

**Base layer, bytes.** The outputs fall into two groups with identical slice, SEI and RPU payloads:

| Group | Producers | NALs (MEL clip) |
|---|---|---|
| A | ffmpeg `hevc_mp4toannexb` extract + `dovi_tool remove` (the fallback); `dovi_tool demux` BL; `dovi_split=mode=bl` read from the MKV | 116, byte-identical |
| B | `dovi_tool remove` reading the MKV directly (the main path with `dovi_tool` ≥ 2.3.4) | 108 |

Group A repeats the container's VPS/SPS/PPS (+SEI) at every IRAP. `inject-rpu` keeps those repeats,
so switching to `dovi_split` would change the final MKV bytes (not the pixels, not the RPU). The
real FATE frame shows the same split (16 vs 11 NALs). Other framing-only variants: `dovi_split` on a
raw `.hevc` input adds one trailing zero byte per NAL; `filter_units=remove_types=62|63` writes some
3-byte start codes.

**Base layer, pixels.** `framemd5` was identical across every producer of each input.

**Defects found:**

- `dovi_rpu=strip=1` left 2 of 259 RPUs on `regular.mkv`: each is an RPU followed by EOS in the same
  access unit. FFmpeg's decoder comment calls the RPU "normally the last non-EOB/EOS NAL", so this
  layout is expected in real files. `filter_units` and `dovi_split` removed all RPUs.
- `dovi_rpu=strip=1` and `filter_units=remove_types=62` keep the Profile 7 EL (all type-63 NALs).

**DV record in the container.** One ffmpeg pass `-map 0 -c copy -bsf:v X out.mkv` vs raw `.hevc` +
mkvmerge:

| Input + filter | One-pass ffmpeg MKV | Raw + mkvmerge |
|---|---|---|
| P8, `dovi_rpu=strip=1` | no DV record (HDR10) | HDR10 |
| P8, `filter_units=remove_types=62` | stale record: profile 8, RPU present, MediaInfo "dvhe.08 BL+RPU" | HDR10 |
| P8, `dovi_split=mode=bl` | record kept with only BL set, MediaInfo "dvhe.08 BL" | HDR10 |
| P7, `dovi_split=mode=bl` | profile 7, BL only, MediaInfo "dvhe.07 BL" | HDR10 |
| P7, `dovi_split=mode=bl_rpu` | profile 7, RPU present, EL absent | mkvmerge writes profile 7 **BL+EL+RPU** (it trusts the profile-7 RPU) |
| P7, `filter_units=remove_types=62\|63` | stale dvcC + hvcE claiming BL+EL+RPU | HDR10 |

**Timing.** 320 MB 4K P8.1 clip (960 frames, ~64 Mb/s), median of 3 runs after a warm-up read:

| Method | Wall | Throughput | Peak RSS |
|---|---|---|---|
| `dovi_tool remove -i in.mkv` (main path) | 0.22 s | ~1456 MB/s | 4.9 MB |
| ffmpeg `dovi_split=mode=bl` → `.hevc` | 0.28 s | ~1144 MB/s | ~58 MB |
| ffmpeg `filter_units=remove_types=62` → `.hevc` | 0.83 s | ~386 MB/s | ~60 MB |
| ffmpeg `dovi_rpu=strip=1` → `.hevc` | 0.88 s | ~364 MB/s | ~61 MB |
| ffmpeg one-pass `dovi_split=mode=bl` → `.mkv` | 0.36 s | ~890 MB/s | ~61 MB |
| Fallback: extract (0.23 s) + `remove` on raw (0.43 s) | 0.66 s | ~485 MB/s | 52 / 7 MB |

These ran on tmpfs on a host that was swapping, so they bound CPU cost only. On the `/srv/data`
block volume (~131 MB/s) every method is disk-bound. ffmpeg beats only the fallback, which
`mkvdovi` uses with `dovi_tool` older than 2.3.4 or when the direct MKV read fails, and that path
also writes and reads a second full-size file.

**Decoder.** The FFmpeg 9 CLI decodes a P7 MEL or FEL stream as the base layer only; `framemd5`
equals the bare BL. `-view_ids -1` still yields one view.

## 5. Limits

- All multi-frame Profile 7 results come from `dovi_tool mux` builds; the only real P7 material was
  one FATE frame. The real-content corpus (on the CUDA host) was not used.
- No audio or subtitle tracks in the mux tests, so replacing mkvmerge with a one-pass ffmpeg mux was
  not evaluated.
- Whether real discs put EOS after the RPU (the `dovi_rpu=strip` miss) is untested.
- Not run: the two-track P7 FATE samples (`dovi-p7.mp4`, `dovi-p7.ts`), timing on a real disk or a
  multi-minute clip, the HDR10+ ffprobe format.
- Open lead, untested: whether `dovi_tool inject-rpu` reads an MKV directly like `remove` and
  `convert`. Its 2.3.4 `--help` speaks only of an "input HEVC file". If it does, the HDR10 path
  could skip the full-size raw extraction; that is a `dovi_tool` question, not an FFmpeg one.

## 6. If this is revisited

- The analyzer links the system FFmpeg via `ffmpeg-next` (8.0.1 on the Oracle host: libavcodec
  62.11, which has `dovi_rpu` but not `dovi_split`). `mkvdovi` runs whatever `ffmpeg` is on PATH.
  FFmpeg ↔ libavcodec: 8.0 = 62.11, 9.0 = 63.1.
- Adopting `dovi_split` would need a capability probe (`ffmpeg -h bsf=dovi_split`) with per-step
  fallback, like `DOVI_TOOL_MKV_INPUT_MIN`; a `resume_settings` entry because the artifact bytes
  change; and a `scripts/rpu-baseline.sh compare` run.
- Never use `dovi_rpu=strip` (misses RPUs before EOS, keeps the P7 EL) or a one-pass ffmpeg MKV mux
  of a filtered DV stream (stale DV record).

## 7. Playback note: Profile 7 on a single-layer player

Checked on the web on 2026-10-13 (NVIDIA forums, Plex statements, Reddit and AVS Forum reports) and
consistent with the owner's Shield Pro 2019 + LG C9: the Nvidia Shield plays a dual-layer Profile 7 MKV as
Dolby Vision, using BL + RPU and ignoring the EL. NVIDIA calls Profile 7 "not officially supported";
Plex has attributed occasional drops to HDR10 to the Nvidia Shield's DV decoder crashing. Some apps and
devices do fall back to HDR10 for Profile 7, and Kodi 21 on Android can convert 7 → 8.1 on the fly
(libdovi).

What that means for `mkvdovi`:

- MEL on such a player already gives the BL + RPU picture. The 8.1 conversion buys an officially
  supported profile (avoiding the reported Profile 7 dropouts), compatibility with players that refuse Profile 7, and a
  slightly smaller file, not a different picture.
- FEL on such a player also shows BL + RPU; the EL residual is lost either way.
- `convert -m 2` "removes mapping" (`dovi_tool` 2.3.4 `--help`): the 8.1 RPU carries no-op
  luma/chroma reshaping curves, while a player handling Profile 7 natively applies the original
  curves. How far the original MEL curves are from identity was not measured here. `-m 5` keeps the
  mapping (dovi_tool's old mode 2).
- `dovi_split=mode=bl_rpu` would likely still play as DV on the Nvidia Shield, but its DV record (P7 with
  EL, after mkvmerge) misdescribes the stream for every other player, which is why the RPU rewrite
  in `convert` matters.
