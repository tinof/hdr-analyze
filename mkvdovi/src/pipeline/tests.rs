use super::analyzer::{
    analysis_quality_notice, analysis_sampling, coarser_sampling, gpu_analysis_missing,
    rejects_coarser_sidecar, resolve_analysis_quality,
};
use super::config::{commit_config, withdraw_stale_rpu};
use super::deliver::{extract_base_layer, inject_rpu, mux_output};
use super::dovi_steps::{
    convert_mel_fast_path, convert_mel_to_profile81, extract_clean_base_layer,
    feed_mkv_to_dovi_tool, parse_packet_count, resolve_dovi_input,
};
use super::hdr10plus::{hdr10plus_peak_nits, hdr10plus_scene_peak_stats, Hdr10PlusPeakStats};
use super::hdr10plus_coverage;
use super::measure::{
    abort_unsupported, acquire_measurements, find_reusable_measurements, hybrid_refusal,
    legacy_targets_notice, total_steps_for, Measured,
};
use super::temp_dir::{locate_temp_dir, prepare_temp_dir, settle_leftover_temp_dir};
use super::timing::{Delivery, MuxTiming};
use super::*;
use crate::cli::{AnalysisQuality, DoviInput, HwAccel, PeakSource};
use clap::Parser;
use serde_json::json;

#[test]
fn cm_version_acceptance_cli_defaults_to_v40_and_accepts_both_versions() {
    let default = Args::try_parse_from(["mkvdovi"]).unwrap();
    assert_eq!(default.cm_version.to_string(), "V40");
    for (option, expected) in [("v29", "V29"), ("v40", "V40")] {
        let args = Args::try_parse_from(["mkvdovi", "--cm-version", option, "--verify"]).unwrap();
        assert_eq!(args.cm_version.to_string(), expected);
        assert!(args.verify);
    }
}

#[test]
fn verify_requires_the_requested_cm_version() {
    // Before P13, --cm-version v29 passed no expected version, so --verify accepted CM v4.0.
    assert_eq!(expected_cm_version(CmVersion::V29), "V29");
    assert_eq!(expected_cm_version(CmVersion::V40), "V40");
}

#[test]
fn analysis_quality_maps_to_analyzer_sampling_args() {
    assert_eq!(analysis_sampling(AnalysisQuality::Fast), (2, 3));
    assert_eq!(analysis_sampling(AnalysisQuality::Balanced), (2, 1));
    assert_eq!(analysis_sampling(AnalysisQuality::Accurate), (1, 1));
    assert_eq!(analysis_sampling(AnalysisQuality::Auto), (1, 1));
}

#[test]
fn auto_quality_resolves_to_accurate_and_explicit_presets_stay() {
    // P1.1: auto is accurate on the CPU too, so a file gets the same L1 on any host.
    assert_eq!(
        resolve_analysis_quality(AnalysisQuality::Auto),
        AnalysisQuality::Accurate
    );
    for quality in [
        AnalysisQuality::Fast,
        AnalysisQuality::Balanced,
        AnalysisQuality::Accurate,
    ] {
        assert_eq!(resolve_analysis_quality(quality), quality);
    }
}

#[test]
fn startup_resolution_sets_accurate_for_cpu_auto_and_keeps_an_explicit_preset() {
    // --hwaccel none skips the NVIDIA probe, so this is the CPU host on any machine.
    let mut auto = Args::parse_from(["mkvdovi", "--hwaccel", "none"]);
    resolve_auto_settings(&mut auto);
    assert_eq!(auto.analysis_quality, AnalysisQuality::Accurate);
    let mut balanced = Args::parse_from([
        "mkvdovi",
        "--hwaccel",
        "none",
        "--analysis-quality",
        "balanced",
    ]);
    resolve_auto_settings(&mut balanced);
    assert_eq!(balanced.analysis_quality, AnalysisQuality::Balanced);
}

fn sidecar_sampled_at(downscale: u32, sample_rate: u32) -> metadata::L1Sidecar {
    metadata::L1Sidecar {
        analysis: Some(metadata::L1SidecarAnalysis {
            downscale,
            sample_rate,
            gpu: false,
            no_crop: false,
            luminance_mapping: None,
        }),
        ..Default::default()
    }
}

#[test]
fn gpu_analysis_missing_needs_cuda_a_cuda_analyzer_and_a_cpu_run() {
    let cpu_run = sidecar_sampled_at(1, 1);
    let mut gpu_run = sidecar_sampled_at(1, 1);
    gpu_run.analysis.as_mut().unwrap().gpu = true;
    assert!(gpu_analysis_missing(&cpu_run, HwAccel::Cuda, || true));
    assert!(!gpu_analysis_missing(&gpu_run, HwAccel::Cuda, || true));
    assert!(!gpu_analysis_missing(&cpu_run, HwAccel::Cuda, || false));
    assert!(!gpu_analysis_missing(&cpu_run, HwAccel::None, || {
        panic!("no analyzer probe without --hwaccel cuda")
    }));
    assert!(!gpu_analysis_missing(
        &metadata::L1Sidecar::default(),
        HwAccel::Cuda,
        || panic!("no analyzer probe without provenance")
    ));
}

#[test]
fn coarser_sampling_compares_against_the_preset() {
    let balanced = sidecar_sampled_at(2, 1);
    assert_eq!(
        coarser_sampling(&balanced, AnalysisQuality::Accurate),
        Some((2, 1))
    );
    assert_eq!(coarser_sampling(&balanced, AnalysisQuality::Balanced), None);
    assert_eq!(coarser_sampling(&balanced, AnalysisQuality::Fast), None);
    let fast = sidecar_sampled_at(2, 3);
    assert_eq!(
        coarser_sampling(&fast, AnalysisQuality::Balanced),
        Some((2, 3))
    );
    let full = sidecar_sampled_at(1, 1);
    assert_eq!(coarser_sampling(&full, AnalysisQuality::Accurate), None);
    // Unknown sampling (no provenance) is reported, not rejected.
    assert_eq!(
        coarser_sampling(&metadata::L1Sidecar::default(), AnalysisQuality::Accurate),
        None
    );
}

#[test]
fn only_accurate_rejects_a_coarser_sidecar() {
    let measurements = Path::new("clip_measurements.bin");
    let mut args = Args::parse_from(["mkvdovi"]);
    args.analysis_quality = AnalysisQuality::Accurate;
    assert!(rejects_coarser_sidecar(
        &sidecar_sampled_at(2, 1),
        measurements,
        &args
    ));
    assert!(!rejects_coarser_sidecar(
        &sidecar_sampled_at(1, 1),
        measurements,
        &args
    ));
    args.analysis_quality = AnalysisQuality::Balanced;
    assert!(!rejects_coarser_sidecar(
        &sidecar_sampled_at(2, 3),
        measurements,
        &args
    ));
}

#[test]
fn quality_notice_warns_about_coarse_presets_only_with_gpu_analysis() {
    for quality in [AnalysisQuality::Balanced, AnalysisQuality::Fast] {
        let notice = analysis_quality_notice(quality, true).unwrap();
        assert!(notice.contains("saves no time"), "{notice}");
        assert!(notice.contains("not parity-checked"), "{notice}");
        assert_eq!(analysis_quality_notice(quality, false), None);
    }
    assert!(analysis_quality_notice(AnalysisQuality::Fast, true)
        .unwrap()
        .contains("MaxFALL"));
    assert!(analysis_quality_notice(AnalysisQuality::Auto, true)
        .unwrap()
        .contains("accurate"));
    let cpu_auto = analysis_quality_notice(AnalysisQuality::Auto, false).unwrap();
    assert!(cpu_auto.contains("using accurate"), "{cpu_auto}");
    assert!(
        cpu_auto.contains("--analysis-quality balanced"),
        "{cpu_auto}"
    );
}

#[test]
fn profile7_fel_input_is_refused() {
    // No file there: the record is unreadable, so only the format decides.
    let missing = "/nonexistent/in.mkv";
    let error = reject_unsupported_input(HdrFormat::DolbyVisionFel, missing).unwrap_err();
    assert_eq!(error.to_string(), FEL_UNSUPPORTED_MESSAGE);
    assert!(FEL_UNSUPPORTED_MESSAGE.starts_with("Profile 7 FEL input is not supported"));
    assert!(FEL_UNSUPPORTED_MESSAGE.contains("docs/FEL_PLAN.md"));

    for supported in [
        HdrFormat::Hdr10Plus,
        HdrFormat::Hlg,
        HdrFormat::Hdr10WithMeasurements,
        HdrFormat::Hdr10Unsupported,
        HdrFormat::DolbyVisionMel,
        HdrFormat::DolbyVisionP8,
        HdrFormat::Unsupported,
    ] {
        assert!(reject_unsupported_input(supported, missing).is_ok());
    }
}

#[test]
fn mdfix_output_does_not_collide_with_dv_input() {
    assert_eq!(
        output_path_for(Path::new("episode.DV.mkv"), true),
        PathBuf::from("episode.mdfix.DV.mkv")
    );
    assert_eq!(
        output_path_for(Path::new("episode.mkv"), false),
        PathBuf::from("episode.DV.mkv")
    );
}

#[test]
fn hdr10plus_outlier_stats_use_selected_scene_peak_source() {
    let metadata = json!({
        "SceneInfo": [
            {
                "LuminanceParameters": {
                    "LuminanceDistributions": { "DistributionValues": [100, 35000] },
                    "MaxScl": [1000, 1100, 1200]
                }
            },
            {
                "LuminanceParameters": {
                    "LuminanceDistributions": { "DistributionValues": [100, 20000] },
                    "MaxScl": [1000, 1100, 1200]
                }
            }
        ],
        "SceneInfoSummary": { "SceneFirstFrameIndex": [0, 1] }
    });

    assert_eq!(
        hdr10plus_scene_peak_stats(&metadata, PeakSource::Histogram, 3000.0).unwrap(),
        Some(Hdr10PlusPeakStats {
            max_peak_nits: 3500.0,
            outlier_scene_count: 1,
        })
    );
    assert_eq!(
        hdr10plus_scene_peak_stats(&metadata, PeakSource::MaxScl, 3000.0).unwrap(),
        None
    );
}

#[test]
fn hdr10plus_max_scl_luminance_matches_upstream_weighting() {
    let scene = json!({
        "LuminanceParameters": {
            "LuminanceDistributions": { "DistributionValues": [100] },
            "MaxScl": [1000, 2000, 3000]
        }
    });

    let peak_nits = hdr10plus_peak_nits(&scene, PeakSource::MaxSclLuminance).unwrap();

    assert!((peak_nits - 179.66).abs() < 1e-9);
}

#[test]
fn hdr10plus_peak_sources_follow_dovi_tool() {
    // histogram99 is the last distribution value (the 99.98th percentile), not the largest.
    let scene = json!({
        "LuminanceParameters": {
            "LuminanceDistributions": { "DistributionValues": [100, 35000, 20000] },
            "MaxScl": [1000, 2000, 3000]
        }
    });

    assert_eq!(
        hdr10plus_peak_nits(&scene, PeakSource::Histogram99),
        Some(2000.0)
    );
    assert_eq!(
        hdr10plus_peak_nits(&scene, PeakSource::Histogram),
        Some(3500.0)
    );
    assert_eq!(hdr10plus_peak_nits(&scene, PeakSource::MaxScl), Some(300.0));
}

#[test]
fn resume_settings_carry_only_a_non_preset_hlg_composer() {
    let preset = Args::try_parse_from(["mkvdovi", "--hlg-composer", "preset"]).unwrap();
    let bt2100 = Args::try_parse_from(["mkvdovi"]).unwrap();

    // Preset fingerprints stay what 0.5.1 wrote (its RPUs carry the preset), so those temp
    // dirs still resume under --hlg-composer preset and are discarded under bt2100.
    let default = resume_settings(&preset, None, false);
    assert_eq!(
            default,
            "hwaccel=Auto analysis_quality=Auto optimizer=Conservative boost=false boost_experimental=false cm=V40 content_type=Movies reference_mode=false source_primaries=None trim_targets=100,600,1000 peak_source=Histogram mdfix=false legacy_madvr_l1=false"
        );
    assert_eq!(
        resume_settings(&preset, Some(Composer::Preset), false),
        default
    );
    // The composer only matters for HLG inputs.
    assert_eq!(resume_settings(&bt2100, None, false), default);
    // bt2100 differs from the preset both ways, so neither reuses the other's RPU.bin.
    assert_eq!(
        resume_settings(&bt2100, Some(Composer::Bt2100V1), false),
        format!("{default} hlg_composer=dovi84-bt2100-v1-spec420")
    );
}

#[test]
fn resume_settings_carry_the_hdr10plus_hybrid_only_for_an_hdr10plus_input() {
    let default = resume_settings(&Args::try_parse_from(["mkvdovi"]).unwrap(), None, false);
    let hybrid = Args::try_parse_from(["mkvdovi", "--hdr10plus-l1", "hybrid"]).unwrap();
    // `convert_file` passes true only for an HDR10+ input resolved to the hybrid.
    assert_eq!(resume_settings(&hybrid, None, false), default);
    assert_eq!(
        resume_settings(&hybrid, None, true),
        format!("{default} hdr10plus_l1=hybrid")
    );
}

#[test]
fn resume_settings_follow_the_resolved_mode_not_the_flag() {
    let settings = |flags: &[&str], capable: bool| {
        let args = Args::try_parse_from(["mkvdovi"].iter().chain(flags)).expect("flags must parse");
        let (mode, _) = resolve_hdr10plus_mode(&args, || capable).unwrap();
        resume_settings(&args, None, mode.is_hybrid())
    };
    const TOKEN: &str = " hdr10plus_l1=hybrid";
    let default = settings(&[], true);
    assert!(default.ends_with(TOKEN), "{default}");
    assert_eq!(settings(&["--hdr10plus-l1", "hybrid"], true), default);
    // A file that fell back holds metadata artifacts: it must not share the hybrid's fingerprint.
    assert!(!settings(&[], false).contains(TOKEN));
    assert!(!settings(&["--hdr10plus-l1", "hdr10plus"], true).contains(TOKEN));
}

#[test]
fn hdr10plus_mode_defaults_to_hybrid_and_only_an_explicit_hybrid_refuses() {
    let args = |flags: &[&str]| {
        Args::try_parse_from(["mkvdovi"].iter().chain(flags)).expect("flags must parse")
    };
    let no_probe = || -> bool { panic!("the analyzer is probed only when the choice needs it") };
    let notice = |flags: &[&str], capable: bool| {
        let (mode, notice) = resolve_hdr10plus_mode(&args(flags), || capable).unwrap();
        assert_eq!(mode, Hdr10PlusMode::Metadata, "{flags:?}");
        notice.expect("a fallback is visible")
    };

    let default = resolve_hdr10plus_mode(&args(&[]), || true).unwrap();
    assert_eq!(default, (Hdr10PlusMode::ImplicitHybrid, None));

    // The default takes the metadata path with a notice for each obstacle.
    let stale = notice(&[], false);
    assert!(
        stale.contains("hdr_analyzer_mvp is missing or does not record"),
        "{stale}"
    );
    let fast = notice(&["--analysis-quality", "fast"], true);
    assert!(fast.contains("--analysis-quality fast"), "{fast}");
    let legacy = notice(&["--legacy-madvr-l1"], true);
    assert!(legacy.contains("--legacy-madvr-l1"), "{legacy}");
    assert_eq!(
        resolve_hdr10plus_mode(&args(&["--analysis-quality", "fast"]), no_probe)
            .unwrap()
            .0,
        Hdr10PlusMode::Metadata
    );
    assert_eq!(
        resolve_hdr10plus_mode(&args(&["--legacy-madvr-l1"]), no_probe)
            .unwrap()
            .0,
        Hdr10PlusMode::Metadata
    );

    // The names keep their meaning and say nothing more.
    let named = resolve_hdr10plus_mode(
        &args(&["--hdr10plus-l1", "hdr10plus", "--analysis-quality", "fast"]),
        no_probe,
    );
    assert_eq!(named.unwrap(), (Hdr10PlusMode::Metadata, None));
    let named = resolve_hdr10plus_mode(&args(&["--hdr10plus-l1", "hybrid"]), no_probe);
    assert_eq!(named.unwrap(), (Hdr10PlusMode::ExplicitHybrid, None));

    let refused = resolve_hdr10plus_mode(
        &args(&["--hdr10plus-l1", "hybrid", "--analysis-quality", "fast"]),
        no_probe,
    )
    .unwrap_err();
    assert!(
        refused.contains("fast analyzes one frame in 3"),
        "{refused}"
    );
    assert!(refused.contains("--hdr10plus-l1 hdr10plus"), "{refused}");
}

#[test]
fn hdr10plus_mode_after_a_failure_falls_back_or_refuses() {
    assert_eq!(
        Hdr10PlusMode::ImplicitHybrid.after_failure("reason"),
        Some(Hdr10PlusMode::Metadata)
    );
    assert_eq!(Hdr10PlusMode::ExplicitHybrid.after_failure("reason"), None);
    let refusal = hybrid_refusal("the analysis failed");
    assert!(refusal.contains("the analysis failed"), "{refusal}");
    assert!(refusal.contains("--hdr10plus-l1 hdr10plus"), "{refusal}");
}

#[test]
fn the_ignored_hybrid_line_needs_the_flag_and_another_input() {
    let default = Args::try_parse_from(["mkvdovi"]).unwrap();
    let named = Args::try_parse_from(["mkvdovi", "--hdr10plus-l1", "hdr10plus"]).unwrap();
    let hybrid = Args::try_parse_from(["mkvdovi", "--hdr10plus-l1", "hybrid"]).unwrap();
    for format in [
        HdrFormat::Hdr10Unsupported,
        HdrFormat::Hlg,
        HdrFormat::DolbyVisionP8,
    ] {
        assert!(!hybrid_flag_ignored(&default, format), "{format:?}");
        assert!(!hybrid_flag_ignored(&named, format), "{format:?}");
        assert!(hybrid_flag_ignored(&hybrid, format), "{format:?}");
    }
    assert!(!hybrid_flag_ignored(&hybrid, HdrFormat::Hdr10Plus));
    assert!(!hybrid_flag_ignored(&default, HdrFormat::Hdr10Plus));
}

#[test]
fn hdr10plus_hybrid_refuses_a_sampling_that_skips_frames() {
    let with_quality = |quality: &str| {
        Args::try_parse_from([
            "mkvdovi",
            "--hdr10plus-l1",
            "hybrid",
            "--analysis-quality",
            quality,
        ])
        .unwrap()
    };
    let refusal = hybrid_sampling_refusal(&with_quality("fast")).unwrap();
    assert!(
        refusal.contains("fast analyzes one frame in 3"),
        "{refusal}"
    );
    assert!(hybrid_sampling_refusal(&with_quality("balanced")).is_none());
    assert!(hybrid_sampling_refusal(&with_quality("accurate")).is_none());
}

#[test]
fn resolve_dovi_input_matrix() {
    assert_eq!(
        resolve_dovi_input(DoviInput::Auto, Some((2, 3, 4))),
        DoviInput::Mkv
    );
    assert_eq!(
        resolve_dovi_input(DoviInput::Auto, Some((2, 10, 0))),
        DoviInput::Mkv
    );
    assert_eq!(
        resolve_dovi_input(DoviInput::Auto, Some((2, 3, 3))),
        DoviInput::Raw
    );
    assert_eq!(resolve_dovi_input(DoviInput::Auto, None), DoviInput::Raw);
    assert_eq!(
        resolve_dovi_input(DoviInput::Raw, Some((9, 9, 9))),
        DoviInput::Raw
    );
    assert_eq!(
        resolve_dovi_input(DoviInput::Mkv, Some((2, 3, 3))),
        DoviInput::Mkv
    );
    assert_eq!(resolve_dovi_input(DoviInput::Mkv, None), DoviInput::Mkv);
}

#[test]
fn feed_mkv_to_dovi_tool_rules() {
    assert!(feed_mkv_to_dovi_tool(DoviInput::Mkv, false));
    assert!(!feed_mkv_to_dovi_tool(DoviInput::Mkv, true));
    assert!(!feed_mkv_to_dovi_tool(DoviInput::Raw, false));
    assert!(!feed_mkv_to_dovi_tool(DoviInput::Auto, false));
}

const ALL_HDR_FORMATS: [HdrFormat; 8] = [
    HdrFormat::Hdr10Plus,
    HdrFormat::Hdr10WithMeasurements,
    HdrFormat::Hdr10Unsupported,
    HdrFormat::Hlg,
    HdrFormat::DolbyVisionMel,
    HdrFormat::DolbyVisionFel,
    HdrFormat::DolbyVisionP8,
    HdrFormat::Unsupported,
];

fn dv_format(format: HdrFormat) -> bool {
    matches!(
        format,
        HdrFormat::DolbyVisionMel | HdrFormat::DolbyVisionFel | HdrFormat::DolbyVisionP8
    )
}

#[test]
fn pq_range_check_covers_exactly_the_generated_8_1_rpus() {
    for format in ALL_HDR_FORMATS {
        for mdfix in [false, true] {
            let expected = match format {
                HdrFormat::Hdr10Plus
                | HdrFormat::Hdr10WithMeasurements
                | HdrFormat::Hdr10Unsupported => true,
                HdrFormat::DolbyVisionMel | HdrFormat::DolbyVisionP8 => mdfix,
                HdrFormat::Hlg | HdrFormat::DolbyVisionFel | HdrFormat::Unsupported => false,
            };
            assert_eq!(
                needs_pq_range_check(format, mdfix),
                expected,
                "{format:?} mdfix={mdfix}"
            );
        }
    }
}

#[test]
fn is_dolby_vision_is_true_only_for_mel_fel_and_p8() {
    for format in ALL_HDR_FORMATS {
        assert_eq!(is_dolby_vision(format), dv_format(format), "{format:?}");
    }
}

#[test]
fn should_keep_source_truth_table() {
    for (flags, keep_flag, mdfix_flag) in [
        (vec![], false, false),
        (vec!["--keep-source"], true, false),
        (vec!["--mdfix"], false, true),
        (vec!["--keep-source", "--mdfix"], true, true),
    ] {
        let mut argv = vec!["mkvdovi"];
        argv.extend(flags);
        let args = Args::try_parse_from(argv).unwrap();
        assert_eq!(args.keep_source, keep_flag);
        assert_eq!(args.mdfix, mdfix_flag);
        for format in ALL_HDR_FORMATS {
            let expected = keep_flag || mdfix_flag || dv_format(format);
            assert_eq!(
                should_keep_source(&args, format),
                expected,
                "keep_source={keep_flag} mdfix={mdfix_flag} {format:?}"
            );
        }
    }
}

#[test]
fn output_path_for_appends_dv_next_to_the_input() {
    let out = output_path_for(Path::new("/media/in/Movie.mkv"), false);
    assert_eq!(out, PathBuf::from("/media/in/Movie.DV.mkv"));
    // Without --mdfix an input already named .DV keeps the suffix and gains another.
    let out = output_path_for(Path::new("/media/in/Movie.DV.mkv"), false);
    assert_eq!(out, PathBuf::from("/media/in/Movie.DV.DV.mkv"));
    // A bare file name resolves against an empty parent.
    let out = output_path_for(Path::new("Movie.mkv"), false);
    assert_eq!(out, PathBuf::from("Movie.DV.mkv"));
}

#[test]
fn output_path_for_mdfix_writes_a_distinct_mdfix_candidate() {
    let out = output_path_for(Path::new("/media/in/Movie.mkv"), true);
    assert_eq!(out, PathBuf::from("/media/in/Movie.mdfix.DV.mkv"));
    // An input already ending .DV does not double the suffix.
    let out = output_path_for(Path::new("/media/in/Movie.DV.mkv"), true);
    assert_eq!(out, PathBuf::from("/media/in/Movie.mdfix.DV.mkv"));
    // Only a trailing ".DV" stem is stripped, not one in the middle.
    let out = output_path_for(Path::new("/media/in/Movie.DV.cut.mkv"), true);
    assert_eq!(out, PathBuf::from("/media/in/Movie.DV.cut.mdfix.DV.mkv"));
}

/// Runs `finish_success` on a fresh source, output and temp dir; returns
/// whether the source and the temp dir still exist afterwards.
fn run_finish_success(
    flags: &[&str],
    format: HdrFormat,
    create_source: bool,
) -> (bool, bool, bool) {
    run_finish_success_with_timing(flags, format, create_source, true)
}

fn run_finish_success_with_timing(
    flags: &[&str],
    format: HdrFormat,
    create_source: bool,
    timing_checked: bool,
) -> (bool, bool, bool) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("in.mkv");
    let output = dir.path().join("in.DV.mkv");
    let temp = dir.path().join("mkvdovi_temp_in");
    if create_source {
        fs::write(&source, b"src").unwrap();
    }
    fs::write(&output, b"out").unwrap();
    fs::create_dir(&temp).unwrap();
    fs::write(temp.join("RPU.bin"), b"x").unwrap();

    let mut argv = vec!["mkvdovi"];
    argv.extend_from_slice(flags);
    let args = Args::try_parse_from(argv).unwrap();
    finish_success(
        source.to_str().unwrap(),
        &output,
        &temp,
        &args,
        format,
        Instant::now(),
        timing_checked,
    );
    (source.exists(), temp.exists(), output.exists())
}

#[test]
fn finish_success_keeps_a_non_dv_source_whose_output_timing_was_not_checked() {
    for format in ALL_HDR_FORMATS.into_iter().filter(|f| !dv_format(*f)) {
        let (source, temp, output) = run_finish_success_with_timing(&[], format, true, false);
        assert!(source, "source should be kept for {format:?}");
        assert!(!temp, "temp dir should be removed for {format:?}");
        assert!(output, "output must stay for {format:?}");
    }
}

#[test]
fn finish_success_deletes_source_for_non_dv_input_and_always_removes_temp_dir() {
    for format in ALL_HDR_FORMATS.into_iter().filter(|f| !dv_format(*f)) {
        let (source, temp, output) = run_finish_success(&[], format, true);
        assert!(!source, "source should be deleted for {format:?}");
        assert!(!temp, "temp dir should be removed for {format:?}");
        assert!(output, "output must stay for {format:?}");
    }
}

#[test]
fn finish_success_keeps_source_with_keep_source_or_mdfix() {
    for flags in [&["--keep-source"][..], &["--mdfix"][..]] {
        for format in ALL_HDR_FORMATS {
            let (source, temp, output) = run_finish_success(flags, format, true);
            assert!(source, "{flags:?} {format:?}");
            assert!(!temp, "{flags:?} {format:?}");
            assert!(output, "{flags:?} {format:?}");
        }
    }
}

#[test]
fn finish_success_keeps_source_for_every_dolby_vision_variant() {
    for format in ALL_HDR_FORMATS.into_iter().filter(|f| dv_format(*f)) {
        let (source, temp, output) = run_finish_success(&[], format, true);
        assert!(source, "source should be kept for {format:?}");
        assert!(!temp, "temp dir should be removed for {format:?}");
        assert!(output);
    }
}

#[test]
fn finish_success_with_a_missing_source_only_warns_and_still_cleans_up() {
    let (source, temp, output) = run_finish_success(&[], HdrFormat::Hdr10Plus, false);
    assert!(!source);
    assert!(!temp);
    assert!(output);
}

/// Runs `convert_file` on `<dir>/in.mkv` after `setup` has populated the directory; returns
/// the result and the directory. Only paths that return before format detection (MediaInfo)
/// are reachable without external tools.
fn run_convert_file(
    flags: &[&str],
    setup: impl FnOnce(&Path),
) -> (Result<bool>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path());
    let mut argv = vec!["mkvdovi"];
    argv.extend_from_slice(flags);
    let args = Args::try_parse_from(argv).unwrap();
    let input = dir.path().join("in.mkv");
    let result = convert_file(input.to_str().unwrap(), &args);
    (result, dir)
}

fn write_source_and_output(dir: &Path) {
    fs::write(dir.join("in.mkv"), b"src").unwrap();
    fs::write(dir.join("in.DV.mkv"), b"out").unwrap();
}

#[test]
fn convert_file_with_a_missing_input_fails_without_temp_work() {
    let (result, dir) = run_convert_file(&[], |_| {});
    assert!(!result.unwrap());
    assert!(!dir.path().join("mkvdovi_temp_in").exists());
}

#[test]
fn convert_file_skips_an_existing_output_without_a_temp_dir() {
    let (result, dir) = run_convert_file(&[], write_source_and_output);
    assert!(result.unwrap());
    assert!(dir.path().join("in.mkv").exists(), "source must be kept");
    assert!(dir.path().join("in.DV.mkv").exists());
    assert!(!dir.path().join("mkvdovi_temp_in").exists());
}

#[test]
fn convert_file_no_resume_skips_an_existing_output_and_leaves_the_temp_dir() {
    let (result, dir) = run_convert_file(&["--no-resume"], |dir| {
        write_source_and_output(dir);
        fs::create_dir(dir.join("mkvdovi_temp_in")).unwrap();
        fs::write(dir.join("mkvdovi_temp_in").join("marker"), b"x").unwrap();
    });
    assert!(result.unwrap());
    assert!(dir.path().join("in.mkv").exists());
    assert!(dir.path().join("mkvdovi_temp_in").join("marker").exists());
}

#[test]
fn convert_file_no_resume_neither_adopts_nor_deletes_a_legacy_temp_dir() {
    let (result, dir) = run_convert_file(&["--no-resume"], |dir| {
        write_source_and_output(dir);
        fs::create_dir(dir.join("mkvdolby_temp_in")).unwrap();
        fs::write(dir.join("mkvdolby_temp_in").join("marker"), b"x").unwrap();
    });
    assert!(result.unwrap());
    assert!(dir.path().join("mkvdolby_temp_in").join("marker").exists());
    assert!(!dir.path().join("mkvdovi_temp_in").exists());
}

#[test]
fn locate_temp_dir_adopts_a_legacy_dir_only_when_resuming_without_a_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join("mkvdovi_temp_in");
    let legacy = dir.path().join("mkvdolby_temp_in");
    let locate = |resume| locate_temp_dir(dir.path(), "mkvdovi_temp_in", "in", resume);

    assert_eq!(locate(true), fresh, "nothing left over");
    fs::create_dir(&legacy).unwrap();
    assert_eq!(locate(true), legacy);
    assert_eq!(locate(false), fresh, "--no-resume never adopts");
    fs::create_dir(&fresh).unwrap();
    assert_eq!(locate(true), fresh, "a new-style dir wins");
}

/// A source, its leftover temp dir (`name`, holding `RPU.bin`) and the fingerprint of `settings`.
struct Leftover {
    root: tempfile::TempDir,
    temp: PathBuf,
    fresh: PathBuf,
    fingerprint: resume::Fingerprint,
}

fn leftover(name: &str) -> Leftover {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("in.mkv");
    fs::write(&source, b"src").unwrap();
    let temp = root.path().join(name);
    fs::create_dir(&temp).unwrap();
    fs::write(temp.join("RPU.bin"), b"rpu").unwrap();
    let fingerprint = resume::Fingerprint::for_input(&source, "settings".into()).unwrap();
    let fresh = root.path().join("mkvdovi_temp_in");
    Leftover {
        root,
        temp,
        fresh,
        fingerprint,
    }
}

#[test]
fn settle_resumes_a_dir_with_a_matching_fingerprint() {
    let left = leftover("mkvdovi_temp_in");
    left.fingerprint.write(&left.temp).unwrap();
    let settled = settle_leftover_temp_dir(
        left.temp.clone(),
        left.fresh.clone(),
        true,
        Some(&left.fingerprint),
        None,
        Composer::Bt2100V1,
    );
    assert_eq!(settled, (left.temp.clone(), true));
    assert!(left.temp.join("RPU.bin").exists());
}

#[test]
fn settle_discards_a_differing_dir_and_resets_to_the_new_name() {
    let left = leftover("mkvdolby_temp_in");
    let other = resume::Fingerprint {
        settings: "other".into(),
        ..left.fingerprint.clone()
    };
    other.write(&left.temp).unwrap();
    let settled = settle_leftover_temp_dir(
        left.temp.clone(),
        left.fresh.clone(),
        true,
        Some(&left.fingerprint),
        None,
        Composer::Bt2100V1,
    );
    assert_eq!(settled, (left.fresh.clone(), false));
    assert!(!left.temp.exists(), "the discarded dir is removed");
    assert!(left.root.path().join("in.mkv").exists());
}

#[test]
fn settle_treats_an_unknown_current_fingerprint_as_differing() {
    let left = leftover("mkvdovi_temp_in");
    left.fingerprint.write(&left.temp).unwrap();
    let settled = settle_leftover_temp_dir(
        left.temp.clone(),
        left.fresh.clone(),
        true,
        None,
        None,
        Composer::Bt2100V1,
    );
    assert_eq!(settled, (left.fresh.clone(), false));
    assert!(!left.temp.exists());
}

#[test]
fn settle_resumes_a_dir_without_fingerprint_unless_a_custom_composer_is_selected() {
    for custom in [None, Some(Composer::Bt2100V1)] {
        let left = leftover("mkvdolby_temp_in");
        let settled = settle_leftover_temp_dir(
            left.temp.clone(),
            left.fresh.clone(),
            true,
            Some(&left.fingerprint),
            custom,
            Composer::Bt2100V1,
        );
        if custom.is_some() {
            assert_eq!(settled, (left.fresh.clone(), false));
            assert!(!left.temp.exists());
        } else {
            assert_eq!(settled, (left.temp.clone(), true), "legacy dir resumes");
            assert!(left.temp.join("RPU.bin").exists());
        }
    }
}

#[test]
fn settle_discards_a_legacy_hlg_dir_whatever_its_fingerprint() {
    let left = leftover("mkvdovi_temp_in");
    left.fingerprint.write(&left.temp).unwrap();
    fs::write(left.temp.join("HLG_to_PQ.mkv"), b"pq").unwrap();
    let settled = settle_leftover_temp_dir(
        left.temp.clone(),
        left.fresh.clone(),
        true,
        Some(&left.fingerprint),
        None,
        Composer::Preset,
    );
    assert_eq!(settled, (left.fresh.clone(), false));
    assert!(!left.temp.exists());
}

#[test]
fn settle_leaves_everything_alone_without_resume_or_leftover() {
    let left = leftover("mkvdovi_temp_in");
    let settled = settle_leftover_temp_dir(
        left.temp.clone(),
        left.fresh.clone(),
        false,
        Some(&left.fingerprint),
        None,
        Composer::Preset,
    );
    assert_eq!(settled, (left.temp.clone(), false));
    assert!(
        left.temp.join("RPU.bin").exists(),
        "only prepare_temp_dir resets it"
    );

    let missing = left.root.path().join("mkvdovi_temp_other");
    let settled = settle_leftover_temp_dir(
        missing.clone(),
        left.fresh.clone(),
        true,
        Some(&left.fingerprint),
        None,
        Composer::Preset,
    );
    assert_eq!(settled, (missing, false));
}

#[test]
fn prepare_temp_dir_resets_a_dir_it_does_not_resume_and_writes_the_fingerprint() {
    let left = leftover("mkvdovi_temp_in");
    prepare_temp_dir(&left.temp, false, Some(&left.fingerprint)).unwrap();
    assert!(!left.temp.join("RPU.bin").exists());
    assert_eq!(
        left.fingerprint.check(&left.temp),
        resume::FingerprintStatus::Matches
    );

    let resumed = leftover("mkvdovi_temp_in");
    prepare_temp_dir(&resumed.temp, true, Some(&resumed.fingerprint)).unwrap();
    assert!(resumed.temp.join("RPU.bin").exists());
    assert_eq!(
        resumed.fingerprint.check(&resumed.temp),
        resume::FingerprintStatus::Matches
    );

    let created = resumed.root.path().join("mkvdovi_temp_new");
    prepare_temp_dir(&created, false, None).unwrap();
    assert!(created.is_dir());
    assert!(!created.join(resume::FINGERPRINT_FILE).exists());
}

#[test]
fn total_steps_for_counts_the_steps_of_each_detected_format() {
    let expected = [
        (HdrFormat::Hdr10Plus, 7),
        (HdrFormat::Hlg, 7),
        (HdrFormat::Hdr10WithMeasurements, 6),
        (HdrFormat::Hdr10Unsupported, 7),
        (HdrFormat::DolbyVisionMel, 7),
        (HdrFormat::DolbyVisionFel, 0),
        (HdrFormat::DolbyVisionP8, 7),
        (HdrFormat::Unsupported, 0),
    ];
    assert_eq!(expected.len(), ALL_HDR_FORMATS.len());
    for (format, steps) in expected {
        assert_eq!(total_steps_for(format), steps, "{format:?}");
    }
}

#[test]
fn abort_unsupported_removes_the_temp_dir_only_for_an_unsupported_format() {
    for format in ALL_HDR_FORMATS {
        let dir = tempfile::tempdir().unwrap();
        let temp = dir.path().join("mkvdovi_temp_in");
        fs::create_dir(&temp).unwrap();
        let unsupported = format == HdrFormat::Unsupported;
        assert_eq!(abort_unsupported(format, &temp), unsupported, "{format:?}");
        assert_eq!(temp.exists(), !unsupported, "{format:?}");
    }
}

/// Runs `acquire_measurements` for `<dir>/in.mkv` with fresh step counters starting at
/// `total_steps_for(format)` and 1; returns the result and the counters.
fn run_acquire_measurements(
    flags: &[&str],
    format: HdrFormat,
    setup: impl FnOnce(&Path),
) -> (Result<Option<Measured>>, u8, u8) {
    run_acquire_measurements_in(Hdr10PlusMode::Metadata, flags, format, setup)
}

/// [`run_acquire_measurements`] in an HDR10+ `mode`. `setup` sees the input directory, with
/// the temp directory `mkvdovi_temp_in` already there.
fn run_acquire_measurements_in(
    mode: Hdr10PlusMode,
    flags: &[&str],
    format: HdrFormat,
    setup: impl FnOnce(&Path),
) -> (Result<Option<Measured>>, u8, u8) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.mkv");
    fs::write(&input, b"src").unwrap();
    let temp = dir.path().join("mkvdovi_temp_in");
    fs::create_dir(&temp).unwrap();
    setup(dir.path());
    let mut argv = vec!["mkvdovi"];
    argv.extend_from_slice(flags);
    let args = Args::try_parse_from(argv).unwrap();
    let hlg_composer = (format == HdrFormat::Hlg).then_some(args.hlg_composer);
    let (mut total, mut current) = (total_steps_for(format), 1);
    let result = acquire_measurements(
        input.to_str().unwrap(),
        &temp,
        &args,
        true,
        hlg_composer,
        format,
        mode,
        None,
        None,
        &mut total,
        &mut current,
    );
    (result, total, current)
}

#[test]
fn acquire_measurements_refuses_legacy_madvr_l1_for_hlg_before_any_step() {
    let (result, total, current) =
        run_acquire_measurements(&["--legacy-madvr-l1"], HdrFormat::Hlg, |_| {});
    assert!(result.unwrap().is_none());
    assert_eq!((total, current), (7, 1));
}

#[test]
fn acquire_measurements_stops_on_profile8_without_mdfix() {
    let (result, total, current) = run_acquire_measurements(&[], HdrFormat::DolbyVisionP8, |_| {});
    assert!(result.unwrap().is_none());
    assert_eq!((total, current), (7, 1));
}

/// Seals the extracted stream and, with `metadata`, the extracted HDR10+ metadata, so that
/// `extract_hdr10plus_metadata` reuses them.
fn seal_hdr10plus(dir: &Path, metadata: Option<&serde_json::Value>) {
    let temp = dir.join("mkvdovi_temp_in");
    fs::write(temp.join("video.hevc"), b"not a stream").unwrap();
    resume::mark_done(&temp.join("video.hevc")).unwrap();
    if let Some(metadata) = metadata {
        let path = temp.join("hdr10plus_metadata.json");
        fs::write(&path, metadata.to_string()).unwrap();
        resume::mark_done(&path).unwrap();
    }
}

/// HDR10+ metadata for the 4 frames of [`hybrid_sidecar`], in one scene.
fn hdr10plus_for_four_frames() -> serde_json::Value {
    json!({
        "SceneInfo": [{}, {}, {}, {}],
        "SceneInfoSummary": {"SceneFirstFrameIndex": [0], "SceneFrameNumbers": [4]},
    })
}

fn measurements_for_four_frames(dir: &Path) {
    let measurements = dir.join("in_measurements.bin");
    fs::write(&measurements, b"bin").unwrap();
    fs::write(
        metadata::l1_sidecar_path(&measurements),
        hybrid_sidecar(6, 1).to_string(),
    )
    .unwrap();
}

const ACCURATE: [&str; 2] = ["--analysis-quality", "accurate"];

#[test]
fn a_good_hdr10plus_structure_reaches_the_hybrid_on_reused_measurements() {
    let (result, _, _) = run_acquire_measurements_in(
        Hdr10PlusMode::ImplicitHybrid,
        &ACCURATE,
        HdrFormat::Hdr10Plus,
        |dir| {
            seal_hdr10plus(dir, Some(&hdr10plus_for_four_frames()));
            measurements_for_four_frames(dir);
        },
    );
    let measured = result.unwrap().expect("the hybrid runs");
    assert_eq!(measured.hdr10plus_mode, Hdr10PlusMode::ImplicitHybrid);
    assert_eq!(measured.hdr_type, HdrFormat::Hdr10WithMeasurements);
    assert!(measured.l1_sidecar.is_some());
}

#[test]
fn a_bad_hdr10plus_structure_falls_back_before_any_analysis_or_refuses_an_explicit_hybrid() {
    let bad = json!({"SceneInfo": [{}, {}, {}, {}],
                     "SceneInfoSummary": {"SceneFirstFrameIndex": [2]}});
    let setup = |dir: &Path| {
        seal_hdr10plus(dir, Some(&bad));
        measurements_for_four_frames(dir);
    };
    let (result, total, _) = run_acquire_measurements_in(
        Hdr10PlusMode::ImplicitHybrid,
        &ACCURATE,
        HdrFormat::Hdr10Plus,
        setup,
    );
    let measured = result.unwrap().expect("the default still converts");
    assert_eq!(measured.hdr10plus_mode, Hdr10PlusMode::Metadata);
    assert_eq!(measured.hdr_type, HdrFormat::Hdr10Plus);
    assert!(measured.hdr10plus_json.is_some());
    assert!(measured.measurements_file.is_none());
    assert!(measured.l1_sidecar.is_none());
    assert_eq!(
        total,
        total_steps_for(HdrFormat::Hdr10Plus),
        "no analysis step"
    );

    let (result, _, _) = run_acquire_measurements_in(
        Hdr10PlusMode::ExplicitHybrid,
        &ACCURATE,
        HdrFormat::Hdr10Plus,
        setup,
    );
    assert!(result.unwrap().is_none());
}

#[test]
fn an_analysis_that_cannot_start_falls_back_only_for_the_default() {
    // A directory in place of the analyzer log makes `run_hdr_analyzer` return an error.
    let setup = |dir: &Path| {
        seal_hdr10plus(dir, Some(&hdr10plus_for_four_frames()));
        fs::create_dir(dir.join("mkvdovi_temp_in/analyzer.log")).unwrap();
    };
    let (result, _, _) = run_acquire_measurements_in(
        Hdr10PlusMode::ImplicitHybrid,
        &ACCURATE,
        HdrFormat::Hdr10Plus,
        setup,
    );
    let measured = result.unwrap().expect("the default still converts");
    assert_eq!(measured.hdr10plus_mode, Hdr10PlusMode::Metadata);
    assert_eq!(measured.hdr_type, HdrFormat::Hdr10Plus);
    assert!(measured.measurements_file.is_none());

    let (result, _, _) = run_acquire_measurements_in(
        Hdr10PlusMode::ExplicitHybrid,
        &ACCURATE,
        HdrFormat::Hdr10Plus,
        setup,
    );
    assert!(!matches!(result, Ok(Some(_))), "an explicit hybrid stops");
}

#[test]
fn hdr10plus_with_no_dynamic_metadata_leaves_the_hybrid_for_hdr10_analysis() {
    if crate::external::find_tool("hdr10plus_tool").is_none() {
        eprintln!("Skipping no-dynamic-metadata test: hdr10plus_tool not found in PATH");
        return;
    }
    for mode in [Hdr10PlusMode::ImplicitHybrid, Hdr10PlusMode::ExplicitHybrid] {
        let (result, _, _) =
            run_acquire_measurements_in(mode, &ACCURATE, HdrFormat::Hdr10Plus, |dir| {
                seal_hdr10plus(dir, None);
                measurements_for_four_frames(dir);
            });
        let measured = result.unwrap().expect("HDR10 analysis takes over");
        // A hybrid mode here would send the configuration to the metadata path with no metadata.
        assert_eq!(measured.hdr10plus_mode, Hdr10PlusMode::Metadata, "{mode:?}");
        assert_eq!(measured.hdr_type, HdrFormat::Hdr10Unsupported, "{mode:?}");
        assert!(measured.hdr10plus_json.is_none(), "{mode:?}");
        assert!(measured.l1_sidecar.is_some(), "{mode:?}");
    }
}

/// Seals an HDR10+ payload count of the stream in the temp directory: `own` of 4 frames.
fn seal_coverage(dir: &Path, own: u64) {
    let path = dir
        .join("mkvdovi_temp_in")
        .join(hdr10plus_coverage::COVERAGE_FILE);
    let gap = (own < 4).then_some(own);
    let record = json!({"version": 1, "coverage": {
        "access_units": 4, "own": own, "gaps": u64::from(own < 4), "longest_gap": 4 - own,
        "first_gap": gap, "ignored_payloads": 0}});
    fs::write(&path, record.to_string()).unwrap();
    resume::mark_done(&path).unwrap();
}

fn acquire_with_coverage(mode: Hdr10PlusMode, own: u64) -> Result<Option<Measured>> {
    let (result, _, _) =
        run_acquire_measurements_in(mode, &ACCURATE, HdrFormat::Hdr10Plus, |dir| {
            seal_hdr10plus(dir, Some(&hdr10plus_for_four_frames()));
            seal_coverage(dir, own);
            measurements_for_four_frames(dir);
        });
    result
}

#[test]
fn partial_hdr10plus_coverage_keeps_the_default_hybrid_and_the_metadata_mode() {
    let measured = acquire_with_coverage(Hdr10PlusMode::ImplicitHybrid, 3)
        .unwrap()
        .expect("the default only warns");
    // Not `after_failure`: that would leave the hybrid for the metadata and move delivered L1.
    assert_eq!(measured.hdr10plus_mode, Hdr10PlusMode::ImplicitHybrid);
    assert_eq!(measured.hdr_type, HdrFormat::Hdr10WithMeasurements);
    let measured = acquire_with_coverage(Hdr10PlusMode::Metadata, 3)
        .unwrap()
        .expect("the metadata mode only warns");
    assert_eq!(measured.hdr10plus_mode, Hdr10PlusMode::Metadata);
    assert_eq!(measured.hdr_type, HdrFormat::Hdr10Plus);
}

#[test]
fn partial_hdr10plus_coverage_refuses_an_explicit_hybrid_and_full_coverage_does_not() {
    let refused = acquire_with_coverage(Hdr10PlusMode::ExplicitHybrid, 3);
    assert!(matches!(refused, Ok(None)), "an explicit hybrid stops");
    let measured = acquire_with_coverage(Hdr10PlusMode::ExplicitHybrid, 4)
        .unwrap()
        .expect("full coverage takes the hybrid");
    assert_eq!(measured.hdr10plus_mode, Hdr10PlusMode::ExplicitHybrid);
    assert_eq!(measured.hdr_type, HdrFormat::Hdr10WithMeasurements);
}

#[test]
fn unextracted_hdr10plus_coverage_refuses_an_explicit_hybrid() {
    if crate::external::find_tool("hdr10plus_tool").is_none() {
        eprintln!("Skipping unextracted-payload test: hdr10plus_tool not found in PATH");
        return;
    }
    let run = |mode| {
        let (result, _, _) =
            run_acquire_measurements_in(mode, &ACCURATE, HdrFormat::Hdr10Plus, |dir| {
                // hdr10plus_tool fails on this stream; the sealed count says 4 frames carry HDR10+.
                seal_hdr10plus(dir, None);
                seal_coverage(dir, 4);
                measurements_for_four_frames(dir);
            });
        result
    };
    let refused = run(Hdr10PlusMode::ExplicitHybrid);
    assert!(matches!(refused, Ok(None)), "an explicit hybrid stops");
    let measured = run(Hdr10PlusMode::ImplicitHybrid)
        .unwrap()
        .expect("the default falls back to HDR10 analysis");
    assert_eq!(measured.hdr10plus_mode, Hdr10PlusMode::Metadata);
    assert_eq!(measured.hdr_type, HdrFormat::Hdr10Unsupported);
}

/// `acquire_with_coverage` at full coverage, on a reused JSON that a `--skip-reorder` retry of
/// an earlier run produced when `skip_reorder`.
fn acquire_reused_json(mode: Hdr10PlusMode, skip_reorder: bool) -> Result<Option<Measured>> {
    let (result, _, _) =
        run_acquire_measurements_in(mode, &ACCURATE, HdrFormat::Hdr10Plus, |dir| {
            seal_hdr10plus(dir, Some(&hdr10plus_for_four_frames()));
            seal_coverage(dir, 4);
            measurements_for_four_frames(dir);
            if skip_reorder {
                let marker = dir
                    .join("mkvdovi_temp_in")
                    .join(hdr10plus::HDR10PLUS_SKIP_REORDER_MARKER);
                fs::write(marker, b"").unwrap();
            }
        });
    result
}

#[test]
fn a_reused_skip_reorder_json_refuses_an_explicit_hybrid_and_a_normal_one_does_not() {
    let refused = acquire_reused_json(Hdr10PlusMode::ExplicitHybrid, true);
    assert!(matches!(refused, Ok(None)), "an explicit hybrid stops");
    let measured = acquire_reused_json(Hdr10PlusMode::ExplicitHybrid, false)
        .unwrap()
        .expect("a normal extraction takes the hybrid");
    assert_eq!(measured.hdr10plus_mode, Hdr10PlusMode::ExplicitHybrid);
    assert_eq!(measured.hdr_type, HdrFormat::Hdr10WithMeasurements);
}

#[test]
fn a_reused_skip_reorder_json_keeps_the_default_hybrid_and_the_metadata_mode() {
    let measured = acquire_reused_json(Hdr10PlusMode::ImplicitHybrid, true)
        .unwrap()
        .expect("the default only warns");
    // Not `after_failure`: the owner kept the default on the hybrid with the fallback's JSON.
    assert_eq!(measured.hdr10plus_mode, Hdr10PlusMode::ImplicitHybrid);
    assert_eq!(measured.hdr_type, HdrFormat::Hdr10WithMeasurements);
    assert!(measured.hdr10plus_json.is_some());
    let measured = acquire_reused_json(Hdr10PlusMode::Metadata, true)
        .unwrap()
        .expect("the metadata mode only warns");
    assert_eq!(measured.hdr10plus_mode, Hdr10PlusMode::Metadata);
    assert_eq!(measured.hdr_type, HdrFormat::Hdr10Plus);
    assert!(measured.hdr10plus_json.is_some());
}

#[test]
fn the_skip_reorder_refusal_names_the_flag_and_the_way_out() {
    let refusal = hdr10plus::skip_reorder_refusal();
    assert!(refusal.contains("--skip-reorder"), "{refusal}");
    assert!(
        refusal.contains("Without --hdr10plus-l1 hybrid the file converts"),
        "{refusal}"
    );
}

#[test]
fn acquire_measurements_refuses_fel() {
    let (result, _, _) = run_acquire_measurements(&[], HdrFormat::DolbyVisionFel, |_| {});
    let error = result.err().expect("FEL is refused");
    assert_eq!(error.to_string(), FEL_UNSUPPORTED_MESSAGE);
}

#[test]
fn acquire_measurements_legacy_madvr_l1_reuses_the_first_candidate_without_sidecar() {
    let (result, total, current) = run_acquire_measurements(
        &["--legacy-madvr-l1"],
        HdrFormat::Hdr10WithMeasurements,
        |dir| {
            fs::write(dir.join("measurements.bin"), b"shared").unwrap();
            fs::write(dir.join("in_measurements.bin"), b"own").unwrap();
        },
    );
    let measured = result.unwrap().expect("legacy path reuses measurements");
    assert_eq!(measured.hdr_type, HdrFormat::Hdr10WithMeasurements);
    let file = measured.measurements_file.unwrap();
    assert_eq!(file.file_name().unwrap(), "in_measurements.bin");
    assert!(measured.l1_sidecar.is_none());
    assert!(measured.hdr10plus_json.is_none());
    assert!(measured.bl_source_file.ends_with("in.mkv"));
    assert_eq!((total, current), (6, 1), "reuse adds no analysis step");
}

#[test]
fn legacy_madvr_l1_warns_about_targets_from_an_analyzer_before_sidecar_v8() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("in_measurements.bin");
    fs::write(&bin, b"madvr").unwrap();
    let sidecar = metadata::l1_sidecar_path(&bin);
    // No sidecar beside the .bin: its analyzer is unknown.
    let notice = legacy_targets_notice(&bin).expect("warns without a sidecar");
    assert!(notice.contains("unknown version"), "{notice}");
    assert!(notice.contains("Delete the measurements file"), "{notice}");
    fs::write(&sidecar, br#"{"version": 7}"#).unwrap();
    let notice = legacy_targets_notice(&bin).expect("warns for v7");
    assert!(notice.contains("wrote sidecar v7"), "{notice}");
    fs::write(&sidecar, b"not json").unwrap();
    assert!(
        legacy_targets_notice(&bin).is_some(),
        "an unreadable sidecar warns"
    );
    for current in [8, 9] {
        fs::write(&sidecar, format!(r#"{{"version": {current}}}"#)).unwrap();
        assert_eq!(legacy_targets_notice(&bin), None, "v{current}");
    }
}

#[test]
fn find_reusable_measurements_without_candidates_loads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.mkv");
    fs::write(&input, b"src").unwrap();
    let args = Args::try_parse_from(["mkvdovi"]).unwrap();
    let (mut measurements_file, mut l1_sidecar) = (None, None);
    assert!(!find_reusable_measurements(
        &input,
        input.to_str().unwrap(),
        &args,
        None,
        false,
        &mut measurements_file,
        &mut l1_sidecar,
    ));
    assert!(measurements_file.is_none());
    assert!(l1_sidecar.is_none());
}

#[test]
fn hdr10plus_hybrid_reanalyzes_measurements_that_skipped_frames() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.mkv");
    fs::write(&input, b"src").unwrap();
    let measurements = dir.path().join("in_measurements.bin");
    fs::write(&measurements, b"bin").unwrap();
    let sidecar = hybrid_sidecar(6, 3);
    fs::write(
        metadata::l1_sidecar_path(&measurements),
        serde_json::to_vec(&sidecar).unwrap(),
    )
    .unwrap();
    let args = Args::try_parse_from(["mkvdovi", "--analysis-quality", "balanced"]).unwrap();
    let reuse = |hybrid| reuse_measurements(&input, &args, hybrid);
    // Plain HDR10 reuses it under balanced (with a warning); the hybrid needs every frame.
    assert_eq!(reuse(false), (true, true, true));
    assert_eq!(reuse(true), (false, false, false));
}

#[test]
fn hdr10plus_hybrid_reanalyzes_measurements_older_than_sidecar_v6() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.mkv");
    fs::write(&input, b"src").unwrap();
    let measurements = dir.path().join("in_measurements.bin");
    fs::write(&measurements, b"bin").unwrap();
    let args = Args::try_parse_from(["mkvdovi", "--analysis-quality", "accurate"]).unwrap();
    let sidecar_path = metadata::l1_sidecar_path(&measurements);

    fs::write(&sidecar_path, hybrid_sidecar(5, 1).to_string()).unwrap();
    assert_eq!(reuse_measurements(&input, &args, false), (true, true, true));
    assert_eq!(
        reuse_measurements(&input, &args, true),
        (false, false, false)
    );

    fs::write(&sidecar_path, hybrid_sidecar(6, 1).to_string()).unwrap();
    assert_eq!(reuse_measurements(&input, &args, true), (true, true, true));
}

#[test]
fn under_hwaccel_cuda_cpu_measurements_before_sidecar_v7_are_re_analyzed() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.mkv");
    fs::write(&input, b"src").unwrap();
    let measurements = dir.path().join("in_measurements.bin");
    fs::write(&measurements, b"bin").unwrap();
    let sidecar_path = metadata::l1_sidecar_path(&measurements);
    let args = |hwaccel| {
        Args::try_parse_from([
            "mkvdovi",
            "--hwaccel",
            hwaccel,
            "--analysis-quality",
            "accurate",
        ])
        .unwrap()
    };
    let (cuda, none) = (args("cuda"), args("none"));
    assert_eq!((cuda.hwaccel, none.hwaccel), (HwAccel::Cuda, HwAccel::None));

    // CPU analysis at downscale 1 before v7, as `hybrid_sidecar` writes it.
    fs::write(&sidecar_path, hybrid_sidecar(6, 1).to_string()).unwrap();
    for hybrid in [false, true] {
        assert_eq!(
            reuse_measurements(&input, &cuda, hybrid),
            (false, false, false),
            "hybrid={hybrid}"
        );
        assert_eq!(
            reuse_measurements(&input, &none, hybrid),
            (true, true, true),
            "hybrid={hybrid}"
        );
    }
    let mut gpu = hybrid_sidecar(6, 1);
    gpu["analysis"]["gpu"] = json!(true);
    fs::write(&sidecar_path, gpu.to_string()).unwrap();
    assert_eq!(reuse_measurements(&input, &cuda, false), (true, true, true));
    fs::write(&sidecar_path, hybrid_sidecar(7, 1).to_string()).unwrap();
    assert_eq!(reuse_measurements(&input, &cuda, false), (true, true, true));
}

/// A sidecar for a 4-frame `in.mkv`. From version 6 on it records the per-frame peak and the
/// analyzer's peak source, as the analyzer writes them.
fn hybrid_sidecar(version: u32, sample_rate: u32) -> serde_json::Value {
    let mut sidecar = json!({
        "version": version,
        "source": {"file_name": "in.mkv", "size_bytes": 3, "width": 1920, "height": 1080,
                   "transfer_function": "PQ (SMPTE 2084)", "stream_frames": 4,
                   "leading_skipped_frames": 0},
        "analysis": {"downscale": 1, "sample_rate": sample_rate, "gpu": false, "no_crop": false,
                     "luminance_mapping": "pq"},
        "scenes": [{"start": 0, "end": 3, "min_pq_12bit": 1, "avg_luma_pq_12bit": 500,
                    "avg_max_rgb_pq_12bit": 520, "max_pq_12bit": 2400}],
        "frames": {"min_pq_12bit": [1, 1, 1, 1], "avg_max_rgb_pq_12bit": [520, 520, 520, 520]}
    });
    if version >= 6 {
        sidecar["peak_source"] = json!("max");
        sidecar["frames"]["max_pq_12bit"] = json!([2400, 2300, 2200, 2100]);
    }
    sidecar
}

/// What `find_reusable_measurements` makes of the measurements next to `input`: whether it
/// reused them, and whether it filled in the file and the sidecar.
fn reuse_measurements(input: &Path, args: &Args, hybrid: bool) -> (bool, bool, bool) {
    let (mut measurements_file, mut l1_sidecar) = (None, None);
    let reused = find_reusable_measurements(
        input,
        input.to_str().unwrap(),
        args,
        None,
        hybrid,
        &mut measurements_file,
        &mut l1_sidecar,
    );
    (reused, measurements_file.is_some(), l1_sidecar.is_some())
}

/// A temp dir from an interrupted run: `extra.json` holding `config`, sealed `RPU.bin` and
/// `BL_RPU.hevc`, and the mux marker.
fn interrupted_run(config: &[u8]) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("extra.json"), config).unwrap();
    for artifact in ["RPU.bin", "BL_RPU.hevc"] {
        fs::write(temp.path().join(artifact), b"payload").unwrap();
        resume::mark_done(&temp.path().join(artifact)).unwrap();
    }
    fs::write(temp.path().join("mux.done"), b"").unwrap();
    temp
}

fn sealed(temp: &Path) -> [bool; 3] {
    [
        resume::is_complete(&temp.join("RPU.bin")),
        resume::is_complete(&temp.join("BL_RPU.hevc")),
        temp.join("mux.done").exists(),
    ]
}

/// `extra.json` bytes generated from `sidecar`, as `prepare_configuration` writes them for 8.1.
fn extra_json_from(sidecar: &serde_json::Value) -> Vec<u8> {
    let sidecar: metadata::L1Sidecar = serde_json::from_value(sidecar.clone()).unwrap();
    let meta: std::collections::HashMap<String, f64> = [
        ("min_dml", 0.0001),
        ("max_dml", 1000.0),
        ("max_cll", 1000.0),
        ("max_fall", 400.0),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    let file = tempfile::NamedTempFile::new().unwrap();
    metadata::generate_extra_json(
        file.path(),
        "8.1",
        &meta,
        &[100],
        None,
        None,
        Some(&sidecar),
    )
    .unwrap();
    fs::read(file.path()).unwrap()
}

#[test]
fn a_re_analysis_that_moves_l1_withdraws_the_rpu_of_an_interrupted_run() {
    // The RPU of an interrupted run was generated from measurements that went through the old
    // NVDEC conversion; the re-analysis under v7 reads one scene peak 19 codes lower.
    let old = hybrid_sidecar(6, 1);
    let mut new = hybrid_sidecar(7, 1);
    new["scenes"][0]["max_pq_12bit"] = json!(2381);
    new["frames"]["max_pq_12bit"][0] = json!(2381);
    let (old_config, new_config) = (extra_json_from(&old), extra_json_from(&new));
    assert_ne!(old_config, new_config, "the scene L1 must reach extra.json");

    let temp = interrupted_run(&old_config);
    let (extra, next) = (
        temp.path().join("extra.json"),
        temp.path().join("extra.json.next"),
    );
    fs::write(&next, &new_config).unwrap();
    commit_config(temp.path(), &next, &extra, true).unwrap();
    assert_eq!(sealed(temp.path()), [false, false, false]);
    assert_eq!(fs::read(&extra).unwrap(), new_config);

    // A re-analysis that measures the same L1 (here on the GPU) keeps the RPU.
    let mut same = new.clone();
    same["analysis"]["gpu"] = json!(true);
    let temp = interrupted_run(&new_config);
    let (extra, next) = (
        temp.path().join("extra.json"),
        temp.path().join("extra.json.next"),
    );
    fs::write(&next, extra_json_from(&same)).unwrap();
    commit_config(temp.path(), &next, &extra, true).unwrap();
    assert_eq!(sealed(temp.path()), [true, true, true]);
}

#[test]
fn withdraw_stale_rpu_withdraws_rpu_and_mux_when_the_config_changed() {
    let temp = interrupted_run(b"{\"new\": 1}");
    let extra = temp.path().join("extra.json");
    assert!(withdraw_stale_rpu(temp.path(), &extra, Some(b"{\"old\": 1}")).unwrap());
    assert_eq!(sealed(temp.path()), [false, false, false]);
    assert!(
        temp.path().join("RPU.bin").exists(),
        "only sentinels are withdrawn"
    );
}

#[test]
fn an_interrupted_withdrawal_leaves_the_old_config_so_the_next_run_withdraws_again() {
    let temp = interrupted_run(b"{\"old\": 1}");
    let extra = temp.path().join("extra.json");
    let next = temp.path().join("extra.json.next");
    fs::write(&next, b"{\"new\": 1}").unwrap();
    // A directory in place of the sentinel makes the withdrawal fail where a kill could stop it.
    let sentinel = temp.path().join("RPU.bin.done");
    fs::remove_file(&sentinel).unwrap();
    fs::create_dir(&sentinel).unwrap();
    assert!(commit_config(temp.path(), &next, &extra, true).is_err());
    assert_eq!(fs::read(&extra).unwrap(), b"{\"old\": 1}");

    fs::remove_dir(&sentinel).unwrap();
    resume::mark_done(&temp.path().join("RPU.bin")).unwrap();
    commit_config(temp.path(), &next, &extra, true).unwrap();
    assert_eq!(sealed(temp.path()), [false, false, false]);
    assert_eq!(fs::read(&extra).unwrap(), b"{\"new\": 1}");
    assert!(!next.exists());
}

#[test]
fn commit_config_without_resume_only_replaces_the_config() {
    let temp = interrupted_run(b"{\"old\": 1}");
    let extra = temp.path().join("extra.json");
    let next = temp.path().join("extra.json.next");
    fs::write(&next, b"{\"new\": 1}").unwrap();
    commit_config(temp.path(), &next, &extra, false).unwrap();
    assert_eq!(sealed(temp.path()), [true, true, true]);
    assert_eq!(fs::read(&extra).unwrap(), b"{\"new\": 1}");
}

#[test]
fn withdraw_stale_rpu_keeps_everything_when_the_config_is_unchanged() {
    let temp = interrupted_run(b"{\"same\": 1}");
    let extra = temp.path().join("extra.json");
    assert!(!withdraw_stale_rpu(temp.path(), &extra, Some(b"{\"same\": 1}")).unwrap());
    assert_eq!(sealed(temp.path()), [true, true, true]);
}

#[test]
fn withdraw_stale_rpu_treats_a_missing_previous_config_as_changed() {
    let temp = interrupted_run(b"{}");
    let extra = temp.path().join("extra.json");
    assert!(withdraw_stale_rpu(temp.path(), &extra, None).unwrap());
    assert_eq!(sealed(temp.path()), [false, false, false]);
}

#[test]
fn withdraw_stale_rpu_without_artifacts_is_not_an_error() {
    let temp = tempfile::tempdir().unwrap();
    let extra = temp.path().join("extra.json");
    fs::write(&extra, b"{}").unwrap();
    assert!(withdraw_stale_rpu(temp.path(), &extra, None).unwrap());
    assert_eq!(sealed(temp.path()), [false, false, false]);
}

/// A temp dir and default `Args` for the delivery phases; the input is `<dir>/in.mkv`.
fn delivery_dirs() -> (tempfile::TempDir, PathBuf, PathBuf, Args) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.mkv");
    fs::write(&input, b"src").unwrap();
    let temp = dir.path().join("mkvdovi_temp_in");
    fs::create_dir(&temp).unwrap();
    (dir, input, temp, Args::try_parse_from(["mkvdovi"]).unwrap())
}

#[test]
fn extract_base_layer_passes_a_raw_hevc_source_through() {
    let (_dir, input, temp, args) = delivery_dirs();
    let clean_bl = temp.join("BL_clean.hevc");
    fs::write(&clean_bl, b"hevc").unwrap();
    let bl = extract_base_layer(
        input.to_str().unwrap(),
        &clean_bl,
        &temp,
        HdrFormat::Hdr10WithMeasurements,
        true,
        &args,
    )
    .unwrap();
    assert_eq!(bl, Some(clean_bl));
    assert!(!temp.join("BL.hevc").exists());
}

#[test]
fn extract_base_layer_reuses_a_sealed_extraction_when_resuming() {
    let (_dir, input, temp, args) = delivery_dirs();
    let bl_hevc = temp.join("BL.hevc");
    fs::write(&bl_hevc, b"hevc").unwrap();
    resume::mark_done(&bl_hevc).unwrap();
    let bl = extract_base_layer(
        input.to_str().unwrap(),
        &input,
        &temp,
        HdrFormat::Hdr10Unsupported,
        true,
        &args,
    )
    .unwrap();
    assert_eq!(bl, Some(bl_hevc));
}

#[test]
fn inject_rpu_reuses_a_sealed_injection_and_keeps_its_input() {
    let (_dir, _input, temp, args) = delivery_dirs();
    let bl_hevc = temp.join("BL.hevc");
    fs::write(&bl_hevc, b"hevc").unwrap();
    let bl_rpu_hevc = temp.join("BL_RPU.hevc");
    fs::write(&bl_rpu_hevc, b"hevc+rpu").unwrap();
    resume::mark_done(&bl_rpu_hevc).unwrap();
    let injected = inject_rpu(&bl_hevc, &temp.join("RPU.bin"), &temp, true, true, &args).unwrap();
    assert_eq!(injected, Some(bl_rpu_hevc));
    assert!(bl_hevc.exists(), "only a fresh injection drops its input");
}

#[test]
fn mux_output_reuses_a_finished_mux_when_resuming() {
    let (dir, input, temp, args) = delivery_dirs();
    let output = dir.path().join("in.DV.mkv");
    fs::write(&output, b"out").unwrap();
    fs::write(temp.join("mux.done"), b"").unwrap();
    let muxed = mux_output(
        input.to_str().unwrap(),
        &temp.join("BL_RPU.hevc"),
        &output,
        &temp,
        true,
        &args,
        &MuxTiming::Stream { sync_ms: 0 },
    )
    .unwrap();
    assert!(muxed);
    assert_eq!(fs::read(&output).unwrap(), b"out");
}

// Delivery steps against fake external tools. The functions under test spawn `ffmpeg`,
// `ffprobe`, `mkvmerge`, `dovi_tool` and `hdr10plus_tool` from PATH, and PATH is shared by every
// test in this binary, so the shims are installed once and pass every call through to the real
// tool unless one of its arguments lies in a `FakeTools` sandbox that scripts that tool.

#[cfg(unix)]
const SHIMMED_TOOLS: [&str; 4] = ["ffmpeg", "ffprobe", "mkvmerge", "dovi_tool"];

/// Shimmed only when found, so that the tests that do not script it still run without it.
#[cfg(unix)]
const OPTIONAL_SHIMMED_TOOLS: [&str; 1] = ["hdr10plus_tool"];

#[cfg(unix)]
const SANDBOX_PREFIX: &str = "mkvdovi-fake-";

/// Puts the pass-through shims first on PATH, once per process. False when a real tool is
/// missing: a shim would then hide that from the tests that skip without it.
#[cfg(unix)]
fn shims_installed() -> bool {
    shimmed_tools().is_some()
}

/// The tools shimmed on PATH, or `None` when one of [`SHIMMED_TOOLS`] is missing.
#[cfg(unix)]
fn shimmed_tools() -> Option<&'static [&'static str]> {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::OnceLock;

    static INSTALLED: OnceLock<Option<Vec<&'static str>>> = OnceLock::new();
    let installed = INSTALLED.get_or_init(|| {
        let mut real = Vec::new();
        for tool in SHIMMED_TOOLS {
            match crate::external::find_tool(tool) {
                Some(path) => real.push((tool, path)),
                None => return None,
            }
        }
        for tool in OPTIONAL_SHIMMED_TOOLS {
            if let Some(path) = crate::external::find_tool(tool) {
                real.push((tool, path));
            }
        }
        let tools: Vec<&'static str> = real.iter().map(|&(tool, _)| tool).collect();
        let mut scripts = Vec::new();
        for (tool, real_path) in real {
            // A sandbox call appends its argv to `<root>/fake/<tool>.calls` and runs
            // `<root>/fake/<tool>.sh` with IN (after -i), OUT (after -o, else the last
            // argument), LAST and REAL (the real tool) set. That script must exit itself.
            let script = format!(
                r#"#!/bin/sh
# mkvmerge's identification (`-J`, the mux timing's frame-rate prediction, ROADMAP E7.4) is
# never scripted or counted: the real tool reads the fake stream, finds no rate, and the mux
# keeps the stream's timing.
case "{tool} $1" in "mkvmerge -J") exec '{real}' "$@" ;; esac
root=
for arg in "$@"; do
  case "$arg" in
    */{SANDBOX_PREFIX}*) root=$(printf '%s\n' "$arg" | sed 's|^\(.*/{SANDBOX_PREFIX}[^/]*\).*|\1|'); break ;;
  esac
done
if [ -n "$root" ] && [ -f "$root/fake/{tool}.sh" ]; then
  printf '%s\n' "$*" >> "$root/fake/{tool}.calls"
  IN= OUT= LAST= prev=
  for arg in "$@"; do
    case "$prev" in -i) IN=$arg ;; -o) OUT=$arg ;; esac
    prev=$arg
    LAST=$arg
  done
  [ -n "$OUT" ] || OUT=$LAST
  REAL='{real}'
  . "$root/fake/{tool}.sh"
  exit 99
fi
exec '{real}' "$@"
"#,
                real = real_path.display()
            );
            scripts.push((tool, script));
        }
        // One directory per set of scripts next to this test binary, reused by every process
        // instead of a temp dir each run leaves behind. Each shim is written under a
        // process-unique name and renamed into place, so concurrent processes never see a
        // partial script.
        let key = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            scripts.hash(&mut hasher);
            hasher.finish()
        };
        let exe = std::env::current_exe().unwrap();
        let bin = exe
            .parent()
            .unwrap()
            .join(format!("mkvdovi-shims-{key:016x}"));
        fs::create_dir_all(&bin).unwrap();
        for (tool, script) in scripts {
            let staged = bin.join(format!(".{tool}.{}", std::process::id()));
            fs::write(&staged, script).unwrap();
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).unwrap();
            fs::rename(&staged, bin.join(tool)).unwrap();
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![bin];
        dirs.extend(std::env::split_paths(&path));
        std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());
        Some(tools)
    });
    installed.as_deref()
}

/// A sandbox holding `in.mkv`, the temp dir `mkvdovi_temp_in` and the output path `in.DV.mkv`.
#[cfg(unix)]
struct FakeTools {
    dir: tempfile::TempDir,
    input: PathBuf,
    temp: PathBuf,
    output: PathBuf,
}

#[cfg(unix)]
impl FakeTools {
    /// `None` (after a skip message) when a real tool is missing.
    fn new(test: &str) -> Option<Self> {
        if !shims_installed() {
            println!("Skipping {test}: ffmpeg, ffprobe, mkvmerge or dovi_tool not found in PATH");
            return None;
        }
        let dir = tempfile::Builder::new()
            .prefix(SANDBOX_PREFIX)
            .tempdir()
            .unwrap();
        fs::create_dir(dir.path().join("fake")).unwrap();
        let input = dir.path().join("in.mkv");
        fs::write(&input, b"source mkv").unwrap();
        let temp = dir.path().join("mkvdovi_temp_in");
        fs::create_dir(&temp).unwrap();
        let output = dir.path().join("in.DV.mkv");
        Some(Self {
            dir,
            input,
            temp,
            output,
        })
    }

    fn input(&self) -> &str {
        self.input.to_str().unwrap()
    }

    fn script(&self, tool: &str, body: &str) {
        fs::write(
            self.dir.path().join("fake").join(format!("{tool}.sh")),
            body,
        )
        .unwrap();
    }

    /// The argv of every call of `tool`, one string each.
    fn calls(&self, tool: &str) -> Vec<String> {
        fs::read_to_string(self.dir.path().join("fake").join(format!("{tool}.calls")))
            .map(|log| log.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// Writes `artifact` with `content`, with its completion sentinel when `sealed`.
    fn leave(&self, artifact: &Path, content: &[u8], sealed: bool) {
        fs::write(artifact, content).unwrap();
        if sealed {
            resume::mark_done(artifact).unwrap();
        }
    }
}

/// Behaves like a working tool: fails on a missing or empty `-i` input, else writes `content`.
#[cfg(unix)]
fn writes(content: &str) -> String {
    format!("[ -z \"$IN\" ] || [ -s \"$IN\" ] || exit 1\nprintf '{content}' > \"$OUT\"\nexit 0\n")
}

#[cfg(unix)]
const EXITS_WITH_EMPTY_OUTPUT: &str = ": > \"$OUT\"\nexit 0\n";

#[cfg(unix)]
const FAILS_AFTER_PARTIAL_OUTPUT: &str = "printf 'partial' > \"$OUT\"\nexit 1\n";

#[cfg(unix)]
const FAILS: &str = "exit 1\n";

/// dovi_tool that reads the MKV with `mkv_body` and a raw HEVC stream with `raw_body`.
#[cfg(unix)]
fn dovi_tool_by_input(mkv_body: &str, raw_body: &str) -> String {
    format!("case \"$IN\" in\n*.mkv)\n{mkv_body}\n;;\nesac\n{raw_body}")
}

/// ffprobe that reads `frames` video frames, 42 ms apart from 0, from every file but a raw HEVC
/// stream (the source and the muxed output), and `units` access units from a raw HEVC stream
/// (a dovi_tool output). `None` makes that read fail.
#[cfg(unix)]
fn probes(frames: Option<usize>, units: Option<usize>) -> String {
    let hevc = units.map_or_else(
        || "exit 1".to_owned(),
        |units| format!("echo {units}\n  exit 0"),
    );
    let other = frames.map_or_else(
        || "exit 1".to_owned(),
        |frames| {
            let packets: Vec<String> = (0..frames)
                .map(|frame| format!("{{\"pts\":{},\"duration\":42}}", frame * 42))
                .collect();
            format!(
                "printf '%s' '{{\"packets\":[{}],\"streams\":[{{\"time_base\":\"1/1000\"}}],\
                 \"format\":{{\"format_name\":\"matroska,webm\"}}}}'\nexit 0",
                packets.join(",")
            )
        },
    );
    format!("case \"$LAST\" in\n*.hevc)\n  {hevc} ;;\nesac\n{other}\n")
}

#[cfg(unix)]
fn args_with_dovi_input(mode: DoviInput) -> Args {
    let mut args = Args::try_parse_from(["mkvdovi"]).unwrap();
    args.dovi_input = mode;
    args
}

#[cfg(unix)]
fn run_clean_base_layer(fake: &FakeTools, mode: DoviInput, resume: bool) -> Result<PathBuf> {
    extract_clean_base_layer(
        fake.input(),
        &fake.temp,
        "Extracting HEVC",
        &args_with_dovi_input(mode),
        resume,
    )
}

#[cfg(unix)]
fn run_mel_to_profile81(fake: &FakeTools, mode: DoviInput, resume: bool) -> bool {
    convert_mel_to_profile81(
        fake.input(),
        &fake.temp,
        &fake.output,
        &args_with_dovi_input(mode),
        resume,
    )
    .unwrap()
        != Delivery::Failed
}

#[cfg(unix)]
fn called_with(calls: &[String], needle: &str) -> bool {
    calls.iter().any(|call| call.contains(needle))
}

/// True when `--verify`'s extraction ran: ffmpeg writing raw HEVC to its stdout, the pipe into
/// `dovi_tool extract-rpu -` (ROADMAP E7.4).
#[cfg(unix)]
fn verify_extraction_ran(ffmpeg_calls: &[String]) -> bool {
    ffmpeg_calls.iter().any(|call| call.ends_with("-f hevc -"))
}

#[cfg(unix)]
#[test]
fn direct_mkv_step_that_succeeds_needs_no_ffmpeg_extraction() {
    let Some(fake) = FakeTools::new("direct_mkv_step_that_succeeds_needs_no_ffmpeg_extraction")
    else {
        return;
    };
    fake.script("ffprobe", &probes(Some(3), Some(3)));
    fake.script("dovi_tool", &writes("clean"));
    fake.script("ffmpeg", &writes("raw"));
    let clean_bl = run_clean_base_layer(&fake, DoviInput::Mkv, true).unwrap();
    assert_eq!(clean_bl, fake.temp.join("BL_clean.hevc"));
    assert!(resume::is_complete(&clean_bl));
    let dovi = fake.calls("dovi_tool");
    assert_eq!(dovi.len(), 1, "{dovi:?}");
    assert!(called_with(&dovi, &format!("-i {}", fake.input())));
    assert!(fake.calls("ffmpeg").is_empty());
}

#[cfg(unix)]
#[test]
fn direct_mkv_step_reads_the_mkv_without_resume_although_a_raw_stream_is_sealed() {
    let Some(fake) = FakeTools::new(
        "direct_mkv_step_reads_the_mkv_without_resume_although_a_raw_stream_is_sealed",
    ) else {
        return;
    };
    fake.script("ffprobe", &probes(Some(3), Some(3)));
    fake.leave(&fake.temp.join("DV_raw.hevc"), b"old raw", true);
    fake.script("dovi_tool", &writes("clean"));
    fake.script("ffmpeg", &writes("raw"));
    run_clean_base_layer(&fake, DoviInput::Mkv, false).unwrap();
    let dovi = fake.calls("dovi_tool");
    assert_eq!(dovi.len(), 1, "{dovi:?}");
    assert!(called_with(&dovi, &format!("-i {}", fake.input())));
    assert!(fake.calls("ffmpeg").is_empty());
}

#[cfg(unix)]
#[test]
fn resume_with_a_sealed_raw_stream_feeds_that_stream_instead_of_the_mkv() {
    let Some(fake) =
        FakeTools::new("resume_with_a_sealed_raw_stream_feeds_that_stream_instead_of_the_mkv")
    else {
        return;
    };
    let raw = fake.temp.join("DV_raw.hevc");
    fake.leave(&raw, b"sealed raw", true);
    fake.script("dovi_tool", &writes("clean"));
    fake.script("ffmpeg", &writes("raw"));
    run_clean_base_layer(&fake, DoviInput::Mkv, true).unwrap();
    let dovi = fake.calls("dovi_tool");
    assert_eq!(dovi.len(), 1, "{dovi:?}");
    assert!(called_with(&dovi, &format!("-i {}", raw.display())));
    assert!(
        fake.calls("ffmpeg").is_empty(),
        "the sealed stream is reused"
    );
}

#[cfg(unix)]
#[test]
fn direct_mkv_step_with_empty_output_falls_back_to_ffmpeg_extraction() {
    let Some(fake) =
        FakeTools::new("direct_mkv_step_with_empty_output_falls_back_to_ffmpeg_extraction")
    else {
        return;
    };
    fake.script("ffprobe", &probes(Some(3), Some(3)));
    fake.script(
        "dovi_tool",
        &dovi_tool_by_input(EXITS_WITH_EMPTY_OUTPUT, &writes("clean from raw")),
    );
    fake.script("ffmpeg", &writes("raw"));
    let clean_bl = run_clean_base_layer(&fake, DoviInput::Mkv, true).unwrap();
    assert_eq!(fake.calls("ffmpeg").len(), 1);
    assert_eq!(fake.calls("dovi_tool").len(), 2);
    assert_eq!(fs::read(&clean_bl).unwrap(), b"clean from raw");
}

#[cfg(unix)]
#[test]
fn direct_mkv_step_that_fails_after_writing_output_falls_back_to_ffmpeg_extraction() {
    let Some(fake) = FakeTools::new(
        "direct_mkv_step_that_fails_after_writing_output_falls_back_to_ffmpeg_extraction",
    ) else {
        return;
    };
    fake.script("ffprobe", &probes(Some(3), Some(3)));
    fake.script(
        "dovi_tool",
        &dovi_tool_by_input(FAILS_AFTER_PARTIAL_OUTPUT, &writes("clean from raw")),
    );
    fake.script("ffmpeg", &writes("raw"));
    let clean_bl = run_clean_base_layer(&fake, DoviInput::Mkv, true).unwrap();
    assert_eq!(fake.calls("ffmpeg").len(), 1);
    assert_eq!(fs::read(&clean_bl).unwrap(), b"clean from raw");
}

/// The clean-BL step under `--dovi-input mkv` with a 48-frame source whose direct dovi_tool
/// output ffprobe counts as `units` access units (`None`: unreadable), and whose raw read
/// writes "clean from raw". Returns the clean BL's content and the ffmpeg and dovi_tool calls.
#[cfg(unix)]
fn clean_base_layer_with_counts(
    test: &str,
    frames: Option<usize>,
    units: Option<usize>,
) -> Option<(Vec<u8>, Vec<String>, Vec<String>)> {
    let fake = FakeTools::new(test)?;
    fake.script("ffprobe", &probes(frames, units));
    fake.script(
        "dovi_tool",
        &dovi_tool_by_input(&writes("clean from mkv"), &writes("clean from raw")),
    );
    fake.script("ffmpeg", &writes("raw"));
    let clean_bl = run_clean_base_layer(&fake, DoviInput::Mkv, true).unwrap();
    assert!(resume::is_complete(&clean_bl));
    Some((
        fs::read(&clean_bl).unwrap(),
        fake.calls("ffmpeg"),
        fake.calls("dovi_tool"),
    ))
}

#[cfg(unix)]
#[test]
fn direct_mkv_step_with_a_truncated_output_falls_back_to_ffmpeg_extraction() {
    let Some((clean, ffmpeg, dovi)) = clean_base_layer_with_counts(
        "direct_mkv_step_with_a_truncated_output_falls_back_to_ffmpeg_extraction",
        Some(48),
        Some(30),
    ) else {
        return;
    };
    assert_eq!(dovi.len(), 2, "{dovi:?}");
    assert_eq!(ffmpeg.len(), 1, "{ffmpeg:?}");
    assert_eq!(clean, b"clean from raw");
}

#[cfg(unix)]
#[test]
fn direct_mkv_step_with_more_access_units_than_frames_falls_back_to_ffmpeg_extraction() {
    let Some((clean, ffmpeg, dovi)) = clean_base_layer_with_counts(
        "direct_mkv_step_with_more_access_units_than_frames_falls_back_to_ffmpeg_extraction",
        Some(48),
        Some(50),
    ) else {
        return;
    };
    assert_eq!(dovi.len(), 2, "{dovi:?}");
    assert_eq!(ffmpeg.len(), 1, "{ffmpeg:?}");
    assert_eq!(clean, b"clean from raw");
}

#[cfg(unix)]
#[test]
fn direct_mkv_step_with_the_source_frame_count_is_accepted_without_extraction() {
    let Some((clean, ffmpeg, dovi)) = clean_base_layer_with_counts(
        "direct_mkv_step_with_the_source_frame_count_is_accepted_without_extraction",
        Some(48),
        Some(48),
    ) else {
        return;
    };
    assert_eq!(dovi.len(), 1, "{dovi:?}");
    assert!(ffmpeg.is_empty(), "{ffmpeg:?}");
    assert_eq!(clean, b"clean from mkv");
}

#[cfg(unix)]
#[test]
fn direct_mkv_step_whose_output_cannot_be_counted_falls_back_to_ffmpeg_extraction() {
    let Some((clean, ffmpeg, dovi)) = clean_base_layer_with_counts(
        "direct_mkv_step_whose_output_cannot_be_counted_falls_back_to_ffmpeg_extraction",
        Some(48),
        None,
    ) else {
        return;
    };
    assert_eq!(dovi.len(), 2, "{dovi:?}");
    assert_eq!(ffmpeg.len(), 1, "{ffmpeg:?}");
    assert_eq!(clean, b"clean from raw");
}

#[cfg(unix)]
#[test]
fn a_source_whose_frames_cannot_be_counted_skips_the_direct_mkv_read() {
    let Some((clean, ffmpeg, dovi)) = clean_base_layer_with_counts(
        "a_source_whose_frames_cannot_be_counted_skips_the_direct_mkv_read",
        None,
        Some(48),
    ) else {
        return;
    };
    assert_eq!(dovi.len(), 1, "{dovi:?}");
    assert!(!called_with(&dovi, ".mkv"), "{dovi:?}");
    assert_eq!(ffmpeg.len(), 1, "{ffmpeg:?}");
    assert_eq!(clean, b"clean from raw");
}

#[cfg(unix)]
#[test]
fn mel_conversion_with_a_truncated_direct_output_muxes_the_extracted_stream() {
    let Some(fake) =
        FakeTools::new("mel_conversion_with_a_truncated_direct_output_muxes_the_extracted_stream")
    else {
        return;
    };
    fake.script("ffprobe", &probes(Some(3), Some(2)));
    fake.script(
        "dovi_tool",
        &dovi_tool_by_input(&writes("p81 from mkv"), &writes("p81 from raw")),
    );
    fake.script("ffmpeg", &writes("raw"));
    fake.script("mkvmerge", &writes("muxed"));
    assert!(run_mel_to_profile81(&fake, DoviInput::Mkv, true));
    assert_eq!(fake.calls("ffmpeg").len(), 1);
    let converted = fake.temp.join("P81_discard.hevc");
    assert_eq!(fs::read(&converted).unwrap(), b"p81 from raw");
    assert!(resume::is_complete(&converted));
    assert_eq!(fake.calls("mkvmerge").len(), 1);
}

#[test]
fn packet_count_is_read_from_a_single_number() {
    assert_eq!(parse_packet_count("48\n"), Ok(48));
    for output in ["", "N/A\n", "48,\n", "-1\n"] {
        assert!(parse_packet_count(output).is_err(), "{output:?}");
    }
}

#[cfg(unix)]
#[test]
fn raw_step_that_succeeds_seals_the_clean_base_layer_and_drops_the_raw_stream() {
    let Some(fake) = FakeTools::new(
        "raw_step_that_succeeds_seals_the_clean_base_layer_and_drops_the_raw_stream",
    ) else {
        return;
    };
    fake.script("dovi_tool", &writes("clean"));
    fake.script("ffmpeg", &writes("raw"));
    let clean_bl = run_clean_base_layer(&fake, DoviInput::Raw, true).unwrap();
    assert_eq!(clean_bl, fake.temp.join("BL_clean.hevc"));
    assert!(resume::is_complete(&clean_bl));
    let ffmpeg = fake.calls("ffmpeg");
    assert_eq!(ffmpeg.len(), 1);
    assert!(called_with(&ffmpeg, &format!("-i {}", fake.input())));
    let raw = fake.temp.join("DV_raw.hevc");
    assert!(called_with(
        &fake.calls("dovi_tool"),
        &format!("-i {}", raw.display())
    ));
    assert!(!raw.exists());
    assert!(!resume::marker_path(&raw).exists());
}

#[cfg(unix)]
#[test]
fn raw_step_with_empty_output_is_a_failure() {
    let Some(fake) = FakeTools::new("raw_step_with_empty_output_is_a_failure") else {
        return;
    };
    fake.script("dovi_tool", EXITS_WITH_EMPTY_OUTPUT);
    fake.script("ffmpeg", &writes("raw"));
    let error = run_clean_base_layer(&fake, DoviInput::Raw, true).unwrap_err();
    assert!(error.to_string().contains("Failed to remove"), "{error:#}");
    assert!(!resume::marker_path(&fake.temp.join("BL_clean.hevc")).exists());
}

#[cfg(unix)]
#[test]
fn raw_step_that_fails_after_writing_output_is_a_failure() {
    let Some(fake) = FakeTools::new("raw_step_that_fails_after_writing_output_is_a_failure") else {
        return;
    };
    fake.script("dovi_tool", FAILS_AFTER_PARTIAL_OUTPUT);
    fake.script("ffmpeg", &writes("raw"));
    assert!(run_clean_base_layer(&fake, DoviInput::Raw, true).is_err());
    assert!(!resume::marker_path(&fake.temp.join("BL_clean.hevc")).exists());
}

#[cfg(unix)]
#[test]
fn clean_base_layer_is_reused_only_when_sealed_and_resuming() {
    // (resume, sealed) -> dovi_tool must run again.
    for (resume_enabled, sealed, reruns) in [
        (true, true, false),
        (true, false, true),
        (false, true, true),
    ] {
        let Some(fake) = FakeTools::new("clean_base_layer_is_reused_only_when_sealed_and_resuming")
        else {
            return;
        };
        let clean_bl = fake.temp.join("BL_clean.hevc");
        fake.leave(&clean_bl, b"previous clean", sealed);
        fake.script("dovi_tool", &writes("new clean"));
        fake.script("ffmpeg", &writes("raw"));
        let returned = run_clean_base_layer(&fake, DoviInput::Raw, resume_enabled).unwrap();
        assert_eq!(returned, clean_bl);
        let case = format!("resume {resume_enabled}, sealed {sealed}");
        assert_eq!(!fake.calls("dovi_tool").is_empty(), reruns, "{case}");
        let expected: &[u8] = if reruns {
            b"new clean"
        } else {
            b"previous clean"
        };
        assert_eq!(fs::read(&clean_bl).unwrap(), expected, "{case}");
    }
}

#[cfg(unix)]
#[test]
fn raw_extraction_is_reused_only_when_sealed_and_resuming() {
    // convert_mel_to_profile81 keeps DV_raw.hevc, so its sentinel can be checked afterwards.
    for (resume_enabled, sealed, extracts) in [
        (true, true, false),
        (true, false, true),
        (false, true, true),
        (false, false, true),
    ] {
        let Some(fake) = FakeTools::new("raw_extraction_is_reused_only_when_sealed_and_resuming")
        else {
            return;
        };
        let raw = fake.temp.join("DV_raw.hevc");
        fake.leave(&raw, b"previous raw", sealed);
        fake.script("ffmpeg", &writes("new raw"));
        fake.script("dovi_tool", &writes("p81"));
        fake.script("mkvmerge", &writes("muxed"));
        assert!(run_mel_to_profile81(&fake, DoviInput::Raw, resume_enabled));
        let case = format!("resume {resume_enabled}, sealed {sealed}");
        assert_eq!(!fake.calls("ffmpeg").is_empty(), extracts, "{case}");
        let expected: &[u8] = if extracts {
            b"new raw"
        } else {
            b"previous raw"
        };
        assert_eq!(fs::read(&raw).unwrap(), expected, "{case}");
        assert!(resume::is_complete(&raw), "{case}");
    }
}

#[cfg(unix)]
#[test]
fn raw_extraction_failure_is_an_error_and_leaves_no_sentinel() {
    for ffmpeg in [FAILS_AFTER_PARTIAL_OUTPUT, FAILS, "exit 0\n"] {
        let Some(fake) =
            FakeTools::new("raw_extraction_failure_is_an_error_and_leaves_no_sentinel")
        else {
            return;
        };
        fake.script("ffmpeg", ffmpeg);
        fake.script("dovi_tool", &writes("clean"));
        let error = run_clean_base_layer(&fake, DoviInput::Raw, true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Failed to extract HEVC bitstream"),
            "{ffmpeg:?}: {error:#}"
        );
        assert!(
            !resume::marker_path(&fake.temp.join("DV_raw.hevc")).exists(),
            "{ffmpeg:?}"
        );
        assert!(fake.calls("dovi_tool").is_empty(), "{ffmpeg:?}");
    }
}

#[cfg(unix)]
#[test]
fn mel_conversion_that_succeeds_seals_the_stream_and_muxes_it() {
    let Some(fake) = FakeTools::new("mel_conversion_that_succeeds_seals_the_stream_and_muxes_it")
    else {
        return;
    };
    fake.script("ffprobe", &probes(Some(3), Some(3)));
    fake.script("dovi_tool", &writes("p81"));
    fake.script("mkvmerge", &writes("muxed"));
    assert!(run_mel_to_profile81(&fake, DoviInput::Mkv, true));
    let converted = fake.temp.join("P81_discard.hevc");
    assert!(resume::is_complete(&converted));
    let mkvmerge = fake.calls("mkvmerge");
    assert_eq!(mkvmerge.len(), 1);
    assert!(called_with(&mkvmerge, &converted.display().to_string()));
    assert!(fake.temp.join("mux.done").exists());
    assert_eq!(fs::read(&fake.output).unwrap(), b"muxed");
}

#[cfg(unix)]
#[test]
fn mel_conversion_that_fails_is_reported_and_never_muxed() {
    let Some(fake) = FakeTools::new("mel_conversion_that_fails_is_reported_and_never_muxed") else {
        return;
    };
    fake.script("ffprobe", &probes(Some(3), Some(3)));
    fake.script("dovi_tool", FAILS);
    fake.script("ffmpeg", &writes("raw"));
    fake.script("mkvmerge", &writes("muxed"));
    assert!(!run_mel_to_profile81(&fake, DoviInput::Mkv, true));
    assert!(!resume::marker_path(&fake.temp.join("P81_discard.hevc")).exists());
    assert!(fake.calls("mkvmerge").is_empty());
    assert!(!fake.output.exists());
}

#[cfg(unix)]
#[test]
fn mel_conversion_is_reused_only_when_sealed_and_resuming() {
    for (resume_enabled, sealed, converts) in [
        (true, true, false),
        (true, false, true),
        (false, true, true),
    ] {
        let Some(fake) = FakeTools::new("mel_conversion_is_reused_only_when_sealed_and_resuming")
        else {
            return;
        };
        fake.script("ffprobe", &probes(Some(3), Some(3)));
        let converted = fake.temp.join("P81_discard.hevc");
        fake.leave(&converted, b"previous p81", sealed);
        fake.script("dovi_tool", &writes("new p81"));
        fake.script("mkvmerge", &writes("muxed"));
        assert!(run_mel_to_profile81(&fake, DoviInput::Mkv, resume_enabled));
        let case = format!("resume {resume_enabled}, sealed {sealed}");
        assert_eq!(!fake.calls("dovi_tool").is_empty(), converts, "{case}");
        let expected: &[u8] = if converts {
            b"new p81"
        } else {
            b"previous p81"
        };
        assert_eq!(fs::read(&converted).unwrap(), expected, "{case}");
    }
}

#[cfg(unix)]
#[test]
fn mel_mux_is_reused_only_with_resume_output_and_marker() {
    // (resume, output exists, mux.done exists) -> mkvmerge must run.
    for (resume_enabled, output, marker, muxes) in [
        (true, true, true, false),
        (true, true, false, true),
        (true, false, true, true),
        (false, true, true, true),
    ] {
        let Some(fake) = FakeTools::new("mel_mux_is_reused_only_with_resume_output_and_marker")
        else {
            return;
        };
        fake.script("ffprobe", &probes(Some(3), Some(3)));
        fake.leave(&fake.temp.join("P81_discard.hevc"), b"p81", true);
        if output {
            fs::write(&fake.output, b"previous mux").unwrap();
        }
        if marker {
            fs::write(fake.temp.join("mux.done"), b"").unwrap();
        }
        fake.script("dovi_tool", &writes("p81"));
        fake.script("mkvmerge", &writes("new mux"));
        assert!(run_mel_to_profile81(&fake, DoviInput::Mkv, resume_enabled));
        let case = format!("resume {resume_enabled}, output {output}, marker {marker}");
        assert_eq!(!fake.calls("mkvmerge").is_empty(), muxes, "{case}");
        let expected: &[u8] = if muxes { b"new mux" } else { b"previous mux" };
        assert_eq!(fs::read(&fake.output).unwrap(), expected, "{case}");
    }
}

#[cfg(unix)]
#[test]
fn mel_mux_failure_is_reported_without_a_mux_marker() {
    let Some(fake) = FakeTools::new("mel_mux_failure_is_reported_without_a_mux_marker") else {
        return;
    };
    fake.script("ffprobe", &probes(Some(3), Some(3)));
    fake.script("dovi_tool", &writes("p81"));
    fake.script("mkvmerge", FAILS_AFTER_PARTIAL_OUTPUT);
    assert!(!run_mel_to_profile81(&fake, DoviInput::Mkv, true));
    assert_eq!(fake.calls("mkvmerge").len(), 1);
    assert!(!fake.temp.join("mux.done").exists());
}

#[cfg(unix)]
fn run_mel_fast_path(fake: &FakeTools, verify: bool) -> bool {
    let mut args = args_with_dovi_input(DoviInput::Mkv);
    args.verify = verify;
    convert_mel_fast_path(
        fake.input(),
        &fake.temp,
        &fake.output,
        &args,
        true,
        HdrFormat::DolbyVisionMel,
        Instant::now(),
    )
    .unwrap()
}

#[cfg(unix)]
#[test]
fn mel_fast_path_without_verify_finishes_and_never_runs_verification() {
    let Some(fake) =
        FakeTools::new("mel_fast_path_without_verify_finishes_and_never_runs_verification")
    else {
        return;
    };
    script_counts(&fake, 3, 3);
    fake.script("mkvmerge", &writes("muxed"));
    // Verification starts by extracting the muxed video with ffmpeg; make that fail.
    fake.script("ffmpeg", FAILS);
    assert!(run_mel_fast_path(&fake, false));
    assert!(fake.calls("ffmpeg").is_empty());
    assert!(!fake.temp.exists(), "finish_success removes the temp dir");
    assert!(fake.input.exists(), "a Dolby Vision source is kept");
    assert_eq!(fs::read(&fake.output).unwrap(), b"muxed");
}

#[cfg(unix)]
#[test]
fn mel_fast_path_with_failed_verification_reports_failure_and_keeps_the_temp_dir() {
    let Some(fake) = FakeTools::new(
        "mel_fast_path_with_failed_verification_reports_failure_and_keeps_the_temp_dir",
    ) else {
        return;
    };
    script_counts(&fake, 3, 3);
    fake.script("mkvmerge", &writes("muxed"));
    fake.script("ffmpeg", FAILS);
    assert!(!run_mel_fast_path(&fake, true));
    assert!(verify_extraction_ran(&fake.calls("ffmpeg")));
    assert!(fake.temp.exists());
    assert!(fake.input.exists());
}

/// Scripts ffprobe (every file has `frames` video frames, 42 ms apart, and a raw HEVC stream as
/// many access units) and dovi_tool, whose
/// `extract-rpu` writes `rpus` RPUs and whose every other call writes `p81`.
#[cfg(unix)]
fn script_counts(fake: &FakeTools, frames: usize, rpus: usize) {
    fake.script("ffprobe", &probes(Some(frames), Some(frames)));
    fake.script(
        "dovi_tool",
        &format!(
            "case \"$1\" in extract-rpu)\n  printf '{}' > \"$OUT\"\n  exit 0 ;;\nesac\n{}",
            "\\000\\000\\000\\001\\031\\200".repeat(rpus),
            writes("p81")
        ),
    );
}

/// Leaves a finished MEL conversion and mux of an earlier run, and an `output_rpu.bin` of
/// `stale_rpus` RPUs that must not stand in for the output's count.
#[cfg(unix)]
fn leave_reusable_mux(fake: &FakeTools, stale_rpus: usize) {
    fake.leave(&fake.temp.join("P81_discard.hevc"), b"p81", true);
    fs::write(&fake.output, b"muxed").unwrap();
    fs::write(fake.temp.join("mux.done"), b"").unwrap();
    let stale = b"\0\0\0\x01\x19\x80".repeat(stale_rpus);
    fs::write(fake.temp.join(rpu_count::RPU_FILE), stale).unwrap();
}

/// A reused mux with fewer RPUs than frames fails the run (ROADMAP E7.2): both reads count 2
/// RPUs for 3 frames, the output and its sentinel are removed, the source is kept.
#[cfg(unix)]
#[test]
fn mel_fast_path_counts_a_reused_output_and_removes_it_when_rpus_are_missing() {
    let Some(fake) =
        FakeTools::new("mel_fast_path_counts_a_reused_output_and_removes_it_when_rpus_are_missing")
    else {
        return;
    };
    script_counts(&fake, 3, 2);
    fake.script("mkvmerge", FAILS);
    fake.script("ffmpeg", &writes("video"));
    leave_reusable_mux(&fake, 3);
    assert!(!run_mel_fast_path(&fake, false));
    assert!(fake.calls("mkvmerge").is_empty(), "the mux is reused");
    let counts = fake.calls("dovi_tool");
    assert!(called_with(&counts, "extract-rpu -i ") && called_with(&counts, "in.DV.mkv"));
    assert!(called_with(&counts, "output_video.hevc"));
    assert!(called_with(&fake.calls("ffmpeg"), "output_video.hevc"));
    assert!(!fake.output.exists(), "the output is removed");
    assert!(
        !fake.temp.join("mux.done").exists(),
        "the next run muxes again"
    );
    assert!(fake.temp.exists(), "a failed run keeps the temp dir");
    let converted = fake.temp.join("P81_discard.hevc");
    assert!(
        !converted.exists() && !resume::marker_path(&converted).exists(),
        "the converted stream is withdrawn, so the next run converts again"
    );
    assert!(fake.input.exists());
}

/// More RPUs than video frames fail the run as well: both reads count 4 RPUs for 3 frames.
#[cfg(unix)]
#[test]
fn mel_fast_path_fails_when_the_output_has_more_rpus_than_frames() {
    let Some(fake) =
        FakeTools::new("mel_fast_path_fails_when_the_output_has_more_rpus_than_frames")
    else {
        return;
    };
    script_counts(&fake, 3, 4);
    fake.script("mkvmerge", &writes("muxed"));
    fake.script("ffmpeg", &writes("video"));
    assert!(!run_mel_fast_path(&fake, false));
    assert!(called_with(&fake.calls("ffmpeg"), "output_video.hevc"));
    assert!(!fake.output.exists());
    assert!(!fake.temp.join("mux.done").exists());
    assert!(fake.input.exists());
}

/// RPUs that cannot be counted fail the run too; the stale `output_rpu.bin`, which holds the
/// right count, does not stand in.
#[cfg(unix)]
#[test]
fn mel_fast_path_fails_when_the_output_rpus_cannot_be_counted() {
    let Some(fake) = FakeTools::new("mel_fast_path_fails_when_the_output_rpus_cannot_be_counted")
    else {
        return;
    };
    script_counts(&fake, 3, 3);
    fake.script(
        "dovi_tool",
        &format!(
            "case \"$1\" in extract-rpu) echo 'Error: Invalid input' >&2; exit 1 ;; esac\n{}",
            writes("p81")
        ),
    );
    fake.script("mkvmerge", FAILS);
    fake.script("ffmpeg", &writes("video"));
    leave_reusable_mux(&fake, 3);
    assert!(!run_mel_fast_path(&fake, false));
    assert!(!fake.output.exists());
    assert!(!fake.temp.join("mux.done").exists());
    assert!(fake.input.exists());
}

#[cfg(unix)]
#[test]
fn mel_fast_path_that_fails_to_convert_skips_verification() {
    let Some(fake) = FakeTools::new("mel_fast_path_that_fails_to_convert_skips_verification")
    else {
        return;
    };
    fake.script("dovi_tool", FAILS);
    fake.script("mkvmerge", &writes("muxed"));
    // The failed direct MKV read falls back to an ffmpeg extraction, which works.
    fake.script(
        "ffmpeg",
        &format!("case \"$OUT\" in -) exit 1 ;; esac\n{}", writes("raw")),
    );
    assert!(!run_mel_fast_path(&fake, true));
    assert!(!verify_extraction_ran(&fake.calls("ffmpeg")));
    assert!(fake.temp.exists());
}

#[cfg(unix)]
#[test]
fn base_layer_extraction_writes_the_hlg_transfer_only_for_hlg_lacking_it() {
    // (format, first-frame transfer ffprobe reports) -> hevc_metadata VUI rewrite.
    for (format, transfer, rewrites) in [
        (HdrFormat::Hlg, "bt709", true),
        (HdrFormat::Hlg, "bt2020-10", true),
        (HdrFormat::Hlg, "arib-std-b67", false),
        (HdrFormat::Hdr10Unsupported, "bt709", false),
        (HdrFormat::Hdr10WithMeasurements, "smpte2084", false),
    ] {
        let Some(fake) =
            FakeTools::new("base_layer_extraction_writes_the_hlg_transfer_only_for_hlg_lacking_it")
        else {
            return;
        };
        fake.script("ffprobe", &format!("echo {transfer}\nexit 0\n"));
        fake.script("ffmpeg", &writes("bl"));
        let args = Args::try_parse_from(["mkvdovi"]).unwrap();
        let bl =
            extract_base_layer(fake.input(), &fake.input, &fake.temp, format, true, &args).unwrap();
        let bl_hevc = fake.temp.join("BL.hevc");
        assert_eq!(bl, Some(bl_hevc.clone()));
        assert!(resume::is_complete(&bl_hevc));
        let ffmpeg = fake.calls("ffmpeg");
        assert_eq!(ffmpeg.len(), 1, "{format:?} {transfer}");
        assert_eq!(
            called_with(
                &ffmpeg,
                "hevc_metadata=colour_primaries=9:transfer_characteristics=18:matrix_coefficients=9"
            ),
            rewrites,
            "{format:?} {transfer}: {ffmpeg:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn base_layer_extraction_is_reused_only_when_sealed_and_resuming() {
    for (resume_enabled, sealed, extracts) in [
        (true, true, false),
        (true, false, true),
        (false, true, true),
    ] {
        let Some(fake) =
            FakeTools::new("base_layer_extraction_is_reused_only_when_sealed_and_resuming")
        else {
            return;
        };
        let bl_hevc = fake.temp.join("BL.hevc");
        fake.leave(&bl_hevc, b"previous bl", sealed);
        fake.script("ffmpeg", &writes("new bl"));
        let args = Args::try_parse_from(["mkvdovi"]).unwrap();
        let bl = extract_base_layer(
            fake.input(),
            &fake.input,
            &fake.temp,
            HdrFormat::Hdr10Unsupported,
            resume_enabled,
            &args,
        )
        .unwrap();
        assert_eq!(bl, Some(bl_hevc.clone()));
        let case = format!("resume {resume_enabled}, sealed {sealed}");
        assert_eq!(!fake.calls("ffmpeg").is_empty(), extracts, "{case}");
        let expected: &[u8] = if extracts { b"new bl" } else { b"previous bl" };
        assert_eq!(fs::read(&bl_hevc).unwrap(), expected, "{case}");
    }
}

#[cfg(unix)]
#[test]
fn base_layer_extraction_failure_returns_none_without_a_sentinel() {
    let Some(fake) =
        FakeTools::new("base_layer_extraction_failure_returns_none_without_a_sentinel")
    else {
        return;
    };
    fake.script("ffmpeg", FAILS_AFTER_PARTIAL_OUTPUT);
    let args = Args::try_parse_from(["mkvdovi"]).unwrap();
    let bl = extract_base_layer(
        fake.input(),
        &fake.input,
        &fake.temp,
        HdrFormat::Hdr10Unsupported,
        true,
        &args,
    )
    .unwrap();
    assert_eq!(bl, None);
    assert!(!resume::marker_path(&fake.temp.join("BL.hevc")).exists());
}

#[cfg(unix)]
#[test]
fn mux_output_is_reused_only_with_resume_output_and_marker() {
    for (resume_enabled, output, marker, muxes) in [
        (true, true, true, false),
        (true, true, false, true),
        (true, false, true, true),
        (false, true, true, true),
    ] {
        let Some(fake) = FakeTools::new("mux_output_is_reused_only_with_resume_output_and_marker")
        else {
            return;
        };
        let bl_rpu = fake.temp.join("BL_RPU.hevc");
        fs::write(&bl_rpu, b"bl+rpu").unwrap();
        if output {
            fs::write(&fake.output, b"previous mux").unwrap();
        }
        if marker {
            fs::write(fake.temp.join("mux.done"), b"").unwrap();
        }
        fake.script("mkvmerge", &writes("new mux"));
        let args = Args::try_parse_from(["mkvdovi"]).unwrap();
        let muxed = mux_output(
            fake.input(),
            &bl_rpu,
            &fake.output,
            &fake.temp,
            resume_enabled,
            &args,
            &MuxTiming::Stream { sync_ms: 0 },
        )
        .unwrap();
        let case = format!("resume {resume_enabled}, output {output}, marker {marker}");
        assert!(muxed, "{case}");
        assert_eq!(!fake.calls("mkvmerge").is_empty(), muxes, "{case}");
        let expected: &[u8] = if muxes { b"new mux" } else { b"previous mux" };
        assert_eq!(fs::read(&fake.output).unwrap(), expected, "{case}");
        assert!(fake.temp.join("mux.done").exists(), "{case}");
    }
}

#[cfg(unix)]
#[test]
fn mux_output_failure_returns_false_without_a_mux_marker() {
    let Some(fake) = FakeTools::new("mux_output_failure_returns_false_without_a_mux_marker") else {
        return;
    };
    let bl_rpu = fake.temp.join("BL_RPU.hevc");
    fs::write(&bl_rpu, b"bl+rpu").unwrap();
    fake.script("mkvmerge", FAILS);
    let args = Args::try_parse_from(["mkvdovi"]).unwrap();
    let muxed = mux_output(
        fake.input(),
        &bl_rpu,
        &fake.output,
        &fake.temp,
        true,
        &args,
        &MuxTiming::Stream { sync_ms: 0 },
    )
    .unwrap();
    assert!(!muxed);
    assert!(!fake.temp.join("mux.done").exists());
}

#[cfg(unix)]
#[test]
fn mux_output_passes_the_timing_before_the_video_file() {
    for (timing, expected) in [
        (MuxTiming::Stream { sync_ms: 0 }, None),
        (MuxTiming::Stream { sync_ms: 2000 }, Some("--sync 0:2000")),
        (
            MuxTiming::File(PathBuf::from("/t/video.timestamps.txt")),
            Some("--timestamps 0:/t/video.timestamps.txt"),
        ),
    ] {
        let Some(fake) = FakeTools::new("mux_output_passes_the_timing_before_the_video_file")
        else {
            return;
        };
        let bl_rpu = fake.temp.join("BL_RPU.hevc");
        fs::write(&bl_rpu, b"bl+rpu").unwrap();
        fake.script("mkvmerge", &writes("muxed"));
        let args = Args::try_parse_from(["mkvdovi"]).unwrap();
        let muxed = mux_output(
            fake.input(),
            &bl_rpu,
            &fake.output,
            &fake.temp,
            false,
            &args,
            &timing,
        )
        .unwrap();
        assert!(muxed);
        let calls = fake.calls("mkvmerge");
        let video_then_source = format!("{} --no-video {}", bl_rpu.display(), fake.input());
        let tail = match expected {
            Some(option) => format!("{option} {video_then_source}"),
            None => video_then_source.clone(),
        };
        assert_eq!(calls.len(), 1, "{timing:?}");
        assert!(calls[0].ends_with(&tail), "{timing:?}: {}", calls[0]);
        if expected.is_none() {
            assert!(!calls[0].contains("--sync") && !calls[0].contains("--timestamps"));
        }
    }
}

// P5.1: the `hdr10plus_tool extract --skip-reorder` fallback, against a real HDR10+ stream.

/// The frames of [`seal_hdr10plus_stream`]'s stream, the count given to the encoder.
#[cfg(unix)]
const HDR10PLUS_STREAM_FRAMES: u64 = 4;

/// Runs `command`; panics with its stderr when it fails.
#[cfg(unix)]
fn run_fixture_tool(command: &mut std::process::Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A [`FakeTools`] sandbox whose temp dir holds a sealed `video.hevc` of
/// [`HDR10PLUS_STREAM_FRAMES`] frames with an HDR10+ payload each, encoded by ffmpeg (libx265)
/// and injected by the real `hdr10plus_tool`. `None` (after a skip message) without the tools.
#[cfg(unix)]
fn seal_hdr10plus_stream(test: &str) -> Option<FakeTools> {
    let fake = FakeTools::new(test)?;
    if !shimmed_tools().is_some_and(|tools| tools.contains(&"hdr10plus_tool")) {
        println!("Skipping {test}: hdr10plus_tool not found in PATH");
        return None;
    }
    let encoders = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .unwrap();
    if !String::from_utf8_lossy(&encoders.stdout).contains("libx265") {
        println!("Skipping {test}: ffmpeg has no libx265 encoder");
        return None;
    }
    // Built in the sandbox before any tool is scripted, so the shims run the real tools.
    let plain = fake.dir.path().join("plain.hevc");
    run_fixture_tool(
        std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=128x72:r=24",
                "-frames:v",
                &HDR10PLUS_STREAM_FRAMES.to_string(),
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-x265-params",
                "log-level=error:bframes=0:colorprim=bt2020:transfer=smpte2084:\
                 colormatrix=bt2020nc",
                "-pix_fmt",
                "yuv420p10le",
                "-f",
                "hevc",
                "-y",
            ])
            .arg(&plain),
    );
    let frame = |index: u64| {
        json!({
            "BezierCurveData": {"Anchors": [102, 205, 307, 410, 512, 614, 717, 819, 922],
                                "KneePointX": 0, "KneePointY": 0},
            "LuminanceParameters": {
                "AverageRGB": 300,
                "LuminanceDistributions": {
                    "DistributionIndex": [1, 5, 10, 25, 50, 75, 90, 95, 99],
                    "DistributionValues": [0, 10, 100, 200, 300, 400, 500, 600, 1000],
                },
                "MaxScl": [0, 0, 0],
            },
            "NumberOfWindows": 1,
            "TargetedSystemDisplayMaximumLuminance": 400,
            "SceneFrameIndex": index,
            "SceneId": 0,
            "SequenceFrameIndex": index,
        })
    };
    let metadata = json!({
        "JSONInfo": {"HDR10plusProfile": "B", "Version": "1.0"},
        "SceneInfo": (0..HDR10PLUS_STREAM_FRAMES).map(frame).collect::<Vec<_>>(),
        "SceneInfoSummary": {"SceneFirstFrameIndex": [0],
                             "SceneFrameNumbers": [HDR10PLUS_STREAM_FRAMES]},
        "ToolInfo": {"Tool": "hdr10plus_tool", "Version": "1.7.2"},
    });
    let metadata_path = fake.dir.path().join("hdr10plus.json");
    fs::write(&metadata_path, metadata.to_string()).unwrap();
    let hevc = fake.temp.join(hdr10plus::HDR10PLUS_HEVC);
    run_fixture_tool(
        std::process::Command::new("hdr10plus_tool")
            .arg("inject")
            .arg("-i")
            .arg(&plain)
            .arg("-j")
            .arg(&metadata_path)
            .arg("-o")
            .arg(&hevc),
    );
    resume::mark_done(&hevc).unwrap();
    Some(fake)
}

/// `extract_hdr10plus_metadata` on the sealed stream of `fake`, resuming so that it extracts
/// from that stream.
#[cfg(unix)]
fn extract_sealed_hdr10plus(fake: &FakeTools) -> Option<PathBuf> {
    hdr10plus::extract_hdr10plus_metadata(fake.input(), &fake.temp, true, 0).unwrap()
}

#[cfg(unix)]
fn is_skip_reorder(call: &str) -> bool {
    call.split(' ').any(|arg| arg == "--skip-reorder")
}

/// hdr10plus_tool whose normal extraction fails and whose `--skip-reorder` run is the real tool.
#[cfg(unix)]
const REORDER_FAILS: &str = r#"case " $* " in *" --skip-reorder "*) exec "$REAL" "$@" ;; esac
echo 'scripted: reordering failed' >&2
exit 1
"#;

#[cfg(unix)]
#[test]
fn a_failed_hdr10plus_extract_retries_with_skip_reorder_and_takes_its_json() {
    let Some(fake) = seal_hdr10plus_stream(
        "a_failed_hdr10plus_extract_retries_with_skip_reorder_and_takes_its_json",
    ) else {
        return;
    };
    fake.script("hdr10plus_tool", REORDER_FAILS);
    let json_path = extract_sealed_hdr10plus(&fake).expect("the fallback extracts");

    assert_eq!(json_path, fake.temp.join("hdr10plus_metadata.json"));
    assert!(resume::is_complete(&json_path), "sealed for resume");
    let calls = fake.calls("hdr10plus_tool");
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(!is_skip_reorder(&calls[0]), "{}", calls[0]);
    assert!(is_skip_reorder(&calls[1]), "{}", calls[1]);
    // The frame count of the fallback's JSON is the video's: the encoder's count, which the
    // payload scan of the same stream confirms.
    assert_eq!(
        hdr10plus_coverage::scene_info_frames(&json_path),
        Some(HDR10PLUS_STREAM_FRAMES)
    );
    let counted = hdr10plus_coverage::count_sealed(&fake.temp, false).unwrap();
    assert_eq!(counted.access_units, HDR10PLUS_STREAM_FRAMES);
    // The first log keeps the failure of the normal extraction.
    let log = fs::read_to_string(fake.temp.join(hdr10plus::HDR10PLUS_TOOL_LOG)).unwrap();
    assert!(log.contains("scripted: reordering failed"), "{log}");
}

#[cfg(unix)]
#[test]
fn a_resumed_run_keeps_the_skip_reorder_marker_of_the_json_it_reuses() {
    let Some(fake) =
        seal_hdr10plus_stream("a_resumed_run_keeps_the_skip_reorder_marker_of_the_json_it_reuses")
    else {
        return;
    };
    fake.script("hdr10plus_tool", REORDER_FAILS);
    let marker = fake.temp.join(hdr10plus::HDR10PLUS_SKIP_REORDER_MARKER);
    extract_sealed_hdr10plus(&fake).expect("the fallback extracts");
    assert!(marker.exists(), "the fallback marks its JSON");

    // The resumed run reuses the sealed JSON without running the tool, and the marker that
    // makes it warn again stays.
    let reused = extract_sealed_hdr10plus(&fake).expect("the sealed JSON is reused");
    assert_eq!(reused, fake.temp.join("hdr10plus_metadata.json"));
    assert_eq!(fake.calls("hdr10plus_tool").len(), 2);
    assert!(
        marker.exists(),
        "the reused JSON still came from --skip-reorder"
    );
}

#[cfg(unix)]
#[test]
fn a_normal_extract_after_a_fallback_removes_the_skip_reorder_marker() {
    let Some(fake) =
        seal_hdr10plus_stream("a_normal_extract_after_a_fallback_removes_the_skip_reorder_marker")
    else {
        return;
    };
    fake.script("hdr10plus_tool", REORDER_FAILS);
    let json_path = extract_sealed_hdr10plus(&fake).expect("the fallback extracts");
    let marker = fake.temp.join(hdr10plus::HDR10PLUS_SKIP_REORDER_MARKER);
    assert!(marker.exists(), "the fallback marks its JSON");

    // A later run re-extracts (the JSON is no longer sealed) and the normal extraction works.
    resume::invalidate(&json_path).unwrap();
    fake.script("hdr10plus_tool", "exec \"$REAL\" \"$@\"\n");
    extract_sealed_hdr10plus(&fake).expect("the normal extraction works");
    let calls = fake.calls("hdr10plus_tool");
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert!(!is_skip_reorder(&calls[2]), "{}", calls[2]);
    assert!(
        !marker.exists(),
        "a normal JSON carries no --skip-reorder marker"
    );
}

#[cfg(unix)]
#[test]
fn a_working_hdr10plus_extract_runs_once_without_skip_reorder() {
    let Some(fake) =
        seal_hdr10plus_stream("a_working_hdr10plus_extract_runs_once_without_skip_reorder")
    else {
        return;
    };
    fake.script("hdr10plus_tool", "exec \"$REAL\" \"$@\"\n");
    let json_path = extract_sealed_hdr10plus(&fake).expect("the normal extraction works");

    assert!(resume::is_complete(&json_path), "sealed for resume");
    let calls = fake.calls("hdr10plus_tool");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(!is_skip_reorder(&calls[0]), "{}", calls[0]);
    assert_eq!(
        hdr10plus_coverage::scene_info_frames(&json_path),
        Some(HDR10PLUS_STREAM_FRAMES)
    );
    assert!(!fake
        .temp
        .join(hdr10plus::HDR10PLUS_SKIP_REORDER_LOG)
        .exists());
}

#[cfg(unix)]
#[test]
fn an_hdr10plus_extract_whose_fallback_fails_too_extracts_nothing() {
    let Some(fake) =
        seal_hdr10plus_stream("an_hdr10plus_extract_whose_fallback_fails_too_extracts_nothing")
    else {
        return;
    };
    // Each run leaves a partial JSON: neither may pass for an extraction.
    fake.script(
        "hdr10plus_tool",
        r#"printf '{"partial":' > "$OUT"
case " $* " in *" --skip-reorder "*) echo 'scripted: fallback failed' >&2; exit 1 ;; esac
echo 'scripted: normal failed' >&2
exit 1
"#,
    );
    assert_eq!(extract_sealed_hdr10plus(&fake), None);

    let calls = fake.calls("hdr10plus_tool");
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(is_skip_reorder(&calls[1]), "{}", calls[1]);
    assert!(!resume::is_complete(
        &fake.temp.join("hdr10plus_metadata.json")
    ));
    let read = |name| fs::read_to_string(fake.temp.join(name)).unwrap();
    let normal = read(hdr10plus::HDR10PLUS_TOOL_LOG);
    assert!(normal.contains("scripted: normal failed"), "{normal}");
    let fallback = read(hdr10plus::HDR10PLUS_SKIP_REORDER_LOG);
    assert!(fallback.contains("scripted: fallback failed"), "{fallback}");
}

#[cfg(unix)]
#[test]
fn a_partial_json_of_the_failed_extract_does_not_pass_for_the_fallbacks() {
    let Some(fake) = seal_hdr10plus_stream(
        "a_partial_json_of_the_failed_extract_does_not_pass_for_the_fallbacks",
    ) else {
        return;
    };
    // The normal run leaves a partial JSON and fails; the fallback exits 0 and writes nothing.
    fake.script(
        "hdr10plus_tool",
        r#"case " $* " in *" --skip-reorder "*) exit 0 ;; esac
printf '{"partial":' > "$OUT"
exit 1
"#,
    );
    assert_eq!(extract_sealed_hdr10plus(&fake), None);
    assert_eq!(fake.calls("hdr10plus_tool").len(), 2);
    assert!(!fake.temp.join("hdr10plus_metadata.json").exists());
}

/// `acquire_measurements` in `mode` on the sealed stream of `fake`, with measurements next to
/// the input that a hybrid reuses (the input gets the size their sidecar records).
#[cfg(unix)]
fn acquire_from_sealed_stream(fake: &FakeTools, mode: Hdr10PlusMode) -> Result<Option<Measured>> {
    fs::write(&fake.input, b"src").unwrap();
    measurements_for_four_frames(fake.dir.path());
    let mut argv = vec!["mkvdovi"];
    argv.extend_from_slice(&ACCURATE);
    let args = Args::try_parse_from(argv).unwrap();
    let (mut total, mut current) = (total_steps_for(HdrFormat::Hdr10Plus), 1);
    acquire_measurements(
        fake.input(),
        &fake.temp,
        &args,
        true,
        None,
        HdrFormat::Hdr10Plus,
        mode,
        None,
        None,
        &mut total,
        &mut current,
    )
}

#[cfg(unix)]
#[test]
fn an_explicit_hybrid_refuses_the_json_of_the_skip_reorder_fallback() {
    let Some(fake) =
        seal_hdr10plus_stream("an_explicit_hybrid_refuses_the_json_of_the_skip_reorder_fallback")
    else {
        return;
    };
    fake.script("hdr10plus_tool", REORDER_FAILS);
    let refused = acquire_from_sealed_stream(&fake, Hdr10PlusMode::ExplicitHybrid);
    assert!(matches!(refused, Ok(None)), "an explicit hybrid stops");

    let calls = fake.calls("hdr10plus_tool");
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(is_skip_reorder(&calls[1]), "{}", calls[1]);
    assert!(fake
        .temp
        .join(hdr10plus::HDR10PLUS_SKIP_REORDER_MARKER)
        .exists());
    // Refused before the payload count, the first check after the extraction.
    assert!(!fake.temp.join(hdr10plus_coverage::COVERAGE_FILE).exists());
}

#[cfg(unix)]
#[test]
fn the_default_and_the_metadata_mode_take_the_json_of_the_skip_reorder_fallback() {
    for (mode, hdr_type) in [
        (
            Hdr10PlusMode::ImplicitHybrid,
            HdrFormat::Hdr10WithMeasurements,
        ),
        (Hdr10PlusMode::Metadata, HdrFormat::Hdr10Plus),
    ] {
        let Some(fake) = seal_hdr10plus_stream(
            "the_default_and_the_metadata_mode_take_the_json_of_the_skip_reorder_fallback",
        ) else {
            return;
        };
        fake.script("hdr10plus_tool", REORDER_FAILS);
        let measured = acquire_from_sealed_stream(&fake, mode)
            .unwrap()
            .unwrap_or_else(|| panic!("{mode:?} converts with the fallback's JSON"));

        assert_eq!(fake.calls("hdr10plus_tool").len(), 2, "{mode:?}");
        assert_eq!(measured.hdr10plus_mode, mode);
        assert_eq!(measured.hdr_type, hdr_type, "{mode:?}");
        assert_eq!(
            measured.hdr10plus_json,
            Some(fake.temp.join("hdr10plus_metadata.json")),
            "{mode:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn an_explicit_hybrid_takes_the_json_of_a_normal_extraction() {
    let Some(fake) =
        seal_hdr10plus_stream("an_explicit_hybrid_takes_the_json_of_a_normal_extraction")
    else {
        return;
    };
    fake.script("hdr10plus_tool", "exec \"$REAL\" \"$@\"\n");
    let measured = acquire_from_sealed_stream(&fake, Hdr10PlusMode::ExplicitHybrid)
        .unwrap()
        .expect("a normal extraction takes the hybrid");

    let calls = fake.calls("hdr10plus_tool");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(!is_skip_reorder(&calls[0]), "{}", calls[0]);
    assert_eq!(measured.hdr10plus_mode, Hdr10PlusMode::ExplicitHybrid);
    assert_eq!(measured.hdr_type, HdrFormat::Hdr10WithMeasurements);
    assert!(fake.temp.join(hdr10plus_coverage::COVERAGE_FILE).exists());
}

/// The base layer and RPU injection steps (ROADMAP E20.1).
mod injection;
