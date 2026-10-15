use super::analyzer::{
    analysis_quality_notice, analysis_sampling, coarser_sampling, gpu_analysis_missing,
    rejects_coarser_sidecar,
};
use super::config::withdraw_stale_rpu;
use super::deliver::{extract_base_layer, inject_rpu, mux_output};
use super::dovi_steps::{
    convert_mel_fast_path, convert_mel_to_profile81, extract_clean_base_layer,
    feed_mkv_to_dovi_tool, resolve_dovi_input,
};
use super::hdr10plus::{hdr10plus_peak_nits, hdr10plus_scene_peak_stats, Hdr10PlusPeakStats};
use super::measure::{
    abort_unsupported, acquire_measurements, find_reusable_measurements, total_steps_for, Measured,
};
use super::temp_dir::{locate_temp_dir, prepare_temp_dir, settle_leftover_temp_dir};
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
    assert_eq!(analysis_sampling(AnalysisQuality::Auto), (2, 1));
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
    assert!(analysis_quality_notice(AnalysisQuality::Auto, false)
        .unwrap()
        .contains("MaxCLL"));
}

#[test]
fn profile7_fel_input_is_refused() {
    let error = reject_unsupported_input(HdrFormat::DolbyVisionFel).unwrap_err();
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
        assert!(reject_unsupported_input(supported).is_ok());
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
    // `convert_file` passes true only for an HDR10+ input under the flag.
    assert_eq!(resume_settings(&hybrid, None, false), default);
    assert_eq!(
        resume_settings(&hybrid, None, true),
        format!("{default} hdr10plus_l1=hybrid")
    );
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
    );
    (source.exists(), temp.exists(), output.exists())
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
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.mkv");
    fs::write(&input, b"src").unwrap();
    setup(dir.path());
    let temp = dir.path().join("mkvdovi_temp_in");
    fs::create_dir(&temp).unwrap();
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
    let sidecar = json!({
        "version": 5,
        "source": {"file_name": "in.mkv", "size_bytes": 3, "width": 1920, "height": 1080,
                   "transfer_function": "PQ (SMPTE 2084)", "stream_frames": 4,
                   "leading_skipped_frames": 0},
        "analysis": {"downscale": 2, "sample_rate": 3, "gpu": false, "no_crop": false,
                     "luminance_mapping": "pq"},
        "scenes": [{"start": 0, "end": 3, "min_pq_12bit": 1, "avg_luma_pq_12bit": 500,
                    "avg_max_rgb_pq_12bit": 520, "max_pq_12bit": 2400}],
        "frames": {"min_pq_12bit": [1, 1, 1, 1], "avg_max_rgb_pq_12bit": [520, 520, 520, 520]}
    });
    fs::write(
        metadata::l1_sidecar_path(&measurements),
        serde_json::to_vec(&sidecar).unwrap(),
    )
    .unwrap();
    let args = Args::try_parse_from(["mkvdovi", "--analysis-quality", "balanced"]).unwrap();
    let reuse = |hybrid| {
        let (mut measurements_file, mut l1_sidecar) = (None, None);
        let reused = find_reusable_measurements(
            &input,
            input.to_str().unwrap(),
            &args,
            None,
            hybrid,
            &mut measurements_file,
            &mut l1_sidecar,
        );
        (reused, measurements_file.is_some(), l1_sidecar.is_some())
    };
    // Plain HDR10 reuses it under balanced (with a warning); the hybrid needs every frame.
    assert_eq!(reuse(false), (true, true, true));
    assert_eq!(reuse(true), (false, false, false));
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
    )
    .unwrap();
    assert!(muxed);
    assert_eq!(fs::read(&output).unwrap(), b"out");
}

// Delivery steps against fake external tools. The functions under test spawn `ffmpeg`,
// `ffprobe`, `mkvmerge` and `dovi_tool` from PATH, and PATH is shared by every test in this
// binary, so the shims are installed once and pass every call through to the real tool unless
// one of its arguments lies in a `FakeTools` sandbox that scripts that tool.

#[cfg(unix)]
const SHIMMED_TOOLS: [&str; 4] = ["ffmpeg", "ffprobe", "mkvmerge", "dovi_tool"];

#[cfg(unix)]
const SANDBOX_PREFIX: &str = "mkvdovi-fake-";

/// Puts the pass-through shims first on PATH, once per process. False when a real tool is
/// missing: a shim would then hide that from the tests that skip without it.
#[cfg(unix)]
fn shims_installed() -> bool {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::OnceLock;

    static INSTALLED: OnceLock<bool> = OnceLock::new();
    *INSTALLED.get_or_init(|| {
        let mut real = Vec::new();
        for tool in SHIMMED_TOOLS {
            match crate::external::find_tool(tool) {
                Some(path) => real.push((tool, path)),
                None => return false,
            }
        }
        let bin = tempfile::Builder::new()
            .prefix("mkvdovi-shims-")
            .tempdir()
            .unwrap()
            .keep();
        for (tool, real_path) in real {
            // A sandbox call appends its argv to `<root>/fake/<tool>.calls` and runs
            // `<root>/fake/<tool>.sh` with IN (after -i), OUT (after -o, else the last
            // argument) and LAST set. That script must exit itself.
            let script = format!(
                r#"#!/bin/sh
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
  . "$root/fake/{tool}.sh"
  exit 99
fi
exec '{}' "$@"
"#,
                real_path.display()
            );
            let shim = bin.join(tool);
            fs::write(&shim, script).unwrap();
            fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![bin];
        dirs.extend(std::env::split_paths(&path));
        std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());
        true
    })
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
}

#[cfg(unix)]
fn called_with(calls: &[String], needle: &str) -> bool {
    calls.iter().any(|call| call.contains(needle))
}

#[cfg(unix)]
#[test]
fn direct_mkv_step_that_succeeds_needs_no_ffmpeg_extraction() {
    let Some(fake) = FakeTools::new("direct_mkv_step_that_succeeds_needs_no_ffmpeg_extraction")
    else {
        return;
    };
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
    fake.script(
        "dovi_tool",
        &dovi_tool_by_input(FAILS_AFTER_PARTIAL_OUTPUT, &writes("clean from raw")),
    );
    fake.script("ffmpeg", &writes("raw"));
    let clean_bl = run_clean_base_layer(&fake, DoviInput::Mkv, true).unwrap();
    assert_eq!(fake.calls("ffmpeg").len(), 1);
    assert_eq!(fs::read(&clean_bl).unwrap(), b"clean from raw");
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
    fake.script("dovi_tool", &writes("p81"));
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
    fake.script("dovi_tool", &writes("p81"));
    fake.script("mkvmerge", &writes("muxed"));
    fake.script("ffmpeg", FAILS);
    assert!(!run_mel_fast_path(&fake, true));
    assert!(called_with(&fake.calls("ffmpeg"), "verify_video.hevc"));
    assert!(fake.temp.exists());
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
        &format!(
            "case \"$OUT\" in *verify_video.hevc) exit 1 ;; esac\n{}",
            writes("raw")
        ),
    );
    assert!(!run_mel_fast_path(&fake, true));
    assert!(!called_with(&fake.calls("ffmpeg"), "verify_video.hevc"));
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
    let muxed = mux_output(fake.input(), &bl_rpu, &fake.output, &fake.temp, true, &args).unwrap();
    assert!(!muxed);
    assert!(!fake.temp.join("mux.done").exists());
}
