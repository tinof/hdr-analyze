//! CPU vs CUDA parity test, gated on operator-supplied clips and a `+cuda` build.
//!
//! Runs the analyzer twice on the same clip, with `--hwaccel none` and `--hwaccel cuda`
//! (both `--downscale 1 --disable-optimizer`), and asserts that the `.bin` files are
//! byte-identical and that the `.l1.json` sidecars agree on `crop`, `scenes` and `frames`.
//! The CUDA sidecar must say `analysis.gpu == true`, so a silent CPU fallback fails the
//! test instead of comparing the CPU path with itself.
//!
//! Clips: `HDR_ANALYZE_CUDA_PARITY_PQ` and `HDR_ANALYZE_CUDA_PARITY_HLG` (HEVC 10-bit).
//! The HLG clip runs once per Profile 8.4 composer listed in
//! `HDR_ANALYZE_CUDA_PARITY_HLG_COMPOSERS` (comma-separated `--hlg-composer` names; unset
//! means every composer), and its sidecars must name that composer's `luminance_mapping`.
//! The PQ clip also runs with `--hlg-composer bt2100`, which must not change its output.
//! Without the clips the test skips with a note. With `HDR_ANALYZE_CUDA_PARITY_REQUIRED` set
//! (any value), a missing variable, a missing file or a build without the `cuda` feature
//! is a failure. `scripts/cuda-parity.sh` generates the clips and runs this test that way.
//!
//! Every run also writes `--dump-pq-hist` (ROADMAP WS7.1): the 4096-bin PQ histograms must be
//! byte-identical too, 64 header bytes plus 16 KiB per frame.
//!
//! `nvdec_cpu_analysis_matches_software_decode` (ROADMAP E12.2) runs CPU analysis of NVDEC
//! frames (`--hwaccel cuda` without GPU analysis) against software decode (`--hwaccel none`):
//! `.bin` and sidecar byte-identical, every NVDEC frame repacked, the same crop-monitor report.
//! A build without `cuda` runs every case; a `+cuda` build only `--pre-denoise median3`, its one
//! CPU analysis under `--hwaccel cuda`. It needs the two clips and the letterboxed PQ clip
//! `HDR_ANALYZE_CUDA_PARITY_LETTERBOX`, and an NVDEC host: without NVDEC the analyzer decodes in
//! software and the repack count fails the test. `scripts/cuda-parity.sh` runs it with both
//! builds.

use std::path::{Path, PathBuf};
use std::process::Command;

use dovi84_composer::Composer;
use serde_json::Value;

const PQ_VAR: &str = "HDR_ANALYZE_CUDA_PARITY_PQ";
const HLG_VAR: &str = "HDR_ANALYZE_CUDA_PARITY_HLG";
const HLG_COMPOSERS_VAR: &str = "HDR_ANALYZE_CUDA_PARITY_HLG_COMPOSERS";
const REQUIRED_VAR: &str = "HDR_ANALYZE_CUDA_PARITY_REQUIRED";
/// A PQ clip with black bars, for crop detection on the analysis frame (`--crop-probes 0`).
const LETTERBOX_VAR: &str = "HDR_ANALYZE_CUDA_PARITY_LETTERBOX";

/// The composers the HLG clip is checked with: [`HLG_COMPOSERS_VAR`], or every composer.
fn hlg_composers() -> Vec<Composer> {
    let Ok(names) = std::env::var(HLG_COMPOSERS_VAR) else {
        return Composer::ALL.to_vec();
    };
    names
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| {
            Composer::from_cli_name(name)
                .unwrap_or_else(|| panic!("{HLG_COMPOSERS_VAR}: unknown composer '{name}'"))
        })
        .collect()
}

/// Why one of the clip variables `vars` cannot be used; `None` when each names a file.
fn clips_unavailable(vars: &[&str]) -> Option<String> {
    for var in vars {
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
    None
}

/// The analyzer's `--version` line.
fn analyzer_version() -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_hdr_analyzer_mvp"))
        .arg("--version")
        .output()
        .expect("run analyzer --version");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Why `cuda_output_matches_cpu` cannot run; `None` when both clips and a `+cuda` analyzer are
/// available.
fn unavailable_reason() -> Option<String> {
    clips_unavailable(&[PQ_VAR, HLG_VAR]).or_else(|| {
        let version = analyzer_version();
        (!version.contains("+cuda"))
            .then(|| format!("analyzer was built without the cuda feature ({version})"))
    })
}

/// The clip a variable names, after [`clips_unavailable`] checked it.
fn clip(var: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(var).expect("checked by clips_unavailable"))
}

struct Run {
    bin: Vec<u8>,
    sidecar: Value,
    /// The sidecar as written, for byte comparisons.
    sidecar_text: String,
    stdout: String,
    /// `--dump-frame-stats` CSV: peak candidates and grain statistics per frame.
    frame_stats: String,
    /// The `--au-table` output, for MKV clips (ROADMAP P15.1).
    au_table: Option<Value>,
    /// The `--dump-pq-hist` output (ROADMAP WS7.1).
    pq_hist: Vec<u8>,
}

/// Where two `--dump-pq-hist` files differ (frame and bin), or a wrong length; `None` if equal
/// and `frames` long.
fn pq_hist_difference(a: &[u8], b: &[u8], frames: usize) -> Option<String> {
    let expected = 64 + frames * 16_384;
    if a.len() != expected || b.len() != expected {
        return Some(format!(
            "{} and {} bytes, {expected} expected",
            a.len(),
            b.len()
        ));
    }
    let byte = a.iter().zip(b).position(|(x, y)| x != y)?;
    Some(match byte.checked_sub(64) {
        None => format!("header byte {byte}"),
        Some(offset) => format!("frame {}, bin {}", offset / 16_384, offset % 16_384 / 4),
    })
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
    let pq_hist_path = PathBuf::from(format!("{}.pqhist", bin.display()));
    command.arg("--dump-pq-hist").arg(&pq_hist_path);
    let au_table = clip
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mkv"));
    if au_table {
        command.arg("--au-table");
    }
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
        sidecar_text,
        stdout: stdout.into_owned(),
        frame_stats: std::fs::read_to_string(&stats_path).expect("read frame stats"),
        au_table: au_table.then(|| {
            let path = PathBuf::from(format!("{}.au.json", bin.display()));
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{label}: read {}: {error}", path.display()));
            serde_json::from_str(&text).expect("parse AU table")
        }),
        pq_hist: std::fs::read(&pq_hist_path).expect("read PQ histogram dump"),
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

/// Asserts CPU/CUDA parity for one configuration and returns the CPU `.bin`.
fn check_parity(
    label: &str,
    clip: &Path,
    mapping: &str,
    no_crop: bool,
    extra_args: &[&str],
) -> Vec<u8> {
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
    // `section_differences` below passes a key both sides lack, so check the frame peaks exist.
    for (name, run) in [("cpu", &cpu), ("gpu", &gpu)] {
        assert_eq!(
            run.sidecar["frames"]["max_pq_12bit"]
                .as_array()
                .map_or(0, Vec::len),
            frame_count,
            "{label}: {name} sidecar frame peaks"
        );
    }
    assert!(
        cpu.sidecar["peak_source"].is_string(),
        "{label}: cpu sidecar has no `peak_source`"
    );
    assert_eq!(
        cpu.sidecar["peak_source"], gpu.sidecar["peak_source"],
        "{label}: CPU and CUDA peak sources differ"
    );

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
    if let Some(difference) = pq_hist_difference(&cpu.pq_hist, &gpu.pq_hist, frame_count) {
        panic!("{label}: CPU and CUDA PQ histogram dumps differ: {difference}");
    }
    // The AU table comes from packets and the decoder's output pts, so NVDEC must give the same
    // table as software decode. Only the sidecar digest differs (`analysis.gpu`).
    if let (Some(cpu_table), Some(gpu_table)) = (&cpu.au_table, &gpu.au_table) {
        let without_digest = |table: &Value| {
            let mut table = table.clone();
            table
                .as_object_mut()
                .expect("AU table is an object")
                .remove("sidecar_sha256");
            table
        };
        assert!(
            without_digest(cpu_table) == without_digest(gpu_table),
            "{label}: CPU and CUDA AU tables differ"
        );
    }
    check_frame_stats(label, &cpu, &gpu, frame_count, extra_args);
    eprintln!(
        "{label}: identical ({frame_count} frames, {} bytes)",
        cpu.bin.len()
    );
    cpu.bin
}

/// Compares the `--dump-frame-stats` CSVs of a CPU and a CUDA run.
// Peak candidates and grain statistics per frame. With the robust estimator every column
// must be equal: sigma and the effective tail count come from the cross-quad difference
// histogram, so this proves the kernel counts like the CPU. Without it the GPU does not
// gather that histogram and reports neutral grain statistics. With st2094-10 the
// reduced-set columns must be equal too: they come from their own kernel.
fn check_frame_stats(label: &str, cpu: &Run, gpu: &Run, frame_count: usize, extra_args: &[&str]) {
    let robust = extra_args.contains(&"robust");
    let reduced_set = extra_args.contains(&"st2094-10");
    if robust || reduced_set {
        assert_eq!(
            cpu.frame_stats.lines().count(),
            frame_count + 1,
            "{label}: frame-stats rows"
        );
        assert_eq!(
            cpu.frame_stats.lines().count(),
            gpu.frame_stats.lines().count(),
            "{label}: frame statistics differ in length"
        );
        let header: Vec<&str> = cpu
            .frame_stats
            .lines()
            .next()
            .expect("frame-stats header")
            .split(',')
            .collect();
        assert_eq!(
            gpu.frame_stats.lines().next(),
            cpu.frame_stats.lines().next(),
            "{label}: frame-stats headers differ"
        );
        let column = |name: &str| {
            header
                .iter()
                .position(|&column| column == name)
                .unwrap_or_else(|| panic!("{label}: frame-stats CSV has no `{name}` column"))
        };
        // The unrounded max-RGB average comes from an f64 sum on the CPU and a u64 fixed-point
        // sum (truncated to 2^-32 per pixel) on the GPU, so it agrees only to within that
        // truncation; every other column must be equal.
        let average = column("avg_max_rgb_pq");
        for (line, (cpu_row, gpu_row)) in cpu
            .frame_stats
            .lines()
            .zip(gpu.frame_stats.lines())
            .enumerate()
            .skip(1)
        {
            let (cpu_values, gpu_values): (Vec<&str>, Vec<&str>) =
                (cpu_row.split(',').collect(), gpu_row.split(',').collect());
            let agree = cpu_values.len() == header.len()
                && gpu_values.len() == header.len()
                && cpu_values.iter().zip(&gpu_values).enumerate().all(|(index, (a, b))| {
                    if index == average {
                        let parse = |value: &str| value.parse::<f64>().ok();
                        matches!((parse(a), parse(b)), (Some(a), Some(b)) if (a - b).abs() <= 1e-9)
                    } else {
                        a == b
                    }
                });
            assert!(
                agree,
                "{label}: frame statistics differ at line {line}:\n  cpu {cpu_row}\n  gpu {gpu_row}"
            );
        }
        let rows = || {
            cpu.frame_stats
                .lines()
                .skip(1)
                .map(|row| row.split(',').collect::<Vec<_>>())
        };
        if robust {
            let correction = column("correction_pq");
            let corrected = rows()
                .filter(|values| values[correction] != "0.000000000000")
                .count();
            assert!(
                corrected > 0,
                "{label}: no frame carries a grain correction"
            );
            eprintln!("{label}: {corrected} of {frame_count} frames carry a grain correction");
        }
        if reduced_set {
            // The reduced-set maximum is the selected peak, and it never exceeds the raw pixel
            // maximum (a block mean cannot exceed its brightest pixel's brightest component).
            let (selected, raw) = (column("selected_pq"), column("raw_max_pq"));
            let (high, low) = (column("reduced_max_pq"), column("reduced_min_pq"));
            for values in rows() {
                let parse = |index: usize| values[index].parse::<f64>().expect("numeric");
                assert_eq!(
                    values[selected], values[high],
                    "{label}: selected is not reduced max"
                );
                assert!(
                    parse(high) > 0.0 && parse(high) <= parse(raw) + 0.5 / 4095.0,
                    "{label}: {values:?}"
                );
                assert!(parse(low) <= parse(high), "{label}: {values:?}");
            }
        }
    }
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
    let pq = clip(PQ_VAR);
    let hlg = clip(HLG_VAR);

    let composers = hlg_composers();
    assert!(
        !composers.is_empty(),
        "{HLG_COMPOSERS_VAR} names no composer"
    );
    let mut cases = vec![("PQ".to_owned(), &pq, "pq", Vec::new())];
    for composer in composers {
        cases.push((
            format!("HLG {}", composer.cli_name()),
            &hlg,
            composer.luminance_mapping(),
            vec!["--hlg-composer", composer.cli_name()],
        ));
    }

    for (name, clip, mapping, composer_args) in &cases {
        for no_crop in [false, true] {
            let crop = if no_crop {
                "--no-crop"
            } else {
                "crop detection"
            };
            check_parity(
                &format!("{name}, {crop}"),
                clip,
                mapping,
                no_crop,
                composer_args,
            );
        }
        // The grain-robust estimator in both peak domains.
        for (domain, no_crop) in [
            ("max-rgb", true),
            ("max-rgb", false),
            ("luma", true),
            ("luma", false),
        ] {
            let mut args = composer_args.clone();
            args.extend([
                "--peak-estimator",
                "robust",
                "--peak-source",
                "max",
                "--peak-domain",
                domain,
            ]);
            check_parity(
                &format!("{name}, robust, {domain}, no_crop={no_crop}"),
                clip,
                mapping,
                no_crop,
                &args,
            );
        }
    }

    // The ST 2094-10 reduced set (PQ only): its own kernel, with crop detection and without.
    for no_crop in [true, false] {
        check_parity(
            &format!("PQ, st2094-10, no_crop={no_crop}"),
            &pq,
            "pq",
            no_crop,
            &["--peak-estimator", "st2094-10", "--peak-source", "max"],
        );
    }

    // The composer only affects HLG: a PQ run with a non-default composer is unchanged.
    let pq_default = check_parity("PQ, default composer", &pq, "pq", true, &[]);
    let pq_bt2100 = check_parity(
        "PQ, --hlg-composer bt2100",
        &pq,
        "pq",
        true,
        &["--hlg-composer", "bt2100"],
    );
    assert!(
        pq_default == pq_bt2100,
        "PQ: --hlg-composer bt2100 changed the measurements"
    );
}

/// One configuration of [`nvdec_cpu_analysis_matches_software_decode`].
struct DecodeCase {
    label: String,
    clip: PathBuf,
    mapping: &'static str,
    no_crop: bool,
    args: Vec<&'static str>,
    /// The clip has black bars, so the committed crop must not be the full frame.
    letterboxed: bool,
}

/// The crop-monitor report of a run (`CropStabilityMonitor::report`).
fn crop_report(run: &Run) -> Vec<&str> {
    run.stdout
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("Crop stability:") || line.starts_with("Variable active"))
        .collect()
}

/// Asserts that NVDEC decode with CPU analysis measures what software decode measures.
fn check_nvdec_cpu_analysis(case: &DecodeCase) {
    let label = case.label.as_str();
    let dir = tempfile::tempdir().expect("tempdir");
    let run = |name: &str, hwaccel: &str| {
        run_analyzer(
            &case.clip,
            &dir.path().join(name),
            hwaccel,
            case.no_crop,
            &case.args,
            label,
        )
    };
    let software = run("software.bin", "none");
    let nvdec = run("nvdec.bin", "cuda");

    for (name, run) in [("software", &software), ("nvdec", &nvdec)] {
        assert_eq!(run.sidecar["version"], 8, "{label}: {name} sidecar version");
        assert_eq!(
            run.sidecar["analysis"]["gpu"],
            Value::Bool(false),
            "{label}: the {name} run must be CPU analysis"
        );
        assert_eq!(
            run.sidecar["analysis"]["luminance_mapping"], case.mapping,
            "{label}: {name} luminance_mapping"
        );
    }
    let frames = software.sidecar["frames"]["min_pq_12bit"]
        .as_array()
        .map_or(0, Vec::len);
    assert!(frames > 0, "{label}: the clip produced no frames");
    // The analyzer falls back to software decode when NVDEC does not open, so only the count
    // proves that the --hwaccel cuda run analyzed NVDEC frames.
    let repacked = format!("NVDEC P010 frames repacked for CPU analysis: {frames}");
    assert!(
        nvdec.stdout.lines().any(|line| line.trim() == repacked),
        "{label}: the --hwaccel cuda run did not repack all {frames} frames from NVDEC:\n{}",
        nvdec.stdout
    );
    assert!(
        !software.stdout.contains("NVDEC P010 frames repacked"),
        "{label}: software decode repacked frames"
    );

    let differences: Vec<String> = ["crop", "light_level", "scenes", "frames"]
        .into_iter()
        .flat_map(|section| {
            section_differences(section, &software.sidecar[section], &nvdec.sidecar[section])
        })
        .collect();
    assert!(
        software.sidecar_text == nvdec.sidecar_text,
        "{label}: the sidecars differ:\n  {}",
        differences.join("\n  ")
    );
    assert!(
        software.bin == nvdec.bin,
        "{label}: the .bin files differ ({} vs {} bytes)",
        software.bin.len(),
        nvdec.bin.len()
    );
    if let Some(difference) = pq_hist_difference(&software.pq_hist, &nvdec.pq_hist, frames) {
        panic!("{label}: the PQ histogram dumps differ: {difference}");
    }
    assert!(
        software.frame_stats == nvdec.frame_stats,
        "{label}: the frame statistics differ"
    );
    assert!(
        software.au_table == nvdec.au_table,
        "{label}: the AU tables differ"
    );

    let source = &software.sidecar["source"];
    let crop = &software.sidecar["crop"];
    let full_frame = crop["x"] == 0
        && crop["y"] == 0
        && crop["width"] == source["width"]
        && crop["height"] == source["height"];
    let (software_report, nvdec_report) = (crop_report(&software), crop_report(&nvdec));
    if case.no_crop {
        assert!(
            full_frame,
            "{label}: --no-crop must keep the full frame, got {crop}"
        );
        assert!(
            software_report.is_empty() && nvdec_report.is_empty(),
            "{label}: --no-crop must not monitor the crop"
        );
    } else {
        assert_eq!(
            software_report, nvdec_report,
            "{label}: the crop-monitor reports differ"
        );
        assert!(
            software_report
                .first()
                .is_some_and(|line| !line.contains("no usable scene cuts")),
            "{label}: no usable scene-cut crop sample: {software_report:?}"
        );
        if case.letterboxed {
            assert!(
                !full_frame,
                "{label}: the letterboxed clip committed the full frame"
            );
        }
    }
    eprintln!(
        "{label}: identical ({frames} NVDEC frames repacked, crop {}x{}+{}+{})",
        crop["width"], crop["height"], crop["x"], crop["y"]
    );
}

// One test for all cases, like `cuda_output_matches_cpu`: NVDEC runs one process at a time.
#[test]
fn nvdec_cpu_analysis_matches_software_decode() {
    if let Some(reason) = clips_unavailable(&[PQ_VAR, HLG_VAR, LETTERBOX_VAR]) {
        assert!(
            std::env::var_os(REQUIRED_VAR).is_none(),
            "{REQUIRED_VAR} is set but the NVDEC CPU-analysis test cannot run: {reason}"
        );
        eprintln!("Skipping: {reason}");
        return;
    }
    let (pq, hlg, letterbox) = (clip(PQ_VAR), clip(HLG_VAR), clip(LETTERBOX_VAR));
    let case = |label: &str, clip: &Path, mapping, no_crop, args: &[&'static str]| DecodeCase {
        label: label.to_owned(),
        clip: clip.to_path_buf(),
        mapping,
        no_crop,
        args: args.to_vec(),
        letterboxed: false,
    };
    let median3 = ["--pre-denoise", "median3"];
    let composers = hlg_composers();
    assert!(
        !composers.is_empty(),
        "{HLG_COMPOSERS_VAR} names no composer"
    );
    let hlg_case = |composer: Composer, extra: &[&'static str], label: &str| {
        let mut args = vec!["--hlg-composer", composer.cli_name()];
        args.extend_from_slice(extra);
        case(
            &format!("HLG {}{label}", composer.cli_name()),
            &hlg,
            composer.luminance_mapping(),
            false,
            &args,
        )
    };

    let version = analyzer_version();
    let cases: Vec<DecodeCase> = if version.contains("+cuda") {
        // Every other case analyzes on the GPU, which `cuda_output_matches_cpu` checks.
        let mut cases = vec![case("PQ, median3", &pq, "pq", false, &median3)];
        cases.extend(
            composers
                .iter()
                .map(|&composer| hlg_case(composer, &median3, ", median3")),
        );
        cases
    } else {
        let mut cases = vec![case("PQ, crop detection", &pq, "pq", false, &[])];
        cases.extend(
            composers
                .iter()
                .map(|&composer| hlg_case(composer, &[], ", crop detection")),
        );
        cases.extend([
            case("PQ, median3", &pq, "pq", false, &median3),
            case(
                "PQ, st2094-10",
                &pq,
                "pq",
                false,
                &["--peak-estimator", "st2094-10", "--peak-source", "max"],
            ),
            case("PQ, --no-crop", &pq, "pq", true, &[]),
            DecodeCase {
                letterboxed: true,
                ..case(
                    "letterboxed PQ, --crop-probes 0",
                    &letterbox,
                    "pq",
                    false,
                    &["--crop-probes", "0"],
                )
            },
        ]);
        cases
    };
    eprintln!("NVDEC CPU analysis with {version}: {} cases", cases.len());
    for case in &cases {
        check_nvdec_cpu_analysis(case);
    }
}
