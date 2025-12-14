// use anyhow::Result;
use colored::Colorize;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::external::{self, run_command};
use crate::metadata;

pub fn verify_post_mux(
    input_file: &str, 
    output_file: &Path, 
    measurements: Option<&Path>, 
    temp_dir: &Path
) -> bool {
    let mut ok = true;

    // 1. Run internal verifier on measurements if available
    /* 
       Note: The original python script runs 'verifier' on the measurements.bin file.
       We should locate 'verifier' (crate in this workspace).
    */
    if let Some(meas_path) = measurements {
        println!("{}", "Verifying measurements...".cyan());
        // find verifier binary
        let tool = "verifier"; 
         // simplistic
        if external::find_tool(tool).is_some() || Path::new("target/release/verifier").exists() {
             let exe = if Path::new("target/release/verifier").exists() {
                 PathBuf::from("target/release/verifier")
             } else {
                 PathBuf::from(tool)
             };
             
             let mut cmd = Command::new(exe);
             cmd.arg(meas_path);
             
             // Capturing output to check for errors/warnings?
             // Python: just ran it. 'verifier' exits non-zero on error?
             // Assuming yes.
             if let Err(_) = run_command(&mut cmd, &temp_dir.join("verifier.log")) {
                 println!("{}", "Verifier tool reported issues.".red());
                 ok = false;
             }
        } else {
            println!("{}", "Verifier tool not found, skipping specific verification.".yellow());
        }
    }

    // 2. dovi_tool info check
    println!("{}", "Checking with dovi_tool info...".cyan());
    let mut dovi = Command::new("dovi_tool");
    dovi.args(["info", "-i", output_file.to_str().unwrap()]);
    // dovi_tool info doesn't fail easily, but if it crashes it's bad.
    if let Err(_) = run_command(&mut dovi, &temp_dir.join("dovi_info.log")) {
        println!("{}", "dovi_tool check failed.".red());
        ok = false;
    }

    // 3. Duration consistency
    // Check if input duration ~ output duration
    if let (Some(d_in), Some(d_out)) = (metadata::get_duration_from_mediainfo(input_file), get_duration_from_file(output_file)) {
        let diff = (d_in - d_out).abs();
        if diff > 1.0 { // 1 second tolerance
            println!("{}", format!("Duration mismatch! Input: {:.2}s, Output: {:.2}s", d_in, d_out).red());
            ok = false;
        }
    }

    ok
}

// Helper needed because metadata::get_duration expects &str but sometimes we have Path
fn get_duration_from_file(path: &Path) -> Option<f64> {
     metadata::get_duration_from_mediainfo(path.to_str()?)
}
