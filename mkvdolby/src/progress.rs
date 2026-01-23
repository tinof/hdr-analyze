//! Progress indicators for mkvdolby CLI
//!
//! Provides spinners and progress bars using indicatif, with automatic
//! TTY detection and verbose mode support.

#![allow(dead_code)] // Helper functions for future use

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

// --- Global State ---

static VERBOSE: AtomicBool = AtomicBool::new(false);
static QUIET: AtomicBool = AtomicBool::new(false);

/// Set global verbose mode (shows raw command output)
pub fn set_verbose(v: bool) {
    VERBOSE.store(v, Ordering::SeqCst);
}

/// Set global quiet mode (minimal output)
pub fn set_quiet(q: bool) {
    QUIET.store(q, Ordering::SeqCst);
}

/// Check if verbose mode is enabled
pub fn is_verbose() -> bool {
    VERBOSE.load(Ordering::SeqCst)
}

/// Check if quiet mode is enabled
pub fn is_quiet() -> bool {
    QUIET.load(Ordering::SeqCst)
}

/// Check if we're running in a TTY (interactive terminal)
pub fn is_tty() -> bool {
    std::io::stderr().is_terminal()
}

// --- Spinner ---

/// A spinner for long-running operations with elapsed time
pub struct Spinner {
    bar: ProgressBar,
    message: String,
}

impl Spinner {
    /// Create and start a new spinner with the given message
    pub fn new(message: &str) -> Self {
        let bar = if is_tty() && !is_verbose() && !is_quiet() {
            let pb = ProgressBar::new_spinner();
            pb.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.cyan} {msg} [{elapsed}]")
                    .expect("Invalid spinner template")
                    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
            );
            pb.set_message(message.to_string());
            pb.enable_steady_tick(Duration::from_millis(100));
            pb
        } else {
            // Hidden spinner for non-TTY or verbose mode
            let pb = ProgressBar::hidden();
            if !is_quiet() {
                eprintln!("  {} {}...", "→".to_string(), message);
            }
            pb
        };

        Self {
            bar,
            message: message.to_string(),
        }
    }

    /// Update the spinner message
    pub fn set_message(&self, msg: &str) {
        self.bar.set_message(msg.to_string());
    }

    /// Finish with success indicator
    pub fn finish_success(&self) {
        if is_tty() && !is_verbose() && !is_quiet() {
            self.bar.finish_with_message(format!(
                "{} {} [{}]",
                "\u{2713}",
                self.message,
                format_duration(self.bar.elapsed())
            ));
        } else if !is_quiet() {
            eprintln!(
                "  {} {} [{}]",
                "\u{2713}",
                self.message,
                format_duration(self.bar.elapsed())
            );
        }
    }

    /// Finish with failure indicator
    pub fn finish_error(&self, err_msg: Option<&str>) {
        let msg = if let Some(e) = err_msg {
            format!("{} - {}", self.message, e)
        } else {
            self.message.clone()
        };

        if is_tty() && !is_verbose() && !is_quiet() {
            self.bar
                .finish_with_message(format!("{} {}", "\u{2717}", msg));
        } else if !is_quiet() {
            eprintln!("  {} {}", "\u{2717}", msg);
        }
    }

    /// Get elapsed time
    pub fn elapsed(&self) -> Duration {
        self.bar.elapsed()
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        if !self.bar.is_finished() {
            self.bar.finish_and_clear();
        }
    }
}

// --- Progress Bar ---

/// A progress bar for operations with known total (e.g., frame processing)
pub struct Progress {
    bar: ProgressBar,
}

impl Progress {
    /// Create a new progress bar with the given total
    pub fn new(total: u64, message: &str) -> Self {
        let bar = if is_tty() && !is_verbose() && !is_quiet() {
            let pb = ProgressBar::new(total);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("  {spinner:.cyan} {msg} [{bar:30.cyan/dim}] {pos}/{len} ({eta})")
                    .expect("Invalid progress template")
                    .progress_chars("━━─"),
            );
            pb.set_message(message.to_string());
            pb.enable_steady_tick(Duration::from_millis(100));
            pb
        } else {
            ProgressBar::hidden()
        };

        Self { bar }
    }

    /// Increment progress by 1
    pub fn inc(&self) {
        self.bar.inc(1);
    }

    /// Set absolute position
    pub fn set_position(&self, pos: u64) {
        self.bar.set_position(pos);
    }

    /// Finish successfully
    pub fn finish(&self) {
        self.bar.finish_and_clear();
    }
}

// --- Step Printer ---

/// Print a step header (for major pipeline stages)
pub fn print_step(step: u8, total: u8, message: &str) {
    if !is_quiet() {
        eprintln!("\n{} {}", format!("[{}/{}]", step, total), message);
    }
}

/// Print an info message
pub fn print_info(message: &str) {
    if !is_quiet() {
        eprintln!("  {} {}", "i", message);
    }
}

/// Print a warning message
pub fn print_warn(message: &str) {
    if !is_quiet() {
        eprintln!("  {} {}", "!", message);
    }
}

/// Print a success message
pub fn print_success(message: &str) {
    if !is_quiet() {
        eprintln!("{} {}", "\u{2713}", message);
    }
}

/// Print an error message
pub fn print_error(message: &str) {
    eprintln!("{} {}", "\u{2717}", message);
}

// --- Helpers ---

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m{}s", secs / 3600, (secs % 3600) / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(5)), "5s");
        assert_eq!(format_duration(Duration::from_secs(65)), "1m5s");
        assert_eq!(format_duration(Duration::from_secs(3665)), "1h1m5s");
    }

    #[test]
    fn test_verbose_mode() {
        set_verbose(true);
        assert!(is_verbose());
        set_verbose(false);
        assert!(!is_verbose());
    }
}
