//! display_sim (ROADMAP WS7.1): a histogram-domain display-mapping simulator. It reads the
//! analyzer's per-frame PQ histograms (`--dump-pq-hist`) and two L1 streams, a reference and a
//! candidate, in the `tools/l1_diff` CSV layout, and puts every frame through libplacebo
//! 7.387.0's spline, st2094-10 and bt2390 curves for 100-, 600- and 1000-nit displays. It
//! reports the candidate's change of displayed luminance against the reference: at 10, 100 and
//! 200 nits and at the frame's peak, histogram-weighted over the picture, the clipped share of
//! pixels, and frame-to-frame steps; per run, per shot (`--per-shot`) and per frame
//! (`--frames-csv`). Every output starts with the limits of the method and the linked version.

mod curves;
mod hist;
mod l1;
mod lp;
mod report;
mod sim;

use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use clap::Parser;

use crate::curves::{Curve, Display, Metadata};
use crate::hist::{Dump, BINS};
use crate::report::{FramesCsv, ShotLine};
use crate::sim::{compare_frame, FrameResult, Response, Summary};

#[derive(Parser, Debug)]
#[command(
    name = "display_sim",
    version,
    about = "Display-mapping simulator (libplacebo 7.387.0 curves) over per-frame PQ histograms"
)]
struct Args {
    /// The analyzer's --dump-pq-hist file (pqhist v1).
    #[arg(long, value_name = "PATH")]
    dump: PathBuf,
    /// Reference L1 CSV: frame,min_pq,max_pq,avg_pq in 12-bit PQ codes (the tools/l1_diff
    /// layout), in stream or decoded-frame coordinates.
    #[arg(long, value_name = "CSV")]
    reference: PathBuf,
    /// Candidate L1 CSV, same layout and coordinates as the reference.
    #[arg(long, value_name = "CSV")]
    candidate: PathBuf,
    /// Shot list (one start frame per line, stream coordinates): adds per-shot results and
    /// leaves frame pairs across a shot start out of the steps.
    #[arg(long, value_name = "PATH")]
    per_shot: Option<PathBuf>,
    /// Display peaks in nits.
    #[arg(long, value_delimiter = ',', default_value = "100,600,1000")]
    panels: Vec<f64>,
    /// Curves to run.
    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        default_value = "spline,st2094-10,bt2390"
    )]
    curves: Vec<Curve>,
    /// Source black in nits (the source range's minimum; L1 min is not read).
    #[arg(long, default_value_t = 0.005)]
    source_black: f64,
    /// Display black in nits.
    #[arg(long, default_value_t = 0.005)]
    panel_black: f64,
    /// Frame-to-frame change of displayed luminance counted as a step (0.01 = 1%).
    #[arg(long, default_value_t = 0.01)]
    step_threshold: f64,
    /// Write one CSV row per frame, curve and display.
    #[arg(long, value_name = "PATH")]
    frames_csv: Option<PathBuf>,
}

/// One curve on one display through the run.
struct Lane {
    curve: Curve,
    display: Display,
    /// The responses of the last metadata seen; L1 rarely changes inside a shot.
    reference: Option<(Metadata, Response)>,
    candidate: Option<(Metadata, Response)>,
    previous: Option<FrameResult>,
    run: Summary,
    shot: Summary,
    shots: Vec<ShotLine>,
}

fn response<'a>(
    slot: &'a mut Option<(Metadata, Response)>,
    curve: Curve,
    display: &Display,
    metadata: Metadata,
) -> Result<&'a Response> {
    if slot.as_ref().is_none_or(|(cached, _)| *cached != metadata) {
        *slot = Some((metadata, Response::new(curve, display, &metadata)?));
    }
    Ok(&slot.as_ref().expect("filled above").1)
}

/// Everything checked before the first record is read.
struct Inputs {
    dump: Dump,
    reference: Vec<Metadata>,
    candidate: Vec<Metadata>,
    shots: Option<Vec<Range<usize>>>,
    describe: Vec<String>,
}

fn read_stream(path: &Path, frames: usize, leading: usize) -> Result<l1::Stream> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading the L1 CSV {}", path.display()))?;
    let rows = l1::parse_csv(&text).with_context(|| format!("reading {}", path.display()))?;
    l1::align(&rows, frames, leading)
        .with_context(|| format!("lining {} up with the dump", path.display()))
}

fn read_inputs(args: &Args) -> Result<Inputs> {
    let dump = Dump::open(&args.dump)?;
    let header = dump.header;
    let frames = usize::try_from(header.frames).context("frame count too large")?;
    let leading = usize::try_from(header.leading_skipped).context("leading count too large")?;
    let reference = read_stream(&args.reference, frames, leading)?;
    let candidate = read_stream(&args.candidate, frames, leading)?;
    l1::check_coordinates(
        reference.coordinates,
        candidate.coordinates,
        args.per_shot.is_some(),
        leading,
    )?;
    let shots = match &args.per_shot {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading the shot list {}", path.display()))?;
            Some(
                l1::parse_shots(&text, frames, leading)
                    .with_context(|| format!("reading {}", path.display()))?,
            )
        }
        None => None,
    };
    let describe = report::describe_inputs(args, &header, &reference, &candidate, shots.as_deref());
    Ok(Inputs {
        dump,
        reference: reference.metadata,
        candidate: candidate.metadata,
        shots,
        describe,
    })
}

/// Refuses every invalid number before libplacebo sees it.
fn check_args(args: &Args) -> Result<Vec<Display>> {
    if let Some(csv) = &args.frames_csv {
        check_frames_csv_path(csv, args)?;
    }
    ensure!(!args.curves.is_empty(), "--curves names no curve");
    ensure!(!args.panels.is_empty(), "--panels names no display");
    ensure!(
        args.step_threshold.is_finite() && args.step_threshold >= 0.0,
        "--step-threshold {} must be a fraction of at least 0",
        args.step_threshold
    );
    args.panels
        .iter()
        .map(|&peak| {
            let display = Display {
                source_black_nits: args.source_black,
                panel_black_nits: args.panel_black,
                panel_peak_nits: peak,
            };
            display.validate().map(|()| display)
        })
        .collect()
}

/// Refuses a `--frames-csv` path that names one of the inputs: creating the CSV would truncate
/// it. The inputs exist, so they compare canonicalized; the CSV compares by its canonicalized
/// directory and file name (it may not exist yet).
fn check_frames_csv_path(csv: &Path, args: &Args) -> Result<()> {
    let csv_dir = match csv.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let Some(name) = csv.file_name() else {
        anyhow::bail!("--frames-csv {} names no file", csv.display());
    };
    let csv_target = match csv_dir.canonicalize() {
        Ok(dir) => dir.join(name),
        // A missing directory holds no input; FramesCsv::create reports it.
        Err(_) => return Ok(()),
    };
    let csv_target = csv_target.canonicalize().unwrap_or(csv_target);
    let mut inputs = vec![
        ("--dump", &args.dump),
        ("--reference", &args.reference),
        ("--candidate", &args.candidate),
    ];
    if let Some(shots) = &args.per_shot {
        inputs.push(("--per-shot", shots));
    }
    for (flag, input) in inputs {
        if input.canonicalize().is_ok_and(|path| path == csv_target) {
            anyhow::bail!(
                "--frames-csv {} is the {flag} input; choose another path",
                csv.display()
            );
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let displays = check_args(&args)?;
    lp::check_version()?;
    let mut inputs = read_inputs(&args)?;
    let limits = report::limits(&lp::version());
    report::print_lines(&limits);
    report::print_lines(&inputs.describe);

    let mut lanes: Vec<Lane> = args
        .curves
        .iter()
        .flat_map(|&curve| {
            displays.iter().map(move |&display| Lane {
                curve,
                display,
                reference: None,
                candidate: None,
                previous: None,
                run: Summary::default(),
                shot: Summary::default(),
                shots: Vec::new(),
            })
        })
        .collect();
    let mut csv = match &args.frames_csv {
        Some(path) => Some(FramesCsv::create(path, &limits, &inputs.describe)?),
        None => None,
    };
    simulate(&args, &mut inputs, &mut lanes, csv.as_mut())?;
    if let Some(csv) = csv {
        csv.finish()?;
    }
    for lane in &lanes {
        report::print_summary(lane.curve, &lane.display, &lane.run, args.step_threshold);
        if inputs.shots.is_some() {
            report::print_shots(&lane.shots);
        }
    }
    Ok(())
}

fn simulate(
    args: &Args,
    inputs: &mut Inputs,
    lanes: &mut [Lane],
    mut csv: Option<&mut FramesCsv>,
) -> Result<()> {
    let leading = inputs.dump.header.leading_skipped;
    let shot_of = shot_index(inputs.shots.as_deref(), inputs.reference.len());
    let mut counts = [0u32; BINS];
    let mut frame = 0usize;
    while inputs.dump.next_frame(&mut counts)? {
        let (reference, candidate) = (inputs.reference[frame], inputs.candidate[frame]);
        let shot_start = frame > 0 && shot_of[frame] != shot_of[frame - 1];
        let shot_end = frame + 1 == shot_of.len() || shot_of[frame + 1] != shot_of[frame];
        for lane in lanes.iter_mut() {
            let result = compare_frame(
                &counts,
                response(&mut lane.reference, lane.curve, &lane.display, reference)
                    .with_context(|| format!("frame {frame}, reference"))?,
                response(&mut lane.candidate, lane.curve, &lane.display, candidate)
                    .with_context(|| format!("frame {frame}, candidate"))?,
            );
            lane.run.add(&result);
            lane.shot.add(&result);
            // With a shot list, pairs across a shot start are left out.
            let previous = lane
                .previous
                .as_ref()
                .filter(|_| !(shot_start && inputs.shots.is_some()));
            if let Some(previous) = previous {
                lane.run.steps.add(previous, &result, args.step_threshold);
                lane.shot.steps.add(previous, &result, args.step_threshold);
            }
            if let Some(csv) = csv.as_deref_mut() {
                csv.row(
                    frame,
                    leading,
                    lane,
                    [&reference, &candidate],
                    previous,
                    &result,
                )?;
            }
            lane.previous = Some(result);
            if shot_end && inputs.shots.is_some() {
                let first = frame + 1 - lane.shot.frames;
                lane.shots.push(ShotLine::new(first..frame + 1, &lane.shot));
                lane.shot = Summary::default();
            }
        }
        frame += 1;
    }
    ensure!(
        frame == inputs.reference.len(),
        "the dump ended after {frame} of {} frames",
        inputs.reference.len()
    );
    Ok(())
}

/// The shot of each measured frame (all 0 without a shot list).
fn shot_index(shots: Option<&[Range<usize>]>, frames: usize) -> Vec<usize> {
    let mut index = vec![0; frames];
    for (shot, range) in shots.unwrap_or_default().iter().enumerate() {
        for slot in &mut index[range.clone()] {
            *slot = shot;
        }
    }
    index
}
