//! The builders against `dovi_tool generate` run now, on the `extra.json` this crate writes
//! today: catches a change to the parameters that the committed fixtures would only show after
//! `make_fixtures.sh`. Skips (with "Skipping") without `dovi_tool` 2.3.4 on PATH; CI installs it.

mod cases;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use dolby_vision::rpu::utils::parse_rpu_file;
use dovi84_composer::{canary_mapping, rewrite_rpu_file, Composer, DEFAULT_CANARY_OFFSET};
use dovi_rpu_builder::{CmVersion, Hdr10PlusFrame, Hdr10PlusHold, RpuBuilder};

const DOVI_TOOL_VERSION: &str = "2.3.4";

fn dovi_tool_ready() -> bool {
    match Command::new("dovi_tool").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            if version.split_whitespace().nth(1) == Some(DOVI_TOOL_VERSION) {
                return true;
            }
            eprintln!(
                "Skipping dovi_tool identity test: {} is not dovi_tool {DOVI_TOOL_VERSION}",
                version.trim()
            );
            false
        }
        _ => {
            eprintln!("Skipping dovi_tool identity test: dovi_tool not on PATH");
            false
        }
    }
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn generate(dir: &Path, name: &str, config: &serde_json::Value, extra: &[&str]) -> PathBuf {
    let config_path = dir.join(format!("{name}.extra.json"));
    fs::write(&config_path, serde_json::to_string_pretty(config).unwrap()).unwrap();
    let rpu = dir.join(format!("{name}.RPU.bin"));
    let output = Command::new("dovi_tool")
        .arg("generate")
        .arg("-j")
        .arg(&config_path)
        .args(extra)
        .arg("--rpu-out")
        .arg(&rpu)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "dovi_tool generate {name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    rpu
}

/// `fit_hlg_composer canary-rpu`'s swap: every frame of a bt2100 RPU gets the canary mapping.
fn install_canary(path: &Path) {
    let canary = canary_mapping(DEFAULT_CANARY_OFFSET).unwrap();
    let mut out = Vec::new();
    for mut rpu in parse_rpu_file(path).unwrap() {
        rpu.rpu_data_mapping = Some(canary.clone());
        rpu.modified = true;
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&rpu.write_hevc_unspec62_nalu().unwrap()[2..]);
    }
    fs::write(path, out).unwrap();
}

fn record(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(&nal[2..]);
}

#[test]
fn shot_cases_equal_dovi_tool_run_now() {
    if !dovi_tool_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for case in cases::shot_cases() {
        let rpu = generate(
            dir.path(),
            case.name,
            &case.params.extra_json(Some(&case.extra_shots())),
            &[],
        );
        match case.post {
            cases::Post::None => {}
            cases::Post::Bt2100 => {
                rewrite_rpu_file(&rpu, Composer::Bt2100V1).unwrap();
            }
            cases::Post::Canary => {
                rewrite_rpu_file(&rpu, Composer::Bt2100V1).unwrap();
                install_canary(&rpu);
            }
        }
        let builder = RpuBuilder::new(&case.params).unwrap();
        let mut ours = Vec::new();
        for &(duration, l1) in &case.shots {
            let mut shot = builder.shot(l1).unwrap();
            for frame in 0..duration {
                record(&mut ours, &shot.write(frame == 0).unwrap());
            }
        }
        let theirs = fs::read(&rpu).unwrap();
        assert!(
            ours == theirs,
            "{}: builder differs from dovi_tool",
            case.name
        );
        let committed = fs::read(fixtures().join(format!("{}.RPU.bin", case.name))).unwrap();
        assert!(
            theirs == committed,
            "{}: dovi_tool output differs from the committed fixture; rerun make_fixtures.sh",
            case.name
        );
    }
}

#[test]
fn hdr10plus_sequences_equal_dovi_tool_run_now() {
    if !dovi_tool_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for sequence in cases::HDR10PLUS_SEQUENCES {
        let json = fixtures().join(format!("{sequence}.json"));
        let data = fs::read(fixtures().join(format!("{sequence}.payloads"))).unwrap();
        let mut payloads = Vec::new();
        let mut rest = data.as_slice();
        while !rest.is_empty() {
            let len = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
            payloads.push(rest[2..2 + len].to_vec());
            rest = &rest[2 + len..];
        }
        for cm in [CmVersion::V29, CmVersion::V40] {
            let params = cases::hdr10plus_params(cm);
            for (peak, name) in cases::PEAK_SOURCES {
                let label = format!("{sequence}.{name}.{}", cases::cm_name(cm));
                let rpu = generate(
                    dir.path(),
                    &label,
                    &params.extra_json(None),
                    &[
                        "--hdr10plus-json",
                        json.to_str().unwrap(),
                        "--hdr10plus-peak-source",
                        name,
                    ],
                );
                let builder = RpuBuilder::new(&params).unwrap();
                let mut hold = Hdr10PlusHold::new(peak, params.l1_avg_pq_cm_version);
                let mut ours = Vec::new();
                for payload in &payloads {
                    let (l1, refresh) =
                        hold.next(&Hdr10PlusFrame::parse(payload).unwrap()).unwrap();
                    record(
                        &mut ours,
                        &builder.shot(l1).unwrap().write(refresh).unwrap(),
                    );
                }
                assert!(
                    ours == fs::read(&rpu).unwrap(),
                    "{label}: builder differs from dovi_tool"
                );
            }
        }
    }
}
