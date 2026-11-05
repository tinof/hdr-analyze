//! Abort-safe parsing of source `RPU.bin` files (E15.1, E15.2).
//!
//! `dolby_vision` 3.4's parser calls `unimplemented!` on a polynomial reshaping piece with
//! `poly_order_minus1 == 0` and `linear_interp_flag` set (`rpu_data_mapping.rs:415`) and
//! `unreachable!` on a CM v4.0 level 8, 9 or 10 block of a length it does not know
//! (`required_bits` in `blocks/level8.rs:181`, `level9.rs:154`, `level10.rs:135`), and it
//! allocates a curve's pivots and pieces, and the extension blocks of the CM v2.9 and v4.0 DM
//! metadata (`extension_metadata/mod.rs:114`), before it reads them. Release builds use
//! `panic = "abort"`, so either one ends the whole mkvdovi run, including the files queued after
//! the one that carries it. [`parse_source_rpu_file`] first walks every RPU's header, data
//! mapping, Profile 7 NLQ data and DM payload with the same bit reads as the crate and returns an
//! error at such a piece or block, or at a pivot or extension-block count larger than the RPU,
//! before `parse_rpu_file` sees the file.
//!
//! The walk stops without an error where the crate itself returns one (a read past the end, a
//! value it refuses), so it never rejects a file the crate parses. It steps over an extension
//! block by its declared length; a fixed-length level the crate refuses with an error when the
//! length differs, so for such a block the walk may go on and refuse a later count or block, but
//! only in a file the crate refuses too. Re-check it against the crate's `RpuDataHeader::parse`, `RpuDataMapping::parse`,
//! `RpuDataNlq::parse` and `vdr_dm_data_payload` whenever `dolby_vision` is bumped; the tests
//! `the_crate_still_panics_on_a_linear_interpolation_piece`,
//! `the_crate_still_panics_on_an_extension_block_count` and
//! `the_crate_still_panics_on_a_block_length` fail once the crate stops aborting.
//!
//! Use this for every RPU that mkvdovi did not generate itself (extracted from a source or
//! imported). RPUs that `dovi_tool generate` or `dovi_rpu_builder` wrote never carry the piece.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use dolby_vision::rpu::dovi_rpu::DoviRpu;
use dolby_vision::rpu::utils::parse_rpu_file;
use dolby_vision::utils::clear_start_code_emulation_prevention_3_byte;

const START_CODE: [u8; 4] = [0, 0, 0, 1];
const COMPONENTS: [&str; 3] = ["Y", "Cb", "Cr"];
/// `MMR_MAX_COEFFS` in the crate (`pub(crate)`).
const MMR_COEFFS: usize = 7;
/// The `0x80` that ends every RPU (`FINAL_BYTE` in the crate).
const FINAL_BYTE: u8 = 0x80;
/// The bits of `VdrDmData::parse` after its three `ue(v)` ids: nine 16-bit YCbCr-to-RGB
/// coefficients, three 32-bit offsets, nine 16-bit RGB-to-LMS coefficients, the signal EOTF and
/// its three parameters (16, 16, 16, 32), bit depth, colour space, chroma format and range
/// (5, 2, 2, 2), and the source min, max and diagonal (12, 12, 10).
const DM_FIXED_BITS: u64 = 9 * 16 + 3 * 32 + 9 * 16 + (16 * 3 + 32) + (5 + 2 + 2 + 2) + 34;
/// `DM_DATA_PAYLOAD2_MIN_BITS`: the crate reads the CM v4.0 metadata only when this many bits
/// are left after the CM v2.9 blocks.
const CMV40_MIN_BITS: usize = 56;

/// `parse_rpu_file`, refused with an error instead of an abort when an RPU in the file carries
/// a piece the crate cannot parse.
pub fn parse_source_rpu_file(path: &Path) -> Result<Vec<DoviRpu>> {
    let bytes = fs::read(path).with_context(|| format!("reading RPU file {}", path.display()))?;
    check_parseable(&bytes).with_context(|| format!("RPU file {}", path.display()))?;
    parse_rpu_file(path).context("Failed to parse Dolby Vision RPU")
}

/// The guard's refusal: RPU `index` (0-based, in file order) would abort the crate's parser.
/// A caller tells it apart from other parse failures with [`is_unparseable`]: the file does
/// carry Dolby Vision RPUs, so it must not be handled as if it had none.
#[derive(Debug)]
pub struct UnparseableRpu {
    index: usize,
    unreadable: Unreadable,
}

impl std::fmt::Display for UnparseableRpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RPU {}: {}", self.index, self.unreadable)
    }
}

impl std::error::Error for UnparseableRpu {}

/// Whether `error` (or one of its causes) is the guard's [`UnparseableRpu`].
pub fn is_unparseable(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<UnparseableRpu>().is_some())
}

/// An [`UnparseableRpu`] error for the first RPU that would abort the crate's parser.
pub fn check_parseable(bytes: &[u8]) -> Result<()> {
    for (index, nal) in split_nals(bytes).enumerate() {
        // The crate refuses these bytes with an error before it reads any field.
        let Ok(trimmed) = DoviRpu::validated_trimmed_data(nal) else {
            continue;
        };
        let payload = clear_start_code_emulation_prevention_3_byte(trimmed);
        // `DoviRpu::parse` reads up to the last non-zero byte and refuses an RPU shorter than
        // seven bytes or one that does not end in 0x80 before it reads any field.
        let end = payload
            .iter()
            .rposition(|&byte| byte != 0)
            .map_or(0, |i| i + 1);
        if end < 7 || payload[end - 1] != FINAL_BYTE {
            continue;
        }
        if let Some(unreadable) = scan(&mut BitReader::new(&payload[..end])).flatten() {
            return Err(UnparseableRpu { index, unreadable }.into());
        }
    }
    Ok(())
}

/// The NAL units of an `RPU.bin`, each starting at its start code, as `parse_rpu_file` cuts
/// them (it also drops a zero byte before a following start code; the walk trims trailing
/// zeros as the crate does).
fn split_nals(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let starts: Vec<usize> = bytes
        .windows(4)
        .enumerate()
        .filter_map(|(i, window)| (window == START_CODE).then_some(i))
        .collect();
    let ends: Vec<usize> = starts
        .iter()
        .skip(1)
        .copied()
        .chain([bytes.len()])
        .collect();
    starts
        .into_iter()
        .zip(ends)
        .map(move |(start, end)| &bytes[start..end])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unreadable {
    /// `unimplemented!` in `DoviPolynomialCurve::parse`.
    LinearInterpolation { component: usize, piece: u64 },
    /// More pivots or pieces than the RPU has bits: the crate allocates them before reading.
    PivotCount { component: usize, pivots: u128 },
    /// More extension blocks than the RPU can hold: the crate allocates them before reading.
    ExtBlockCount { dm_version: DmVersion, count: u64 },
    /// A CM v4.0 level 8, 9 or 10 block of a length the crate has no field layout for.
    BlockLength { level: u64, length: u64 },
}

/// The DM metadata versions in the order the crate reads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DmVersion {
    V29,
    V40,
}

impl DmVersion {
    /// `ALLOWED_BLOCK_LEVELS` of `CmV29DmData` and `CmV40DmData`: any other level is an error.
    fn allows(self, level: u64) -> bool {
        match self {
            Self::V29 => matches!(level, 1 | 2 | 4 | 5 | 6 | 255),
            Self::V40 => matches!(level, 3 | 8 | 9 | 10 | 11 | 254),
        }
    }
}

impl std::fmt::Display for DmVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::V29 => "CM v2.9",
            Self::V40 => "CM v4.0",
        })
    }
}

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::LinearInterpolation { component, piece } => write!(
                f,
                "piece {piece} of the {} reshaping curve uses linear interpolation, which the \
                 dolby_vision parser cannot read (it would abort mkvdovi)",
                COMPONENTS[component]
            ),
            Self::PivotCount { component, pivots } => write!(
                f,
                "the {} reshaping curve declares {pivots} pivots, more than the RPU holds (the \
                 dolby_vision parser would abort mkvdovi allocating them)",
                COMPONENTS[component]
            ),
            Self::ExtBlockCount { dm_version, count } => write!(
                f,
                "the {dm_version} DM metadata declares {count} extension blocks, more than the \
                 RPU holds (the dolby_vision parser would abort mkvdovi allocating them)"
            ),
            Self::BlockLength { level, length } => write!(
                f,
                "a level {level} DM metadata block declares {length} bytes, a length the \
                 dolby_vision parser cannot read (it would abort mkvdovi)"
            ),
        }
    }
}

/// The header fields the data mapping and the DM payload depend on.
struct Header {
    coefficient_data_type: u8,
    coefficient_log2_denom_length: u32,
    bl_bit_depth: u32,
    el_bit_depth: u32,
    profile7_nlq: bool,
    /// `reserved_zero_3bits == 1`: the DM payload carries only its three ids.
    compressed_dm: bool,
    dm_present: bool,
    /// `use_prev_vdr_rpu_flag`: no data mapping follows, the DM payload still may.
    reuse_previous: bool,
}

/// Walk one RPU payload (prefix byte first, emulation prevention removed, up to the final
/// byte). `None`: the crate stops with an error first; `Some(None)`: nothing that aborts it.
fn scan(r: &mut BitReader<'_>) -> Option<Option<Unreadable>> {
    if r.bits(8)? != 25 {
        return None;
    }
    let header = read_header(r)?;
    if !header.reuse_previous {
        if let Some(unreadable) = scan_mapping(r, &header)? {
            return Some(Some(unreadable));
        }
    }
    if header.dm_present {
        return scan_dm(r, &header);
    }
    Some(None)
}

/// `RpuDataHeader::parse`.
fn read_header(r: &mut BitReader<'_>) -> Option<Header> {
    if r.bits(6)? != 2 {
        return None;
    }
    let rpu_format = r.bits(11)?;
    r.bits(4 + 4)?; // vdr_rpu_profile, vdr_rpu_level
    let mut coefficient_data_type = 0;
    let mut coefficient_log2_denom = 0;
    let mut coefficient_log2_denom_length = 0;
    let mut bl_bit_depth_minus8 = 0u64;
    let mut el_bit_depth_minus8 = 0u64;
    let mut reserved_zero_3bits = 0;
    let mut disable_residual_flag = false;
    if r.bit()? {
        r.bit()?; // chroma_resampling_explicit_filter_flag
        coefficient_data_type = r.bits(2)? as u8;
        if coefficient_data_type == 0 {
            coefficient_log2_denom = r.ue()?;
        }
        r.bits(2 + 1)?; // vdr_rpu_normalized_idc, bl_video_full_range_flag
        if rpu_format & 0x700 == 0 {
            bl_bit_depth_minus8 = r.ue()?;
            // ext_mapping_idc in the bits above the lowest 8
            el_bit_depth_minus8 = r.ue()? & 0xFF;
            r.ue()?; // vdr_bit_depth_minus8
            r.bit()?; // spatial_resampling_filter_flag
            reserved_zero_3bits = r.bits(3)?;
            r.bit()?; // el_spatial_resampling_filter_flag
            disable_residual_flag = r.bit()?;
        }
        coefficient_log2_denom_length = match coefficient_data_type {
            0 => coefficient_log2_denom as u32,
            1 => 32,
            _ => return None,
        };
    }
    let dm_present = r.bit()?;
    let reuse_previous = r.bit()?;
    if reuse_previous {
        r.ue()?; // prev_vdr_rpu_id
    }
    Some(Header {
        coefficient_data_type,
        coefficient_log2_denom_length,
        bl_bit_depth: bl_bit_depth_minus8.wrapping_add(8) as u32,
        el_bit_depth: el_bit_depth_minus8 as u32 + 8,
        profile7_nlq: rpu_format & 0x700 == 0 && !disable_residual_flag,
        compressed_dm: reserved_zero_3bits == 1,
        dm_present,
        reuse_previous,
    })
}

/// `RpuDataMapping::parse`, its curve pieces and its Profile 7 NLQ data.
fn scan_mapping(r: &mut BitReader<'_>, header: &Header) -> Option<Option<Unreadable>> {
    r.ue()?; // vdr_rpu_id
    r.ue()?; // mapping_color_space
    r.ue()?; // mapping_chroma_format_idc
    let mut num_pieces = [0u64; 3];
    for (component, pieces) in num_pieces.iter_mut().enumerate() {
        let num_pivots_minus2 = r.ue()?;
        let pivots = num_pivots_minus2.wrapping_add(2);
        *pieces = num_pivots_minus2.wrapping_add(1);
        // The crate wraps the count in release builds; `pieces` is the larger one at the wrap.
        if pivots.max(*pieces) > r.remaining() as u64 {
            let pivots = u128::from(num_pivots_minus2) + 2;
            return Some(Some(Unreadable::PivotCount { component, pivots }));
        }
        for _ in 0..pivots {
            r.var(header.bl_bit_depth, 16)?;
        }
    }
    if header.profile7_nlq {
        if r.bits(3)? != 0 {
            return None;
        }
        r.var(header.bl_bit_depth, 16)?;
        r.var(header.bl_bit_depth, 16)?;
    }
    r.ue()?; // num_x_partitions_minus1
    r.ue()?; // num_y_partitions_minus1
    for (component, &pieces) in num_pieces.iter().enumerate() {
        for piece in 0..pieces {
            let found = match r.ue()? {
                0 => scan_polynomial(r, header)?,
                1 => {
                    scan_mmr(r, header)?;
                    false
                }
                _ => return None,
            };
            if found {
                return Some(Some(Unreadable::LinearInterpolation { component, piece }));
            }
        }
    }
    if header.profile7_nlq {
        scan_nlq(r, header)?;
    }
    Some(None)
}

/// `RpuDataNlq::parse`, with the linear dead-zone method (`nlq_method_idc` 0, the only one the
/// crate accepts).
fn scan_nlq(r: &mut BitReader<'_>, header: &Header) -> Option<()> {
    for _ in COMPONENTS {
        r.var(header.el_bit_depth, 16)?; // nlq_offset
        coefficient(r, header)?; // vdr_in_max
        coefficient(r, header)?; // linear_deadzone_slope
        coefficient(r, header)?; // linear_deadzone_threshold
    }
    Some(())
}

/// `vdr_dm_data_payload`: the DM ids and fields, the CM v2.9 blocks, then the CM v4.0 blocks
/// when enough bits are left.
fn scan_dm(r: &mut BitReader<'_>, header: &Header) -> Option<Option<Unreadable>> {
    r.ue()?; // affected_dm_metadata_id
    r.ue()?; // current_dm_metadata_id
    r.ue()?; // scene_refresh_flag
    if !header.compressed_dm {
        r.skip(DM_FIXED_BITS)?;
    }
    if let Some(unreadable) = scan_ext_blocks(r, DmVersion::V29)? {
        return Some(Some(unreadable));
    }
    if r.remaining() >= CMV40_MIN_BITS {
        return scan_ext_blocks(r, DmVersion::V40);
    }
    Some(None)
}

/// `DmData::parse`: the block count, the alignment, then each block as its declared length.
fn scan_ext_blocks(r: &mut BitReader<'_>, dm_version: DmVersion) -> Option<Option<Unreadable>> {
    let count = r.ue()?;
    // Each block takes at least nine bits (its length and level), so the crate cannot read
    // more blocks than a ninth of the bits left.
    if count > r.remaining() as u64 / 9 {
        return Some(Some(Unreadable::ExtBlockCount { dm_version, count }));
    }
    while r.pos % 8 != 0 {
        if r.bit()? {
            return None; // dm_alignment_zero_bit
        }
    }
    for _ in 0..count {
        let length = r.ue()?;
        let level = r.bits(8)?;
        if !dm_version.allows(level) {
            return None;
        }
        if let Some(bits) = unknown_length_reads(level, length) {
            // The crate reads these fields before it reaches `required_bits`; a read past
            // the end stops it with an error first.
            return (bits <= r.remaining() as u64)
                .then_some(Some(Unreadable::BlockLength { level, length }));
        }
        // The level's fields and its ext_dm_alignment_zero_bits fill exactly `length` bytes
        // in every block the crate accepts.
        r.skip(length.checked_mul(8)?)?;
    }
    Some(None)
}

/// Levels 8, 9 and 10 take their length from the block: `parse` reads the fields the length
/// gates, then `required_bits` calls `unreachable!` for a length outside the crate's set. The
/// bits that `parse` reads for such a length; `None` for a length the crate knows, or any other
/// level (a fixed-length level refuses a different length with an error).
fn unknown_length_reads(level: u64, length: u64) -> Option<u64> {
    let over = |limit: u64, bits: u64| if length > limit { bits } else { 0 };
    let (known, bits): (&[u64], u64) = match level {
        // Index (8) and six 12-bit trims; mid contrast, clip trim, then six 8-bit saturation
        // and six 8-bit hue fields.
        8 => (
            &[10, 12, 13, 19, 25],
            80 + over(10, 12) + over(12, 12) + over(13, 48) + over(19, 48),
        ),
        // Primary index; then eight 16-bit primaries.
        9 => (&[1, 17], 8 + over(1, 128)),
        // Index, max and min PQ, primary index (8, 12, 12, 8); then eight 16-bit primaries.
        10 => (&[5, 21], 40 + over(5, 128)),
        _ => return None,
    };
    (!known.contains(&length)).then_some(bits)
}

/// `DoviPolynomialCurve::parse`; `Some(true)` at the piece it cannot parse.
fn scan_polynomial(r: &mut BitReader<'_>, header: &Header) -> Option<bool> {
    let poly_order_minus1 = r.ue()?;
    if poly_order_minus1 > 1 {
        return None;
    }
    if poly_order_minus1 == 0 && r.bit()? {
        return Some(true);
    }
    for _ in 0..poly_order_minus1 + 2 {
        coefficient(r, header)?;
    }
    Some(false)
}

/// `DoviMMRCurve::parse`.
fn scan_mmr(r: &mut BitReader<'_>, header: &Header) -> Option<()> {
    let mmr_order_minus1 = r.bits(2)?;
    if mmr_order_minus1 > 2 {
        return None;
    }
    coefficient(r, header)?; // mmr_constant
    for _ in 0..(mmr_order_minus1 + 1) as usize * MMR_COEFFS {
        coefficient(r, header)?;
    }
    Some(())
}

/// An integer part (`se`, only for fixed-point data) and a fraction of the denominator's length.
fn coefficient(r: &mut BitReader<'_>, header: &Header) -> Option<()> {
    if header.coefficient_data_type == 0 {
        r.ue()?; // se(v) costs the same bits as ue(v)
    }
    r.var(header.coefficient_log2_denom_length, 64)
}

/// MSB-first reads with the bounds of `bitvec_helpers`' `BsIoSliceReader`.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    fn bit(&mut self) -> Option<bool> {
        let byte = *self.data.get(self.pos / 8)?;
        let bit = (byte >> (7 - self.pos % 8)) & 1 == 1;
        self.pos += 1;
        Some(bit)
    }

    /// Step over `n` bits; `None` when fewer are left.
    fn skip(&mut self, n: u64) -> Option<()> {
        if n > self.remaining() as u64 {
            return None;
        }
        self.pos += n as usize;
        Some(())
    }

    fn bits(&mut self, n: u32) -> Option<u64> {
        if n > 64 || n as usize > self.remaining() {
            return None;
        }
        let mut value = 0u64;
        for _ in 0..n {
            value = (value << 1) | u64::from(self.bit()?);
        }
        Some(value)
    }

    /// `read_var::<T>(n)` for a `T` of `width` bits: the crate's read fails when `n > width`.
    fn var(&mut self, n: u32, width: u32) -> Option<()> {
        if n > width {
            return None;
        }
        self.bits(n).map(drop)
    }

    /// Exp-Golomb `ue(v)`, wrapping as the crate's release build does.
    fn ue(&mut self) -> Option<u64> {
        let mut leading_zeroes = 0u32;
        while !self.bit()? {
            leading_zeroes += 1;
        }
        if leading_zeroes == 0 {
            return Some(0);
        }
        let value = self.bits(leading_zeroes)?;
        Some(
            value
                .wrapping_add(1u64.wrapping_shl(leading_zeroes))
                .wrapping_sub(1),
        )
    }
}

#[cfg(test)]
pub(crate) mod fixtures;

#[cfg(test)]
mod tests;
