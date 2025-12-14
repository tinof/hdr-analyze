use anyhow::{Context, Result};
use colored::Colorize;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

/// Find a specific tool, checking local directory first, then PATH.
pub fn find_tool(tool_name: &str) -> Option<PathBuf> {
    // 1. Check current directory
    let local_path = Path::new(".").join(tool_name);
    if local_path.exists() {
        // Simple check, on unix we might wanna check executable bit but simple existence is usually enough
        return Some(local_path);
    }

    // 2. Check PATH
    // "which" command is a simple cross-platform way if we don't want extra deps,
    // or just try to spawn it.
    // However, explicit checking is better for error messages.
    // For simplicity without 'which' crate:
    if Command::new("which")
        .arg(tool_name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Some(PathBuf::from(tool_name));
    }
    
     // Windows fallback
    if cfg!(target_os = "windows") {
         if Command::new("where")
            .arg(tool_name)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false) {
                 return Some(PathBuf::from(tool_name));
            }
    }

    None
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
            if n == 0 { break; }
            let _ = tx.send((false, binding[..n].to_vec()));
        }
    });

    let t_err = thread::spawn(move || {
        let reader = BufReader::new(stderr);
        let mut reader = reader;
         let mut binding = [0u8; 1024];
        while let Ok(n) = reader.read(&mut binding) {
            if n == 0 { break; }
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
            println!("{}", format!("Error: Required command '{}' not found in PATH.", tool).red());
            missing = true;
        }
    }

    if find_tool("mediainfo").is_none() && find_tool("ffprobe").is_none() {
        println!("{}", "Error: Neither 'mediainfo' nor 'ffprobe' found. One is required.".red());
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
            if n == 0 { break; }
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
