//! Compare hdr_analyzer_mvp measurements against reference Dolby Vision L1 metadata.
//!
//! Reference input is a CSV with header `frame,min_pq,max_pq,avg_pq` where PQ values
//! are 12-bit codes (0..4095; `max_pq` may carry decimals), e.g. extracted from `dovi_tool export -d all=rpu.json`
//! (dovi_tool 2.3.3+ `export --levels level1` writes a much smaller per-frame L1 CSV;
//! check its header against the one above before use).
//!
//! Definitional caveats (reported, never silently corrected):
//! 1. Direct peaks may be max-RGB or Y-luma (`--peak-domain`); DV L1 max is max-RGB derived.
//! 2. For Profile 7 FEL sources, reference L1 describes the composed BL+EL picture,
//!    while measurements taken on the BL alone see a 10-bit subset of that signal.
//!
//! Without limits the tool only reports. The `--max-*` options turn it into a gate: any
//! breach is listed and the exit status is nonzero. `--export-reference` writes an analyzer
//! run as a reference CSV, for regression references and CPU/GPU comparisons.
//!
//! Frame coordinates: an open-GOP cut that starts at a CRA has leading (RASL) pictures no
//! decoder outputs. Sidecar v5 records them (`source.leading_skipped_frames`): our measured
//! frame `i` is stream frame `i + leading`. The reference's frame labels say which frames it
//! describes. A reference over the whole stream (labels from 0, one row per stream picture, as
//! Dolby Vision RPU exports are) has its rows for the leading pictures skipped; nothing is
//! measured there, so they are not scored. `--export-reference` labels rows with stream frames,
//! so an export reads back the same way. A reference labelled from 0 with one row per decoded
//! frame (exports from before this) is read in decoded-frame coordinates, and then refuses shot
//! and cut lists, which cannot be told apart from stream lists. `--scenes` and `--per-shot`
//! lists are in the reference's coordinates. Any other count difference is an error.

use anyhow::{bail, Context, Result};
use clap::Parser;
use madvr_parse::MadVRMeasurements;
use serde::Deserialize;
use std::ffi::OsString;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

const ST2084_Y_MAX: f64 = 10000.0;
const ST2084_M1: f64 = 2610.0 / 16384.0;
const ST2084_M2: f64 = (2523.0 / 4096.0) * 128.0;
const ST2084_C1: f64 = 3424.0 / 4096.0;
const ST2084_C2: f64 = (2413.0 / 4096.0) * 32.0;
const ST2084_C3: f64 = (2392.0 / 4096.0) * 32.0;

fn pq_to_nits(pq: f64) -> f64 {
    if pq <= 0.0 {
        return 0.0;
    }
    let y = ((pq.powf(1.0 / ST2084_M2) - ST2084_C1).max(0.0)
        / (ST2084_C2 - ST2084_C3 * pq.powf(1.0 / ST2084_M2)))
    .powf(1.0 / ST2084_M1);
    y * ST2084_Y_MAX
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Our measurement .bin file (madVR format, from hdr_analyzer_mvp)
    #[arg(long)]
    ours: PathBuf,

    /// Reference L1 CSV: frame,min_pq,max_pq,avg_pq (12-bit PQ codes). Frame labels count up by
    /// one; they cover the whole stream (rows for undecodable leading pictures are skipped) or
    /// exactly our measured frames.
    #[arg(long, required_unless_present = "export_reference")]
    reference: Option<PathBuf>,

    /// Write our run as a reference CSV (minimum, peak with three decimals, max-RGB average)
    /// and exit without scoring. Needs the L1 sidecar. Rows are labelled with stream frames, so
    /// they start at the sidecar's leading_skipped_frames.
    #[arg(long, value_name = "CSV", conflicts_with = "reference")]
    export_reference: Option<PathBuf>,

    /// Analyzer L1 JSON sidecar. Defaults to <ours>.l1.json.
    #[arg(long)]
    sidecar: Option<PathBuf>,

    /// Optional reference scene-cut list (one start frame per line, dovi_tool `scenes` export),
    /// in the reference's frame coordinates. A trailing sentinel equal to the frame count is
    /// dropped; a cut beyond it is an error.
    #[arg(long)]
    scenes: Option<PathBuf>,

    /// Optional shotlist for per-shot aggregation: one 0-based shot-start frame per line,
    /// optionally ending with a sentinel line equal to the total frame count, in the
    /// reference's frame coordinates. Both series are aggregated per shot (peak = max,
    /// average = mean, minimum = min) before scoring.
    #[arg(long, value_name = "SHOTLIST")]
    per_shot: Option<PathBuf>,

    /// Optional per-frame delta dump as CSV
    #[arg(long)]
    csv: Option<PathBuf>,

    /// Fail when |bias| of the peak exceeds this many 12-bit PQ codes. All limits apply to the
    /// series that is scored: per shot with --per-shot, per frame otherwise.
    #[arg(long, value_name = "CODES")]
    max_peak_bias: Option<f64>,

    /// Fail when the largest absolute peak error exceeds this many codes.
    #[arg(long, value_name = "CODES")]
    max_peak_error: Option<f64>,

    /// Fail when |bias| of the minimum exceeds this many codes. Needs the L1 sidecar.
    #[arg(long, value_name = "CODES")]
    max_min_bias: Option<f64>,

    /// Fail when the largest absolute minimum error exceeds this many codes. Needs the L1 sidecar.
    #[arg(long, value_name = "CODES")]
    max_min_error: Option<f64>,

    /// Fail when |bias| of the max-RGB average (the Dolby Vision L1 average) exceeds this many
    /// codes. The Y-luma average is reported but never gated. Needs the L1 sidecar.
    #[arg(long, value_name = "CODES")]
    max_avg_bias: Option<f64>,

    /// Fail when the largest absolute max-RGB average error exceeds this many codes. Needs the
    /// L1 sidecar.
    #[arg(long, value_name = "CODES")]
    max_avg_error: Option<f64>,

    /// Fail when more than this many scene cuts differ from --scenes (reference cuts we miss
    /// plus cuts of ours the reference lacks, ±1 frame).
    #[arg(long, value_name = "COUNT", requires = "scenes")]
    max_scene_mismatches: Option<usize>,
}

/// Signed bias and largest absolute error of one scored metric, in 12-bit PQ codes.
#[derive(Clone, Copy, Debug)]
struct MetricSummary {
    bias: f64,
    max_error: f64,
}

/// Describe every limit the metric breaks. A missing limit is not checked.
fn limit_breaches(
    name: &str,
    summary: MetricSummary,
    max_bias: Option<f64>,
    max_error: Option<f64>,
) -> Vec<String> {
    let mut breaches = Vec::new();
    if let Some(limit) = max_bias {
        if summary.bias.abs() > limit || summary.bias.is_nan() {
            breaches.push(format!(
                "{name}: bias {:+.2} codes exceeds the limit of ±{limit}",
                summary.bias
            ));
        }
    }
    if let Some(limit) = max_error {
        if summary.max_error > limit || summary.max_error.is_nan() {
            breaches.push(format!(
                "{name}: largest error {:.2} codes exceeds the limit of {limit}",
                summary.max_error
            ));
        }
    }
    breaches
}

/// Count cuts with no partner within ±1 frame: (reference cuts we miss, cuts only we have).
/// A cut is the partner of at most one cut on the other side.
fn scene_mismatches(reference: &[i64], ours: &[i64]) -> (usize, usize) {
    let mut reference = reference.to_vec();
    let mut ours = ours.to_vec();
    reference.sort_unstable();
    ours.sort_unstable();

    // Both lists are sorted, so pairing the earliest compatible cuts is a maximum matching.
    let (mut r, mut o, mut matched) = (0, 0, 0);
    while r < reference.len() && o < ours.len() {
        if (reference[r] - ours[o]).abs() <= 1 {
            matched += 1;
            r += 1;
            o += 1;
        } else if reference[r] < ours[o] {
            r += 1;
        } else {
            o += 1;
        }
    }
    (reference.len() - matched, ours.len() - matched)
}

struct RefL1 {
    /// 1-based line in the CSV, for error messages.
    line: usize,
    frame: usize,
    min_pq: u16,
    /// Whole codes in Dolby Vision exports; `--export-reference` keeps three decimals.
    max_pq: f64,
    avg_pq: u16,
}

/// First sidecar version that records the stream's picture count and leading pictures.
const STREAM_FRAMES_VERSION: u32 = 5;

#[derive(Deserialize)]
struct L1Sidecar {
    version: u32,
    min_percentile: f64,
    #[serde(default)]
    source: Option<SidecarSource>,
    frames: SidecarFrames,
}

#[derive(Deserialize)]
struct SidecarSource {
    /// Coded pictures in the video stream (version 5+).
    #[serde(default)]
    stream_frames: Option<u64>,
    /// Undecodable leading pictures before the first measured frame (version 5+).
    #[serde(default)]
    leading_skipped_frames: Option<u64>,
}

#[derive(Deserialize)]
struct SidecarFrames {
    min_pq_12bit: Vec<u16>,
    avg_luma_pq_12bit: Vec<u16>,
    avg_max_rgb_pq_12bit: Vec<u16>,
}

fn default_sidecar_path(ours: &Path) -> PathBuf {
    let mut path: OsString = ours.as_os_str().to_owned();
    path.push(".l1.json");
    PathBuf::from(path)
}

fn read_sidecar(path: &Path, required: bool) -> Result<Option<L1Sidecar>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("reading sidecar {}", path.display()));
        }
    };
    let sidecar: L1Sidecar = serde_json::from_reader(file)
        .with_context(|| format!("parsing sidecar {}", path.display()))?;
    // Versions 2 and 3 only add provenance (full-resolution crop, luminance mapping); version 4
    // stores unfiltered averages; version 5 records the stream's picture count and undecodable
    // leading pictures (`leading_pictures`). The L1 data layout, indexed by measured (decoded)
    // frame, is unchanged.
    if !matches!(sidecar.version, 1..=5) {
        bail!(
            "unsupported L1 sidecar version {} in {}",
            sidecar.version,
            path.display()
        );
    }
    Ok(Some(sidecar))
}

/// Check the sidecar against the measurement file's `decoded` frames and return the
/// undecodable leading pictures in front of them: our frame `i` is stream frame
/// `i + leading`. Without a sidecar, or before version 5, nothing records them and this is 0.
fn leading_pictures(sidecar: Option<&L1Sidecar>, decoded: usize) -> Result<usize> {
    if decoded == 0 {
        bail!("the measurement file has no frames");
    }
    let Some(sidecar) = sidecar else {
        return Ok(0);
    };
    let frames = &sidecar.frames;
    for (name, count) in [
        ("min", frames.min_pq_12bit.len()),
        ("luma average", frames.avg_luma_pq_12bit.len()),
        ("max-RGB average", frames.avg_max_rgb_pq_12bit.len()),
    ] {
        if count != decoded {
            bail!(
                "{name} sidecar frame count mismatch: sidecar {count} vs measurement file {decoded}"
            );
        }
    }
    if sidecar.version < STREAM_FRAMES_VERSION {
        return Ok(0);
    }
    let source = sidecar.source.as_ref();
    let (Some(stream), Some(leading)) = (
        source.and_then(|source| source.stream_frames),
        source.and_then(|source| source.leading_skipped_frames),
    ) else {
        bail!(
            "sidecar v{} does not record the stream's picture count",
            sidecar.version
        );
    };
    if decoded as u64 + leading != stream {
        bail!(
            "sidecar does not describe this measurement file: {decoded} measured frames after \
             {leading} leading pictures, but the stream has {stream}"
        );
    }
    usize::try_from(leading).context("leading_skipped_frames does not fit in memory")
}

/// Which frames the reference's labels count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Coordinates {
    /// Stream pictures in presentation order, the leading pictures included.
    Stream,
    /// Our decoded frames, from 0 (references exported before stream labels).
    Decoded,
}

/// How the reference rows line up with our frames.
#[derive(Debug, PartialEq, Eq)]
struct Alignment {
    /// Reference rows in front of our first frame (rows for undecodable leading pictures).
    skip: usize,
    /// Subtract from a reference frame number to get our frame number.
    offset: usize,
    coordinates: Coordinates,
}

/// Line the reference rows up with our `decoded` frames after `leading` undecodable pictures.
/// Labels must count up by one. A reference in stream coordinates ends at the last stream
/// picture and starts at or before our first frame; one in decoded coordinates (only when
/// `leading > 0`; with no leading pictures both are the same) has one row per decoded frame,
/// labelled from 0. Any other layout is an error. `stream_recorded` says whether a v5 sidecar
/// was read, for the hint.
fn align_reference(
    rows: &[RefL1],
    decoded: usize,
    leading: usize,
    stream_recorded: bool,
) -> Result<Alignment> {
    let Some(first) = rows.first() else {
        bail!("reference has no frames");
    };
    for pair in rows.windows(2) {
        if pair[1].frame != pair[0].frame + 1 {
            bail!(
                "reference CSV line {}: frame {} follows frame {}; frame labels must count up by one",
                pair[1].line,
                pair[1].frame,
                pair[0].frame
            );
        }
    }
    let first_label = first.frame;
    let end_label = first_label + rows.len();
    let stream = decoded + leading;
    if first_label <= leading && end_label == stream {
        return Ok(Alignment {
            skip: leading - first_label,
            offset: leading,
            coordinates: Coordinates::Stream,
        });
    }
    if leading > 0 && first_label == 0 && rows.len() == decoded {
        return Ok(Alignment {
            skip: 0,
            offset: 0,
            coordinates: Coordinates::Decoded,
        });
    }
    let mut message = format!(
        "frame count mismatch: ours {decoded} decoded + {leading} leading = {stream} stream \
         frames vs reference {} rows labelled {first_label}..={}",
        rows.len(),
        end_label - 1
    );
    if rows.len() > decoded && !stream_recorded {
        message.push_str(
            "; if the cut starts with undecodable leading pictures, analyze it with an \
             hdr_analyzer_mvp that writes sidecar v5, which records them",
        );
    }
    bail!(message)
}

/// Shift shot ranges from reference coordinates to our frames: subtract `offset` and clip at 0.
/// A shot that lies entirely in the leading pictures is dropped; the count is returned. The
/// input covers `0..decoded + offset` (`parse_shotlist_text`), so the result covers
/// `0..decoded` and keeps at least the last shot.
fn to_decoded_ranges(ranges: &[Range<usize>], offset: usize) -> (Vec<Range<usize>>, usize) {
    let mut kept = Vec::with_capacity(ranges.len());
    let mut dropped = 0;
    for range in ranges {
        let start = range.start.saturating_sub(offset);
        let end = range.end.saturating_sub(offset);
        if end > start {
            kept.push(start..end);
        } else {
            dropped += 1;
        }
    }
    (kept, dropped)
}

/// Shift reference scene cuts to our frames. Cuts are in reference coordinates, which end at
/// `decoded + offset`: that value is the dovi_tool sentinel and is dropped, and a cut beyond it
/// is an error, so a list for another cut cannot pass by being filtered. 0 starts the first
/// scene and is not a cut. A cut at or before `offset` falls in the leading pictures or on our
/// first frame, where we cannot detect one: it is dropped and counted. The rest are returned
/// as our frame numbers, all in `1..decoded`.
fn to_decoded_cuts(cuts: &[i64], offset: usize, decoded: usize) -> Result<(Vec<i64>, usize)> {
    let offset = offset as i64;
    let end = decoded as i64 + offset;
    let mut kept = Vec::with_capacity(cuts.len());
    let mut dropped = 0;
    for &cut in cuts {
        if cut < 0 || cut > end {
            bail!("scene cut {cut} is outside the reference's frames 0..={end}");
        }
        if cut == 0 || cut == end {
            continue;
        }
        if cut <= offset {
            dropped += 1;
        } else {
            kept.push(cut - offset);
        }
    }
    Ok((kept, dropped))
}

fn parse_reference(path: &PathBuf) -> Result<Vec<RefL1>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading reference CSV {}", path.display()))?;
    let mut rows = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 && line.starts_with("frame") {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 4 {
            bail!(
                "reference CSV line {} has {} columns, expected 4",
                i + 1,
                cols.len()
            );
        }
        rows.push(RefL1 {
            line: i + 1,
            frame: cols[0].trim().parse().context("frame column")?,
            min_pq: cols[1].trim().parse().context("min_pq column")?,
            max_pq: cols[2].trim().parse().context("max_pq column")?,
            avg_pq: cols[3].trim().parse().context("avg_pq column")?,
        });
    }
    Ok(rows)
}

struct Stats {
    mean: f64,
    median: f64,
    p95: f64,
    max: f64,
}

/// How a per-frame series collapses to one value per shot.
#[derive(Clone, Copy)]
enum ShotAggregate {
    Min,
    Max,
    Mean,
}

/// Parse shotlist text into per-shot frame ranges covering `frame_count` frames.
///
/// Lines are 0-based shot-start frames, strictly increasing, starting at 0; a trailing
/// line equal to `frame_count` (dovi_tool scene-export sentinel) is accepted and dropped.
fn parse_shotlist_text(text: &str, frame_count: usize) -> Result<Vec<Range<usize>>> {
    let mut starts = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let start: usize = line
            .parse()
            .with_context(|| format!("shotlist line {}: expected a frame number", i + 1))?;
        starts.push(start);
    }
    if starts.last() == Some(&frame_count) {
        starts.pop();
    }
    if starts.is_empty() {
        bail!("shotlist contains no shot starts");
    }
    if starts[0] != 0 {
        bail!(
            "shotlist must start at frame 0 (first start is {})",
            starts[0]
        );
    }
    for pair in starts.windows(2) {
        if pair[1] <= pair[0] {
            bail!(
                "shotlist starts must be strictly increasing ({} followed by {})",
                pair[0],
                pair[1]
            );
        }
    }
    let last = starts[starts.len() - 1];
    if last >= frame_count {
        bail!("shotlist start {last} is outside the {frame_count}-frame sequence");
    }
    let mut ranges = Vec::with_capacity(starts.len());
    for (index, &start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(frame_count);
        ranges.push(start..end);
    }
    Ok(ranges)
}

fn parse_shotlist(path: &Path, frame_count: usize) -> Result<Vec<Range<usize>>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("reading shotlist {}", path.display()))?;
    parse_shotlist_text(&text, frame_count)
        .with_context(|| format!("validating shotlist {}", path.display()))
}

fn aggregate_shots(series: &[f64], shots: &[Range<usize>], mode: ShotAggregate) -> Vec<f64> {
    shots
        .iter()
        .map(|range| {
            let window = &series[range.clone()];
            match mode {
                ShotAggregate::Min => window.iter().copied().fold(f64::INFINITY, f64::min),
                ShotAggregate::Max => window.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                ShotAggregate::Mean => window.iter().sum::<f64>() / window.len() as f64,
            }
        })
        .collect()
}

/// Percentile/summary stats over signed deltas, computed on absolute values.
fn stats(deltas: &[f64]) -> Stats {
    let mut abs: Vec<f64> = deltas.iter().map(|d| d.abs()).collect();
    abs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = abs.len();
    let mean = abs.iter().sum::<f64>() / n as f64;
    let idx = |q: f64| abs[((n as f64 * q).floor() as usize).min(n - 1)];
    Stats {
        mean,
        median: idx(0.50),
        p95: idx(0.95),
        max: abs[n - 1],
    }
}

fn print_metric(name: &str, ref_codes: &[f64], our_codes: &[f64], unit: &str) -> MetricSummary {
    debug_assert_eq!(ref_codes.len(), our_codes.len(), "{name}: series lengths");
    let signed: Vec<f64> = ref_codes
        .iter()
        .zip(our_codes)
        .map(|(r, o)| o - r)
        .collect();
    let s = stats(&signed);
    let bias = signed.iter().sum::<f64>() / signed.len() as f64;
    // Worst-case nits difference evaluated at the actual code pair, not on the abstract delta.
    let worst_nits = ref_codes
        .iter()
        .zip(our_codes)
        .map(|(r, o)| (pq_to_nits(o / 4095.0) - pq_to_nits(r / 4095.0)).abs())
        .fold(0.0f64, f64::max);
    println!("\n{name} (12-bit PQ codes, ours - reference):");
    println!("  bias (signed mean): {bias:+.1}");
    println!(
        "  |error|: mean {:.1} / median {:.1} / p95 {:.1} / max {:.1}",
        s.mean, s.median, s.p95, s.max
    );
    println!("  worst per-{unit} difference in nits: {worst_nits:.1}");
    MetricSummary {
        bias,
        max_error: s.max,
    }
}

/// Score one metric, per-frame by default or per-shot when a shotlist is given.
fn score(
    name: &str,
    ref_codes: &[f64],
    our_codes: &[f64],
    shots: Option<&[Range<usize>]>,
    mode: ShotAggregate,
) -> MetricSummary {
    match shots {
        Some(shots) => {
            let ref_shots = aggregate_shots(ref_codes, shots, mode);
            let our_shots = aggregate_shots(our_codes, shots, mode);
            print_metric(
                &format!("{name} — per-shot ({} shots)", shots.len()),
                &ref_shots,
                &our_shots,
                "shot",
            )
        }
        None => print_metric(name, ref_codes, our_codes, "frame"),
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let data = fs::read(&args.ours).with_context(|| format!("reading {}", args.ours.display()))?;
    let ours = MadVRMeasurements::parse_measurements(&data)
        .map_err(|e| anyhow::anyhow!("parsing {}: {e}", args.ours.display()))?;
    let sidecar_required = args.sidecar.is_some();
    let sidecar_path = args
        .sidecar
        .clone()
        .unwrap_or_else(|| default_sidecar_path(&args.ours));
    let sidecar = read_sidecar(&sidecar_path, sidecar_required)?;
    let decoded = ours.frames.len();
    let leading = leading_pictures(sidecar.as_ref(), decoded)?;

    if let Some(path) = &args.export_reference {
        let Some(sidecar) = &sidecar else {
            bail!(
                "--export-reference needs the L1 sidecar ({} not found)",
                sidecar_path.display()
            );
        };
        return export_reference(path, &ours, sidecar, leading);
    }
    let sidecar_limits = [
        args.max_min_bias,
        args.max_min_error,
        args.max_avg_bias,
        args.max_avg_error,
    ];
    if sidecar.is_none() && sidecar_limits.iter().any(Option::is_some) {
        bail!(
            "--max-min-* and --max-avg-* need the L1 sidecar ({} not found)",
            sidecar_path.display()
        );
    }

    let reference_path = args.reference.as_ref().context("--reference is required")?;
    let rows = parse_reference(reference_path)?;
    if rows.is_empty() {
        bail!("reference CSV {} has no frames", reference_path.display());
    }
    let mut breaches = Vec::new();

    println!("=== l1_diff: analyzer output vs reference DV L1 ===");
    println!(
        "ours:      {} ({} frames)",
        args.ours.display(),
        ours.frames.len()
    );
    if let Some(sidecar) = &sidecar {
        println!("sidecar:   {}", sidecar_path.display());
        println!(
            "minimum:   P{} (configured lower percentile)",
            sidecar.min_percentile
        );
    } else {
        println!(
            "sidecar:   not found ({}); using legacy .bin average fallback",
            sidecar_path.display()
        );
    }
    println!(
        "reference: {} ({} frames)",
        reference_path.display(),
        rows.len()
    );
    println!("\nCaveats: (1) compare DV L1 max against max-RGB direct-peak output;");
    println!("Y-luma output underreads saturated highlights; (2) for P7 FEL sources the reference");
    println!("describes the composed BL+EL picture while ours sees the BL only.");

    let stream_recorded = sidecar
        .as_ref()
        .is_some_and(|sidecar| sidecar.version >= STREAM_FRAMES_VERSION);
    let alignment = align_reference(&rows, decoded, leading, stream_recorded)?;
    println!(
        "\nframes:    {decoded} decoded + {leading} leading = {} stream; reference in {} coordinates",
        decoded + leading,
        match alignment.coordinates {
            Coordinates::Stream => "stream",
            Coordinates::Decoded => "decoded-frame",
        }
    );
    if alignment.skip > 0 {
        println!(
            "           {} reference frames describe undecodable leading pictures and are not scored",
            alignment.skip
        );
    }
    if alignment.coordinates == Coordinates::Decoded {
        println!(
            "warning:   the reference has one row per decoded frame, labelled from 0; it is \
             read in decoded-frame coordinates, not shifted by the {leading} leading pictures"
        );
        if args.per_shot.is_some() || args.scenes.is_some() {
            bail!(
                "--per-shot and --scenes cannot be lined up with a reference in decoded-frame \
                 coordinates when the stream has {leading} leading pictures; re-export the \
                 reference with this l1_diff, which labels it with stream frames"
            );
        }
    }
    // Only the rows that describe our frames from here on.
    let reference = &rows[alignment.skip..];
    if reference.len() != decoded {
        bail!(
            "internal: {} aligned reference rows for {decoded} frames",
            reference.len()
        );
    }

    let shots = match &args.per_shot {
        Some(path) => {
            let ranges = parse_shotlist(path, decoded + alignment.offset)?;
            let (ranges, dropped) = to_decoded_ranges(&ranges, alignment.offset);
            if dropped > 0 {
                println!(
                    "           {dropped} shots lie in the leading pictures and are not scored"
                );
            }
            Some(ranges)
        }
        None => None,
    };
    let shots = shots.as_deref();

    if let Some(sidecar) = &sidecar {
        let ref_min: Vec<f64> = reference.iter().map(|row| row.min_pq as f64).collect();
        let our_min: Vec<f64> = sidecar
            .frames
            .min_pq_12bit
            .iter()
            .map(|code| f64::from(*code))
            .collect();
        let summary = score(
            "Minimum (robust active-area min_pq)",
            &ref_min,
            &our_min,
            shots,
            ShotAggregate::Min,
        );
        breaches.extend(limit_breaches(
            "minimum",
            summary,
            args.max_min_bias,
            args.max_min_error,
        ));
    } else {
        println!("Minimum: unavailable without an L1 sidecar.");
    }

    let ref_max: Vec<f64> = reference.iter().map(|r| r.max_pq).collect();
    let our_max: Vec<f64> = ours
        .frames
        .iter()
        .map(|f| f.peak_pq_2020 * 4095.0)
        .collect();
    let summary = score(
        "Peak (L1 max_pq)",
        &ref_max,
        &our_max,
        shots,
        ShotAggregate::Max,
    );
    breaches.extend(limit_breaches(
        "peak",
        summary,
        args.max_peak_bias,
        args.max_peak_error,
    ));

    let ref_avg: Vec<f64> = reference.iter().map(|r| r.avg_pq as f64).collect();
    if let Some(sidecar) = &sidecar {
        let our_avg_luma: Vec<f64> = sidecar
            .frames
            .avg_luma_pq_12bit
            .iter()
            .map(|code| f64::from(*code))
            .collect();
        score(
            "Average (Y-luma mean)",
            &ref_avg,
            &our_avg_luma,
            shots,
            ShotAggregate::Mean,
        );
        let our_avg_max_rgb: Vec<f64> = sidecar
            .frames
            .avg_max_rgb_pq_12bit
            .iter()
            .map(|code| f64::from(*code))
            .collect();
        let summary = score(
            "Average (max-RGB mean)",
            &ref_avg,
            &our_avg_max_rgb,
            shots,
            ShotAggregate::Mean,
        );
        breaches.extend(limit_breaches(
            "max-RGB average",
            summary,
            args.max_avg_bias,
            args.max_avg_error,
        ));
    } else {
        let our_avg: Vec<f64> = ours
            .frames
            .iter()
            .map(|frame| frame.avg_pq * 4095.0)
            .collect();
        score(
            "Average (legacy embedded .bin avg_pq)",
            &ref_avg,
            &our_avg,
            shots,
            ShotAggregate::Mean,
        );
        println!("Max-RGB average: unavailable without an L1 sidecar.");
    }

    if let Some(scenes_path) = &args.scenes {
        let text = fs::read_to_string(scenes_path)
            .with_context(|| format!("reading scenes {}", scenes_path.display()))?;
        let listed: Vec<i64> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim().parse().context("scene frame"))
            .collect::<Result<Vec<i64>>>()?;
        let (ref_cuts, dropped) = to_decoded_cuts(&listed, alignment.offset, decoded)
            .with_context(|| format!("validating scenes {}", scenes_path.display()))?;
        if dropped > 0 {
            println!(
                "\n{dropped} reference cuts fall at or before our first frame (leading pictures) and are not scored"
            );
        }
        // Frame 0 starts the first scene on both sides and is not a cut.
        let our_cuts: Vec<i64> = ours
            .scenes
            .iter()
            .map(|s| s.start as i64)
            .filter(|cut| *cut > 0)
            .collect();
        let (missed, extra) = scene_mismatches(&ref_cuts, &our_cuts);
        let matched = ref_cuts.len() - missed;
        if let Some(limit) = args.max_scene_mismatches {
            if missed + extra > limit {
                breaches.push(format!(
                    "scene cuts: {missed} reference cuts missed and {extra} extra cuts exceed the limit of {limit}"
                ));
            }
        }
        println!("\nScene cuts (±1 frame tolerance):");
        println!("  reference: {}   ours: {}", ref_cuts.len(), our_cuts.len());
        println!(
            "  reference cuts matched by ours: {}/{} ({:.0}%)",
            matched,
            ref_cuts.len(),
            100.0 * matched as f64 / ref_cuts.len().max(1) as f64
        );
        println!("  cuts of ours without a reference partner: {extra}");
    }

    if let Some(csv_path) = &args.csv {
        let out = if let Some(sidecar) = &sidecar {
            let mut out = String::from(
                "frame,ref_min_pq,our_min_pq,ref_max_pq,our_max_pq,ref_avg_pq,our_avg_luma_pq,our_avg_max_rgb_pq\n",
            );
            for (index, (reference_frame, our_frame)) in
                reference.iter().zip(&ours.frames).enumerate()
            {
                out.push_str(&format!(
                    "{},{},{},{},{:.1},{},{},{}\n",
                    reference_frame.frame,
                    reference_frame.min_pq,
                    sidecar.frames.min_pq_12bit[index],
                    reference_frame.max_pq,
                    our_frame.peak_pq_2020 * 4095.0,
                    reference_frame.avg_pq,
                    sidecar.frames.avg_luma_pq_12bit[index],
                    sidecar.frames.avg_max_rgb_pq_12bit[index],
                ));
            }
            out
        } else {
            let mut out = String::from("frame,ref_max_pq,our_max_pq,ref_avg_pq,our_avg_pq\n");
            for (reference_frame, our_frame) in reference.iter().zip(&ours.frames) {
                out.push_str(&format!(
                    "{},{},{:.1},{},{:.1}\n",
                    reference_frame.frame,
                    reference_frame.max_pq,
                    our_frame.peak_pq_2020 * 4095.0,
                    reference_frame.avg_pq,
                    our_frame.avg_pq * 4095.0,
                ));
            }
            out
        };
        fs::write(csv_path, out).with_context(|| format!("writing {}", csv_path.display()))?;
        println!("\nPer-frame deltas written to {}", csv_path.display());
    }

    if !breaches.is_empty() {
        println!("\nLimits exceeded:");
        for breach in &breaches {
            println!("  {breach}");
        }
        bail!("{} limit(s) exceeded", breaches.len());
    }

    Ok(())
}

/// Write our run in the reference CSV layout. The peak keeps three decimals, so scoring the
/// same run against its own export reports no error. Rows are labelled with stream frames
/// (`index + leading`), so the export reads back in stream coordinates and lines up with shot
/// and cut lists of the stream. `leading_pictures` has checked the sidecar's frame counts.
fn export_reference(
    path: &Path,
    ours: &MadVRMeasurements,
    sidecar: &L1Sidecar,
    leading: usize,
) -> Result<()> {
    let frames = &sidecar.frames;
    let mut out = String::from("frame,min_pq,max_pq,avg_pq\n");
    for (index, frame) in ours.frames.iter().enumerate() {
        out.push_str(&format!(
            "{},{},{:.3},{}\n",
            index + leading,
            frames.min_pq_12bit[index],
            frame.peak_pq_2020 * 4095.0,
            frames.avg_max_rgb_pq_12bit[index],
        ));
    }
    fs::write(path, out).with_context(|| format!("writing {}", path.display()))?;
    let labels = if leading > 0 {
        format!(", labelled from stream frame {leading}")
    } else {
        String::new()
    };
    println!(
        "Reference CSV written to {} ({} frames{labels})",
        path.display(),
        ours.frames.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shotlist_builds_ranges_and_drops_sentinel() {
        let ranges = parse_shotlist_text("0\n10\n25\n40\n", 40).unwrap();
        assert_eq!(ranges, vec![0..10, 10..25, 25..40]);
    }

    #[test]
    fn shotlist_last_shot_extends_to_frame_count_without_sentinel() {
        let ranges = parse_shotlist_text("0\n10\n25\n", 40).unwrap();
        assert_eq!(ranges, vec![0..10, 10..25, 25..40]);
    }

    #[test]
    fn shotlist_rejects_non_increasing_starts() {
        let error = parse_shotlist_text("0\n10\n10\n", 40).unwrap_err();
        assert!(error.to_string().contains("strictly increasing"));
    }

    #[test]
    fn shotlist_rejects_out_of_range_start() {
        let error = parse_shotlist_text("0\n50\n", 40).unwrap_err();
        assert!(error.to_string().contains("outside"));
    }

    #[test]
    fn shotlist_rejects_nonzero_first_start() {
        let error = parse_shotlist_text("5\n10\n", 40).unwrap_err();
        assert!(error.to_string().contains("start at frame 0"));
    }

    #[test]
    fn shotlist_rejects_empty_input() {
        assert!(parse_shotlist_text("\n\n", 40).is_err());
    }

    #[test]
    fn aggregation_applies_min_max_mean_per_shot() {
        let series = [1.0, 5.0, 3.0, 8.0, 2.0, 4.0];
        let shots = vec![0..3, 3..6];
        assert_eq!(
            aggregate_shots(&series, &shots, ShotAggregate::Max),
            vec![5.0, 8.0]
        );
        assert_eq!(
            aggregate_shots(&series, &shots, ShotAggregate::Min),
            vec![1.0, 2.0]
        );
        assert_eq!(
            aggregate_shots(&series, &shots, ShotAggregate::Mean),
            vec![3.0, 14.0 / 3.0]
        );
    }

    #[test]
    fn limits_pass_at_the_boundary_and_without_limits() {
        let summary = MetricSummary {
            bias: -0.5,
            max_error: 1.0,
        };
        assert!(limit_breaches("peak", summary, Some(0.5), Some(1.0)).is_empty());
        assert!(limit_breaches("peak", summary, None, None).is_empty());
    }

    #[test]
    fn bias_limit_applies_to_the_absolute_value() {
        for bias in [0.6, -0.6] {
            let summary = MetricSummary {
                bias,
                max_error: 0.0,
            };
            let breaches = limit_breaches("minimum", summary, Some(0.5), Some(1.0));
            assert_eq!(breaches.len(), 1, "bias {bias}");
            assert!(breaches[0].starts_with("minimum: bias"));
        }
    }

    #[test]
    fn error_limit_and_nan_are_breaches() {
        let summary = MetricSummary {
            bias: 0.0,
            max_error: 1.5,
        };
        let breaches = limit_breaches("max-RGB average", summary, Some(0.5), Some(1.0));
        assert_eq!(breaches.len(), 1);
        assert!(breaches[0].starts_with("max-RGB average: largest error"));

        let nan = MetricSummary {
            bias: f64::NAN,
            max_error: f64::NAN,
        };
        assert_eq!(limit_breaches("peak", nan, Some(0.5), Some(1.0)).len(), 2);
    }

    #[test]
    fn scene_mismatches_counts_missed_and_extra_cuts() {
        assert_eq!(scene_mismatches(&[0, 24, 48], &[0, 25, 48]), (0, 0));
        assert_eq!(scene_mismatches(&[0, 24, 48], &[0, 48, 60]), (1, 1));
        assert_eq!(scene_mismatches(&[0, 24], &[0, 24, 30, 40]), (0, 2));
        // One reference cut cannot absorb two of ours, nor the other way round.
        assert_eq!(scene_mismatches(&[24], &[23, 25]), (0, 1));
        assert_eq!(scene_mismatches(&[23, 25], &[24]), (1, 0));
        assert_eq!(scene_mismatches(&[10, 11], &[11, 12]), (0, 0));
    }

    fn rows(labels: std::ops::Range<usize>) -> Vec<RefL1> {
        labels
            .enumerate()
            .map(|(index, frame)| RefL1 {
                line: index + 2,
                frame,
                min_pq: 0,
                max_pq: 0.0,
                avg_pq: 0,
            })
            .collect()
    }

    fn sidecar(json: serde_json::Value) -> L1Sidecar {
        serde_json::from_value(json).unwrap()
    }

    fn sidecar_v(version: u32, decoded: usize, source: serde_json::Value) -> L1Sidecar {
        sidecar(serde_json::json!({
            "version": version,
            "min_percentile": 0.01,
            "source": source,
            "frames": {
                "min_pq_12bit": vec![0; decoded],
                "avg_luma_pq_12bit": vec![0; decoded],
                "avg_max_rgb_pq_12bit": vec![0; decoded],
            },
        }))
    }

    #[test]
    fn leading_pictures_come_from_a_consistent_v5_sidecar() {
        let source = serde_json::json!({ "stream_frames": 12, "leading_skipped_frames": 2 });
        assert_eq!(
            leading_pictures(Some(&sidecar_v(5, 10, source)), 10).unwrap(),
            2
        );
        assert_eq!(leading_pictures(None, 10).unwrap(), 0);
        // Before version 5 nothing records them, even if a field is present.
        let source = serde_json::json!({ "stream_frames": 12, "leading_skipped_frames": 2 });
        assert_eq!(
            leading_pictures(Some(&sidecar_v(4, 10, source)), 10).unwrap(),
            0
        );
    }

    #[test]
    fn leading_pictures_refuse_an_inconsistent_sidecar() {
        let error = leading_pictures(Some(&sidecar_v(5, 10, serde_json::json!({}))), 10)
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not record"), "{error}");

        let source = serde_json::json!({ "stream_frames": 13, "leading_skipped_frames": 2 });
        let error = leading_pictures(Some(&sidecar_v(5, 10, source)), 10)
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not describe"), "{error}");

        let source = serde_json::json!({ "stream_frames": 12, "leading_skipped_frames": 2 });
        let error = leading_pictures(Some(&sidecar_v(5, 10, source)), 9)
            .unwrap_err()
            .to_string();
        assert!(error.contains("sidecar frame count mismatch"), "{error}");

        assert!(leading_pictures(None, 0).is_err());
    }

    #[test]
    fn a_stream_reference_skips_the_leading_pictures() {
        assert_eq!(
            align_reference(&rows(0..12), 10, 2, true).unwrap(),
            Alignment {
                skip: 2,
                offset: 2,
                coordinates: Coordinates::Stream
            }
        );
        // Our own export: one row per decoded frame, labelled with stream frames.
        assert_eq!(
            align_reference(&rows(2..12), 10, 2, true).unwrap(),
            Alignment {
                skip: 0,
                offset: 2,
                coordinates: Coordinates::Stream
            }
        );
    }

    #[test]
    fn without_leading_pictures_alignment_is_unchanged() {
        assert_eq!(
            align_reference(&rows(0..10), 10, 0, true).unwrap(),
            Alignment {
                skip: 0,
                offset: 0,
                coordinates: Coordinates::Stream
            }
        );
        assert!(align_reference(&rows(0..11), 10, 0, true).is_err());
        assert!(align_reference(&rows(0..9), 10, 0, true).is_err());
    }

    #[test]
    fn decoded_labels_from_zero_keep_decoded_coordinates() {
        assert_eq!(
            align_reference(&rows(0..10), 10, 2, true).unwrap(),
            Alignment {
                skip: 0,
                offset: 0,
                coordinates: Coordinates::Decoded
            }
        );
    }

    #[test]
    fn other_layouts_are_refused() {
        // One row too many, a stream reference that misses its last frame, a label start that
        // is neither 0 nor inside the leading pictures.
        for labels in [0..13, 0..11, 3..12, 1..11] {
            let error = align_reference(&rows(labels.clone()), 10, 2, true)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("frame count mismatch"),
                "{labels:?}: {error}"
            );
            assert!(!error.contains("sidecar v5"), "{labels:?}: {error}");
        }
        let error = align_reference(&rows(0..12), 10, 0, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("sidecar v5"), "{error}");
    }

    #[test]
    fn labels_must_count_up_by_one() {
        let mut gap = rows(0..12);
        gap[5].frame = 6;
        let error = align_reference(&gap, 10, 2, true).unwrap_err().to_string();
        assert!(error.contains("line 7"), "{error}");

        let mut duplicate = rows(0..10);
        duplicate[3].frame = 2;
        assert!(align_reference(&duplicate, 10, 0, true).is_err());
    }

    #[test]
    fn shots_move_to_our_frames() {
        // Joker-like: two leading pictures, the stream shot list starts at 0.
        let stream = parse_shotlist_text("0\n151\n301\n1448\n", 1448).unwrap();
        let (shots, dropped) = to_decoded_ranges(&stream, 2);
        assert_eq!(shots, vec![0..149, 149..299, 299..1446]);
        assert_eq!(dropped, 0);

        // A shot entirely in the leading pictures is dropped; the rest still covers 0..n.
        let stream = parse_shotlist_text("0\n2\n5\n", 12).unwrap();
        let (shots, dropped) = to_decoded_ranges(&stream, 2);
        assert_eq!(shots, vec![0..3, 3..10]);
        assert_eq!(dropped, 1);
        let stream = parse_shotlist_text("0\n1\n5\n", 12).unwrap();
        assert_eq!(to_decoded_ranges(&stream, 2), (vec![0..3, 3..10], 1));
    }

    #[test]
    fn cuts_move_to_our_frames() {
        // Leading = 2, decoded = 10, stream end 12: L-1 and L are dropped, L+1 becomes 1, the
        // sentinel 12 is dropped.
        assert_eq!(
            to_decoded_cuts(&[0, 1, 2, 3, 7, 12], 2, 10).unwrap(),
            (vec![1, 5], 2)
        );
        assert_eq!(to_decoded_cuts(&[0, 4, 10], 0, 10).unwrap(), (vec![4], 0));
    }

    #[test]
    fn cuts_beyond_the_reference_are_refused() {
        assert!(to_decoded_cuts(&[4, 13], 2, 10).is_err());
        assert!(to_decoded_cuts(&[4, 11], 0, 10).is_err());
        assert!(to_decoded_cuts(&[-1], 0, 10).is_err());
    }

    #[test]
    fn aggregation_is_exact_for_per_shot_expanded_reference() {
        // A per-frame series expanded from per-shot XML is constant within each shot;
        // max and mean must both recover the original per-shot value exactly.
        let expanded = [7.0, 7.0, 7.0, 2.0, 2.0];
        let shots = vec![0..3, 3..5];
        assert_eq!(
            aggregate_shots(&expanded, &shots, ShotAggregate::Max),
            vec![7.0, 2.0]
        );
        assert_eq!(
            aggregate_shots(&expanded, &shots, ShotAggregate::Mean),
            vec![7.0, 2.0]
        );
    }
}
