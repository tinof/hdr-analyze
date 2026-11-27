//! Pass 2: every access unit of the stream rewritten with an AUD first and its RPU last, as
//! dovi_tool's `RpuInjector` writes it (`src/dovi/rpu_injector.rs` 2.3.4, lines 237-402) with its
//! defaults: `--start-code four`, no `--no-add-aud`, no `--drop-hdr10plus`.
//!
//! Four substitutions, none of which changes a byte: the frame index replaces
//! `frames.iter().find`, an error replaces its `unwrap()`, one byte buffer per access unit
//! replaces `Vec<NalBuffer>`, and the RPUs are serialised once up front. Two upstream quirks are
//! kept on purpose: NAL units after the last picture's access unit are never written, and the AUD
//! type comes from the first slice of the picture only.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use hevc_parser::hevc::{Frame, NALUnit, NAL_AUD, NAL_UNSPEC62};
use hevc_parser::io::IoProcessor;
use hevc_parser::HevcParser;

use super::frame_index::{rpu_index, FrameIndex};

/// dovi_tool's `StartCodePreset::Four`: every NAL unit it writes starts with this.
const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// The pass-2 processor: buffers one access unit at a time and writes it on the first NAL unit
/// of the next one, or in `finalize` for the last.
pub(super) struct Interleaver<'a> {
    index: &'a FrameIndex,
    rpus: &'a [Vec<u8>],
    mismatched: bool,
    out: &'a mut dyn Write,
    /// Decoded number of the access unit being buffered (dovi_tool's `frame_buffer.frame_number`).
    frame: u64,
    /// Its NAL units, each behind [`START_CODE`], without AUDs and RPUs.
    buffer: Vec<u8>,
    buffered: usize,
    /// Every NAL unit the parser handed over, written or not.
    pub(super) nals: u64,
    /// Access units written, each with one RPU.
    pub(super) frames_written: usize,
    pub(super) bytes_written: u64,
    /// The pictures this pass's parser found (`ordered_frames().len()`).
    pub(super) stream_frames: usize,
    input: PathBuf,
}

impl<'a> Interleaver<'a> {
    /// `rpus` are the serialised RPU NAL units (`7C 01` header included, no start code);
    /// `mismatched` is dovi_tool's `mismatched_length`: the RPU count differs from the pictures.
    pub(super) fn new(
        index: &'a FrameIndex,
        rpus: &'a [Vec<u8>],
        mismatched: bool,
        out: &'a mut dyn Write,
    ) -> Self {
        Self {
            index,
            rpus,
            mismatched,
            out,
            frame: 0,
            buffer: Vec::with_capacity(1 << 20),
            buffered: 0,
            nals: 0,
            frames_written: 0,
            bytes_written: 0,
            stream_frames: 0,
            input: PathBuf::new(),
        }
    }

    /// One NAL unit, in stream order; its bytes are `chunk[nal.start..nal.end]`.
    pub(super) fn push(&mut self, nal: &NALUnit, chunk: &[u8]) -> Result<()> {
        self.nals += 1;
        if self.frame != nal.decoded_frame_index {
            // An existing AUD is skipped before it can start the next access unit.
            if nal.nal_type == NAL_AUD {
                return Ok(());
            }
            self.write_frame()?;
            self.frame = nal.decoded_frame_index;
        }
        // Existing RPUs and AUDs are replaced.
        if nal.nal_type != NAL_UNSPEC62 && nal.nal_type != NAL_AUD {
            self.buffer.extend_from_slice(&START_CODE);
            self.buffer.extend_from_slice(&chunk[nal.start..nal.end]);
            self.buffered += 1;
        }
        Ok(())
    }

    /// The end of the stream: writes the last access unit unless the buffered NAL units belong
    /// to no picture of the `stream_frames` the parser found, and flushes the output.
    pub(super) fn finish(&mut self, stream_frames: usize) -> Result<()> {
        self.stream_frames = stream_frames;
        if usize::try_from(self.frame).ok() != Some(stream_frames) && self.buffered > 0 {
            self.write_frame()?;
        }
        self.out.flush()?;
        Ok(())
    }

    /// Writes the buffered access unit: its AUD, its NAL units, its RPU.
    fn write_frame(&mut self) -> Result<()> {
        let entry = self.index.get(self.frame).ok_or_else(|| {
            anyhow!(
                "decoded frame {} is not in the frame order of the first pass",
                self.frame
            )
        })?;
        let rpu = rpu_index(entry.presentation, self.rpus.len(), self.mismatched)
            .map(|index| &self.rpus[index])
            .ok_or_else(|| anyhow!("No RPU found for presentation frame {}", entry.presentation))?;
        let picture = Frame {
            frame_type: entry.frame_type,
            ..Frame::default()
        };
        let aud = hevc_parser::utils::aud_for_frame(&picture, None)?;
        for part in [&START_CODE[..], &aud, &self.buffer, &START_CODE, rpu] {
            self.out.write_all(part)?;
            self.bytes_written += part.len() as u64;
        }
        self.frames_written += 1;
        self.buffer.clear();
        self.buffered = 0;
        Ok(())
    }
}

impl IoProcessor for Interleaver<'_> {
    fn input(&self) -> &PathBuf {
        &self.input
    }

    fn update_progress(&mut self, _delta: u64) {}

    fn process_nals(&mut self, _parser: &HevcParser, nals: &[NALUnit], chunk: &[u8]) -> Result<()> {
        nals.iter().try_for_each(|nal| self.push(nal, chunk))
    }

    fn finalize(&mut self, parser: &HevcParser) -> Result<()> {
        self.finish(parser.ordered_frames().len())
    }
}
