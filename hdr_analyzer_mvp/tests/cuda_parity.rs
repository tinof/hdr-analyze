//! CPU vs CUDA parity test, gated on operator-supplied clips and a `+cuda` build.
//!
//! Runs the analyzer twice on the same clip, with `--hwaccel none` and `--hwaccel cuda`
//! (both `--downscale 1 --disable-optimizer`), and asserts that the `.bin` files are
//! byte-identical and that the `.l1.json` sidecars agree on `crop`, `scenes` and `frames`.
//! The CUDA sidecar must say `analysis.gpu == true`, so a silent CPU fallback fails the
//! test instead of comparing the CPU path with itself.
//!
//! Clips: `HDR_ANALYZE_CUDA_PARITY_PQ` and `HDR_ANALYZE_CUDA_PARITY_HLG` (HEVC 10-bit).
//! Without them the test skips with a note. With `HDR_ANALYZE_CUDA_PARITY_REQUIRED` set
//! (any value), a missing variable, a missing file or a build without the `cuda` feature
//! is a failure. `scripts/cuda-parity.sh` generates the clips and runs this test that way.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const PQ_VAR: &str = "HDR_ANALYZE_CUDA_PARITY_PQ";
const HLG_VAR: &str = "HDR_ANALYZE_CUDA_PARITY_HLG";
const REQUIRED_VAR: &str = "HDR_ANALYZE_CUDA_PARITY_REQUIRED";

/// Why the test cannot run; `None` when both clips and a `+cuda` analyzer are available.
fn unavailable_reason() -> Option<String> {
    for var in [PQ_VAR, HLG_VAR] {
        let Some(value) = std::env::var_os(var) else {
            return Some(format!("{var} is not set"));
        };
        if !Path::new(&value).is_file() {
            return Some(format!(
                "{var} does not exist: {}",
                Path::new(&value).display()
            ));
        }
    }
    let output = Command::new(env!("CARGO_BIN_EXE_hdr_analyzer_mvp"))
        .arg("--version")
        .output()
        .expect("run analyzer --version");
    let version = String::from_utf8_lossy(&output.stdout);
    if !version.contains("+cuda") {
        return Some(format!(
            "analyzer was built without the cuda feature ({})",
            version.trim()
        ));
    }
    None
}

struct Run {
    bin: Vec<u8>,
    sidecar: Value,
    /// `--dump-frame-stats` CSV: peak candidates and grain statistics per frame.
    frame_stats: String,
}

fn run_analyzer(
    clip: &Path,
    bin: &Path,
    hwaccel: &str,
    no_crop: bool,
    extra_args: &[&str],
    label: &str,
) -> Run {
    let stats_path = PathBuf::from(format!("{}.stats.csv", bin.display()));
    let mut command = Command::new(env!("CARGO_BIN_EXE_hdr_analyzer_mvp"));
    command.arg(clip).arg("-o").arg(bin).args([
        "--hwaccel",
        hwaccel,
        "--downscale",
        "1",
        "--disable-optimizer",
    ]);
    if no_crop {
        command.arg("--no-crop");
    }
    command.args(extra_args);
    command.arg("--dump-frame-stats").arg(&stats_path);
    let output = command.output().expect("run analyzer");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{label}: analyzer --hwaccel {hwaccel} failed ({})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status
    );
    // Shows whether NVDEC frames were analyzed in place or downloaded (visible with --nocapture).
    for line in stdout.lines().chain(stderr.lines()) {
        if line.contains("CUDA") {
            eprintln!("{label} [{hwaccel}]: {}", line.trim());
        }
    }

    let sidecar_path = PathBuf::from(format!("{}.l1.json", bin.display()));
    let sidecar_text = std::fs::read_to_string(&sidecar_path)
        .unwrap_or_else(|error| panic!("{label}: read {}: {error}", sidecar_path.display()));
    Run {
        bin: std::fs::read(bin).expect("read measurements"),
        sidecar: serde_json::from_str(&sidecar_text).expect("parse L1 sidecar"),
        frame_stats: std::fs::read_to_string(&stats_path).expect("read frame stats"),
    }
}

/// Index of the first differing element when both values are arrays of the same length.
fn first_array_difference(cpu: &Value, gpu: &Value) -> Option<String> {
    let (cpu, gpu) = (cpu.as_array()?, gpu.as_array()?);
    if cpu.len() != gpu.len() {
        return Some(format!("length {} vs {}", cpu.len(), gpu.len()));
    }
    let index = cpu.iter().zip(gpu).position(|(a, b)| a != b)?;
    let count = cpu.iter().zip(gpu).filter(|(a, b)| a != b).count();
    Some(format!(
        "{count} of {} entries differ, first at index {index}: cpu {} vs gpu {}",
        cpu.len(),
        cpu[index],
        gpu[index]
    ))
}

/// Human-readable differences between two sidecar sections (empty when equal).
fn section_differences(section: &str, cpu: &Value, gpu: &Value) -> Vec<String> {
    if cpu == gpu {
        return Vec::new();
    }
    if let Some(difference) = first_array_difference(cpu, gpu) {
        return vec![format!("{section}: {difference}")];
    }
    if let (Some(cpu_map), Some(gpu_map)) = (cpu.as_object(), gpu.as_object()) {
        let mut out = Vec::new();
        for (key, cpu_value) in cpu_map {
            let gpu_value = gpu_map.get(key).unwrap_or(&Value::Null);
            if cpu_value == gpu_value {
                continue;
            }
            let detail = first_array_difference(cpu_value, gpu_value)
                .unwrap_or_else(|| format!("cpu {cpu_value} vs gpu {gpu_value}"));
            out.push(format!("{section}.{key}: {detail}"));
        }
        for key in gpu_map.keys().filter(|key| !cpu_map.contains_key(*key)) {
            out.push(format!("{section}.{key}: missing in the CPU sidecar"));
        }
        return out;
    }
    vec![format!("{section}: cpu {cpu} vs gpu {gpu}")]
}

fn check_parity(label: &str, clip: &Path, mapping: &str, no_crop: bool, extra_args: &[&str]) {
    let dir = tempfile::tempdir().expect("tempdir");
    let run = |name: &str, hwaccel: &str| {
        run_analyzer(
            clip,
            &dir.path().join(name),
            hwaccel,
            no_crop,
            extra_args,
            label,
        )
    };
    let cpu = run("cpu.bin", "none");
    let gpu = run("gpu.bin", "cuda");

    assert_eq!(
        cpu.sidecar["analysis"]["gpu"],
        Value::Bool(false),
        "{label}: the --hwaccel none run must be CPU analysis"
    );
    assert_eq!(
        gpu.sidecar["analysis"]["gpu"],
        Value::Bool(true),
        "{label}: the --hwaccel cuda run did not analyze on the GPU (CPU fallback)"
    );
    for (name, run) in [("cpu", &cpu), ("gpu", &gpu)] {
        assert_eq!(
            run.sidecar["analysis"]["luminance_mapping"], mapping,
            "{label}: unexpected {name} luminance_mapping"
        );
        assert_eq!(
            run.sidecar["analysis"]["no_crop"],
            Value::Bool(no_crop),
            "{label}: unexpected {name} no_crop"
        );
        for section in ["crop", "scenes", "frames"] {
            assert!(
                !run.sidecar[section].is_null(),
                "{label}: {name} sidecar has no `{section}`"
            );
        }
    }
    let frame_count = cpu.sidecar["frames"]["min_pq_12bit"]
        .as_array()
        .map_or(0, Vec::len);
    assert!(frame_count > 0, "{label}: the clip produced no frames");

    // The light-level block is only written for max-RGB runs.
    assert!(
        extra_args.contains(&"luma") || !cpu.sidecar["light_level"].is_null(),
        "{label}: cpu sidecar has no `light_level`"
    );
    let differences: Vec<String> = ["crop", "light_level", "scenes", "frames"]
        .into_iter()
        .flat_map(|section| {
            section_differences(section, &cpu.sidecar[section], &gpu.sidecar[section])
        })
        .collect();
    assert!(
        differences.is_empty(),
        "{label}: CPU and CUDA sidecars differ:\n  {}",
        differences.join("\n  ")
    );
    assert!(
        cpu.bin == gpu.bin,
        "{label}: CPU and CUDA .bin files differ ({} vs {} bytes) although the sidecars agree",
        cpu.bin.len(),
        gpu.bin.len()
    );
    // Peak candidates and grain statistics per frame. With the robust estimator every column
    // must be equal: sigma and the effective tail count come from the cross-quad difference
    // histogram, so this proves the kernel counts like the CPU. Without it the GPU does not
    // gather that histogram and reports neutral grain statistics.
    if extra_args.contains(&"robust") {
        assert_eq!(
            cpu.frame_stats.lines().count(),
            frame_count + 1,
            "{label}: frame-stats rows"
        );
        if let Some((line, (cpu_row, gpu_row))) = cpu
            .frame_stats
            .lines()
            .zip(gpu.frame_stats.lines())
            .enumerate()
            .find(|(_, (cpu_row, gpu_row))| cpu_row != gpu_row)
        {
            panic!("{label}: frame statistics differ at line {line}:\n  cpu {cpu_row}\n  gpu {gpu_row}");
        }
        assert_eq!(
            cpu.frame_stats.len(),
            gpu.frame_stats.len(),
            "{label}: frame statistics differ in length"
        );
        let corrected = cpu
            .frame_stats
            .lines()
            .skip(1)
            .filter(|row| {
                row.split(',')
                    .nth(6)
                    .is_some_and(|value| value != "0.000000000000")
            })
            .count();
        assert!(
            corrected > 0,
            "{label}: no frame carries a grain correction"
        );
        eprintln!("{label}: {corrected} of {frame_count} frames carry a grain correction");
    }
    eprintln!(
        "{label}: identical ({frame_count} frames, {} bytes)",
        cpu.bin.len()
    );
}

// One test for all cases: parallel tests would run several CUDA processes at once.
#[test]
fn cuda_output_matches_cpu() {
    if let Some(reason) = unavailable_reason() {
        assert!(
            std::env::var_os(REQUIRED_VAR).is_none(),
            "{REQUIRED_VAR} is set but the CUDA parity test cannot run: {reason}"
        );
        eprintln!("Skipping: {reason}");
        return;
    }
    let pq = PathBuf::from(std::env::var_os(PQ_VAR).expect("checked above"));
    let hlg = PathBuf::from(std::env::var_os(HLG_VAR).expect("checked above"));

    for (name, clip, mapping) in [("PQ", &pq, "pq"), ("HLG", &hlg, "dovi84-v2")] {
        for no_crop in [false, true] {
            let crop = if no_crop {
                "--no-crop"
            } else {
                "crop detection"
            };
            check_parity(&format!("{name}, {crop}"), clip, mapping, no_crop, &[]);
        }
        // The grain-robust estimator in both peak domains.
        for domain in ["max-rgb", "luma"] {
            check_parity(
                &format!("{name}, robust, {domain}"),
                clip,
                mapping,
                true,
                &[
                    "--peak-estimator",
                    "robust",
                    "--peak-source",
                    "max",
                    "--peak-domain",
                    domain,
                ],
            );
        }
    }
}
