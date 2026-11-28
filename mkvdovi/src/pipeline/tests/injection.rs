//! The base layer and RPU injection steps (`deliver::inject_base_layer`, ROADMAP E20.1) against
//! the fake external tools of `pipeline/tests.rs`. A fake ffmpeg pipes the committed fixture
//! clip (`mkvdovi/tests/fixtures/inject/`), and the expected output is dovi_tool's injection of
//! it, committed beside it.

use super::*;
use crate::cli::DoviInject;
use crate::pipeline::deliver::inject_base_layer;
use crate::pipeline::measure::Hdr10PlusMode as Mode;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/inject")
        .join(name)
}

/// What the measurement phase hands over: the base layer read from `bl_source_file`, with an
/// RPU built from measurements when `measured`.
fn measured(bl_source_file: PathBuf, hdr_type: HdrFormat, measured: bool) -> Measured {
    Measured {
        hdr_type,
        measurements_file: measured.then(|| PathBuf::from("in_measurements.bin")),
        hdr10plus_json: None,
        bl_source_file,
        l1_sidecar: None,
        hdr10plus_mode: Mode::Metadata,
    }
}

/// An ffmpeg that writes the fixture clip to its stdout for a pipe (`-` as the output) and fails
/// for any other call.
#[cfg(unix)]
fn pipes_the_clip() -> String {
    format!(
        "case \"$OUT\" in -) exec cat '{}' ;; esac\nexit 1\n",
        fixture("clip.hevc").display()
    )
}

/// The fixture `RPU.bin` with its first `keep` RPUs, at `path`.
fn write_first_rpus(path: &Path, keep: usize) {
    let bytes = fs::read(fixture("RPU.bin")).unwrap();
    let starts: Vec<usize> = (0..bytes.len() - 3)
        .filter(|&at| bytes[at..].starts_with(&[0, 0, 0, 1]))
        .collect();
    fs::write(path, &bytes[..starts[keep]]).unwrap();
}

/// `inject_base_layer` on `fake` with `rpu`; returns the result and how many steps it took.
#[cfg(unix)]
fn run_injection(
    fake: &FakeTools,
    measured: &Measured,
    rpu: &Path,
    resume: bool,
    args: &Args,
) -> (Option<PathBuf>, u8) {
    let mut step = 3;
    let injected = inject_base_layer(
        fake.input(),
        measured,
        rpu,
        &fake.temp,
        resume,
        args,
        (&mut step, 6),
    )
    .unwrap();
    (injected, step - 3)
}

#[cfg(unix)]
fn default_args() -> Args {
    Args::try_parse_from(["mkvdovi"]).unwrap()
}

/// The fixture `RPU.bin` copied into the temp dir, where `generate_rpu` leaves it.
#[cfg(unix)]
fn rpu_in(fake: &FakeTools) -> PathBuf {
    let rpu = fake.temp.join("RPU.bin");
    fs::copy(fixture("RPU.bin"), &rpu).unwrap();
    rpu
}

#[cfg(unix)]
fn assert_no_output(fake: &FakeTools) {
    let bl_rpu_hevc = fake.temp.join("BL_RPU.hevc");
    assert!(!bl_rpu_hevc.exists(), "no BL_RPU.hevc is left");
    assert!(!resume::marker_path(&bl_rpu_hevc).exists(), "no sentinel");
}

#[cfg(unix)]
#[test]
fn the_in_process_injection_reads_ffmpegs_pipe_twice_and_writes_dovi_tools_bytes() {
    let Some(fake) = FakeTools::new(
        "the_in_process_injection_reads_ffmpegs_pipe_twice_and_writes_dovi_tools_bytes",
    ) else {
        return;
    };
    fake.script("ffmpeg", &pipes_the_clip());
    fake.script("dovi_tool", FAILS);
    let rpu = rpu_in(&fake);
    let measured = measured(fake.input.clone(), HdrFormat::Hdr10WithMeasurements, true);
    let (injected, steps) = run_injection(&fake, &measured, &rpu, true, &default_args());

    let bl_rpu_hevc = fake.temp.join("BL_RPU.hevc");
    assert_eq!(injected, Some(bl_rpu_hevc.clone()));
    assert_eq!(steps, 2, "two step headers, as the external path prints");
    assert!(fs::read(&bl_rpu_hevc).unwrap() == fs::read(fixture("BL_RPU.hevc")).unwrap());
    assert!(resume::is_complete(&bl_rpu_hevc));
    assert!(!fake.temp.join("BL.hevc").exists(), "no full-size BL.hevc");
    let ffmpeg = fake.calls("ffmpeg");
    assert_eq!(ffmpeg.len(), 2, "{ffmpeg:?}");
    assert_eq!(ffmpeg[0], ffmpeg[1], "both passes read the same stream");
    assert!(ffmpeg[0].ends_with("-f hevc -"), "{ffmpeg:?}");
    assert!(called_with(&ffmpeg, &format!("-i {}", fake.input())));
    assert!(!called_with(&ffmpeg, "hevc_metadata"));
    assert!(
        fake.calls("dovi_tool").is_empty(),
        "dovi_tool inject-rpu never runs"
    );
    assert!(fake.input.exists(), "the source is kept");
}

#[cfg(unix)]
#[test]
fn the_hlg_vui_edit_is_on_both_passes_after_one_probe() {
    let Some(fake) = FakeTools::new("the_hlg_vui_edit_is_on_both_passes_after_one_probe") else {
        return;
    };
    fake.script("ffprobe", "echo bt709\nexit 0\n");
    fake.script("ffmpeg", &pipes_the_clip());
    let rpu = rpu_in(&fake);
    let measured = measured(fake.input.clone(), HdrFormat::Hlg, true);
    let (injected, _) = run_injection(&fake, &measured, &rpu, true, &default_args());

    assert!(injected.is_some());
    let ffmpeg = fake.calls("ffmpeg");
    assert_eq!(ffmpeg.len(), 2, "{ffmpeg:?}");
    let edit = "hevc_metadata=colour_primaries=9:transfer_characteristics=18:matrix_coefficients=9";
    assert!(ffmpeg.iter().all(|call| call.contains(edit)), "{ffmpeg:?}");
    assert_eq!(fake.calls("ffprobe").len(), 1, "the input is probed once");
}

#[cfg(unix)]
#[test]
fn a_failed_second_ffmpeg_leaves_no_output_and_no_sentinel() {
    let Some(fake) = FakeTools::new("a_failed_second_ffmpeg_leaves_no_output_and_no_sentinel")
    else {
        return;
    };
    // The whole stream both times, but the second run exits 1 after writing it.
    fake.script(
        "ffmpeg",
        &format!(
            "if [ -e \"$root/fake/ran\" ]; then cat '{clip}'; exit 1; fi\n: > \"$root/fake/ran\"\nexec cat '{clip}'\n",
            clip = fixture("clip.hevc").display()
        ),
    );
    let rpu = rpu_in(&fake);
    let measured = measured(fake.input.clone(), HdrFormat::Hdr10WithMeasurements, true);
    let (injected, _) = run_injection(&fake, &measured, &rpu, true, &default_args());

    assert_eq!(injected, None);
    assert_eq!(fake.calls("ffmpeg").len(), 2);
    assert_no_output(&fake);
    assert!(
        fs::read_to_string(fake.temp.join("ffmpeg_inject_pass2.log"))
            .unwrap()
            .starts_with("Running command:")
    );
}

#[cfg(unix)]
#[test]
fn a_second_pass_that_reads_other_bytes_fails_the_injection() {
    let Some(fake) = FakeTools::new("a_second_pass_that_reads_other_bytes_fails_the_injection")
    else {
        return;
    };
    // The second run writes the clip with one byte of its last SEI payload changed: the parser
    // never reads it, so only the comparison of the two passes can see it.
    let mut changed = fs::read(fixture("clip.hevc")).unwrap();
    let at = changed.len() - 3;
    changed[at] ^= 0x80;
    let other = fake.temp.parent().unwrap().join("other.hevc");
    fs::write(&other, changed).unwrap();
    fake.script(
        "ffmpeg",
        &format!(
            "if [ -e \"$root/fake/ran\" ]; then exec cat '{}'; fi\n: > \"$root/fake/ran\"\nexec cat '{}'\n",
            other.display(),
            fixture("clip.hevc").display()
        ),
    );
    let rpu = rpu_in(&fake);
    let measured = measured(fake.input.clone(), HdrFormat::Hdr10WithMeasurements, true);
    let (injected, _) = run_injection(&fake, &measured, &rpu, true, &default_args());

    assert_eq!(injected, None);
    assert_eq!(fake.calls("ffmpeg").len(), 2);
    assert_no_output(&fake);
}

#[cfg(unix)]
#[test]
fn a_sealed_extracted_base_layer_is_read_without_ffmpeg_when_resuming() {
    let Some(fake) =
        FakeTools::new("a_sealed_extracted_base_layer_is_read_without_ffmpeg_when_resuming")
    else {
        return;
    };
    fake.script("ffmpeg", FAILS);
    let bl_hevc = fake.temp.join("BL.hevc");
    fake.leave(&bl_hevc, &fs::read(fixture("clip.hevc")).unwrap(), true);
    let rpu = rpu_in(&fake);
    let measured = measured(fake.input.clone(), HdrFormat::Hdr10WithMeasurements, true);
    let (injected, _) = run_injection(&fake, &measured, &rpu, true, &default_args());

    let bl_rpu_hevc = injected.expect("injected from BL.hevc");
    assert!(fs::read(&bl_rpu_hevc).unwrap() == fs::read(fixture("BL_RPU.hevc")).unwrap());
    assert!(fake.calls("ffmpeg").is_empty());
    assert!(
        !bl_hevc.exists(),
        "the superseded temp-dir input is removed"
    );
    assert_not_sealed_when_rewritten(&bl_hevc);
}

/// A removed stream leaves no sentinel: the start of a later extraction that was interrupted
/// must not count as a finished file.
fn assert_not_sealed_when_rewritten(removed: &Path) {
    assert!(!removed.exists(), "{} was removed", removed.display());
    fs::write(removed, b"the start of an interrupted extraction").unwrap();
    assert!(
        !resume::is_complete(removed),
        "{} is sealed by the sentinel of the stream removed before it",
        removed.display()
    );
}

#[cfg(unix)]
#[test]
fn a_raw_hevc_base_layer_is_read_directly_and_removed_only_inside_the_temp_dir() {
    for inside in [true, false] {
        let Some(fake) = FakeTools::new(
            "a_raw_hevc_base_layer_is_read_directly_and_removed_only_inside_the_temp_dir",
        ) else {
            return;
        };
        fake.script("ffmpeg", FAILS);
        let dir = if inside {
            fake.temp.clone()
        } else {
            fake.temp.parent().unwrap().to_path_buf()
        };
        let clean_bl = dir.join("BL_clean.hevc");
        fs::copy(fixture("clip.hevc"), &clean_bl).unwrap();
        // Sealed, as `extract_clean_base_layer` leaves it.
        resume::mark_done(&clean_bl).unwrap();
        let rpu = rpu_in(&fake);
        let measured = measured(clean_bl.clone(), HdrFormat::Hdr10WithMeasurements, true);
        let (injected, _) = run_injection(&fake, &measured, &rpu, true, &default_args());

        let bl_rpu_hevc = injected.expect("injected from the raw file");
        assert!(fs::read(&bl_rpu_hevc).unwrap() == fs::read(fixture("BL_RPU.hevc")).unwrap());
        assert!(fake.calls("ffmpeg").is_empty());
        assert_eq!(clean_bl.exists(), !inside, "inside the temp dir: {inside}");
        if inside {
            assert_not_sealed_when_rewritten(&clean_bl);
        } else {
            assert!(resume::is_complete(&clean_bl), "a kept input stays sealed");
        }
    }
}

#[cfg(unix)]
#[test]
fn a_measured_rpu_of_another_length_is_refused_and_a_metadata_rpu_is_injected() {
    for measured_rpu in [true, false] {
        let Some(fake) = FakeTools::new(
            "a_measured_rpu_of_another_length_is_refused_and_a_metadata_rpu_is_injected",
        ) else {
            return;
        };
        fake.script("ffmpeg", &pipes_the_clip());
        let rpu = fake.temp.join("RPU.bin");
        write_first_rpus(&rpu, 30);
        let measured = measured(
            fake.input.clone(),
            HdrFormat::Hdr10WithMeasurements,
            measured_rpu,
        );
        let (injected, steps) = run_injection(&fake, &measured, &rpu, true, &default_args());

        if measured_rpu {
            assert_eq!(injected, None);
            assert_eq!(steps, 1, "refused after the first pass");
            assert_eq!(fake.calls("ffmpeg").len(), 1);
            assert_no_output(&fake);
        } else {
            let bl_rpu_hevc = injected.expect("a metadata RPU is injected");
            assert!(resume::is_complete(&bl_rpu_hevc));
            assert_eq!(fake.calls("ffmpeg").len(), 2);
        }
    }
}

#[cfg(unix)]
#[test]
fn an_rpu_file_the_in_process_path_cannot_read_falls_back_to_dovi_tool() {
    let Some(fake) =
        FakeTools::new("an_rpu_file_the_in_process_path_cannot_read_falls_back_to_dovi_tool")
    else {
        return;
    };
    fake.script("ffmpeg", &writes("bl"));
    fake.script("dovi_tool", &writes("injected"));
    // The abort guard refuses this file (`rpu_guard`), so only dovi_tool can inject it.
    let rpu = fake.temp.join("RPU.bin");
    fs::copy(fixture("../linear_interp_RPU.bin"), &rpu).unwrap();
    let measured = measured(fake.input.clone(), HdrFormat::Hdr10WithMeasurements, true);
    let (injected, steps) = run_injection(&fake, &measured, &rpu, true, &default_args());

    let bl_rpu_hevc = fake.temp.join("BL_RPU.hevc");
    assert_eq!(injected, Some(bl_rpu_hevc.clone()));
    assert_eq!(steps, 2);
    assert_eq!(fs::read(&bl_rpu_hevc).unwrap(), b"injected");
    let ffmpeg = fake.calls("ffmpeg");
    assert_eq!(ffmpeg.len(), 1, "{ffmpeg:?}");
    assert!(ffmpeg[0].ends_with(&format!(
        "-f hevc -y {}",
        fake.temp.join("BL.hevc").display()
    )));
    assert!(called_with(&fake.calls("dovi_tool"), "inject-rpu"));
}

#[cfg(unix)]
#[test]
fn external_mode_extracts_bl_hevc_and_runs_dovi_tool_inject_rpu() {
    let Some(fake) = FakeTools::new("external_mode_extracts_bl_hevc_and_runs_dovi_tool_inject_rpu")
    else {
        return;
    };
    fake.script("ffmpeg", &writes("bl"));
    fake.script("dovi_tool", &writes("injected"));
    let rpu = rpu_in(&fake);
    let mut args = default_args();
    args.dovi_inject = DoviInject::External;
    let measured = measured(fake.input.clone(), HdrFormat::Hdr10WithMeasurements, true);
    let (injected, steps) = run_injection(&fake, &measured, &rpu, true, &args);

    assert_eq!(injected, Some(fake.temp.join("BL_RPU.hevc")));
    assert_eq!(steps, 2);
    let ffmpeg = fake.calls("ffmpeg");
    assert_eq!(ffmpeg.len(), 1, "{ffmpeg:?}");
    assert!(called_with(&ffmpeg, "-stats"));
    let inject_call = format!(
        "inject-rpu -i {} --rpu-in {}",
        fake.temp.join("BL.hevc").display(),
        rpu.display()
    );
    assert!(called_with(&fake.calls("dovi_tool"), &inject_call));
    // dovi_tool's injection supersedes the sealed BL.hevc it read.
    assert_not_sealed_when_rewritten(&fake.temp.join("BL.hevc"));
}

#[cfg(unix)]
#[test]
fn a_sealed_injection_is_reused_without_reading_the_rpu_or_the_base_layer() {
    let Some(fake) =
        FakeTools::new("a_sealed_injection_is_reused_without_reading_the_rpu_or_the_base_layer")
    else {
        return;
    };
    fake.script("ffmpeg", FAILS);
    let bl_rpu_hevc = fake.temp.join("BL_RPU.hevc");
    fake.leave(&bl_rpu_hevc, b"previous injection", true);
    let measured = measured(fake.input.clone(), HdrFormat::Hdr10WithMeasurements, true);
    let missing_rpu = fake.temp.join("missing_RPU.bin");
    let (injected, steps) = run_injection(&fake, &measured, &missing_rpu, true, &default_args());

    assert_eq!(injected, Some(bl_rpu_hevc.clone()));
    assert_eq!(steps, 2);
    assert_eq!(fs::read(&bl_rpu_hevc).unwrap(), b"previous injection");
    assert!(fake.calls("ffmpeg").is_empty());
}

#[test]
fn resume_settings_carry_the_injection_path_only_for_external() {
    // The default fingerprint is the one 0.9.0 writes: its temp dirs still resume.
    let default = resume_settings(&Args::try_parse_from(["mkvdovi"]).unwrap(), None, false);
    assert_eq!(
        default,
        "hwaccel=Auto analysis_quality=Auto optimizer=Conservative boost=false boost_experimental=false cm=V40 content_type=Movies reference_mode=false source_primaries=None trim_targets=100,600,1000 peak_source=Histogram mdfix=false legacy_madvr_l1=false"
    );
    let internal = Args::try_parse_from(["mkvdovi", "--dovi-inject", "internal"]).unwrap();
    assert_eq!(resume_settings(&internal, None, false), default);
    let external = Args::try_parse_from(["mkvdovi", "--dovi-inject", "external"]).unwrap();
    assert_eq!(
        resume_settings(&external, None, false),
        format!("{default} dovi_inject=external")
    );
}
