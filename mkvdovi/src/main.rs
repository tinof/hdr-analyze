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
mod resume;
mod rpu_check;
mod rpu_guard;
mod verify;

use cli::{Args, SubCmd};

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
                let name = c.as_os_str().to_string_lossy();
                // Old prefix kept one release for pre-rename temp dirs.
                name.starts_with("mkvdovi_temp_") || name.starts_with("mkvdolby_temp_")
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

/// Runs a subcommand. Called before `check_dependencies`, which needs the full toolchain.
fn dispatch_subcommand(subcmd: &SubCmd) -> anyhow::Result<()> {
    match subcmd {
        SubCmd::Inspect(inspect_args) => rpu_check::inspect_file(&inspect_args.input),
        SubCmd::Manifest(manifest_args) => {
            // Before `check_dependencies`: a manifest needs no mkvmerge, and it checks
            // the tools it does need itself.
            if !pipeline::manifest_file(manifest_args)? {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

/// Graceful interrupt: a dropped session (SIGHUP), SIGTERM, or Ctrl+C (SIGINT) prints a
/// resume hint and exits. The temp directory is only cleaned on a successful conversion, so
/// an interrupted run leaves its artifacts in place and the next invocation resumes from
/// them (unless `--no-resume` is passed).
fn install_interrupt_handler() {
    let _ = ctrlc::set_handler(|| {
        eprintln!(
            "\n{}",
            "Interrupted — partial work preserved in mkvdovi_temp_*; re-run to resume.".yellow()
        );
        std::process::exit(130);
    });
}

/// Basic handling for "boost" logic modification.
fn boosted_peak_source(peak_source: cli::PeakSource, boost: bool) -> cli::PeakSource {
    if !boost {
        return peak_source;
    }
    match peak_source {
        cli::PeakSource::MaxSclLuminance | cli::PeakSource::MaxScl | cli::PeakSource::Histogram => {
            if !progress::is_quiet() {
                eprintln!(
                    "{}",
                    "Boost mode enabled: using --peak-source=histogram99 for HDR10+ peak detection."
                        .green()
                );
            }
            cli::PeakSource::Histogram99
        }
        // Already default, no op
        cli::PeakSource::Histogram99 => peak_source,
    }
}

/// Applies `--boost`, resolves the `auto` settings and validates the flags.
fn resolve_run_args(args: &Args) -> anyhow::Result<Args> {
    let peak_source = boosted_peak_source(args.peak_source, args.boost);

    let mut final_args = args.clone();
    final_args.peak_source = peak_source;

    // Resolve `auto` hwaccel / analysis-quality to concrete values for this machine
    // (one-time detection; all downstream code sees only concrete settings).
    pipeline::resolve_auto_settings(&mut final_args);

    let trim_targets = final_args.trim_targets_nits().map_err(|_| {
        anyhow::anyhow!("--trim-targets must be a comma-separated list of integers.")
    })?;

    // Validate trim target parsing early
    if trim_targets.is_empty() {
        anyhow::bail!("--trim-targets cannot be empty");
    }
    if let Some(conflict) = final_args.flag_conflict() {
        anyhow::bail!(conflict);
    }
    Ok(final_args)
}

/// Drops already-converted `.DV.mkv` inputs (unless `--mdfix`); returns the rest and
/// the number skipped.
fn filter_converted(files: Vec<String>, mdfix: bool) -> (Vec<String>, usize) {
    let mut skipped: usize = 0;
    let actionable_files: Vec<String> = files
        .into_iter()
        .filter(|f| {
            if f.ends_with(".DV.mkv") && !mdfix {
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
    (actionable_files, skipped)
}

/// Converts each file in turn; returns the succeeded and failed counts.
fn convert_all(files: &[String], args: &Args) -> (usize, usize) {
    let total_files = files.len();
    let mut succeeded: usize = 0;
    let mut failed: usize = 0;

    for (idx, file) in files.iter().enumerate() {
        if total_files > 1 && !progress::is_quiet() {
            eprintln!(
                "\n{}",
                format!("── File {}/{} ──", idx + 1, total_files)
                    .cyan()
                    .dimmed()
            );
        }

        match pipeline::convert_file(file, args) {
            Ok(true) => succeeded += 1,
            Ok(false) => {
                progress::print_error(&format!("Failed to process: {}", file));
                failed += 1;
            }
            Err(e) => {
                progress::print_error(&format!("Error processing '{}': {:#}", file, e));
                failed += 1;
            }
        }
    }
    (succeeded, failed)
}

fn print_summary(succeeded: usize, failed: usize, skipped: usize, run_start: Instant) {
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
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Dispatch subcommands first (before dependency checks that require full toolchain)
    if let Some(subcmd) = &args.subcmd {
        return dispatch_subcommand(subcmd);
    }

    // Initialize progress module with verbosity settings
    progress::set_verbose(args.verbose);
    progress::set_quiet(args.quiet);

    install_interrupt_handler();

    // Check dependencies after parsing so `--help`/`--version` work without tools installed.
    if let Err(e) = external::check_dependencies() {
        eprintln!("{}", format!("Dependency check failed: {}", e).red());
        std::process::exit(1);
    }

    let final_args = resolve_run_args(&args)?;

    if !progress::is_quiet() {
        eprintln!("{} mkvdovi", "Starting".green().bold());
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
    let (actionable_files, skipped) = filter_converted(files, final_args.mdfix);

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
    let (succeeded, failed) = convert_all(&actionable_files, &final_args);

    // --- Summary ---
    print_summary(succeeded, failed, skipped, run_start);

    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
