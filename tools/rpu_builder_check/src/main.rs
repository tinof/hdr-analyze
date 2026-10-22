//! Fixtures and checks for `dovi_rpu_builder`, outside the workspace (see `Cargo.toml`).
//!
//! ```text
//! rpu_builder_check extra-json <out-dir>          extra.json of every fixture case
//! rpu_builder_check hdr10plus-fixtures <out-dir>  synthetic HDR10+ payloads + their JSON
//! rpu_builder_check payloads <input.mkv|.hevc> <out.payloads>
//!     the HDR10+ payloads of a stream in presentation order, gaps filled in decode order
//!     (as hdr10plus_tool 1.7.2 orders them)
//! rpu_builder_check compare-hdr10plus <in.payloads> <hdr10plus.json> <RPU.bin> <peak> <v29|v40>
//!     the builder over the payloads against dovi_tool's RPU.bin, the parsed values against the
//!     JSON, and the scene starts against SceneInfoSummary
//! ```
//!
//! `.payloads` files: per frame a big-endian u16 length and the payload (T.35 bytes from the
//! country code, emulation prevention removed).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use dovi_rpu_builder::{CmVersion, Hdr10PlusFrame, Hdr10PlusHold, PeakSource, RpuBuilder};

#[path = "../../../dovi_rpu_builder/tests/cases/mod.rs"]
mod cases;
mod hdr10plus_fixtures;
mod payloads;

#[cfg(test)]
mod equivalence;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["extra-json", dir] => write_extra_json(Path::new(dir)),
        ["hdr10plus-fixtures", dir] => hdr10plus_fixtures::write(Path::new(dir)),
        ["payloads", input, output] => payloads::extract(Path::new(input), Path::new(output)),
        ["compare-hdr10plus", payloads, json, rpu, peak, cm] => compare_hdr10plus(
            Path::new(payloads),
            Path::new(json),
            Path::new(rpu),
            peak,
            cm,
        ),
        _ => bail!("usage: see the doc comment of tools/rpu_builder_check/src/main.rs"),
    }
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    fs::write(path, serde_json::to_string_pretty(value)? + "\n")
        .with_context(|| format!("writing {}", path.display()))
}

fn write_extra_json(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    for case in cases::shot_cases() {
        let path = dir.join(format!("{}.extra.json", case.name));
        write_json(&path, &case.params.extra_json(Some(&case.extra_shots())))?;
    }
    for cm in [CmVersion::V29, CmVersion::V40] {
        let path = dir.join(format!("hdr10plus.{}.extra.json", cases::cm_name(cm)));
        write_json(&path, &cases::hdr10plus_params(cm).extra_json(None))?;
    }
    Ok(())
}

pub fn read_payloads(path: &Path) -> Result<Vec<Vec<u8>>> {
    let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut frames = Vec::new();
    let mut rest = data.as_slice();
    while !rest.is_empty() {
        ensure!(rest.len() >= 2, "truncated length in {}", path.display());
        let len = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
        ensure!(
            rest.len() >= 2 + len,
            "truncated payload in {}",
            path.display()
        );
        frames.push(rest[2..2 + len].to_vec());
        rest = &rest[2 + len..];
    }
    Ok(frames)
}

pub fn write_payloads(path: &Path, frames: &[Vec<u8>]) -> Result<()> {
    let mut out = Vec::new();
    for frame in frames {
        out.extend_from_slice(&u16::try_from(frame.len())?.to_be_bytes());
        out.extend_from_slice(frame);
    }
    fs::write(path, out).with_context(|| format!("writing {}", path.display()))
}

/// The records of an `RPU.bin` (`00 00 00 01` + NAL without its 2-byte header), as NAL bodies.
pub fn rpu_records(data: &[u8]) -> Result<Vec<Vec<u8>>> {
    const START: [u8; 4] = [0, 0, 0, 1];
    ensure!(
        data.starts_with(&START),
        "RPU.bin does not start with a start code"
    );
    // An escaped RPU payload never contains 00 00 00 01.
    let mut records = Vec::new();
    let mut start = 4;
    let mut i = 4;
    while i + 4 <= data.len() {
        if data[i..i + 4] == START {
            records.push(data[start..i].to_vec());
            start = i + 4;
            i += 4;
        } else {
            i += 1;
        }
    }
    records.push(data[start..].to_vec());
    Ok(records)
}

fn peak_source(name: &str) -> Result<PeakSource> {
    cases::PEAK_SOURCES
        .iter()
        .find(|(_, cli)| *cli == name)
        .map(|(source, _)| *source)
        .with_context(|| format!("unknown peak source {name}"))
}

fn cm_version(name: &str) -> Result<CmVersion> {
    match name {
        "v29" => Ok(CmVersion::V29),
        "v40" => Ok(CmVersion::V40),
        _ => bail!("unknown CM version {name}"),
    }
}

/// The raw values the JSON states for frame `index` against the parsed payload.
fn check_against_json(
    index: usize,
    frame: &serde_json::Value,
    metadata: &hdr10plus::metadata::Hdr10PlusMetadata,
) -> Vec<String> {
    let lp = &frame["LuminanceParameters"];
    let mut problems = Vec::new();
    let expect = |what: &str, json: Option<u64>, raw: u64, problems: &mut Vec<String>| {
        if json != Some(raw) {
            problems.push(format!("frame {index}: {what} JSON {json:?} parsed {raw}"));
        }
    };
    expect(
        "AverageRGB",
        lp["AverageRGB"].as_u64(),
        u64::from(metadata.average_maxrgb),
        &mut problems,
    );
    let max_scl: Vec<u64> = lp["MaxScl"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_u64())
        .collect();
    if max_scl
        != metadata
            .maxscl
            .iter()
            .map(|&v| u64::from(v))
            .collect::<Vec<_>>()
    {
        problems.push(format!(
            "frame {index}: MaxScl JSON {max_scl:?} parsed {:?}",
            metadata.maxscl
        ));
    }
    let values: Vec<u64> = lp["LuminanceDistributions"]["DistributionValues"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_u64())
        .collect();
    let parsed: Vec<u64> = metadata
        .distribution_maxrgb
        .iter()
        .map(|d| u64::from(d.percentile))
        .collect();
    if values != parsed {
        problems.push(format!(
            "frame {index}: DistributionValues JSON {values:?} parsed {parsed:?}"
        ));
    }
    problems
}

fn compare_hdr10plus(payloads: &Path, json: &Path, rpu: &Path, peak: &str, cm: &str) -> Result<()> {
    use hdr10plus::metadata::VariablePeakBrightness;

    let peak = peak_source(peak)?;
    let cm = cm_version(cm)?;
    let frames = read_payloads(payloads)?;
    let json: serde_json::Value = serde_json::from_slice(&fs::read(json)?)?;
    let scene_info = json["SceneInfo"]
        .as_array()
        .context("JSON has no SceneInfo")?;
    let first_frames: Vec<u64> = json["SceneInfoSummary"]["SceneFirstFrameIndex"]
        .as_array()
        .context("JSON has no SceneFirstFrameIndex")?
        .iter()
        .filter_map(|v| v.as_u64())
        .collect();
    let base = *first_frames.first().context("no scenes")?;
    let first_frames: Vec<u64> = first_frames.iter().map(|f| f - base).collect();
    let records = rpu_records(&fs::read(rpu)?)?;
    ensure!(
        frames.len() == scene_info.len() && frames.len() == records.len(),
        "{} payloads, {} JSON frames, {} RPU records",
        frames.len(),
        scene_info.len(),
        records.len()
    );

    let params = cases::hdr10plus_params(cm);
    let builder = RpuBuilder::new(&params)?;
    let mut hold = Hdr10PlusHold::new(peak, params.l1_avg_pq_cm_version);
    let mut problems = Vec::new();
    let mut our_starts = Vec::new();
    let mut rpu_mismatches = 0usize;
    for (index, payload) in frames.iter().enumerate() {
        let frame = Hdr10PlusFrame::parse(payload).with_context(|| format!("frame {index}"))?;
        let metadata = hdr10plus::metadata::Hdr10PlusMetadata::parse(payload)?;
        problems.extend(check_against_json(index, &scene_info[index], &metadata));
        // Every peak source: the struct's value against the JSON frame's.
        let json_frame: hdr10plus::metadata_json::Hdr10PlusJsonMetadata =
            serde_json::from_value(scene_info[index].clone())?;
        for (source, name) in cases::PEAK_SOURCES {
            let parsed = metadata.peak_brightness_nits(to_hdr10plus(source));
            let from_json = json_frame.peak_brightness_nits(to_hdr10plus(source));
            if parsed != from_json {
                problems.push(format!(
                    "frame {index}: {name} parsed {parsed:?} JSON {from_json:?}"
                ));
            }
        }
        let (l1, scene_refresh) = hold.next(&frame)?;
        if scene_refresh {
            our_starts.push(index as u64);
        }
        let nal = builder.shot(l1)?.write(scene_refresh)?;
        if nal[2..] != records[index][..] {
            rpu_mismatches += 1;
            if rpu_mismatches <= 5 {
                problems.push(format!(
                    "frame {index}: RPU differs (L1 {l1:?}, scene_refresh {scene_refresh})"
                ));
            }
        }
    }
    if our_starts != first_frames {
        problems.push(format!(
            "scene starts differ: {} ours, {} JSON; first difference at {:?}",
            our_starts.len(),
            first_frames.len(),
            our_starts
                .iter()
                .zip(&first_frames)
                .position(|(a, b)| a != b)
        ));
    }
    println!(
        "{}: {} frames, {} scenes, {rpu_mismatches} RPU mismatches, {} problems",
        payloads.display(),
        frames.len(),
        first_frames.len(),
        problems.len()
    );
    for problem in &problems {
        println!("  {problem}");
    }
    ensure!(problems.is_empty(), "{} problems", problems.len());
    Ok(())
}

fn to_hdr10plus(source: PeakSource) -> hdr10plus::metadata::PeakBrightnessSource {
    use hdr10plus::metadata::PeakBrightnessSource as P;
    match source {
        PeakSource::Histogram => P::Histogram,
        PeakSource::Histogram99 => P::Histogram99,
        PeakSource::MaxScl => P::MaxScl,
        PeakSource::MaxSclLuminance => P::MaxSclLuminance,
    }
}

#[allow(dead_code)]
fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dovi_rpu_builder/tests/fixtures")
}
