use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;

use super::dovi_steps::resolve_dovi_input;
use super::DOVI_TOOL_MKV_INPUT_MIN;
use crate::cli::{AnalysisQuality, Args, DoviInput, HwAccel};
use crate::external;
use crate::metadata;
use crate::progress;

/// Locate the hdr_analyzer_mvp binary, preferring a fresh local release build.
///
/// A sibling next to this mkvdovi binary wins first, so a stale PATH install
/// (e.g. an old version without the L1 sidecar) is never silently picked up.
pub fn analyzer_executable() -> PathBuf {
    const TOOL: &str = "hdr_analyzer_mvp";
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(sibling) = current_exe.parent().map(|dir| dir.join(TOOL)) {
            if sibling.exists() {
                return sibling;
            }
        }
    }
    let local = Path::new("target/release/hdr_analyzer_mvp");
    if local.exists() {
        local.to_path_buf()
    } else {
        PathBuf::from(TOOL)
    }
}

/// Resolve `auto` settings to concrete values for this machine, once at startup.
/// `--hwaccel auto` becomes `cuda` when an NVIDIA GPU is detected (else `none`);
/// `--analysis-quality auto` becomes `accurate` only when GPU analysis is actually
/// available (CUDA resolved + analyzer built with the cuda feature), because
/// full-resolution every-frame analysis on the CPU would be slower than today's
/// balanced default. With GPU analysis an explicit `balanced` or `fast` is kept but warned
/// about: it saves no time on an NVDEC-bound run, and only `accurate` is parity-checked.
pub fn resolve_auto_settings(args: &mut Args) {
    if args.hwaccel == HwAccel::Auto {
        if external::detect_nvidia_gpu() {
            args.hwaccel = HwAccel::Cuda;
            progress::print_info(
                "Auto-detected NVIDIA GPU: CUDA acceleration enabled (GPU decode + analysis).",
            );
        } else {
            args.hwaccel = HwAccel::None;
            progress::print_info("No NVIDIA GPU detected: using the CPU pipeline.");
        }
    }
    let gpu_analysis = args.hwaccel == HwAccel::Cuda && analyzer_has_gpu_analysis();
    if args.analysis_quality != AnalysisQuality::Accurate {
        if let Some(message) = analysis_quality_notice(args.analysis_quality, gpu_analysis) {
            if args.analysis_quality == AnalysisQuality::Auto {
                progress::print_info(message);
            } else {
                progress::print_warn(message);
            }
        }
        if args.analysis_quality == AnalysisQuality::Auto {
            args.analysis_quality = if gpu_analysis {
                AnalysisQuality::Accurate
            } else {
                AnalysisQuality::Balanced
            };
        }
    }
    let version = external::dovi_tool_version();
    if args.dovi_input == DoviInput::Auto {
        let resolved = resolve_dovi_input(args.dovi_input, version);
        args.dovi_input = resolved;
        if resolved == DoviInput::Mkv {
            if let Some((maj, min, pat)) = version {
                progress::print_info(&format!(
                    "dovi_tool {maj}.{min}.{pat} reads MKV directly: skipping the full-size HEVC extraction (--dovi-input raw to disable)."
                ));
            }
        }
    } else if args.dovi_input == DoviInput::Mkv {
        if version.is_none() || version < Some(DOVI_TOOL_MKV_INPUT_MIN) {
            progress::print_warn(
                "Direct MKV input needs dovi_tool 2.3.4+; the ffmpeg fallback will likely be used.",
            );
        }
    }
}

/// Whether the analyzer mkvdovi runs was built with the cuda feature; warns when it was not.
fn analyzer_has_gpu_analysis() -> bool {
    let analyzer = analyzer_executable();
    let available = external::analyzer_has_cuda_feature(&analyzer);
    if !available {
        progress::print_warn(&format!(
            "{} was built without the cuda feature: NVDEC decodes, but the analysis runs on the CPU. Build it with `--features cuda` for GPU analysis.",
            analyzer.display()
        ));
    }
    available
}

pub(super) fn run_hdr_analyzer(
    input: &str,
    temp_dir: &Path,
    extra_args: &[String],
    args: &Args,
) -> Result<Option<PathBuf>> {
    let exe = analyzer_executable();

    let dir = Path::new(input).parent().unwrap_or(Path::new("."));
    let stem = Path::new(input).file_stem().unwrap().to_string_lossy();
    let out_path = dir.join(format!("{}_measurements.bin", stem));

    let (downscale, sample_rate) = analysis_sampling(args.analysis_quality);

    let mut cmd = Command::new(&exe);
    cmd.arg(input).arg("-o").arg(&out_path);
    cmd.arg("--downscale").arg(downscale.to_string());
    cmd.arg("--sample-rate").arg(sample_rate.to_string());
    cmd.args(extra_args);

    if args.hwaccel != HwAccel::None {
        cmd.arg("--hwaccel").arg(args.hwaccel.to_string());
    }

    // Use inherit_stderr so indicatif progress bar works correctly (detects TTY)
    if external::run_command_inherit_stderr(&mut cmd, &temp_dir.join("analyzer.log"))?
        && out_path.exists()
    {
        return Ok(Some(out_path));
    }
    Ok(None)
}

/// Print the advisory warnings that came back with a valid L1 sidecar.
pub(super) fn print_sidecar_advisories(advisories: &[String]) {
    for advisory in advisories {
        progress::print_warn(advisory);
    }
}

/// Print provenance for reused measurements and warn when they are coarser than the
/// resolved --analysis-quality preset.
pub(super) fn report_reused_sidecar(
    sidecar: &metadata::L1Sidecar,
    measurements: &Path,
    args: &Args,
) {
    progress::print_info(&format!(
        "Existing measurements provenance: {}",
        sidecar.provenance_summary()
    ));
    let (downscale, sample_rate) = analysis_sampling(args.analysis_quality);
    if let Some((coarse_downscale, coarse_sample_rate)) =
        coarser_sampling(sidecar, args.analysis_quality)
    {
        progress::print_warn(&format!(
            "Existing measurements were analyzed at downscale {coarse_downscale} / sample-rate {coarse_sample_rate}, coarser than --analysis-quality {} (downscale {downscale} / sample-rate {sample_rate}). Delete '{}' to re-analyze.",
            format!("{:?}", args.analysis_quality).to_lowercase(),
            measurements.display()
        ));
    } else if sidecar.analysis.is_none() {
        progress::print_warn(&format!(
            "Existing measurements carry no analysis provenance (sidecar v{}); delete '{}' to re-analyze with the current analyzer.",
            sidecar.version,
            measurements.display()
        ));
    }
}

/// Under `--analysis-quality accurate`, measurements sampled more coarsely are not reused but
/// re-analyzed: `accurate` is the only parity-checked sampling, and on CUDA the re-analysis costs
/// only decode time. Other presets keep reusing them with the warning in
/// [`report_reused_sidecar`]. Returns true when the candidate was rejected (and says so).
pub(super) fn rejects_coarser_sidecar(
    sidecar: &metadata::L1Sidecar,
    measurements: &Path,
    args: &Args,
) -> bool {
    if args.analysis_quality != AnalysisQuality::Accurate {
        return false;
    }
    let Some((downscale, sample_rate)) = coarser_sampling(sidecar, args.analysis_quality) else {
        return false;
    };
    progress::print_warn(&format!(
        "Existing measurements '{}' are not reused: they were analyzed at downscale {downscale} / sample-rate {sample_rate}, coarser than --analysis-quality accurate.",
        measurements.display()
    ));
    true
}

/// The sidecar's `(downscale, sample_rate)` when it sampled more coarsely than `quality`.
pub(super) fn coarser_sampling(
    sidecar: &metadata::L1Sidecar,
    quality: AnalysisQuality,
) -> Option<(u32, u32)> {
    let (downscale, sample_rate) = analysis_sampling(quality);
    sidecar
        .analysis
        .as_ref()
        .map(|analysis| (analysis.downscale, analysis.sample_rate))
        .filter(|&(used_downscale, used_sample_rate)| {
            used_downscale > downscale || used_sample_rate > sample_rate
        })
}

/// Warn when a run that expected GPU analysis was analyzed on the CPU (the sidecar records
/// `gpu: false` for a CPU run and for a mid-run CPU fallback). The analyzer's own messages say why.
pub(super) fn warn_if_gpu_analysis_missing(sidecar: &metadata::L1Sidecar, args: &Args) {
    if gpu_analysis_missing(sidecar, args.hwaccel, || {
        external::analyzer_has_cuda_feature(&analyzer_executable())
    }) {
        progress::print_warn(
            "GPU analysis was expected (--hwaccel cuda, analyzer built with +cuda), but the analyzer ran on the CPU for all or part of the run; its messages above say why.",
        );
    }
}

/// Whether `sidecar` was analyzed on the CPU (for all or part of the run) although
/// `--hwaccel cuda` and an analyzer with the cuda feature should have analyzed it on the GPU.
/// `analyzer_has_cuda` runs only when the other conditions hold, because it starts a process.
pub(super) fn gpu_analysis_missing(
    sidecar: &metadata::L1Sidecar,
    hwaccel: HwAccel,
    analyzer_has_cuda: impl FnOnce() -> bool,
) -> bool {
    let analyzed_on_cpu = sidecar
        .analysis
        .as_ref()
        .is_some_and(|analysis| !analysis.gpu);
    hwaccel == HwAccel::Cuda && analyzed_on_cpu && analyzer_has_cuda()
}

/// What `--analysis-quality` means on this host, or `None` when there is nothing to say.
/// `quality` is the value given on the command line, before `auto` is resolved.
pub(super) fn analysis_quality_notice(
    quality: AnalysisQuality,
    gpu_analysis: bool,
) -> Option<&'static str> {
    match (quality, gpu_analysis) {
        (AnalysisQuality::Auto, true) => {
            Some("GPU analysis available: using accurate (full-resolution) analysis quality.")
        }
        (AnalysisQuality::Auto, false) => Some(
            "No GPU analysis: using balanced (half-resolution) analysis quality. MaxCLL is then not taken from the measurements; --analysis-quality accurate measures it, at a higher CPU cost.",
        ),
        (AnalysisQuality::Balanced, true) => Some(
            "--analysis-quality balanced saves no time with GPU analysis (the run is limited by NVDEC decoding), drops the measured MaxCLL, and is not parity-checked (CUDA samples every second pixel, the CPU resizes). Use auto or accurate unless you want this on purpose.",
        ),
        (AnalysisQuality::Fast, true) => Some(
            "--analysis-quality fast saves no time with GPU analysis (the run is limited by NVDEC decoding), skips frames, drops the measured MaxCLL and MaxFALL, and is not parity-checked (CUDA samples every second pixel, the CPU resizes). Use auto or accurate unless you want this on purpose.",
        ),
        (AnalysisQuality::Balanced | AnalysisQuality::Fast, false) | (AnalysisQuality::Accurate, _) => {
            None
        }
    }
}

/// `(downscale, sample_rate)` passed to the analyzer for a resolved --analysis-quality preset.
pub(super) fn analysis_sampling(quality: AnalysisQuality) -> (u32, u32) {
    match quality {
        // Auto is resolved to a concrete value at startup; map it defensively.
        AnalysisQuality::Auto | AnalysisQuality::Balanced => (2, 1),
        AnalysisQuality::Fast => (2, 3),
        AnalysisQuality::Accurate => (1, 1),
    }
}
