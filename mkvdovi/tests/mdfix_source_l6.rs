//! `--mdfix` on a Profile 8.1 source whose container states no mastering display (ROADMAP P12.1):
//! the rebuilt RPU takes the mastering display from the source RPU's L6, not the 0.005-nit
//! default, so its source range starts at PQ code 7 (0.0001 nits), not 62. MaxCLL/MaxFALL stay
//! with the measurements, never the RPU's. The fixture is a real P8.1 clip: x265 without
//! mastering-display SEI, an RPU from `dovi_tool generate` with its own L6, `dovi_tool
//! inject-rpu`, mkvmerge.
//!
//! Skipped (with a message) when ffmpeg with libx265, ffprobe, mkvmerge, mediainfo or dovi_tool is
//! missing, or when no current `hdr_analyzer_mvp` is built next to mkvdovi.

#[macro_use]
#[path = "support/playback.rs"]
mod playback;

use std::path::Path;
use std::process::Command;

use dolby_vision::rpu::dovi_rpu::DoviRpu;
use dolby_vision::rpu::extension_metadata::blocks::ExtMetadataBlock;
use serde_json::json;

use self::playback::*;

const FRAMES: usize = 48;
/// The source RPU's L6: 1000 nits, 0.0001 nits (raw 1), and light levels the measurements of
/// this dim clip cannot reach.
const SOURCE_MAX_DML: u16 = 1000;
const SOURCE_MIN_DML_RAW: u16 = 1;
const SOURCE_MAX_CLL: u16 = 9000;
const SOURCE_MAX_FALL: u16 = 8000;
/// 12-bit PQ code of 0.0001 nits (ST 2084), as the r5 Inside Out 2 source states it.
const PQ_0_0001_NITS: u16 = 7;
/// 12-bit PQ code of 1000 nits.
const PQ_1000_NITS: u16 = 3079;

/// A Profile 8.1 clip without mastering-display metadata in the stream or the container, carrying
/// an RPU whose L6 states the values above.
fn p81_source_without_container_mastering(dir: &Path) -> std::path::PathBuf {
    let raw = encode(
        dir,
        "p81",
        STEP,
        FRAMES as u32,
        &format!("keyint=24:min-keyint=24:bframes=2:b-adapt=0:{PQ_TAGS}"),
    );
    let config = dir.join("generate.json");
    std::fs::write(
        &config,
        json!({
            "cm_version": "V40",
            "length": FRAMES,
            "profile": "8.1",
            "level6": {
                "max_display_mastering_luminance": SOURCE_MAX_DML,
                "min_display_mastering_luminance": SOURCE_MIN_DML_RAW,
                "max_content_light_level": SOURCE_MAX_CLL,
                "max_frame_average_light_level": SOURCE_MAX_FALL,
            },
            // The RPU sample needs L1, as every real source RPU has.
            "shots": [{
                "start": 0,
                "duration": FRAMES,
                "metadata_blocks": [{"Level1": {"min_pq": 0, "max_pq": 2800, "avg_pq": 1500}}],
            }],
        })
        .to_string(),
    )
    .unwrap();
    let rpu = dir.join("RPU.bin");
    run(Command::new("dovi_tool")
        .arg("generate")
        .arg("-j")
        .arg(&config)
        .arg("-o")
        .arg(&rpu));
    let injected = dir.join("injected.hevc");
    run(Command::new("dovi_tool")
        .arg("inject-rpu")
        .arg("-i")
        .arg(&raw)
        .arg("--rpu-in")
        .arg(&rpu)
        .arg("-o")
        .arg(&injected));
    mux(dir, SOURCE, &injected, &[])
}

/// (L6, source_min_pq, source_max_pq) of every access unit's RPU.
fn l6_and_source_range(mkv: &Path) -> Vec<((u16, u16, u16, u16), u16, u16)> {
    rpu_per_access_unit(mkv)
        .iter()
        .map(|unit| {
            let rpu = DoviRpu::parse_unspec62_nalu(unit).expect("parse RPU NAL");
            let dm = rpu.vdr_dm_data.as_ref().expect("RPU with DM data");
            let l6 = match dm.get_block(6) {
                Some(ExtMetadataBlock::Level6(l6)) => (
                    l6.max_display_mastering_luminance,
                    l6.min_display_mastering_luminance,
                    l6.max_content_light_level,
                    l6.max_frame_average_light_level,
                ),
                _ => panic!("RPU without L6"),
            };
            (l6, dm.source_min_pq, dm.source_max_pq)
        })
        .collect()
}

#[test]
fn mdfix_takes_the_mastering_display_from_the_source_rpu_when_the_container_states_none() {
    require_tools!();
    let dir = tempfile::tempdir().unwrap();
    let source = p81_source_without_container_mastering(dir.path());

    // The fixture is what the test claims: no mastering display MediaInfo could read, and a
    // source RPU that carries the L6.
    let info = run(Command::new("mediainfo").arg(&source));
    let info = String::from_utf8_lossy(&info.stdout);
    assert!(!info.contains("Mastering display"), "{info}");
    let source_rpus = l6_and_source_range(&source);
    assert_eq!(source_rpus.len(), FRAMES);
    assert_eq!(
        source_rpus[0].0,
        (
            SOURCE_MAX_DML,
            SOURCE_MIN_DML_RAW,
            SOURCE_MAX_CLL,
            SOURCE_MAX_FALL
        )
    );

    let work = dir.path().join("run");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::copy(&source, work.join(SOURCE)).unwrap();
    let output = run(mkvdovi_cmd().current_dir(&work).arg(SOURCE).args([
        "--mdfix",
        "--keep-source",
        "--verify",
        "--hwaccel",
        "none",
    ]));
    let rebuilt = l6_and_source_range(&work.join("clip.mdfix.DV.mkv"));
    assert_eq!(rebuilt.len(), FRAMES);
    for (frame, &((max_dml, min_dml, max_cll, max_fall), min_pq, max_pq)) in
        rebuilt.iter().enumerate()
    {
        assert_eq!(
            (max_dml, min_dml),
            (SOURCE_MAX_DML, SOURCE_MIN_DML_RAW),
            "frame {frame}"
        );
        assert_eq!(
            (min_pq, max_pq),
            (PQ_0_0001_NITS, PQ_1000_NITS),
            "frame {frame}"
        );
        // Measured, not the source RPU's: this clip stays far below 8000 nits.
        assert!(
            max_cll > 0 && max_cll < SOURCE_MAX_FALL && max_fall > 0 && max_fall <= max_cll,
            "frame {frame}: MaxCLL {max_cll}, MaxFALL {max_fall}"
        );
    }
    let log = log_of(&output);
    assert!(
        log.contains(
            "Mastering display peak 1000 nits, minimum 0.0001 nits from the source RPU's L6"
        ),
        "{log}"
    );
}
