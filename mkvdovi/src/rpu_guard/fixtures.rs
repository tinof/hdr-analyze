//! RPUs written bit by bit, for the cases the crate's own writer cannot produce (it panics on a
//! linear-interpolation piece too). Field order follows the Dolby Vision RPU syntax as
//! `dolby_vision` 3.4 reads it: `RpuDataHeader::parse`, then `RpuDataMapping::parse`.

/// One reshaping-curve piece.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Piece {
    Polynomial {
        order_minus1: u64,
        linear_interp: bool,
    },
    Mmr {
        order_minus1: u64,
    },
}

/// A neutral first-order polynomial piece, as `dovi_tool generate` writes the luma curve.
pub(crate) const POLY: Piece = Piece::Polynomial {
    order_minus1: 0,
    linear_interp: false,
};
/// The piece `dolby_vision` 3.4 cannot parse.
pub(crate) const LINEAR_INTERP: Piece = Piece::Polynomial {
    order_minus1: 0,
    linear_interp: true,
};
pub(crate) const MMR: Piece = Piece::Mmr { order_minus1: 2 };

#[derive(Clone, Debug)]
pub(crate) struct Rpu {
    /// Profile 7 layout: the residual is enabled, so the mapping carries NLQ pivots.
    pub profile7: bool,
    /// `coefficient_data_type` 1: 32-bit coefficients without an integer part.
    pub float_coefficients: bool,
    /// `coefficient_log2_denom` (fixed-point coefficients only): the fraction's bit length.
    pub log2_denom: u64,
    /// `use_prev_vdr_rpu_flag`: no data mapping follows the header.
    pub reuse_previous: bool,
    /// `num_pivots_minus2` per component; the pieces below must match it plus one.
    pub num_pivots_minus2: [u64; 3],
    /// Declare the pivot counts but write no pivots and no pieces (a count the RPU cannot hold).
    pub omit_pivots: bool,
    pub pieces: [Vec<Piece>; 3],
}

impl Rpu {
    /// A Profile 8.1 RPU with one piece per component.
    pub(crate) fn p81(y: Piece, cb: Piece, cr: Piece) -> Self {
        Self {
            profile7: false,
            float_coefficients: false,
            log2_denom: 23,
            reuse_previous: false,
            num_pivots_minus2: [0; 3],
            omit_pivots: false,
            pieces: [vec![y], vec![cb], vec![cr]],
        }
    }

    /// The RPU payload from the prefix byte (0x19) to the final 0x80, before emulation
    /// prevention. The CRC is not computed: every test that parses such an RPU expects the
    /// crate to stop before it, or to refuse it.
    pub(crate) fn payload(&self) -> Vec<u8> {
        let mut w = BitWriter::default();
        w.bits(25, 8); // rpu_nal_prefix
        w.bits(2, 6); // rpu_type
        w.bits(18, 11); // rpu_format
        w.bits(1, 4); // vdr_rpu_profile
        w.bits(0, 4); // vdr_rpu_level
        w.bit(true); // vdr_seq_info_present_flag
        w.bit(false); // chroma_resampling_explicit_filter_flag
        w.bits(u64::from(self.float_coefficients), 2); // coefficient_data_type
        if !self.float_coefficients {
            w.ue(self.log2_denom); // coefficient_log2_denom
        }
        w.bits(1, 2); // vdr_rpu_normalized_idc
        w.bit(false); // bl_video_full_range_flag
        w.ue(2); // bl_bit_depth_minus8
        w.ue(2); // el_bit_depth_minus8
        w.ue(4); // vdr_bit_depth_minus8
        w.bits(0, 1 + 3 + 1); // spatial filter, reserved_zero_3bits, EL spatial filter
        w.bit(!self.profile7); // disable_residual_flag
        w.bit(false); // vdr_dm_metadata_present_flag
        w.bit(self.reuse_previous); // use_prev_vdr_rpu_flag
        if self.reuse_previous {
            w.ue(0); // prev_vdr_rpu_id
        } else {
            self.write_mapping(&mut w);
        }
        w.align();
        // Room for the crate's 25-byte minimum, then a CRC placeholder and the final byte.
        w.bytes.resize(w.bytes.len().max(28), 0);
        w.bytes.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x80]);
        w.bytes
    }

    fn write_mapping(&self, w: &mut BitWriter) {
        let denom_bits = if self.float_coefficients {
            32
        } else {
            self.log2_denom as u32
        };
        w.ue(0); // vdr_rpu_id
        w.ue(0); // mapping_color_space
        w.ue(0); // mapping_chroma_format_idc
        for &num_pivots_minus2 in &self.num_pivots_minus2 {
            w.ue(num_pivots_minus2);
            if self.omit_pivots {
                return;
            }
            for i in 0..num_pivots_minus2 + 2 {
                w.bits(if i == 0 { 0 } else { 1023 }, 10);
            }
        }
        if self.profile7 {
            w.bits(0, 3); // nlq_method_idc
            w.bits(0, 10); // nlq_pred_pivot_value
            w.bits(1023, 10);
        }
        w.ue(0); // num_x_partitions_minus1
        w.ue(0); // num_y_partitions_minus1
        let coefficient = |w: &mut BitWriter| {
            if !self.float_coefficients {
                w.ue(0); // se(0)
            }
            w.bits(0, denom_bits);
        };
        for piece in self.pieces.iter().flatten() {
            match *piece {
                Piece::Polynomial {
                    order_minus1,
                    linear_interp,
                } => {
                    w.ue(0); // mapping_idc: polynomial
                    w.ue(order_minus1);
                    if order_minus1 == 0 {
                        w.bit(linear_interp);
                    }
                    if !(order_minus1 == 0 && linear_interp) {
                        for _ in 0..order_minus1 + 2 {
                            coefficient(w);
                        }
                    }
                }
                Piece::Mmr { order_minus1 } => {
                    w.ue(1); // mapping_idc: MMR
                    w.bits(order_minus1, 2);
                    coefficient(w); // mmr_constant
                    for _ in 0..(order_minus1 + 1) * 7 {
                        coefficient(w);
                    }
                }
            }
        }
    }
}

/// An `RPU.bin` as `dovi_tool extract-rpu` writes it: a start code before each payload, with
/// emulation prevention bytes inserted.
pub(crate) fn rpu_bin(payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for payload in payloads {
        out.extend_from_slice(&[0, 0, 0, 1]);
        let mut zeros = 0;
        for &byte in payload {
            if zeros >= 2 && byte <= 3 {
                out.push(3);
                zeros = 0;
            }
            out.push(byte);
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
    }
    out
}

/// One RPU whose luma piece is a linear-interpolation polynomial: the file
/// `mkvdovi/tests/fixtures/linear_interp_RPU.bin` holds exactly these bytes.
pub(crate) fn linear_interp_rpu_bin() -> Vec<u8> {
    rpu_bin(&[Rpu::p81(LINEAR_INTERP, MMR, MMR).payload()])
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    used: u32,
}

impl BitWriter {
    fn bit(&mut self, bit: bool) {
        if self.used % 8 == 0 {
            self.bytes.push(0);
        }
        if bit {
            *self.bytes.last_mut().unwrap() |= 0x80 >> (self.used % 8);
        }
        self.used += 1;
    }

    fn bits(&mut self, value: u64, n: u32) {
        for i in (0..n).rev() {
            self.bit(i < 64 && (value >> i) & 1 == 1);
        }
    }

    fn ue(&mut self, value: u64) {
        let code = value + 1;
        let len = 64 - code.leading_zeros();
        self.bits(0, len - 1);
        self.bits(code, len);
    }

    fn align(&mut self) {
        while self.used % 8 != 0 {
            self.bit(false);
        }
    }
}
