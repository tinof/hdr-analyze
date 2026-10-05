//! Pictures at the start of an HEVC stream that no decoder outputs.
//!
//! A stream cut at a CRA picture (open GOP, the x265 default) keeps the RASL pictures that follow
//! the CRA in decode order but precede it in display order. They reference pictures from before
//! the cut, so a decoder skips them (H.265 §8.1.3: NoRaslOutputFlag is set for the first IRAP);
//! RASL pictures of a BLA picture are skipped the same way. They are still access units of the
//! stream: `dovi_tool inject-rpu` gives RPU `n` to the picture with presentation number `n`, and
//! these pictures come first in presentation order. Per-frame measurements of the decoded
//! pictures therefore start at stream frame `skipped()`, not at 0.

/// How NAL units are delimited inside a demuxed packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NalFraming {
    /// Start codes (raw `.hevc` / MPEG-TS).
    AnnexB,
    /// Big-endian length prefixes of the given size (MKV / MP4, `hvcC` extradata).
    LengthPrefixed(usize),
}

/// Framing from the codec extradata: an `hvcC` record (configurationVersion 1) stores
/// `lengthSizeMinusOne` in the low two bits of byte 21; anything else carries start codes.
pub fn hevc_framing(extradata: &[u8]) -> NalFraming {
    if extradata.len() >= 23 && extradata[0] == 1 {
        NalFraming::LengthPrefixed(usize::from(extradata[21] & 0x03) + 1)
    } else {
        NalFraming::AnnexB
    }
}

const RADL_N: u8 = 6;
const RADL_R: u8 = 7;
const RASL_N: u8 = 8;
const RASL_R: u8 = 9;
/// BLA_W_LP (16) to the reserved IRAP types (22, 23).
const IRAP: std::ops::RangeInclusive<u8> = 16..=23;
/// VCL NAL unit types are 0..=31.
const FIRST_NON_VCL: u8 = 32;

fn nal_type(header: u8) -> u8 {
    (header >> 1) & 0x3F
}

/// NAL unit type of the first VCL NAL unit (a slice) in a packet.
pub fn first_vcl_nal_type(data: &[u8], framing: NalFraming) -> Option<u8> {
    match framing {
        NalFraming::LengthPrefixed(size) => {
            let mut position = 0;
            while position + size < data.len() {
                let length = data[position..position + size]
                    .iter()
                    .fold(0usize, |acc, &byte| (acc << 8) | usize::from(byte));
                let start = position + size;
                let kind = nal_type(data[start]);
                if kind < FIRST_NON_VCL {
                    return Some(kind);
                }
                position = start.checked_add(length)?;
            }
            None
        }
        NalFraming::AnnexB => data
            .windows(4)
            .enumerate()
            .filter(|(_, window)| window[..3] == [0, 0, 1])
            .map(|(index, _)| nal_type(data[index + 3]))
            .find(|&kind| kind < FIRST_NON_VCL),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    BeforeFirstPicture,
    Leading,
    Done,
}

/// Counts the RASL pictures of the stream's first IRAP picture, packet by packet in decode
/// order. Leading pictures precede the IRAP's trailing pictures in decode order (H.265 §7.4.3),
/// so the scan ends at the first picture that is not a leading picture.
#[derive(Debug)]
pub struct LeadingPictureScan {
    framing: NalFraming,
    phase: Phase,
    starts_with_irap: bool,
    skipped: u64,
}

impl LeadingPictureScan {
    pub fn new(framing: NalFraming) -> Self {
        Self {
            framing,
            phase: Phase::BeforeFirstPicture,
            starts_with_irap: false,
            skipped: 0,
        }
    }

    /// Feeds the next packet in decode order; returns whether it carries a picture (a packet with
    /// only parameter sets, SEI or an end-of-sequence NAL unit does not).
    pub fn observe_packet(&mut self, data: &[u8]) -> bool {
        let Some(kind) = first_vcl_nal_type(data, self.framing) else {
            return false;
        };
        match self.phase {
            Phase::BeforeFirstPicture if IRAP.contains(&kind) => {
                self.starts_with_irap = true;
                self.phase = Phase::Leading;
            }
            // A stream that starts with a non-IRAP picture is not a clean cut; nothing it drops
            // is explained here.
            Phase::BeforeFirstPicture => self.phase = Phase::Done,
            Phase::Leading if kind == RASL_N || kind == RASL_R => self.skipped += 1,
            Phase::Leading if kind == RADL_N || kind == RADL_R => {}
            Phase::Leading | Phase::Done => self.phase = Phase::Done,
        }
        true
    }

    /// Whether the first picture of the stream is an IRAP picture.
    pub fn starts_with_irap(&self) -> bool {
        self.starts_with_irap
    }

    /// RASL pictures of the first IRAP picture: present in the stream, never output.
    pub fn skipped(&self) -> u64 {
        self.skipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRA: u8 = 21;
    const IDR_W_RADL: u8 = 19;
    const TRAIL_R: u8 = 1;
    const PREFIX_SEI: u8 = 39;

    fn nal(kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut unit = vec![kind << 1, 1];
        unit.extend_from_slice(payload);
        unit
    }

    fn length_prefixed(units: &[Vec<u8>]) -> Vec<u8> {
        units
            .iter()
            .flat_map(|unit| {
                let mut out = (unit.len() as u32).to_be_bytes().to_vec();
                out.extend_from_slice(unit);
                out
            })
            .collect()
    }

    fn annex_b(units: &[Vec<u8>]) -> Vec<u8> {
        units
            .iter()
            .flat_map(|unit| {
                let mut out = vec![0, 0, 0, 1];
                out.extend_from_slice(unit);
                out
            })
            .collect()
    }

    fn scan(kinds: &[u8]) -> LeadingPictureScan {
        let mut scan = LeadingPictureScan::new(NalFraming::LengthPrefixed(4));
        for &kind in kinds {
            scan.observe_packet(&length_prefixed(&[nal(kind, &[0x80, 7])]));
        }
        scan
    }

    #[test]
    fn hvcc_extradata_selects_its_length_size() {
        let mut hvcc = vec![0u8; 23];
        hvcc[0] = 1;
        hvcc[21] = 0xFC | 3;
        assert_eq!(hevc_framing(&hvcc), NalFraming::LengthPrefixed(4));
        hvcc[21] = 0xFC | 1;
        assert_eq!(hevc_framing(&hvcc), NalFraming::LengthPrefixed(2));
        assert_eq!(hevc_framing(&[]), NalFraming::AnnexB);
        assert_eq!(hevc_framing(&[0, 0, 0, 1, 0x40, 1]), NalFraming::AnnexB);
    }

    #[test]
    fn the_first_slice_is_found_after_parameter_sets_and_sei() {
        let units = [
            nal(32, &[1, 2]),
            nal(33, &[3]),
            nal(PREFIX_SEI, &[4, 5, 6]),
            nal(RASL_R, &[0x80]),
        ];
        assert_eq!(
            first_vcl_nal_type(&length_prefixed(&units), NalFraming::LengthPrefixed(4)),
            Some(RASL_R)
        );
        assert_eq!(
            first_vcl_nal_type(&annex_b(&units), NalFraming::AnnexB),
            Some(RASL_R)
        );
        assert_eq!(
            first_vcl_nal_type(
                &length_prefixed(&[nal(32, &[1])]),
                NalFraming::LengthPrefixed(4)
            ),
            None
        );
    }

    #[test]
    fn rasl_pictures_after_a_starting_cra_are_counted() {
        // The measured cut: CRA, RASL_R, RASL_N, then trailing pictures.
        let scan = scan(&[CRA, RASL_R, RASL_N, TRAIL_R, TRAIL_R, RASL_N]);
        assert!(scan.starts_with_irap());
        assert_eq!(scan.skipped(), 2);
    }

    #[test]
    fn radl_pictures_are_decodable_and_not_counted() {
        let scan = scan(&[CRA, RADL_N, RASL_N, RADL_R, TRAIL_R]);
        assert_eq!(scan.skipped(), 1);
    }

    #[test]
    fn an_idr_start_skips_nothing() {
        let scan = scan(&[IDR_W_RADL, RADL_N, TRAIL_R, CRA, RASL_N]);
        assert!(scan.starts_with_irap());
        assert_eq!(scan.skipped(), 0);
    }

    #[test]
    fn a_packet_without_a_slice_is_not_a_picture() {
        const EOS: u8 = 36;
        let mut scan = LeadingPictureScan::new(NalFraming::LengthPrefixed(4));
        assert!(scan.observe_packet(&length_prefixed(&[nal(CRA, &[0x80])])));
        assert!(!scan.observe_packet(&length_prefixed(&[nal(EOS, &[])])));
        assert!(!scan.observe_packet(&[]));
    }

    #[test]
    fn a_stream_that_starts_mid_gop_explains_nothing() {
        let scan = scan(&[TRAIL_R, RASL_N, CRA, RASL_N]);
        assert!(!scan.starts_with_irap());
        assert_eq!(scan.skipped(), 0);
    }
}
