use anyhow::{Context, Result};
use colored::Colorize;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::progress::{self, ByteProgress, Spinner};

/// Find a specific tool on PATH.
pub fn find_tool(tool_name: &str) -> Option<PathBuf> {
    let locator = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };
    let output = Command::new(locator)
        .arg(tool_name)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
}

/// Run a command and log its output to a file.
/// Returns true if success code.
pub fn run_command(cmd: &mut Command, log_path: &Path) -> Result<bool> {
    let log_file = File::create(log_path).context("Failed to create log file")?;
    let mut writer = std::io::BufWriter::new(log_file);

    // Write command line for debugging
    writeln!(writer, "Running command: {:?}", cmd)?;
    writer.flush()?;

    // Redirect stderr to stdout to capture everything
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // Actually, std::process::Command doesn't support "stderr -> stdout" fd redirection easily without shell.
    // Better to pipe both.
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context("Failed to spawn command")?;

    let _stdout = child.stdout.take().expect("Failed to open stdout");
    let _stderr = child.stderr.take().expect("Failed to open stderr");

    // We want to stream both to the log file.
    // We can use threads to drive this.

    // Simplification: For non-live commands, just wait_with_output is easier,
    // but we want to log it potentially.
    // Let's use wait_with_output for simple commands and dump to file.

    let output = child.wait_with_output()?;

    writer.write_all(&output.stdout)?;
    writer.write_all(&output.stderr)?;

    Ok(output.status.success())
}

/// Run a command with a spinner, logging output to a file.
/// Shows elapsed time and success/failure status.
pub fn run_command_with_spinner(cmd: &mut Command, log_path: &Path, message: &str) -> Result<bool> {
    let spinner = Spinner::new(message);

    // In verbose mode, use live output instead
    if progress::is_verbose() {
        let result = run_command_live(cmd, log_path)?;
        if result {
            spinner.finish_success();
        } else {
            spinner.finish_error(Some("command failed"));
        }
        return Ok(result);
    }

    let log_file = File::create(log_path).context("Failed to create log file")?;
    let mut writer = std::io::BufWriter::new(log_file);

    // Write command line for debugging
    writeln!(writer, "Running command: {:?}", cmd)?;
    writer.flush()?;

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context("Failed to spawn command")?;

    let _stdout = child.stdout.take().expect("Failed to open stdout");
    let _stderr = child.stderr.take().expect("Failed to open stderr");

    let output = child.wait_with_output()?;

    writer.write_all(&output.stdout)?;
    writer.write_all(&output.stderr)?;

    if output.status.success() {
        spinner.finish_success();
        Ok(true)
    } else {
        // Try to extract error from output
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        let err_hint = stderr_str.lines().last().unwrap_or("check log for details");
        spinner.finish_error(Some(err_hint));
        Ok(false)
    }
}

/// Run a long, file-producing command with a byte-progress bar driven by the growth of
/// `output_path`. The child's stdout+stderr are streamed to `log_path` live (so the log
/// fills during the run, surfacing tool warnings), and a warning is shown if the output
/// stops growing for `stall_secs` seconds (`0` disables the stall check).
///
/// This is the robust replacement for `run_command_with_spinner` on steps that move many
/// gigabytes (extract / inject / mux / encode): the bytes + throughput + ETA readout makes
/// a slow-but-working step distinguishable from a hung one.
pub fn run_command_with_progress(
    cmd: &mut Command,
    log_path: &Path,
    message: &str,
    output_path: &Path,
    expected_total: Option<u64>,
    stall_secs: u64,
) -> Result<bool> {
    // Verbose mode: stream raw output to the terminal instead of drawing a bar.
    if progress::is_verbose() {
        let bar = ByteProgress::new(message, expected_total);
        let result = run_command_live(cmd, log_path)?;
        if result {
            bar.finish_success();
        } else {
            bar.finish_error(Some("command failed"));
        }
        return Ok(result);
    }

    let log_file = File::create(log_path).context("Failed to create log file")?;
    let mut log_writer = std::io::BufWriter::new(log_file);
    writeln!(log_writer, "Running command: {:?}", cmd)?;
    log_writer.flush()?;

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().context("Failed to spawn command")?;

    let stdout = child.stdout.take().expect("Failed to open stdout");
    let stderr = child.stderr.take().expect("Failed to open stderr");

    // Drain both pipes to the log on dedicated threads so the child never blocks on a full
    // pipe buffer while we poll the output file.
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let tx_err = tx.clone();
    let t_out = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            let _ = tx.send(buf[..n].to_vec());
        }
    });
    let t_err = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            let _ = tx_err.send(buf[..n].to_vec());
        }
    });
    let t_log = thread::spawn(move || {
        for data in rx {
            let s = String::from_utf8_lossy(&data);
            let _ = log_writer.write_all(s.replace('\r', "\n").as_bytes());
        }
        let _ = log_writer.flush();
    });

    // Poll output-file growth and update the bar until the child exits.
    let bar = ByteProgress::new(message, expected_total);
    let poll = Duration::from_millis(500);
    let mut last_size: u64 = 0;
    let mut last_growth = Instant::now();
    let mut warned = false;
    let status = loop {
        let size = std::fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);
        bar.set_bytes(size);
        if size > last_size {
            last_size = size;
            last_growth = Instant::now();
            if warned {
                bar.set_stall(None);
                warned = false;
            }
        } else if stall_secs > 0 {
            let stalled = last_growth.elapsed();
            if stalled.as_secs() >= stall_secs {
                bar.set_stall(Some(stalled));
                warned = true;
            }
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        thread::sleep(poll);
    };

    // Final position update + drain the logging pipeline.
    let final_size = std::fs::metadata(output_path)
        .map(|m| m.len())
        .unwrap_or(last_size);
    bar.set_bytes(final_size);
    let _ = t_out.join();
    let _ = t_err.join();
    let _ = t_log.join();

    if status.success() {
        bar.finish_success();
        Ok(true)
    } else {
        bar.finish_error(Some("check log for details"));
        Ok(false)
    }
}

/// Run a command and stream output to both terminal (stderr mainly for progress) and a log file.
/// This matches `run_command_live` from Python.
pub fn run_command_live(cmd: &mut Command, log_path: &Path) -> Result<bool> {
    let log_file = File::create(log_path).context("Failed to create log file")?;
    // We clone the file handle for the threads
    let mut log_writer = std::io::BufWriter::new(log_file);

    writeln!(log_writer, "Running command live: {:?}", cmd)?;

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context("Failed to spawn command")?;

    let stdout = child.stdout.take().expect("Stdout capture failed");
    let stderr = child.stderr.take().expect("Stderr capture failed");

    // Channels to send output back to main thread or just distinct threads handling writing
    // The Python script uses `select`. In Rust, threads are easier for cross-platform.

    let (tx, rx) = mpsc::channel();
    let tx_err = tx.clone();

    let t_out = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        // We read byte by byte or chunk to preserve exact output (including \r)
        // copy() might buffer too much?
        // Let's just read chunks.
        let mut reader = reader;
        let mut binding = [0u8; 1024];
        while let Ok(n) = reader.read(&mut binding) {
            if n == 0 {
                break;
            }
            let _ = tx.send((false, binding[..n].to_vec()));
        }
    });

    let t_err = thread::spawn(move || {
        let reader = BufReader::new(stderr);
        let mut reader = reader;
        let mut binding = [0u8; 1024];
        while let Ok(n) = reader.read(&mut binding) {
            if n == 0 {
                break;
            }
            let _ = tx_err.send((true, binding[..n].to_vec()));
        }
    });

    // Main loop: receive from channel, write to log + screen
    let mut stdout_handle = std::io::stdout();
    let mut stderr_handle = std::io::stderr();

    for (is_err, data) in rx {
        // Write to log (replacing \r with \n for readability in logs, as python did)
        // Python: chunk.decode(...).replace('\r', '\n')
        // We'll just write raw bytes to log? Or try sanitize.
        // Valid utf8 is safer for replacing strings.
        let s = String::from_utf8_lossy(&data);
        let clean_s = s.replace('\r', "\n");
        let _ = log_writer.write_all(clean_s.as_bytes());

        // Write to terminal (raw)
        if is_err {
            let _ = stderr_handle.write_all(&data);
            let _ = stderr_handle.flush();
        } else {
            let _ = stdout_handle.write_all(&data);
            let _ = stdout_handle.flush();
        }
    }

    // Close up
    let _ = t_out.join();
    let _ = t_err.join();

    let status = child.wait()?;
    Ok(status.success())
}

pub fn check_dependencies() -> Result<()> {
    let required = ["ffmpeg", "mkvmerge"];
    let mut missing = false;

    for tool in required {
        if find_tool(tool).is_none() {
            println!(
                "{}",
                format!("Error: Required command '{}' not found in PATH.", tool).red()
            );
            missing = true;
        }
    }

    if find_tool("mediainfo").is_none() && find_tool("ffprobe").is_none() {
        println!(
            "{}",
            "Error: Neither 'mediainfo' nor 'ffprobe' found. One is required.".red()
        );
        missing = true;
    }

    if find_tool("dovi_tool").is_none() {
        println!("{}", "Error: Required command 'dovi_tool' not found.".red());
        missing = true;
    }

    if missing {
        anyhow::bail!("Missing dependencies");
    }
    Ok(())
}

/// Run a command and capture its stdout as a string.
pub fn get_command_output(cmd: &mut Command) -> Result<String> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null()); // Silence stderr for data fetching commands usually

    let output = cmd.output().context("Failed to execute command")?;

    if output.status.success() {
        let s = String::from_utf8(output.stdout).context("Command output is not valid UTF-8")?;
        Ok(s)
    } else {
        anyhow::bail!("Command failed with status: {}", output.status)
    }
}

/// Run a command, inheriting stderr (so progress bars work naturally) but capturing/logging stdout.
pub fn run_command_inherit_stderr(cmd: &mut Command, log_path: &Path) -> Result<bool> {
    let log_file = File::create(log_path).context("Failed to create log file")?;
    let mut log_writer = std::io::BufWriter::new(log_file);

    writeln!(log_writer, "Running command (stderr inherited): {:?}", cmd)?;

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::inherit());

    let mut child = cmd.spawn().context("Failed to spawn command")?;

    let stdout = child.stdout.take().expect("Stdout capture failed");

    // We only need one thread for stdout
    let (tx, rx) = mpsc::channel();

    let t_out = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut reader = reader;
        let mut binding = [0u8; 1024];
        while let Ok(n) = reader.read(&mut binding) {
            if n == 0 {
                break;
            }
            let _ = tx.send(binding[..n].to_vec());
        }
    });

    let mut stdout_handle = std::io::stdout();

    for data in rx {
        // Write to log
        let s = String::from_utf8_lossy(&data);
        let clean_s = s.replace('\r', "\n");
        let _ = log_writer.write_all(clean_s.as_bytes());

        // Write to terminal
        let _ = stdout_handle.write_all(&data);
        let _ = stdout_handle.flush();
    }

    let _ = t_out.join();

    let status = child.wait()?;
    Ok(status.success())
}
