//! Abort-safe parsing of source `RPU.bin` files (E15.1).
//!
//! `dolby_vision` 3.4's parser calls `unimplemented!` on a polynomial reshaping piece with
//! `poly_order_minus1 == 0` and `linear_interp_flag` set (`rpu_data_mapping.rs:415`), and it
//! allocates a curve's pivots and pieces before it reads them. Release builds use
//! `panic = "abort"`, so either one ends the whole mkvdovi run, including the files queued after
//! the one that carries it. [`parse_source_rpu_file`] first walks every RPU's header and data
//! mapping with the same bit reads as the crate and returns an error at such a piece or at a
//! pivot count larger than the RPU, before `parse_rpu_file` sees the file.
//!
//! The walk stops without an error where the crate itself returns one (a read past the end, a
//! value it refuses), so it never rejects a file the crate parses. Re-check it against the
//! crate's `RpuDataHeader::parse` and `RpuDataMapping::parse` whenever `dolby_vision` is bumped;
//! the test `the_crate_still_panics_on_a_linear_interpolation_piece` fails once the crate can
//! read the piece.
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

/// `parse_rpu_file`, refused with an error instead of an abort when an RPU in the file carries
/// a piece the crate cannot parse.
pub fn parse_source_rpu_file(path: &Path) -> Result<Vec<DoviRpu>> {
    let bytes = fs::read(path).with_context(|| format!("reading RPU file {}", path.display()))?;
    check_parseable(&bytes).with_context(|| format!("RPU file {}", path.display()))?;
    parse_rpu_file(path).context("Failed to parse Dolby Vision RPU")
}

/// An error naming the first RPU (0-based, in file order) that would abort the crate's parser.
pub fn check_parseable(bytes: &[u8]) -> Result<()> {
    for (index, nal) in split_nals(bytes).enumerate() {
        // The crate refuses these bytes with an error before it reads any field.
        let Ok(trimmed) = DoviRpu::validated_trimmed_data(nal) else {
            continue;
        };
        let payload = clear_start_code_emulation_prevention_3_byte(trimmed);
        if let Some(unreadable) = scan(&mut BitReader::new(&payload)).flatten() {
            anyhow::bail!("RPU {index}: {unreadable}");
        }
    }
    Ok(())
}

/// The NAL units of an `RPU.bin`, each starting at its start code, as `parse_rpu_file` cuts
/// them (it drops one more trailing byte before a following start code; the walk never reads
/// that far).
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
        }
    }
}

/// The header fields the data mapping depends on.
struct Header {
    coefficient_data_type: u8,
    coefficient_log2_denom_length: u32,
    bl_bit_depth: u32,
    profile7_nlq: bool,
}

/// Walk one RPU payload (prefix byte first, emulation prevention removed). `None`: the crate
/// stops with an error first; `Some(None)`: nothing that aborts it.
fn scan(r: &mut BitReader<'_>) -> Option<Option<Unreadable>> {
    if r.bits(8)? != 25 {
        return None;
    }
    match read_header(r)? {
        Some(header) => scan_mapping(r, &header),
        // `use_prev_vdr_rpu_flag`: no data mapping follows.
        None => Some(None),
    }
}

/// `RpuDataHeader::parse`. `Some(None)` when the RPU reuses the previous mapping.
fn read_header(r: &mut BitReader<'_>) -> Option<Option<Header>> {
    if r.bits(6)? != 2 {
        return None;
    }
    let rpu_format = r.bits(11)?;
    r.bits(4 + 4)?; // vdr_rpu_profile, vdr_rpu_level
    let mut coefficient_data_type = 0;
    let mut coefficient_log2_denom = 0;
    let mut coefficient_log2_denom_length = 0;
    let mut bl_bit_depth_minus8 = 0u64;
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
            r.ue()?; // el_bit_depth_minus8 with ext_mapping_idc
            r.ue()?; // vdr_bit_depth_minus8
            r.bits(1 + 3 + 1)?; // spatial filter, reserved_zero_3bits, EL spatial filter
            disable_residual_flag = r.bit()?;
        }
        coefficient_log2_denom_length = match coefficient_data_type {
            0 => coefficient_log2_denom as u32,
            1 => 32,
            _ => return None,
        };
    }
    r.bit()?; // vdr_dm_metadata_present_flag
    if r.bit()? {
        r.ue()?; // prev_vdr_rpu_id
        return Some(None);
    }
    Some(Some(Header {
        coefficient_data_type,
        coefficient_log2_denom_length,
        bl_bit_depth: bl_bit_depth_minus8.wrapping_add(8) as u32,
        profile7_nlq: rpu_format & 0x700 == 0 && !disable_residual_flag,
    }))
}

/// `RpuDataMapping::parse` up to and through every curve piece.
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
    Some(None)
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
