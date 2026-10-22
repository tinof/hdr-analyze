//! Ported from hdr10plus_tool 1.7.2 `src/core/parser.rs` and `src/core/mod.rs`, Copyright (c)
//! 2025 quietvoid, MIT License; the full notice is in `NOTICE` at the repository root.
//!
//! The HDR10+ payloads of a stream in presentation order, picked, gap-filled and reordered the
//! way `hdr10plus_tool` 1.7.2 does it: every SEI prefix
//! NAL's first `user_data_registered_itu_t_t35` message with the HDR10+ T.35 identity; a frame
//! without one takes the previous frame's payload in decode order; then presentation order.

use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use hevc_parser::hevc::{NALUnit, SeiMessage, NAL_SEI_PREFIX, USER_DATA_REGISTERED_ITU_T_35};
use hevc_parser::io::processor::{HevcProcessor, HevcProcessorOpts};
use hevc_parser::io::{format_from_path, IoProcessor};
use hevc_parser::utils::clear_start_code_emulation_prevention_3_byte;
use hevc_parser::HevcParser;

use crate::write_payloads;

struct Collector {
    input: PathBuf,
    /// (decoded frame index, payload) in decode order.
    frames: Vec<(u64, Option<Vec<u8>>)>,
    ordered: Vec<Vec<u8>>,
}

fn hdr10plus_payload(sei: &[u8]) -> Result<Option<Vec<u8>>> {
    if sei.len() < 4 {
        return Ok(None);
    }
    for msg in SeiMessage::parse_sei_rbsp(sei)? {
        if msg.payload_type != USER_DATA_REGISTERED_ITU_T_35 || msg.payload_size < 7 {
            continue;
        }
        let bytes = &sei[msg.payload_offset..msg.payload_offset + msg.payload_size];
        let identity = bytes[0] == 0xB5
            && bytes[1..3] == [0x00, 0x3C]
            && bytes[3..5] == [0x00, 0x01]
            && bytes[5] == 4
            && bytes[6] == 1;
        if identity {
            return Ok(Some(bytes.to_vec()));
        }
    }
    Ok(None)
}

impl IoProcessor for Collector {
    fn input(&self) -> &PathBuf {
        &self.input
    }

    fn update_progress(&mut self, _delta: u64) {}

    fn process_nals(&mut self, _parser: &HevcParser, nals: &[NALUnit], chunk: &[u8]) -> Result<()> {
        for nal in nals {
            if nal.nal_type == NAL_SEI_PREFIX {
                let sei = clear_start_code_emulation_prevention_3_byte(&chunk[nal.start..nal.end]);
                if let Some(payload) = hdr10plus_payload(&sei)? {
                    self.frames.push((nal.decoded_frame_index, Some(payload)));
                }
            }
            if let Some(&(last, _)) = self.frames.last() {
                if nal.is_slice() && last < nal.decoded_frame_index {
                    self.frames.push((nal.decoded_frame_index, None));
                }
            }
        }
        Ok(())
    }

    fn finalize(&mut self, parser: &HevcParser) -> Result<()> {
        let ordered_frames = parser.ordered_frames();
        ensure!(
            self.frames.len() == ordered_frames.len(),
            "{} payload slots for {} frames",
            self.frames.len(),
            ordered_frames.len()
        );
        // Gaps take the previous payload in decode order.
        let mut previous: Option<Vec<u8>> = None;
        for (_, payload) in &mut self.frames {
            match payload {
                Some(bytes) => previous = Some(bytes.clone()),
                None => *payload = previous.clone(),
            }
        }
        let mut keyed = Vec::with_capacity(self.frames.len());
        for (decoded, payload) in &self.frames {
            let frame = ordered_frames
                .iter()
                .find(|f| f.decoded_number == *decoded)
                .with_context(|| format!("no frame for decoded index {decoded}"))?;
            let Some(payload) = payload else {
                bail!("decoded frame {decoded} has no HDR10+ payload before it");
            };
            keyed.push((frame.presentation_number, payload.clone()));
        }
        keyed.sort_by_key(|(presentation, _)| *presentation);
        self.ordered = keyed.into_iter().map(|(_, payload)| payload).collect();
        Ok(())
    }
}

pub fn extract(input: &Path, output: &Path) -> Result<()> {
    let format = format_from_path(input)?;
    let mut processor = HevcProcessor::new(
        format,
        HevcProcessorOpts {
            parse_nals: true,
            ..Default::default()
        },
        100_000,
    );
    let mut collector = Collector {
        input: input.to_path_buf(),
        frames: Vec::new(),
        ordered: Vec::new(),
    };
    processor.process_file(&mut collector, Some(input))?;
    ensure!(
        !collector.ordered.is_empty(),
        "no HDR10+ metadata in {}",
        input.display()
    );
    write_payloads(output, &collector.ordered)?;
    println!("{}: {} frames", output.display(), collector.ordered.len());
    Ok(())
}
