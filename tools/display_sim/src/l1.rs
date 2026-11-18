//! L1 streams in the `tools/l1_diff` CSV layout (`frame,min_pq,max_pq,avg_pq`, 12-bit PQ codes,
//! decimals allowed) and shot lists, lined up with the dump's measured frames as `l1_diff`
//! does. A dump record `i` is stream frame `i + leading` (`leading`: the dump header's
//! undecodable leading pictures). A CSV in stream coordinates ends at the last stream frame
//! and starts at or before the first measured one (`0..stream` from a Dolby Vision export,
//! `leading..stream` from `l1_diff --export-reference`); one in decoded coordinates has one row
//! per measured frame, labelled from 0. Labels count up by one. Shot lists are in stream
//! coordinates and shifted by `leading`.

use std::ops::Range;

use anyhow::{bail, ensure, Context, Result};

use crate::curves::Metadata;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct L1Row {
    /// 1-based CSV line, for messages.
    pub line: usize,
    pub frame: u64,
    pub metadata: Metadata,
}

/// Parses the CSV text. L1 min is read and checked but not used: the curves do not read it.
pub fn parse_csv(text: &str) -> Result<Vec<L1Row>> {
    let mut rows = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if (index == 0 && line.starts_with("frame")) || line.trim().is_empty() {
            continue;
        }
        let columns: Vec<&str> = line.split(',').map(str::trim).collect();
        ensure!(
            columns.len() >= 4,
            "line {number} has {} columns; expected frame,min_pq,max_pq,avg_pq",
            columns.len()
        );
        let code = |column: usize, name: &str| -> Result<f64> {
            let value: f64 = columns[column]
                .parse()
                .with_context(|| format!("line {number}: {name} '{}'", columns[column]))?;
            ensure!(
                value.is_finite() && (0.0..=4095.0).contains(&value),
                "line {number}: {name} {value} is not a 12-bit PQ code (0 to 4095)"
            );
            Ok(value)
        };
        code(1, "min_pq")?;
        rows.push(L1Row {
            line: number,
            frame: columns[0]
                .parse()
                .with_context(|| format!("line {number}: frame '{}'", columns[0]))?,
            metadata: Metadata {
                max_code: code(2, "max_pq")?,
                avg_code: code(3, "avg_pq")?,
            },
        });
    }
    Ok(rows)
}

/// Which frames a CSV's labels count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coordinates {
    /// Stream pictures, the leading pictures included.
    Stream,
    /// Measured frames from 0 (only told apart from stream when there are leading pictures).
    Decoded,
}

/// One L1 value per measured frame.
#[derive(Debug)]
pub struct Stream {
    pub metadata: Vec<Metadata>,
    pub coordinates: Coordinates,
    /// Rows before the first measured frame (for the leading pictures), not used.
    pub skipped: usize,
}

/// Lines `rows` up with `frames` measured frames after `leading` undecodable pictures.
pub fn align(rows: &[L1Row], frames: usize, leading: usize) -> Result<Stream> {
    let Some(first) = rows.first() else {
        bail!("the CSV has no frames");
    };
    for pair in rows.windows(2) {
        if pair[1].frame != pair[0].frame + 1 {
            bail!(
                "line {}: frame {} follows frame {}; frame labels must count up by one",
                pair[1].line,
                pair[1].frame,
                pair[0].frame
            );
        }
    }
    let first_label = usize::try_from(first.frame).context("frame label too large")?;
    let end_label = first_label + rows.len();
    let stream = frames + leading;
    let (skip, coordinates) = if first_label <= leading && end_label == stream {
        (leading - first_label, Coordinates::Stream)
    } else if leading > 0 && first_label == 0 && rows.len() == frames {
        (0, Coordinates::Decoded)
    } else {
        bail!(
            "frame count mismatch: the dump has {frames} measured frames after {leading} \
             leading pictures ({stream} stream frames); the CSV has {} rows labelled {first_label}..={}",
            rows.len(),
            end_label - 1
        );
    };
    Ok(Stream {
        metadata: rows[skip..].iter().map(|row| row.metadata).collect(),
        coordinates,
        skipped: skip,
    })
}

/// Refuses a reference and a candidate in different coordinate systems, and decoded
/// coordinates with a shot list (which is in stream coordinates) when there are leading
/// pictures; with none, both systems are the same.
pub fn check_coordinates(
    reference: Coordinates,
    candidate: Coordinates,
    per_shot: bool,
    leading: usize,
) -> Result<()> {
    if leading == 0 {
        return Ok(());
    }
    ensure!(
        reference == candidate,
        "the reference CSV is in {} coordinates and the candidate in {}: with {leading} leading \
         pictures they cannot both describe the dump's frames; export both with l1_diff, which \
         labels rows with stream frames",
        name(reference),
        name(candidate)
    );
    ensure!(
        !(per_shot && reference == Coordinates::Decoded),
        "--per-shot cannot be lined up with CSVs in decoded-frame coordinates when the stream \
         has {leading} leading pictures; re-export them with l1_diff, which labels stream frames"
    );
    Ok(())
}

fn name(coordinates: Coordinates) -> &'static str {
    match coordinates {
        Coordinates::Stream => "stream",
        Coordinates::Decoded => "decoded-frame",
    }
}

/// Shot ranges over the measured frames from a shot list in stream coordinates: one start
/// frame per line, from 0, strictly increasing; a last line equal to the stream frame count
/// (the dovi_tool sentinel) is dropped. Shots inside the leading pictures are dropped.
pub fn parse_shots(text: &str, frames: usize, leading: usize) -> Result<Vec<Range<usize>>> {
    let stream = frames + leading;
    let mut starts = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let start: usize = line
            .parse()
            .with_context(|| format!("shot list line {}: expected a frame number", index + 1))?;
        starts.push(start);
    }
    if starts.last() == Some(&stream) {
        starts.pop();
    }
    ensure!(!starts.is_empty(), "the shot list has no shot starts");
    ensure!(
        starts[0] == 0,
        "the shot list must start at frame 0 (first start {})",
        starts[0]
    );
    for pair in starts.windows(2) {
        ensure!(
            pair[1] > pair[0],
            "shot starts must increase ({} then {})",
            pair[0],
            pair[1]
        );
    }
    let last = starts[starts.len() - 1];
    ensure!(
        last < stream,
        "shot start {last} lies beyond the {stream} stream frames"
    );
    let mut shots = Vec::with_capacity(starts.len());
    for (index, &start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(stream);
        let (start, end) = (start.saturating_sub(leading), end.saturating_sub(leading));
        if end > start {
            shots.push(start..end);
        }
    }
    Ok(shots)
}

#[cfg(test)]
#[path = "l1_tests.rs"]
mod tests;
