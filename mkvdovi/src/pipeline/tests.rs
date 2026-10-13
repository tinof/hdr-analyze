use super::analyzer::{
    analysis_quality_notice, analysis_sampling, coarser_sampling, gpu_analysis_missing,
    rejects_coarser_sidecar,
};
use super::dovi_steps::{feed_mkv_to_dovi_tool, resolve_dovi_input};
use super::hdr10plus::{hdr10plus_peak_nits, hdr10plus_scene_peak_stats, Hdr10PlusPeakStats};
use super::measure::{
    abort_unsupported, acquire_measurements, find_reusable_measurements, total_steps_for,
};
use super::temp_dir::{locate_temp_dir, prepare_temp_dir, settle_leftover_temp_dir};
use super::*;
use crate::cli::{AnalysisQuality, DoviInput, HwAccel, PeakSource};
use clap::Parser;
use serde_json::json;

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
fn resume_settings_carry_only_a_non_preset_hlg_composer() {
    let preset = Args::try_parse_from(["mkvdovi", "--hlg-composer", "preset"]).unwrap();
    let bt2100 = Args::try_parse_from(["mkvdovi"]).unwrap();

    // Preset fingerprints stay what 0.5.1 wrote (its RPUs carry the preset), so those temp
    // dirs still resume under --hlg-composer preset and are discarded under bt2100.
    let default = resume_settings(&preset, None);
    assert_eq!(
            default,
            "hwaccel=Auto analysis_quality=Auto optimizer=Conservative boost=false boost_experimental=false cm=V40 content_type=Movies reference_mode=false source_primaries=None trim_targets=100,600,1000 peak_source=Histogram mdfix=false legacy_madvr_l1=false"
        );
    assert_eq!(resume_settings(&preset, Some(Composer::Preset)), default);
    // The composer only matters for HLG inputs.
    assert_eq!(resume_settings(&bt2100, None), default);
    // bt2100 differs from the preset both ways, so neither reuses the other's RPU.bin.
    assert_eq!(
        resume_settings(&bt2100, Some(Composer::Bt2100V1)),
        format!("{default} hlg_composer=dovi84-bt2100-v1-spec420")
    );
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
        &mut measurements_file,
        &mut l1_sidecar,
    ));
    assert!(measurements_file.is_none());
    assert!(l1_sidecar.is_none());
}
