//! What display_sim prints and writes. Every output starts with the limits of the method and the
//! libplacebo version the run linked.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::ops::Range;
use std::path::Path;

use anyhow::{Context, Result};

use crate::curves::{Curve, Display, Metadata};
use crate::hist::{Header, PeakDomain};
use crate::l1::{Coordinates, Stream};
use crate::sim::{level_steps, FrameResult, Summary, LEVELS_NITS};
use crate::{Args, Lane};

/// The limits stated with every result (ROADMAP WS7) and the pin.
pub fn limits(version: &str) -> Vec<String> {
    [
        "display_sim (ROADMAP WS7.1): candidate L1 against reference L1 as change in displayed luminance.",
        "Limits: open curves are not a Dolby display. This is the tone curve of libplacebo 7.387.0",
        "alone, with its parameters set as its renderer sets them. The 7.387.0 renderer also maps every",
        "frame with an average whose L1 max exceeds the display peak by more than a few codes into a",
        "dynamic target and scales the whole picture by an exposure change (1000-nit display, L1 avg",
        "10 nits: target 1027 nits, exposure 0.973), and under default parameters runs peak detection",
        "on the picture beside the Dolby Vision L1; neither is modelled, so a libplacebo render",
        "differs from these numbers. The curve reads only L1 max and avg (L1 max as",
        "luminance) and the source black: trims, L1 min, L4 and L11 cannot be evaluated. L1 values are",
        "used as given; the renderer falls back to static HDR10 metadata when L1 max or avg is 0. An",
        "L1 max of 0, or one at or below the source black, reads as the PQ peak (10,000 nits), as the",
        "renderer's last fallback does.",
    ]
    .iter()
    .map(|line| (*line).to_owned())
    .chain([
        format!(
            "libplacebo: linked {version}, pinned 7.387.0 (commit 275f7dea27924cdd34ab2e6c20e448b6bc4fab65)."
        ),
        format!(
            "Curves: {}.",
            Curve::ALL.map(Curve::label).join(", ")
        ),
    ])
    .collect()
}

pub fn print_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

fn coordinates_name(stream: &Stream) -> String {
    let name = match stream.coordinates {
        Coordinates::Stream => "stream",
        Coordinates::Decoded => "decoded-frame",
    };
    if stream.skipped > 0 {
        format!(
            "{name} coordinates, {} rows before the first measured frame skipped",
            stream.skipped
        )
    } else {
        format!("{name} coordinates")
    }
}

pub fn describe_inputs(
    args: &Args,
    header: &Header,
    reference: &Stream,
    candidate: &Stream,
    shots: Option<&[Range<usize>]>,
) -> Vec<String> {
    let domain = match header.peak_domain {
        PeakDomain::MaxRgb => "max-RGB",
        PeakDomain::Luma => "luma",
    };
    let transfer = if header.hlg {
        "HLG (Profile 8.4 decode)"
    } else {
        "PQ"
    };
    let mut lines = vec![
        format!(
            "Dump: {}: {} frames after {} leading pictures; {domain} peak domain, {transfer}, \
             sample rate {}, downscale {}.",
            args.dump.display(),
            header.frames,
            header.leading_skipped,
            header.sample_rate,
            header.downscale
        ),
        format!(
            "Reference: {} ({}). Candidate: {} ({}).",
            args.reference.display(),
            coordinates_name(reference),
            args.candidate.display(),
            coordinates_name(candidate)
        ),
        format!(
            "Source black {} nits, display black {} nits; a step is a frame-to-frame change above {}%.",
            args.source_black,
            args.panel_black,
            args.step_threshold * 100.0
        ),
    ];
    if let (Some(shots), Some(path)) = (shots, &args.per_shot) {
        lines.push(format!(
            "Shots: {} from {}; steps across a shot start are left out.",
            shots.len(),
            path.display()
        ));
    }
    lines
}

/// A signed percentage with two decimals, or n/a.
fn pct(value: Option<f64>) -> String {
    value.map_or_else(
        || "n/a".to_owned(),
        |value| format!("{:+.2}%", value * 100.0),
    )
}

fn spread(values: Option<[f64; 3]>) -> String {
    match values {
        Some([min, median, max]) => format!(
            "min {}  median {}  max {}",
            pct(Some(min)),
            pct(Some(median)),
            pct(Some(max))
        ),
        None => "n/a".to_owned(),
    }
}

pub fn print_summary(curve: Curve, display: &Display, run: &Summary, threshold: f64) {
    println!();
    println!(
        "{}, {}-nit display: {} frames",
        curve.label(),
        display.panel_peak_nits,
        run.frames
    );
    println!(
        "  displayed light, histogram-weighted: {}  (per frame: {})",
        pct(run.light_change()),
        spread(run.frame_light.digest())
    );
    for (nits, values) in LEVELS_NITS.iter().zip(&run.levels) {
        println!("  at {nits} nits: {}", spread(values.digest()));
    }
    println!("  at the frame's peak: {}", spread(run.peak.digest()));
    println!(
        "  the renderer skips the curve (display clips the source) on {} reference and {} candidate frames",
        run.noop_reference, run.noop_candidate
    );
    if let Some((reference, candidate)) = run.clipped_shares() {
        println!(
            "  clipped pixels: reference {:.3}%, candidate {:.3}%",
            reference * 100.0,
            candidate * 100.0
        );
    }
    let steps = &run.steps;
    println!(
        "  steps above {}% at 10/100/200 nits ({} frame pairs): reference {} (largest {:.2}%), \
         candidate {} (largest {:.2}%), added by the candidate {}",
        threshold * 100.0,
        steps.pairs,
        steps.above_reference,
        steps.max_reference * 100.0,
        steps.above_candidate,
        steps.max_candidate * 100.0,
        steps.added
    );
}

/// One shot's line of the per-shot table.
#[derive(Clone, Debug)]
pub struct ShotLine {
    frames: Range<usize>,
    light: Option<f64>,
    levels: [Option<f64>; 3],
    peak: Option<f64>,
    clipped: Option<(f64, f64)>,
    steps: (usize, usize, usize),
}

impl ShotLine {
    pub fn new(frames: Range<usize>, shot: &Summary) -> Self {
        let median = |spread: &crate::sim::Spread| spread.digest().map(|[_, median, _]| median);
        Self {
            frames,
            light: shot.light_change(),
            levels: std::array::from_fn(|level| median(&shot.levels[level])),
            peak: median(&shot.peak),
            clipped: shot.clipped_shares(),
            steps: (
                shot.steps.above_reference,
                shot.steps.above_candidate,
                shot.steps.added,
            ),
        }
    }
}

pub fn print_shots(shots: &[ShotLine]) {
    println!("  per shot (measured frames; level and peak changes are the shot's median):");
    println!(
        "  {:>13} {:>9} {:>9} {:>9} {:>9} {:>9} {:>17} {:>14}",
        "frames", "light", "@10", "@100", "@200", "@peak", "clipped ref/cand", "steps r/c/add"
    );
    for shot in shots {
        let clipped = shot.clipped.map_or_else(
            || "n/a".to_owned(),
            |(reference, candidate)| format!("{:.2}/{:.2}%", reference * 100.0, candidate * 100.0),
        );
        println!(
            "  {:>13} {:>9} {:>9} {:>9} {:>9} {:>9} {:>17} {:>14}",
            format!("{}-{}", shot.frames.start, shot.frames.end - 1),
            pct(shot.light),
            pct(shot.levels[0]),
            pct(shot.levels[1]),
            pct(shot.levels[2]),
            pct(shot.peak),
            clipped,
            format!("{}/{}/{}", shot.steps.0, shot.steps.1, shot.steps.2)
        );
    }
}

/// `--frames-csv`: one row per frame, curve and display; changes and steps in percent.
pub struct FramesCsv {
    out: BufWriter<File>,
}

const CSV_HEADER: &str = "frame,stream_frame,curve,panel_nits,reference_max_pq,reference_avg_pq,\
candidate_max_pq,candidate_avg_pq,change_10_pct,change_100_pct,change_200_pct,peak_bin,\
change_peak_pct,change_light_pct,clipped_reference_pct,clipped_candidate_pct,\
step_reference_10_pct,step_reference_100_pct,step_reference_200_pct,\
step_candidate_10_pct,step_candidate_100_pct,step_candidate_200_pct";

/// A percentage with four decimals, empty when there is none.
fn cell(value: Option<f64>) -> String {
    value.map_or_else(String::new, |value| format!("{:.4}", value * 100.0))
}

impl FramesCsv {
    /// The limits and the inputs go first, as `#` lines.
    pub fn create(path: &Path, limits: &[String], describe: &[String]) -> Result<Self> {
        let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let mut out = BufWriter::new(file);
        for line in limits.iter().chain(describe) {
            writeln!(out, "# {line}")?;
        }
        writeln!(out, "{CSV_HEADER}")?;
        Ok(Self { out })
    }

    /// `metadata`: the frame's reference and candidate L1.
    pub fn row(
        &mut self,
        frame: usize,
        leading: u64,
        lane: &Lane,
        [reference, candidate]: [&Metadata; 2],
        previous: Option<&FrameResult>,
        result: &FrameResult,
    ) -> Result<()> {
        let changes = result.level_changes();
        let (clipped_reference, clipped_candidate) = if result.pixels > 0 {
            let pixels = result.pixels as f64;
            (
                Some(result.clipped_reference as f64 / pixels),
                Some(result.clipped_candidate as f64 / pixels),
            )
        } else {
            (None, None)
        };
        let steps = |side: fn(&FrameResult) -> &[f64; 3]| {
            previous.map_or([None; 3], |previous| {
                level_steps(side(previous), side(result))
            })
        };
        let step_reference = steps(|frame| &frame.level_reference);
        let step_candidate = steps(|frame| &frame.level_candidate);
        let cells: Vec<String> = [
            frame.to_string(),
            (frame as u64 + leading).to_string(),
            lane.curve.name().to_owned(),
            lane.display.panel_peak_nits.to_string(),
            reference.max_code.to_string(),
            reference.avg_code.to_string(),
            candidate.max_code.to_string(),
            candidate.avg_code.to_string(),
        ]
        .into_iter()
        .chain(changes.iter().map(|&value| cell(value)))
        .chain([
            result
                .peak_bin
                .map_or_else(String::new, |bin| bin.to_string()),
            cell(result.peak_change),
            cell(result.light_change()),
            cell(clipped_reference),
            cell(clipped_candidate),
        ])
        .chain(
            step_reference
                .iter()
                .chain(&step_candidate)
                .map(|&value| cell(value)),
        )
        .collect();
        writeln!(self.out, "{}", cells.join(",")).context("writing the frames CSV")
    }

    pub fn finish(mut self) -> Result<()> {
        self.out.flush().context("writing the frames CSV")
    }
}
