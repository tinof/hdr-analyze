//! Compare hdr_analyzer_mvp measurements against reference Dolby Vision L1 metadata.
//!
//! Reference input is a CSV with header `frame,min_pq,max_pq,avg_pq` where PQ values
//! are 12-bit codes (0..4095), e.g. extracted from `dovi_tool export -d all=rpu.json`.
//!
//! Definitional caveats (reported, never silently corrected):
//! 1. Direct peaks may be max-RGB or Y-luma (`--peak-domain`); DV L1 max is max-RGB derived.
//! 2. For Profile 7 FEL sources, reference L1 describes the composed BL+EL picture,
//!    while measurements taken on the BL alone see a 10-bit subset of that signal.

use anyhow::{bail, Context, Result};
use clap::Parser;
use madvr_parse::MadVRMeasurements;
use serde::Deserialize;
use std::ffi::OsString;
use std::fs;
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

    /// Reference L1 CSV: frame,min_pq,max_pq,avg_pq (12-bit PQ codes)
    #[arg(long)]
    reference: PathBuf,

    /// Analyzer L1 JSON sidecar. Defaults to <ours>.l1.json.
    #[arg(long)]
    sidecar: Option<PathBuf>,

    /// Optional reference scene-cut list (one start frame per line, dovi_tool `scenes` export)
    #[arg(long)]
    scenes: Option<PathBuf>,

    /// Optional per-frame delta dump as CSV
    #[arg(long)]
    csv: Option<PathBuf>,
}

struct RefL1 {
    frame: usize,
    min_pq: u16,
    max_pq: u16,
    avg_pq: u16,
}

#[derive(Deserialize)]
struct L1Sidecar {
    version: u32,
    min_percentile: f64,
    frames: SidecarFrames,
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
    if sidecar.version != 1 {
        bail!(
            "unsupported L1 sidecar version {} in {}",
            sidecar.version,
            path.display()
        );
    }
    Ok(Some(sidecar))
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

fn print_metric(name: &str, ref_codes: &[f64], our_codes: &[f64]) {
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
    println!("  worst per-frame difference in nits: {worst_nits:.1}");
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
    let reference = parse_reference(&args.reference)?;

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
        args.reference.display(),
        reference.len()
    );
    println!("\nCaveats: (1) compare DV L1 max against max-RGB direct-peak output;");
    println!("Y-luma output underreads saturated highlights; (2) for P7 FEL sources the reference");
    println!("describes the composed BL+EL picture while ours sees the BL only.");

    if ours.frames.len() != reference.len() {
        bail!(
            "frame count mismatch: ours {} vs reference {}",
            ours.frames.len(),
            reference.len()
        );
    }
    if let Some(sidecar) = &sidecar {
        for (name, count) in [
            ("min", sidecar.frames.min_pq_12bit.len()),
            ("luma average", sidecar.frames.avg_luma_pq_12bit.len()),
            ("max-RGB average", sidecar.frames.avg_max_rgb_pq_12bit.len()),
        ] {
            if count != reference.len() {
                bail!(
                    "{name} sidecar frame count mismatch: sidecar {count} vs reference {}",
                    reference.len()
                );
            }
        }

        let ref_min: Vec<f64> = reference.iter().map(|row| row.min_pq as f64).collect();
        let our_min: Vec<f64> = sidecar
            .frames
            .min_pq_12bit
            .iter()
            .map(|code| f64::from(*code))
            .collect();
        print_metric("Minimum (robust active-area min_pq)", &ref_min, &our_min);
    } else {
        println!("Minimum: unavailable without an L1 sidecar.");
    }

    let ref_max: Vec<f64> = reference.iter().map(|r| r.max_pq as f64).collect();
    let our_max: Vec<f64> = ours
        .frames
        .iter()
        .map(|f| f.peak_pq_2020 * 4095.0)
        .collect();
    print_metric("Peak (L1 max_pq)", &ref_max, &our_max);

    let ref_avg: Vec<f64> = reference.iter().map(|r| r.avg_pq as f64).collect();
    if let Some(sidecar) = &sidecar {
        let our_avg_luma: Vec<f64> = sidecar
            .frames
            .avg_luma_pq_12bit
            .iter()
            .map(|code| f64::from(*code))
            .collect();
        print_metric("Average (Y-luma mean)", &ref_avg, &our_avg_luma);
        let our_avg_max_rgb: Vec<f64> = sidecar
            .frames
            .avg_max_rgb_pq_12bit
            .iter()
            .map(|code| f64::from(*code))
            .collect();
        print_metric("Average (max-RGB mean)", &ref_avg, &our_avg_max_rgb);
    } else {
        let our_avg: Vec<f64> = ours
            .frames
            .iter()
            .map(|frame| frame.avg_pq * 4095.0)
            .collect();
        print_metric("Average (legacy embedded .bin avg_pq)", &ref_avg, &our_avg);
        println!("Max-RGB average: unavailable without an L1 sidecar.");
    }

    if let Some(scenes_path) = &args.scenes {
        let text = fs::read_to_string(scenes_path)
            .with_context(|| format!("reading scenes {}", scenes_path.display()))?;
        let ref_cuts: Vec<i64> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim().parse().context("scene frame"))
            .collect::<Result<_>>()?;
        let our_cuts: Vec<i64> = ours.scenes.iter().map(|s| s.start as i64).collect();
        let matched = ref_cuts
            .iter()
            .filter(|r| our_cuts.iter().any(|o| (o - **r).abs() <= 1))
            .count();
        println!("\nScene cuts (±1 frame tolerance):");
        println!("  reference: {}   ours: {}", ref_cuts.len(), our_cuts.len());
        println!(
            "  reference cuts matched by ours: {}/{} ({:.0}%)",
            matched,
            ref_cuts.len(),
            100.0 * matched as f64 / ref_cuts.len().max(1) as f64
        );
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

    Ok(())
}
