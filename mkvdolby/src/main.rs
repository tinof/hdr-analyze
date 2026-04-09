use std::cmp::Ordering;
use std::time::Instant;

use clap::Parser;
use colored::Colorize;
use regex::Regex;
use walkdir::WalkDir;

mod cli;
mod external;
mod metadata;
mod pipeline;
mod progress;
mod verify;

use cli::Args;

fn natural_segment_cmp(a: &str, b: &str) -> Ordering {
    let mut ia = 0usize;
    let mut ib = 0usize;
    let ba = a.as_bytes();
    let bb = b.as_bytes();

    while ia < ba.len() && ib < bb.len() {
        let ca = ba[ia];
        let cb = bb[ib];

        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let sa = ia;
            let sb = ib;
            while ia < ba.len() && ba[ia].is_ascii_digit() {
                ia += 1;
            }
            while ib < bb.len() && bb[ib].is_ascii_digit() {
                ib += 1;
            }

            let na = &a[sa..ia];
            let nb = &b[sb..ib];

            let na_trim = na.trim_start_matches('0');
            let nb_trim = nb.trim_start_matches('0');

            let na_eff = if na_trim.is_empty() { "0" } else { na_trim };
            let nb_eff = if nb_trim.is_empty() { "0" } else { nb_trim };

            match na_eff.len().cmp(&nb_eff.len()) {
                Ordering::Equal => match na_eff.cmp(nb_eff) {
                    Ordering::Equal => match na.len().cmp(&nb.len()) {
                        Ordering::Equal => {}
                        ord => return ord,
                    },
                    ord => return ord,
                },
                ord => return ord,
            }
        } else {
            let la = ca.to_ascii_lowercase();
            let lb = cb.to_ascii_lowercase();
            match la.cmp(&lb) {
                Ordering::Equal => {
                    ia += 1;
                    ib += 1;
                }
                ord => return ord,
            }
        }
    }

    ba.len().cmp(&bb.len())
}

fn episode_sort_key(path: &str, episode_re: &Regex) -> (u32, u32, String) {
    let lower = path.to_lowercase();

    if let Some(caps) = episode_re.captures(&lower) {
        let season = caps
            .get(1)
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(u32::MAX);
        let episode = caps
            .get(2)
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(u32::MAX);
        return (season, episode, lower);
    }

    (u32::MAX, u32::MAX, lower)
}

fn collect_default_inputs() -> anyhow::Result<Vec<String>> {
    let mut files: Vec<String> = WalkDir::new(".")
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            !e.path().components().any(|c| {
                c.as_os_str()
                    .to_string_lossy()
                    .starts_with("mkvdolby_temp_")
            })
        })
        .filter(|e| e.file_type().is_file() || e.file_type().is_symlink())
        .map(|e| e.into_path())
        .filter(|p| {
            p.extension()
                .map_or(false, |e| e.eq_ignore_ascii_case("mkv"))
        })
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let episode_re = Regex::new(r"s(\d{1,2})e(\d{1,3})")
        .map_err(|e| anyhow::anyhow!("Invalid episode regex: {}", e))?;

    files.sort_by(|a, b| {
        let ka = episode_sort_key(a, &episode_re);
        let kb = episode_sort_key(b, &episode_re);

        match ka.0.cmp(&kb.0) {
            Ordering::Equal => match ka.1.cmp(&kb.1) {
                Ordering::Equal => natural_segment_cmp(a, b),
                ord => ord,
            },
            ord => ord,
        }
    });

    Ok(files)
}

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
        files = collect_default_inputs()?;
        if files.is_empty() {
            eprintln!("No MKV files found in the current directory tree.");
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
