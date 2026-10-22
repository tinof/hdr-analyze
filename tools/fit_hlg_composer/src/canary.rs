//! A deliberately wrong Profile 8.4 composer for playback tests: the `bt2100` composer with
//! both chroma MMR constants shifted by the same offset, so a device that applies the RPU's
//! composer shows a strong tint (green for a negative offset) and a device that ignores it
//! shows the normal picture. Diagnostic only: never written by mkvdovi, and no analyzer
//! measures through it, so the L1 of the input RPU (measured through `bt2100`) is kept. The
//! mapping itself is `dovi84_composer::canary_mapping`, shared with `dovi_rpu_builder`.
//!
//! ```text
//! fit_hlg_composer canary-rpu <bt2100 RPU.bin> <out RPU.bin> [--offset <v>]
//! fit_hlg_composer canary-check <RPU.bin> [--offset <v>]
//! ```

use std::fs;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use dolby_vision::rpu::dovi_rpu::DoviRpu;
use dolby_vision::rpu::rpu_data_mapping::RpuDataMapping;
use dolby_vision::rpu::utils::parse_rpu_file;
use dovi84_composer::{canary_mapping, mappings_equal, Composer, DEFAULT_CANARY_OFFSET};

use crate::model::Decoder;

/// Start code before each RPU in an `RPU.bin` (as `dovi84_composer::rewrite` writes it).
const RPU_START_CODE: [u8; 4] = [0, 0, 0, 1];

fn encode_rpu_file(rpus: &[DoviRpu]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (index, rpu) in rpus.iter().enumerate() {
        let nal = rpu
            .write_hevc_unspec62_nalu()
            .with_context(|| format!("encoding RPU frame {index}"))?;
        out.extend_from_slice(&RPU_START_CODE);
        out.extend_from_slice(&nal[2..]);
    }
    Ok(out)
}

fn parse_offset(args: &[String]) -> Result<f64> {
    match args {
        [] => Ok(DEFAULT_CANARY_OFFSET),
        [flag, value] if flag == "--offset" => Ok(value.parse()?),
        _ => bail!("expected [--offset <v>]"),
    }
}

/// Neutral greys through `bt2100` and the canary, as 12-bit PQ R′/G′/B′ codes.
fn print_neutral_decode(canary: &RpuDataMapping) {
    let normal = Decoder::new(&Composer::Bt2100V1.rpu_data_mapping());
    let tinted = Decoder::new(canary);
    let codes = |rgb: [f32; 3]| rgb.map(|c| (f64::from(c) * 4095.0).round() as i64);
    println!("neutral HLG (Cb = Cr = 512), 12-bit PQ R'/G'/B': bt2100 -> canary");
    for y in [200_u16, 502, 721, 940] {
        let [r0, g0, b0] = codes(normal.rgb_pq(y, 512, 512).0);
        let [r1, g1, b1] = codes(tinted.rgb_pq(y, 512, 512).0);
        println!("  Y {y:4}: {r0}/{g0}/{b0} -> {r1}/{g1}/{b1}");
    }
}

fn check_frames(rpus: &[DoviRpu], expected: &RpuDataMapping, what: &str) -> Result<()> {
    ensure!(!rpus.is_empty(), "the RPU contains no frames");
    for (index, rpu) in rpus.iter().enumerate() {
        ensure!(
            !rpu.header.use_prev_vdr_rpu_flag,
            "RPU frame {index}: use_prev_vdr_rpu_flag is set"
        );
        let mapping = rpu
            .rpu_data_mapping
            .as_ref()
            .with_context(|| format!("RPU frame {index}: no rpu_data_mapping"))?;
        ensure!(
            mappings_equal(mapping, expected),
            "RPU frame {index} does not carry the {what} composer"
        );
    }
    Ok(())
}

/// `canary-rpu <bt2100 RPU.bin> <out RPU.bin> [--offset <v>]`
pub fn run_rewrite(args: &[String]) -> Result<()> {
    let [input, output, rest @ ..] = args else {
        bail!("canary-rpu <bt2100 RPU.bin> <out RPU.bin> [--offset <v>]");
    };
    let offset = parse_offset(rest)?;
    let canary = canary_mapping(offset)?;
    let input = Path::new(input);
    let original = fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let mut rpus = parse_rpu_file(input).with_context(|| format!("parsing {}", input.display()))?;
    check_frames(&rpus, &Composer::Bt2100V1.rpu_data_mapping(), "bt2100")?;
    ensure!(
        encode_rpu_file(&rpus)? == original,
        "{} does not survive an unchanged round trip through the dolby_vision crate",
        input.display()
    );
    for rpu in &mut rpus {
        rpu.rpu_data_mapping = Some(canary.clone());
        rpu.modified = true;
    }
    fs::write(output, encode_rpu_file(&rpus)?).with_context(|| format!("writing {output}"))?;
    let written = parse_rpu_file(output).with_context(|| format!("re-parsing {output}"))?;
    ensure!(written.len() == rpus.len(), "frame count changed on write");
    check_frames(&written, &canary, "canary")?;
    println!(
        "{output}: {} frames carry the canary composer (bt2100, chroma offset {offset:+})",
        written.len()
    );
    print_neutral_decode(&canary);
    Ok(())
}

/// `canary-check <RPU.bin> [--offset <v>]`
pub fn run_check(args: &[String]) -> Result<()> {
    let [path, rest @ ..] = args else {
        bail!("canary-check <RPU.bin> [--offset <v>]");
    };
    let offset = parse_offset(rest)?;
    let rpus = parse_rpu_file(path).with_context(|| format!("parsing {path}"))?;
    check_frames(&rpus, &canary_mapping(offset)?, "canary")?;
    println!(
        "{path}: all {} frames carry the canary composer (offset {offset:+})",
        rpus.len()
    );
    Ok(())
}
