use colored::Colorize;
use std::path::Path;
use std::process::Command;

use crate::external::{self, run_command};
use crate::metadata;

#[allow(dead_code)]
pub fn verify_post_mux(
    input_file: &str,
    output_file: &Path,
    measurements: Option<&Path>,
    temp_dir: &Path,
) -> bool {
    verify_post_mux_with_options(input_file, output_file, measurements, temp_dir, None)
}

/// Full verification with optional expected CM version for RPU content assertions.
pub fn verify_post_mux_with_options(
    input_file: &str,
    output_file: &Path,
    measurements: Option<&Path>,
    temp_dir: &Path,
    expected_cm_version: Option<&str>,
) -> bool {
    let mut ok = true;

    // 1. Run internal verifier on measurements if available.
    if let Some(meas_path) = measurements {
        println!("{}", "Verifying measurements...".cyan());
        if let Some(exe) = external::find_tool("verifier") {
            let mut cmd = Command::new(exe);
            cmd.arg(meas_path);
            if !run_logged_command(&mut cmd, &temp_dir.join("verifier.log")) {
                println!("{}", "Verifier tool reported issues.".red());
                ok = false;
            }
        } else {
            println!(
                "{}",
                "Verifier binary not found on PATH; skipping measurement check. \
                 Install with: cargo install --path verifier"
                    .yellow()
            );
        }
    }

    // 2. Extract RPU from the muxed output for structural inspection.
    println!("{}", "Checking with dovi_tool info...".cyan());
    let hevc_path = temp_dir.join("verify_video.hevc");
    let rpu_path = temp_dir.join("verify_rpu.bin");

    let mut ffmpeg = Command::new("ffmpeg");
    ffmpeg.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        output_file.to_str().unwrap(),
        "-map",
        "0:v:0",
        "-c:v",
        "copy",
        "-f",
        "hevc",
        "-y",
        hevc_path.to_str().unwrap(),
    ]);

    let mut extract_rpu = Command::new("dovi_tool");
    extract_rpu.args([
        "extract-rpu",
        "-i",
        hevc_path.to_str().unwrap(),
        "-o",
        rpu_path.to_str().unwrap(),
    ]);

    if !run_logged_command(&mut ffmpeg, &temp_dir.join("verify_extract_hevc.log"))
        || !run_logged_command(&mut extract_rpu, &temp_dir.join("verify_extract_rpu.log"))
    {
        println!("{}", "RPU extraction for verification failed.".red());
        return false;
    }

    let mut summary_cmd = Command::new("dovi_tool");
    summary_cmd.args(["info", "--summary", "-i", rpu_path.to_str().unwrap()]);
    if let Ok(summary) = external::get_command_output(&mut summary_cmd) {
        let _ = std::fs::write(temp_dir.join("dovi_info_summary.log"), summary);
    }

    let mut frame_cmd = Command::new("dovi_tool");
    frame_cmd.args(["info", "--frame", "0", "-i", rpu_path.to_str().unwrap()]);
    match external::get_command_output(&mut frame_cmd) {
        Ok(frame_output) => {
            let _ = std::fs::write(temp_dir.join("dovi_info_frame_0.log"), &frame_output);
            match parse_dovi_frame_json(&frame_output) {
                Ok(frame) => {
                    if !assert_rpu_invariants(&frame, expected_cm_version) {
                        ok = false;
                    }
                }
                Err(e) => {
                    println!(
                        "{}",
                        format!("Failed to parse dovi_tool frame JSON: {e}").red()
                    );
                    ok = false;
                }
            }
        }
        Err(e) => {
            println!("{}", format!("dovi_tool info failed: {e}").red());
            ok = false;
        }
    }

    // 3. Duration consistency check (1-second tolerance).
    if let (Some(d_in), Some(d_out)) = (
        metadata::get_duration_from_mediainfo(input_file),
        get_duration_from_file(output_file),
    ) {
        let diff = (d_in - d_out).abs();
        if diff > 1.0 {
            println!(
                "{}",
                format!(
                    "Duration mismatch! Input: {:.2}s, Output: {:.2}s",
                    d_in, d_out
                )
                .red()
            );
            ok = false;
        }
    }

    ok
}

fn parse_dovi_frame_json(output: &str) -> Result<serde_json::Value, String> {
    let json_start = output
        .find('{')
        .ok_or_else(|| "dovi_tool output did not contain a JSON object".to_string())?;
    serde_json::from_str(&output[json_start..]).map_err(|e| e.to_string())
}

fn metadata_block<'a>(
    frame: &'a serde_json::Value,
    cm_version: &str,
    level: &str,
) -> Option<&'a serde_json::Value> {
    frame
        .pointer(&format!(
            "/vdr_dm_data/{cm_version}_metadata/ext_metadata_blocks"
        ))
        .and_then(serde_json::Value::as_array)
        .and_then(|blocks| blocks.iter().find_map(|block| block.get(level)))
}

/// Assert structural invariants from structured `dovi_tool info --frame 0` JSON.
/// Returns false if any hard invariant is violated.
fn assert_rpu_invariants(frame: &serde_json::Value, expected_cm_version: Option<&str>) -> bool {
    let mut failures = Vec::new();

    if frame
        .get("dovi_profile")
        .and_then(serde_json::Value::as_u64)
        != Some(8)
    {
        failures.push("expected Dolby Vision profile 8 output".to_string());
    }

    match metadata_block(frame, "cmv29", "Level1") {
        Some(level1) => {
            let min_pq = level1.get("min_pq").and_then(serde_json::Value::as_u64);
            let avg_pq = level1.get("avg_pq").and_then(serde_json::Value::as_u64);
            let max_pq = level1.get("max_pq").and_then(serde_json::Value::as_u64);
            if !matches!((min_pq, avg_pq, max_pq), (Some(min), Some(avg), Some(max)) if min <= avg && avg <= max)
            {
                failures.push("L1 metadata must satisfy min_pq <= avg_pq <= max_pq".to_string());
            }
        }
        None => failures.push("required L1 metadata block is missing".to_string()),
    }

    match metadata_block(frame, "cmv29", "Level6") {
        Some(level6) => {
            let required_positive = [
                "max_display_mastering_luminance",
                "max_content_light_level",
                "max_frame_average_light_level",
            ];
            for field in required_positive {
                if level6.get(field).and_then(serde_json::Value::as_u64) == Some(0)
                    || level6
                        .get(field)
                        .and_then(serde_json::Value::as_u64)
                        .is_none()
                {
                    failures.push(format!("L6 field {field} must be a positive integer"));
                }
            }
            if level6
                .get("min_display_mastering_luminance")
                .and_then(serde_json::Value::as_u64)
                .is_none()
            {
                failures.push(
                    "L6 field min_display_mastering_luminance must be an integer".to_string(),
                );
            }
        }
        None => failures.push("required L6 metadata block is missing".to_string()),
    }

    if expected_cm_version == Some("V40") {
        if metadata_block(frame, "cmv40", "Level9").is_none() {
            failures.push("required CM v4.0 L9 metadata block is missing".to_string());
        }
        if metadata_block(frame, "cmv40", "Level11").is_none() {
            failures.push("required CM v4.0 L11 metadata block is missing".to_string());
        }
        if metadata_block(frame, "cmv40", "Level254")
            .and_then(|level254| level254.get("dm_version_index"))
            .and_then(serde_json::Value::as_u64)
            != Some(2)
        {
            failures.push("CM v4.0 Level254 dm_version_index must be 2".to_string());
        }
    }

    for failure in &failures {
        println!("{}", format!("RPU invariant FAIL: {failure}").red());
    }

    if failures.is_empty() {
        println!("{}", "RPU structural invariants passed.".green());
        true
    } else {
        false
    }
}

// Helper needed because metadata::get_duration expects &str but sometimes we have Path
fn get_duration_from_file(path: &Path) -> Option<f64> {
    metadata::get_duration_from_mediainfo(path.to_str()?)
}

fn run_logged_command(cmd: &mut Command, log_path: &Path) -> bool {
    matches!(run_command(cmd, log_path), Ok(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_frame() -> serde_json::Value {
        json!({
            "dovi_profile": 8,
            "vdr_dm_data": {
                "cmv29_metadata": {
                    "ext_metadata_blocks": [
                        { "Level1": { "min_pq": 0, "avg_pq": 1310, "max_pq": 2081 } },
                        { "Level6": {
                            "max_display_mastering_luminance": 1000,
                            "min_display_mastering_luminance": 1,
                            "max_content_light_level": 997,
                            "max_frame_average_light_level": 200
                        } }
                    ]
                },
                "cmv40_metadata": {
                    "ext_metadata_blocks": [
                        { "Level9": { "source_primary_index": 0 } },
                        { "Level11": { "content_type": 1, "reference_mode_flag": false } },
                        { "Level254": { "dm_mode": 0, "dm_version_index": 2 } }
                    ]
                }
            }
        })
    }

    #[test]
    fn parse_dovi_frame_json_ignores_status_prefix() {
        let parsed = parse_dovi_frame_json("Parsing RPU file...\n{\"dovi_profile\":8}").unwrap();

        assert_eq!(parsed["dovi_profile"], 8);
    }

    #[test]
    fn rpu_invariants_accept_valid_v40_frame() {
        assert!(assert_rpu_invariants(&valid_frame(), Some("V40")));
    }

    #[test]
    fn rpu_invariants_reject_invalid_l1_ordering() {
        let mut frame = valid_frame();
        frame["vdr_dm_data"]["cmv29_metadata"]["ext_metadata_blocks"][0]["Level1"]["avg_pq"] =
            json!(3000);

        assert!(!assert_rpu_invariants(&frame, Some("V40")));
    }

    #[test]
    fn rpu_invariants_reject_missing_v40_blocks() {
        let mut frame = valid_frame();
        frame["vdr_dm_data"]["cmv40_metadata"]["ext_metadata_blocks"] = json!([]);

        assert!(!assert_rpu_invariants(&frame, Some("V40")));
    }
}
