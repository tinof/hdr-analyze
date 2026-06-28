//! Progress indicators for mkvdolby CLI
//!
//! Provides spinners and progress bars using indicatif, with automatic
//! TTY detection and verbose mode support.

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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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

// --- Byte Progress Bar ---

/// A progress bar for long, file-producing steps (extract / inject / mux), driven by the
/// growth of the output file. Shows bytes written, throughput, and ETA (when a total
/// estimate is known) so a slow-but-moving step is visibly distinct from a stalled one.
pub struct ByteProgress {
    bar: ProgressBar,
    message: String,
    total: Option<u64>,
    /// True when a real (visible) bar is being drawn (TTY, non-verbose, non-quiet).
    active: bool,
}

impl ByteProgress {
    /// Create and start a byte progress bar. `total` is an estimate of the final output
    /// size; pass `None` when it cannot be estimated (e.g. a re-encode).
    pub fn new(message: &str, total: Option<u64>) -> Self {
        let active = is_tty() && !is_verbose() && !is_quiet();
        let bar = if active {
            let (pb, template) = match total {
                Some(t) => (
                    ProgressBar::new(t.max(1)),
                    "  {spinner:.cyan} {msg} [{bytes}/{total_bytes}] {binary_bytes_per_sec} ETA {eta} [{elapsed}]",
                ),
                None => (
                    ProgressBar::new_spinner(),
                    "  {spinner:.cyan} {msg} {bytes} {binary_bytes_per_sec} [{elapsed}]",
                ),
            };
            pb.set_style(
                ProgressStyle::default_bar()
                    .template(template)
                    .expect("Invalid byte-progress template")
                    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
            );
            pb.set_message(message.to_string());
            pb.enable_steady_tick(Duration::from_millis(120));
            pb
        } else {
            let pb = ProgressBar::hidden();
            if !is_quiet() {
                eprintln!("  {} {}...", "→", message);
            }
            pb
        };

        Self {
            bar,
            message: message.to_string(),
            total,
            active,
        }
    }

    /// Update the bytes-written position (capped at the known total).
    pub fn set_bytes(&self, bytes: u64) {
        let pos = match self.total {
            Some(t) => bytes.min(t),
            None => bytes,
        };
        self.bar.set_position(pos);
    }

    /// Show (`Some`) or clear (`None`) a stall warning suffix on the bar message.
    pub fn set_stall(&self, stalled_for: Option<Duration>) {
        match stalled_for {
            Some(d) => self.bar.set_message(format!(
                "{}  \u{26a0} no progress for {}",
                self.message,
                format_duration(d)
            )),
            None => self.bar.set_message(self.message.clone()),
        }
    }

    /// Finish with a success indicator.
    pub fn finish_success(&self) {
        let line = format!(
            "{} {} [{}]",
            "\u{2713}",
            self.message,
            format_duration(self.bar.elapsed())
        );
        if self.active {
            self.bar.finish_with_message(line);
        } else if !is_quiet() {
            eprintln!("  {}", line);
        }
    }

    /// Finish with a failure indicator.
    pub fn finish_error(&self, err_msg: Option<&str>) {
        let msg = match err_msg {
            Some(e) => format!("{} - {}", self.message, e),
            None => self.message.clone(),
        };
        if self.active {
            self.bar
                .finish_with_message(format!("{} {}", "\u{2717}", msg));
        } else if !is_quiet() {
            eprintln!("  {} {}", "\u{2717}", msg);
        }
    }
}

impl Drop for ByteProgress {
    fn drop(&mut self) {
        if !self.bar.is_finished() {
            self.bar.finish_and_clear();
        }
    }
}

// --- Step Printer ---

/// Print a step header (for major pipeline stages).
/// If `total` is 0, the step number is shown without a total (e.g. during detection
/// before the total is known).
pub fn print_step(step: u8, total: u8, message: &str) {
    if !is_quiet() {
        if total > 0 {
            eprintln!("\n{} {}", format!("[{}/{}]", step, total), message);
        } else {
            eprintln!("\n[{}] {}", step, message);
        }
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
#[allow(dead_code)]
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

/// Public wrapper for format_duration, usable from other modules.
pub fn format_duration_pub(d: Duration) -> String {
    format_duration(d)
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
