use std::time::Instant;

use clap::Parser;
use colored::Colorize;

mod cli;
mod external;
mod metadata;
mod pipeline;
mod progress;
mod verify;

use cli::Args;

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Initialize progress module with verbosity settings
    progress::set_verbose(args.verbose);
    progress::set_quiet(args.quiet);

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
                if !progress::is_quiet() {
                    eprintln!(
                        "{}",
                        "Boost mode enabled: using --peak-source=histogram99 for HDR10+ peak detection."
                            .green()
                    );
                }
                peak_source = cli::PeakSource::Histogram99;
            }
            cli::PeakSource::Histogram99 => {
                // Already default, no op
            }
        }
    }

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

    if !progress::is_quiet() {
        eprintln!("{} mkvdolby", "Starting".green().bold());
    }

    // Process files
    let mut files = final_args.input.clone();
    if files.is_empty() {
        // Glob *.mkv in current dir
        for entry in std::fs::read_dir(".")? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "mkv") {
                files.push(path.to_string_lossy().to_string());
            }
        }
        if files.is_empty() {
            eprintln!("No MKV files found in the current directory.");
            return Ok(());
        }
    }

    // Filter out already-converted files
    let mut skipped: usize = 0;
    let actionable_files: Vec<String> = files
        .into_iter()
        .filter(|f| {
            if f.ends_with(".DV.mkv") {
                if !progress::is_quiet() {
                    eprintln!("{}", format!("Skipping already converted: {}", f).yellow());
                }
                skipped += 1;
                false
            } else {
                true
            }
        })
        .collect();

    let total_files = actionable_files.len();
    if total_files == 0 {
        if !progress::is_quiet() {
            eprintln!("No files to process (all skipped or already converted).");
        }
        return Ok(());
    }

    if total_files > 1 && !progress::is_quiet() {
        eprintln!(
            "{}",
            format!("Queued {} file(s) for processing.", total_files).cyan()
        );
    }

    let run_start = Instant::now();
    let mut succeeded: usize = 0;
    let mut failed: usize = 0;

    for (idx, file) in actionable_files.iter().enumerate() {
        if total_files > 1 && !progress::is_quiet() {
            eprintln!(
                "\n{}",
                format!("── File {}/{} ──", idx + 1, total_files)
                    .cyan()
                    .dimmed()
            );
        }

        match pipeline::convert_file(file, &final_args) {
            Ok(true) => succeeded += 1,
            Ok(false) => {
                progress::print_error(&format!("Failed to process: {}", file));
                failed += 1;
            }
            Err(e) => {
                progress::print_error(&format!("Error processing '{}': {}", file, e));
                failed += 1;
            }
        }
    }

    // --- Summary ---
    let total_elapsed = run_start.elapsed();
    let elapsed_str = progress::format_duration_pub(total_elapsed);

    if !progress::is_quiet() {
        eprintln!(); // blank line
        if failed == 0 {
            eprintln!(
                "{}",
                format!(
                    "✓ All done — {} file(s) converted in {}",
                    succeeded, elapsed_str
                )
                .green()
                .bold()
            );
        } else {
            eprintln!(
                "{}",
                format!(
                    "Done — {} succeeded, {} failed ({} total)",
                    succeeded, failed, elapsed_str
                )
                .yellow()
                .bold()
            );
        }
        if skipped > 0 {
            eprintln!(
                "{}",
                format!("  ({} file(s) skipped — already converted)", skipped).dimmed()
            );
        }
    }

    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
