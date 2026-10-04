use colored::Colorize;
use dolby_vision::rpu::dovi_rpu::DoviRpu;
use dolby_vision::rpu::extension_metadata::blocks::{ExtMetadataBlock, ExtMetadataBlockLevel1};
use dolby_vision::rpu::utils::parse_rpu_file;
use dolby_vision::rpu::vdr_dm_data::CmVersion;
use dovi84_composer::Composer;
use std::path::Path;
use std::process::Command;

use crate::external::{self, run_command};
use crate::metadata;
use crate::progress;

#[allow(dead_code)]
pub fn verify_post_mux(
    input_file: &str,
    output_file: &Path,
    measurements: Option<&Path>,
    temp_dir: &Path,
) -> bool {
    verify_post_mux_with_options(
        input_file,
        output_file,
        measurements,
        temp_dir,
        None,
        None,
        None,
    )
}

/// What an RPU mkvdovi generated must deliver, checked frame by frame on the RPU extracted from
/// the muxed output. Only for generated RPUs: the `dolby_vision` crate wrote them, so it always
/// parses them, while a passed-through source RPU (Profile 7 MEL) can carry levels the crate does
/// not read (L253) and gets the external checks only.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeliveryExpectation<'a> {
    /// The measured L1 the RPU was generated from. `None` when the RPU's L1 does not come from
    /// the sidecar (HDR10+ metadata, `--legacy-madvr-l1`).
    pub l1_sidecar: Option<&'a metadata::L1Sidecar>,
    /// Source range every frame must carry ([`metadata::expected_source_range`]).
    pub source_range: Option<(u16, u16)>,
}

/// Full verification with optional expected CM version for RPU content assertions.
/// `hlg_composer` is `Some` for HLG input: the sidecar must then load and name that Dolby Vision
/// 8.4 composer, and every RPU frame must carry the composer the sidecar names.
pub fn verify_post_mux_with_options(
    input_file: &str,
    output_file: &Path,
    measurements: Option<&Path>,
    temp_dir: &Path,
    expected_cm_version: Option<&str>,
    hlg_composer: Option<Composer>,
    delivery: Option<DeliveryExpectation<'_>>,
) -> bool {
    let mut ok = true;
    let hlg_source = hlg_composer.is_some();

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

    // Profile 8.4 must be signalled as HLG-compatible (ID 4). mkvmerge derives the ID from the
    // bitstream, so a missing HLG tag there would silently yield ID 2 (SDR).
    if hlg_source {
        match dv_bl_signal_compatibility_id(output_file) {
            Some(4) => println!("Dolby Vision compatibility ID 4 (HLG)."),
            Some(id) => {
                println!(
                    "{}",
                    format!("Dolby Vision compatibility ID is {id}; Profile 8.4 needs 4 (HLG).")
                        .red()
                );
                ok = false;
            }
            None => println!(
                "{}",
                "Could not read the Dolby Vision compatibility ID with ffprobe; skipping that check."
                    .yellow()
            ),
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

    let mut extract_rpu = external::dovi_tool_command();
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

    let mut summary_cmd = external::dovi_tool_command();
    summary_cmd.args(["info", "--summary", "-i", rpu_path.to_str().unwrap()]);
    let summary_frames = match external::get_command_output(&mut summary_cmd) {
        Ok(summary) => {
            let _ = std::fs::write(temp_dir.join("dovi_info_summary.log"), &summary);
            summary_frame_count(&summary)
        }
        Err(_) => None,
    };
    // A generated RPU is parsed in-process: its frame count then does not depend on the summary
    // output, and a parse failure fails verification instead of skipping the delivery checks.
    let parsed_rpus = match delivery {
        Some(_) => match parse_rpu_file(&rpu_path) {
            Ok(rpus) => Some(rpus),
            Err(error) => {
                println!(
                    "{}",
                    format!("Cannot parse the extracted RPU ({error:#}).").red()
                );
                ok = false;
                None
            }
        },
        None => None,
    };
    let rpu_frames = parsed_rpus
        .as_ref()
        .map(|rpus| rpus.len() as u64)
        .or(summary_frames);

    let mut frame_cmd = external::dovi_tool_command();
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

    // 3. Completeness: the RPU must cover every frame of the muxed video track. MediaInfo reads
    // mkvmerge's NUMBER_OF_FRAMES statistics tag, so the output count is exact.
    let output_frames = output_file.to_str().and_then(metadata::get_frame_count);
    let input_frames = metadata::get_frame_count(input_file);
    match (rpu_frames, output_frames) {
        (Some(rpu), Some(video)) if rpu != video => {
            println!(
                "{}",
                format!("RPU covers {rpu} frames but the output video track has {video}.").red()
            );
            ok = false;
        }
        (Some(rpu), Some(video)) => {
            println!("Frame count: RPU {rpu} = output video {video}.");
        }
        _ => println!(
            "{}",
            "Could not compare RPU and output video frame counts (dovi_tool summary or MediaInfo FrameCount missing)."
                .yellow()
        ),
    }
    // The input count can be a duration-based estimate when the source muxer wrote no
    // statistics tags, so a mismatch here is advisory.
    if let (Some(input), Some(video)) = (input_frames, output_frames) {
        if input != video {
            println!(
                "{}",
                format!("Output video has {video} frames but the input reports {input}; check for truncation.")
                    .yellow()
            );
        }
    }
    // An HLG output is only right when its RPU carries the composer the L1 was measured through,
    // so for HLG an unusable sidecar is a failure, not a skipped check.
    let sidecar = match (measurements, hlg_composer) {
        (Some(meas_path), _) => match metadata::load_l1_sidecar(
            meas_path,
            &metadata::SidecarExpectation::default().with_hlg_composer(hlg_composer),
        ) {
            Ok((sidecar, _advisories)) => Some(sidecar),
            Err(error) if hlg_source => {
                println!(
                    "{}",
                    format!("HLG output: the L1 sidecar cannot be verified ({error}).").red()
                );
                ok = false;
                None
            }
            Err(_) => None,
        },
        (None, Some(_)) => {
            println!(
                "{}",
                "HLG output: no measurements to verify the L1 sidecar and composer against.".red()
            );
            ok = false;
            None
        }
        (None, None) => None,
    };
    if let (Some(sidecar), Some(rpu)) = (&sidecar, rpu_frames) {
        if sidecar.frame_count() != rpu {
            println!(
                "{}",
                format!(
                    "RPU covers {rpu} frames but the L1 measurements cover {}.",
                    sidecar.frame_count()
                )
                .red()
            );
            ok = false;
        }
    }
    if let (true, Some(sidecar)) = (hlg_source, &sidecar) {
        if !verify_composer(sidecar, &rpu_path, parsed_rpus.as_deref()) {
            ok = false;
        }
    }

    // 4. What the generated RPU delivers against what was measured and the mastering range.
    if let (Some(delivery), Some(rpus)) = (delivery, &parsed_rpus) {
        let report = check_delivery(rpus, delivery);
        report.print(progress::is_verbose());
        if !report.passed() {
            ok = false;
        }
    }

    // 5. Duration consistency check on the video track (1-second tolerance).
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

/// Largest number of individual mismatches listed before the rest are only counted.
const MAX_LISTED_MISMATCHES: usize = 10;

/// How often the generator moved one L1 field away from the measured value, and by how much.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct FieldChange {
    scenes: usize,
    largest: u16,
}

impl FieldChange {
    fn record(&mut self, measured: u16, delivered: u16) -> bool {
        if measured == delivered {
            return false;
        }
        self.scenes += 1;
        self.largest = self.largest.max(measured.abs_diff(delivered));
        true
    }
}

/// Measured against delivered L1, per scene of the sidecar.
#[derive(Debug, Default)]
struct L1Delivery {
    scenes: usize,
    /// Scenes whose every frame carries exactly the measured L1.
    unchanged: usize,
    /// Scenes with at least one frame that differs from the clamped measurement (failures).
    mismatched: usize,
    min: FieldChange,
    max: FieldChange,
    avg: FieldChange,
    /// Scenes whose delivered L1 max lies above the RPU's `source_max_pq`.
    above_source_max: usize,
    /// One line per scene the generator changed (printed with `--verbose`).
    details: Vec<String>,
}

#[derive(Debug, Default)]
struct DeliveryReport {
    failures: Vec<String>,
    /// Failures beyond [`MAX_LISTED_MISMATCHES`], counted only.
    unlisted_failures: usize,
    /// The source range, when every frame carries the expected one.
    source_range: Option<(u16, u16)>,
    l1: Option<L1Delivery>,
}

impl DeliveryReport {
    fn passed(&self) -> bool {
        self.failures.is_empty() && self.unlisted_failures == 0
    }

    fn fail(&mut self, message: String) {
        if self.failures.len() < MAX_LISTED_MISMATCHES {
            self.failures.push(message);
        } else {
            self.unlisted_failures += 1;
        }
    }

    fn print(&self, verbose: bool) {
        if let Some((min, max)) = self.source_range {
            println!("Source range: source_min_pq {min}, source_max_pq {max} on every frame.");
        }
        if let Some(l1) = &self.l1 {
            println!("{}", l1_summary(l1));
            if verbose {
                for line in &l1.details {
                    println!("  {line}");
                }
            }
            if l1.above_source_max > 0 {
                println!(
                    "{}",
                    format!(
                        "{} scene(s) carry an L1 max above source_max_pq (brighter than the mastering display peak).",
                        l1.above_source_max
                    )
                    .yellow()
                );
            }
        }
        for failure in &self.failures {
            println!("{}", format!("Delivery FAIL: {failure}").red());
        }
        if self.unlisted_failures > 0 {
            println!(
                "{}",
                format!(
                    "Delivery FAIL: {} more mismatch(es) not listed.",
                    self.unlisted_failures
                )
                .red()
            );
        }
    }
}

fn l1_summary(l1: &L1Delivery) -> String {
    let mut text = format!(
        "L1 delivered as measured in {} of {} scenes",
        l1.unchanged, l1.scenes
    );
    if l1.mismatched > 0 {
        text.push_str(&format!(
            "; {} scene(s) do not carry the measured L1 even after the generator's limits",
            l1.mismatched
        ));
    }
    let fields = [
        ("min lowered to the generator's limit", l1.min),
        ("max raised to the generator's limit", l1.max),
        ("avg moved to the generator's limits", l1.avg),
    ];
    for (what, change) in fields {
        if change.scenes > 0 {
            text.push_str(&format!(
                "; {what} in {} (largest change {} codes)",
                change.scenes, change.largest
            ));
        }
    }
    text.push('.');
    text
}

/// L1 block of one RPU frame (CM v2.9 metadata, where every Profile 8 RPU carries it).
fn frame_l1(rpu: &DoviRpu) -> Option<&ExtMetadataBlockLevel1> {
    match rpu.vdr_dm_data.as_ref()?.get_block(1)? {
        ExtMetadataBlock::Level1(level1) => Some(level1),
        _ => None,
    }
}

fn l1_codes(level1: &ExtMetadataBlockLevel1) -> (u16, u16, u16) {
    (level1.min_pq, level1.avg_pq, level1.max_pq)
}

/// Compare a parsed, generated RPU with what it must deliver: the source range on every frame,
/// and for every sidecar scene the measured L1 after the generator's documented limits
/// (`dovi_tool generate` clamps with `l1_avg_pq_cm_version: V29`, which mkvdovi sets whenever it
/// embeds measured shots). A frame that differs from the clamped measurement is a failure; the
/// clamps themselves are counted.
fn check_delivery(rpus: &[DoviRpu], delivery: DeliveryExpectation<'_>) -> DeliveryReport {
    let mut report = DeliveryReport::default();

    if let Some((min, max)) = delivery.source_range {
        for (index, rpu) in rpus.iter().enumerate() {
            match rpu.vdr_dm_data.as_ref() {
                Some(dm) if (dm.source_min_pq, dm.source_max_pq) == (min, max) => {}
                Some(dm) => report.fail(format!(
                    "frame {index} carries source range {}..{}, expected {min}..{max}",
                    dm.source_min_pq, dm.source_max_pq
                )),
                None => report.fail(format!("frame {index} carries no display management data")),
            }
        }
        if report.passed() {
            report.source_range = Some((min, max));
        }
    }

    let Some(sidecar) = delivery.l1_sidecar else {
        return report;
    };
    if sidecar.frame_count() != rpus.len() as u64 {
        report.fail(format!(
            "L1 not compared, because the frame counts differ (RPU {}, measurements {})",
            rpus.len(),
            sidecar.frame_count()
        ));
        return report;
    }

    let mut l1 = L1Delivery {
        scenes: sidecar.scenes.len(),
        ..Default::default()
    };
    for (scene_index, scene) in sidecar.scenes.iter().enumerate() {
        let measured = (
            scene.min_pq_12bit,
            scene.avg_max_rgb_pq_12bit,
            scene.max_pq_12bit,
        );
        let expected = l1_codes(&ExtMetadataBlockLevel1::from_stats_cm_version(
            scene.min_pq_12bit,
            scene.max_pq_12bit,
            scene.avg_max_rgb_pq_12bit,
            CmVersion::V29,
        ));

        let mut scene_ok = true;
        for frame in scene.start..=scene.end {
            let delivered = rpus.get(frame as usize).and_then(frame_l1).map(l1_codes);
            if delivered != Some(expected) {
                scene_ok = false;
                report.fail(match delivered {
                    Some((min, avg, max)) => format!(
                        "scene {scene_index} frame {frame}: L1 min/avg/max {min}/{avg}/{max}, expected {}/{}/{} (measured {}/{}/{})",
                        expected.0, expected.1, expected.2, measured.0, measured.1, measured.2
                    ),
                    None => format!("scene {scene_index} frame {frame}: no L1 block"),
                });
            }
        }
        if !scene_ok {
            l1.mismatched += 1;
            continue;
        }

        let changed_min = l1.min.record(measured.0, expected.0);
        let changed_avg = l1.avg.record(measured.1, expected.1);
        let changed_max = l1.max.record(measured.2, expected.2);
        if !(changed_min || changed_avg || changed_max) {
            l1.unchanged += 1;
        } else {
            l1.details.push(format!(
                "scene {scene_index} (frames {}-{}): measured min/avg/max {}/{}/{}, delivered {}/{}/{}",
                scene.start,
                scene.end,
                measured.0,
                measured.1,
                measured.2,
                expected.0,
                expected.1,
                expected.2
            ));
        }
        let source_max = rpus
            .get(scene.start as usize)
            .and_then(|rpu| rpu.vdr_dm_data.as_ref())
            .map(|dm| dm.source_max_pq);
        if source_max.is_some_and(|source_max| expected.2 > source_max) {
            l1.above_source_max += 1;
        }
    }
    report.l1 = Some(l1);
    report
}

/// Every frame of the extracted RPU must carry the composer the (already validated) sidecar
/// names, the one the L1 was measured through.
/// `parsed` is the already parsed RPU when there is one, so a long film is not held in memory twice.
fn verify_composer(
    sidecar: &metadata::L1Sidecar,
    rpu_path: &Path,
    parsed: Option<&[DoviRpu]>,
) -> bool {
    let Some(composer) = sidecar
        .luminance_mapping()
        .and_then(Composer::from_luminance_mapping)
    else {
        println!(
            "{}",
            format!(
                "The L1 sidecar names no known HLG composer (mapping {}).",
                sidecar.luminance_mapping().unwrap_or("none")
            )
            .red()
        );
        return false;
    };
    let checked = match parsed {
        Some(rpus) => dovi84_composer::check_rpus(rpus, composer),
        None => dovi84_composer::check_rpu_file(rpu_path, composer),
    };
    match checked {
        Ok(frames) => {
            println!(
                "HLG composer: all {frames} RPU frames carry {} ({}), as measured.",
                composer.cli_name(),
                composer.luminance_mapping()
            );
            true
        }
        Err(error) => {
            println!(
                "{}",
                format!(
                    "HLG composer check failed: the RPU does not carry {} ({}) on every frame: {error:#}",
                    composer.cli_name(),
                    composer.luminance_mapping()
                )
                .red()
            );
            false
        }
    }
}

/// Frame count from `dovi_tool info --summary` output (`Frames: N`).
fn summary_frame_count(summary: &str) -> Option<u64> {
    let after = &summary[summary.find("Frames:")? + "Frames:".len()..];
    let digits: String = after
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
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

/// `dv_bl_signal_compatibility_id` of the first video stream's Dolby Vision configuration record.
fn dv_bl_signal_compatibility_id(output_file: &Path) -> Option<u8> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_streams",
            "-of",
            "default=nw=1",
        ])
        .arg(output_file)
        .output()
        .ok()?;
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("dv_bl_signal_compatibility_id="))
        .and_then(|value| value.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use dolby_vision::rpu::generate::{GenerateConfig, VideoShot};

    /// One measured scene: start, end, min/avg/max in 12-bit PQ codes.
    type Scene = (u64, u64, (u16, u16, u16));

    fn sidecar(scenes: &[Scene]) -> metadata::L1Sidecar {
        metadata::L1Sidecar {
            version: 4,
            scenes: scenes
                .iter()
                .map(|&(start, end, (min, avg, max))| metadata::L1SidecarScene {
                    start,
                    end,
                    min_pq_12bit: min,
                    avg_luma_pq_12bit: avg,
                    avg_max_rgb_pq_12bit: avg,
                    max_pq_12bit: max,
                })
                .collect(),
            ..Default::default()
        }
    }

    /// RPUs as `dovi_tool generate` writes them from an extra.json with these shots: the L1 is
    /// clamped (`fixup_l1` with `l1_avg_pq_cm_version: V29`) unless `clamp` is false.
    fn generated_rpus(scenes: &[Scene], source_range: (u16, u16), clamp: bool) -> Vec<DoviRpu> {
        let shots: Vec<VideoShot> = scenes
            .iter()
            .map(|&(start, end, (min, avg, max))| VideoShot {
                start: start as usize,
                duration: (end - start + 1) as usize,
                metadata_blocks: vec![ExtMetadataBlock::Level1(ExtMetadataBlockLevel1::new(
                    min, max, avg,
                ))],
                ..Default::default()
            })
            .collect();
        let mut config = GenerateConfig {
            length: shots.iter().map(|shot| shot.duration).sum(),
            shots,
            source_min_pq: Some(source_range.0),
            source_max_pq: Some(source_range.1),
            l1_avg_pq_cm_version: Some(CmVersion::V29),
            ..Default::default()
        };
        if clamp {
            config.fixup_l1();
        }
        config.generate_rpu_list().unwrap()
    }

    fn delivery(sidecar: &metadata::L1Sidecar, range: (u16, u16)) -> DeliveryExpectation<'_> {
        DeliveryExpectation {
            l1_sidecar: Some(sidecar),
            source_range: Some(range),
        }
    }

    const RANGE: (u16, u16) = (62, 3079);

    #[test]
    fn delivery_passes_when_the_rpu_carries_the_measured_l1() {
        let scenes = [(0, 2, (5, 1500, 2900)), (3, 4, (10, 1200, 2500))];
        let measured = sidecar(&scenes);
        let report = check_delivery(
            &generated_rpus(&scenes, RANGE, true),
            delivery(&measured, RANGE),
        );

        assert!(report.passed(), "{:?}", report.failures);
        let l1 = report.l1.unwrap();
        assert_eq!((l1.scenes, l1.unchanged, l1.mismatched), (2, 2, 0));
        assert_eq!(l1.above_source_max, 0);
        assert_eq!(
            l1_summary(&l1),
            "L1 delivered as measured in 2 of 2 scenes."
        );
    }

    #[test]
    fn delivery_counts_the_generator_clamps() {
        // min above 12, max below 2081 (and the avg pulled under it), avg below 819.
        let scenes = [
            (0, 1, (40, 1500, 2900)),
            (2, 3, (5, 2050, 2060)),
            (4, 4, (0, 700, 2600)),
        ];
        let measured = sidecar(&scenes);
        let report = check_delivery(
            &generated_rpus(&scenes, RANGE, true),
            delivery(&measured, RANGE),
        );

        assert!(report.passed(), "{:?}", report.failures);
        let l1 = report.l1.unwrap();
        assert_eq!((l1.unchanged, l1.mismatched), (0, 0));
        assert_eq!(
            l1.min,
            FieldChange {
                scenes: 1,
                largest: 28
            }
        );
        assert_eq!(
            l1.max,
            FieldChange {
                scenes: 1,
                largest: 21
            }
        );
        assert_eq!(
            l1.avg,
            FieldChange {
                scenes: 1,
                largest: 119
            }
        );
        assert_eq!(l1.details.len(), 3);
        assert!(l1_summary(&l1)
            .contains("min lowered to the generator's limit in 1 (largest change 28 codes)"));
    }

    #[test]
    fn delivery_follows_the_avg_pull_down_below_max() {
        // A luma-domain sidecar can carry avg > max; the generator writes avg = max - 1.
        let scenes = [(0, 1, (0, 2700, 2600))];
        let measured = sidecar(&scenes);
        let report = check_delivery(
            &generated_rpus(&scenes, RANGE, true),
            delivery(&measured, RANGE),
        );

        assert!(report.passed(), "{:?}", report.failures);
        assert_eq!(
            report.l1.unwrap().avg,
            FieldChange {
                scenes: 1,
                largest: 101
            }
        );
    }

    #[test]
    fn delivery_fails_when_the_rpu_differs_from_the_clamped_measurement() {
        let scenes = [(0, 1, (40, 1500, 2900)), (2, 2, (0, 1000, 2500))];
        let measured = sidecar(&scenes);
        // Unclamped: min 40 is not what the generator delivers.
        let report = check_delivery(
            &generated_rpus(&scenes, RANGE, false),
            delivery(&measured, RANGE),
        );
        assert!(!report.passed());
        assert_eq!(report.failures.len(), 2, "one failure per frame of scene 0");
        assert!(report.failures[0].contains("scene 0 frame 0"));

        // Other L1 than measured.
        let other = sidecar(&[(0, 1, (40, 1500, 2900)), (2, 2, (0, 1001, 2500))]);
        let report = check_delivery(
            &generated_rpus(&scenes, RANGE, true),
            delivery(&other, RANGE),
        );
        assert!(!report.passed());
        assert!(report.failures[0].contains("scene 1 frame 2"));
        // A failed scene is never counted as delivered as measured.
        let l1 = report.l1.unwrap();
        assert_eq!((l1.scenes, l1.unchanged, l1.mismatched), (2, 0, 1));
        assert_eq!(l1.min.scenes, 1, "scene 0 is still counted as clamped");
        assert!(l1_summary(&l1).starts_with(
            "L1 delivered as measured in 0 of 2 scenes; 1 scene(s) do not carry the measured L1"
        ));
    }

    #[test]
    fn delivery_fails_on_a_wrong_source_range() {
        let scenes = [(0, 2, (0, 1500, 2900))];
        let measured = sidecar(&scenes);
        let report = check_delivery(
            &generated_rpus(&scenes, (7, 3079), true),
            delivery(&measured, RANGE),
        );

        assert!(!report.passed());
        assert_eq!(report.failures.len(), 3, "one failure per frame");
        assert!(report.failures[0].contains("source range 7..3079, expected 62..3079"));
    }

    #[test]
    fn delivery_fails_when_frame_counts_differ() {
        let scenes = [(0, 2, (0, 1500, 2900))];
        let longer = sidecar(&[(0, 3, (0, 1500, 2900))]);
        let report = check_delivery(
            &generated_rpus(&scenes, RANGE, true),
            delivery(&longer, RANGE),
        );

        assert!(!report.passed());
        assert!(report.l1.is_none());
        assert!(report.failures[0].contains("frame counts differ (RPU 3, measurements 4)"));
    }

    #[test]
    fn delivery_reports_l1_max_above_the_source_range() {
        let scenes = [(0, 0, (0, 1500, 3200)), (1, 1, (0, 1500, 2900))];
        let measured = sidecar(&scenes);
        let report = check_delivery(
            &generated_rpus(&scenes, RANGE, true),
            delivery(&measured, RANGE),
        );

        assert!(report.passed());
        assert_eq!(report.l1.unwrap().above_source_max, 1);
    }

    #[test]
    fn delivery_without_a_sidecar_checks_the_source_range_only() {
        let scenes = [(0, 1, (0, 1500, 2900))];
        let rpus = generated_rpus(&scenes, (7, 2851), true);
        let only_range = DeliveryExpectation {
            l1_sidecar: None,
            source_range: Some((7, 2851)),
        };

        let report = check_delivery(&rpus, only_range);
        assert!(report.passed());
        assert!(report.l1.is_none());
        assert!(check_delivery(&rpus, DeliveryExpectation::default()).passed());
    }

    #[test]
    fn delivery_caps_the_listed_mismatches() {
        let scenes = [(0, 29, (0, 1500, 2900))];
        let measured = sidecar(&scenes);
        let report = check_delivery(
            &generated_rpus(&scenes, (7, 3079), true),
            delivery(&measured, RANGE),
        );

        assert_eq!(report.failures.len(), MAX_LISTED_MISMATCHES);
        assert_eq!(report.unlisted_failures, 30 - MAX_LISTED_MISMATCHES);
    }

    #[test]
    fn summary_frame_count_is_parsed() {
        let summary = "Parsing RPU file...\nSummary:\n  Frames: 2908\n  Profile: 8\n";
        assert_eq!(summary_frame_count(summary), Some(2908));
        assert_eq!(summary_frame_count("no summary"), None);
    }

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
