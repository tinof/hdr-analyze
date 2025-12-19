use clap::Parser;
use colored::Colorize;

mod cli;
mod external;
mod metadata;
mod pipeline;
mod verify;

use cli::Args;

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Check dependencies after parsing so `--help`/`--version` work without tools installed.
    if let Err(e) = external::check_dependencies() {
        eprintln!("{}", format!("Dependency check failed: {}", e).red());
        std::process::exit(1);
    }

    // Basic handling for "boost" logic modification
    let mut peak_source = args.peak_source;
    if args.boost {
        match peak_source {
            cli::PeakSource::MaxSclLuminance | cli::PeakSource::Histogram => {
                println!(
                    "{}",
                    "Boost mode enabled: using --peak-source=histogram99 for HDR10+ peak detection."
                        .green()
                );
                peak_source = cli::PeakSource::Histogram99;
            }
            cli::PeakSource::Histogram99 => {
                // Already default, no op
            }
        }
    }
    // Note: We need to propagate the modified peak_source to the pipeline maybe via a modified Args struct
    // or passing it explicitly. The pipeline currently takes &Args.
    // For MVP, since Args is owned in main, we can mutate it if we make it mutable, but Clap struct fields are pub.
    // Let's hack it: struct update syntax or just mutability.
    let mut final_args = args.clone();
    final_args.peak_source = peak_source;

    let trim_targets: Vec<u32> = final_args
        .trim_targets
        .split(',')
        .map(|s| s.trim().parse::<u32>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| {
            anyhow::anyhow!("--trim-targets must be a comma-separated list of integers.")
        })?;

    // Validate trim target parsing early
    if trim_targets.is_empty() {
        anyhow::bail!("--trim-targets cannot be empty");
    }

    println!("{} mkvdolby", "Starting".green().bold());

    // Process files
    let mut files = final_args.input.clone();
    if files.is_empty() {
        // Glob *.mkv in current dir
        // Using walkdir or just standard fs::read_dir
        for entry in std::fs::read_dir(".")? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "mkv") {
                files.push(path.to_string_lossy().to_string());
            }
        }
        if files.is_empty() {
            println!("No MKV files found in the current directory.");
            return Ok(());
        }
    }

    let mut had_failure = false;
    for file in files {
        // Skip already converted
        if file.ends_with(".DV.mkv") {
            println!(
                "{}",
                format!("Skipping already converted file: {}", file).yellow()
            );
            continue;
        }

        match pipeline::convert_file(&file, &final_args) {
            Ok(success) => {
                if !success {
                    had_failure = true;
                }
            }
            Err(e) => {
                println!(
                    "{}",
                    format!("Error processing file '{}': {}", file, e).red()
                );
                had_failure = true;
            }
        }
    }

    if had_failure {
        std::process::exit(1);
    }
    Ok(())
}
