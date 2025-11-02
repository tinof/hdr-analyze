use anyhow::Result;
use clap::Parser;
use itertools::izip;
use madvr_parse::MadVRMeasurements;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the directory with baseline .bin files
    #[arg(short, long)]
    baseline: PathBuf,

    /// Path to the directory with new .bin files to compare
    #[arg(short, long)]
    current: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!(
        "Comparing baseline measurements in '{}' with current measurements in '{}'",
        args.baseline.display(),
        args.current.display()
    );

    let baseline_files = find_bin_files(&args.baseline)?;
    let current_files = find_bin_files(&args.current)?;

    for baseline_path in &baseline_files {
        let file_name = baseline_path.file_name().unwrap();
        if let Some(current_path) = current_files
            .iter()
            .find(|p| p.file_name() == Some(file_name))
        {
            println!("\n--- Comparing {} ---", file_name.to_string_lossy());
            compare_files(baseline_path, current_path)?;
        } else {
            println!(
                "\n--- Skipping {} (not found in current directory) ---",
                file_name.to_string_lossy()
            );
        }
    }

    Ok(())
}

fn find_bin_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().map_or(false, |e| e == "bin") {
            files.push(path);
        }
    }
    Ok(files)
}

fn compare_files(baseline_path: &Path, current_path: &Path) -> Result<()> {
    let baseline_measurements = MadVRMeasurements::parse_file(baseline_path)?;
    let current_measurements = MadVRMeasurements::parse_file(current_path)?;

    // Scene Count
    println!("Scene Count:");
    println!("  Baseline: {}", baseline_measurements.scenes.len());
    println!("  Current:  {}", current_measurements.scenes.len());
    println!(
        "  Delta:    {}",
        current_measurements.scenes.len() as isize - baseline_measurements.scenes.len() as isize
    );

    // Overall MaxCLL and MaxFALL
    let baseline_maxcll = baseline_measurements.header.maxcll;
    let current_maxcll = current_measurements.header.maxcll;
    let baseline_maxfall = baseline_measurements.header.maxfall;
    let current_maxfall = current_measurements.header.maxfall;

    println!("\nOverall MaxCLL:");
    println!("  Baseline: {}", baseline_maxcll);
    println!("  Current:  {}", current_maxcll);
    println!(
        "  Delta:    {}",
        current_maxcll as i32 - baseline_maxcll as i32
    );

    println!("\nOverall MaxFALL:");
    println!("  Baseline: {}", baseline_maxfall);
    println!("  Current:  {}", current_maxfall);
    println!(
        "  Delta:    {}",
        current_maxfall as i32 - baseline_maxfall as i32
    );

    // Per-frame target_nits 95th-pct delta
    let baseline_targets: Vec<u16> = baseline_measurements
        .frames
        .iter()
        .map(|m| m.target_nits.unwrap_or(0))
        .collect();
    let current_targets: Vec<u16> = current_measurements
        .frames
        .iter()
        .map(|m| m.target_nits.unwrap_or(0))
        .collect();

    if baseline_targets.len() == current_targets.len() {
        let mut deltas: Vec<f64> = izip!(&baseline_targets, &current_targets)
            .map(|(b, c)| (*c as f64 - *b as f64).abs())
            .collect();

        deltas.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let percentile_index = (deltas.len() as f64 * 0.95).floor() as usize;
        let p95_delta = deltas.get(percentile_index).unwrap_or(&0.0);

        println!("\nPer-frame target_nits 95th-percentile absolute delta:");
        println!("  Value: {:.2}", p95_delta);
    } else {
        println!("\nPer-frame target_nits comparison skipped: frame counts differ.");
        println!("  Baseline frames: {}", baseline_targets.len());
        println!("  Current frames:  {}", current_targets.len());
    }

    Ok(())
}
