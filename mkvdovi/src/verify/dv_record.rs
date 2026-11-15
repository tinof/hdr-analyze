//! The Dolby Vision configuration record of the muxed output (ROADMAP E17.2).
//!
//! Players pick the Dolby Vision decoder and the fallback from this record, not from the RPU, so
//! `--verify` reads it back with ffprobe: Profile 8, a level, RPU and base layer present, no
//! enhancement layer, and the base-layer compatibility ID of the source transfer (1 for PQ, 4 for
//! HLG). mkvmerge derives the ID from the bitstream, so a missing HLG tag there would silently
//! yield ID 2 (SDR).

use colored::Colorize;
use std::path::Path;
use std::process::Command;

/// The fields of a Dolby Vision configuration record as ffprobe's `-show_streams` prints them.
/// A field ffprobe did not print, or printed as something other than a small number, is `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct DvConfigRecord {
    profile: Option<u8>,
    level: Option<u8>,
    rpu_present: Option<u8>,
    el_present: Option<u8>,
    bl_present: Option<u8>,
    compatibility_id: Option<u8>,
}

/// What ffprobe said about the first video stream.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Probe {
    /// ffprobe did not run, failed or printed something unreadable: the check is skipped.
    Unreadable,
    /// ffprobe read the stream, and it has no Dolby Vision configuration record.
    NoRecord,
    Record(DvConfigRecord),
}

/// Checks the record of `output_file`'s first video stream and prints the result. Returns
/// `false` only on a failed check; an ffprobe that cannot run, or reads no record, is a warning.
pub(super) fn verify(output_file: &Path, hlg_source: bool) -> bool {
    let (passed, lines) = report(probe(output_file), hlg_source);
    for line in lines {
        match line {
            Line::Info(text) => println!("{text}"),
            Line::Warn(text) => println!("{}", text.yellow()),
            Line::Fail(text) => println!("{}", text.red()),
        }
    }
    passed
}

/// One printed line of the record check.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Line {
    Info(String),
    Warn(String),
    Fail(String),
}

/// Whether the record check passes, and the lines it prints. An ffprobe that reads the stream
/// but no record only warns: an older ffprobe does not read the record from Matroska at all, and
/// `dovi_tool info` and the RPU count still check that the output carries Dolby Vision.
pub(super) fn report(probe: Probe, hlg_source: bool) -> (bool, Vec<Line>) {
    match probe {
        Probe::Unreadable => (
            true,
            vec![Line::Warn(
                "Could not read the Dolby Vision configuration record with ffprobe; skipping that \
                 check."
                    .into(),
            )],
        ),
        Probe::NoRecord => (
            true,
            vec![Line::Warn(
                "ffprobe shows no Dolby Vision configuration record for the output (an older \
                 ffprobe does not read it from Matroska); skipping that check."
                    .into(),
            )],
        ),
        Probe::Record(record) => match check(&record, hlg_source) {
            Ok(lines) => (true, lines.into_iter().map(Line::Info).collect()),
            Err(problems) => {
                let mut lines: Vec<Line> = problems.into_iter().map(Line::Fail).collect();
                if !hlg_source && record.compatibility_id.is_some_and(|id| id != 1) {
                    lines.push(Line::Warn(
                        "mkvmerge derives the compatibility ID from the HEVC bitstream: the \
                         source's VUI probably lacks the PQ transfer tag (transfer_characteristics \
                         16), which the Matroska colour element alone does not set."
                            .into(),
                    ));
                }
                (false, lines)
            }
        },
    }
}

fn probe(output_file: &Path) -> Probe {
    match Command::new("ffprobe")
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
    {
        Ok(output) => classify(output.status.success(), &output.stdout),
        Err(_) => Probe::Unreadable,
    }
}

/// Reads ffprobe's stdout: a failed run or non-UTF-8 output is unreadable, a successful one
/// either carries a record or does not.
pub(super) fn classify(succeeded: bool, stdout: &[u8]) -> Probe {
    if !succeeded {
        return Probe::Unreadable;
    }
    match std::str::from_utf8(stdout) {
        Ok(text) => parse(text).map_or(Probe::NoRecord, Probe::Record),
        Err(_) => Probe::Unreadable,
    }
}

/// The configuration record in ffprobe's `-show_streams -of default=nw=1` output for one stream,
/// `None` when the output has neither the record's side-data entry nor any of its fields.
pub(super) fn parse(ffprobe_streams: &str) -> Option<DvConfigRecord> {
    let mut found = false;
    let mut record = DvConfigRecord::default();
    for line in ffprobe_streams.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        let field = match key {
            "side_data_type" => {
                found |= value == "DOVI configuration record";
                continue;
            }
            "dv_profile" => &mut record.profile,
            "dv_level" => &mut record.level,
            "rpu_present_flag" => &mut record.rpu_present,
            "el_present_flag" => &mut record.el_present,
            "bl_present_flag" => &mut record.bl_present,
            "dv_bl_signal_compatibility_id" => &mut record.compatibility_id,
            _ => continue,
        };
        found = true;
        *field = value.trim().parse().ok();
    }
    found.then_some(record)
}

/// The lines to print for a record mkvdovi may deliver, or every reason it may not. The
/// compatibility ID follows the source transfer: 4 (HLG) for Profile 8.4, 1 (PQ) for 8.1.
pub(super) fn check(record: &DvConfigRecord, hlg_source: bool) -> Result<Vec<String>, Vec<String>> {
    const PREFIX: &str = "Dolby Vision configuration record";
    let mut problems = Vec::new();
    let mut require = |name: &str, value: Option<u8>, valid: fn(u8) -> bool, need: &str| match value
    {
        Some(value) if valid(value) => {}
        Some(value) => problems.push(format!("{PREFIX}: {name} is {value}; {need}.")),
        None => problems.push(format!("{PREFIX}: no readable {name}; {need}.")),
    };
    require(
        "dv_profile",
        record.profile,
        |v| v == 8,
        "mkvdovi writes Profile 8",
    );
    require(
        "dv_level",
        record.level,
        |v| v >= 1,
        "the stream needs a level",
    );
    require(
        "rpu_present_flag",
        record.rpu_present,
        |v| v == 1,
        "the output must carry an RPU",
    );
    require(
        "bl_present_flag",
        record.bl_present,
        |v| v == 1,
        "the output must carry a base layer",
    );
    require(
        "el_present_flag",
        record.el_present,
        |v| v == 0,
        "Profile 8 has no enhancement layer",
    );
    let (expected_id, transfer, profile) = if hlg_source {
        (4, "HLG", "8.4")
    } else {
        (1, "PQ", "8.1")
    };
    match record.compatibility_id {
        Some(id) if id == expected_id => {}
        Some(id) => problems.push(format!(
            "Dolby Vision compatibility ID is {id}; Profile {profile} needs {expected_id} ({transfer})."
        )),
        None => problems.push(format!(
            "{PREFIX}: no readable dv_bl_signal_compatibility_id; Profile {profile} needs {expected_id} ({transfer})."
        )),
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(vec![
        format!(
            "{PREFIX}: Profile 8, level {}, RPU and base layer present, no enhancement layer.",
            record.level.unwrap_or_default()
        ),
        format!("Dolby Vision compatibility ID {expected_id} ({transfer})."),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ffprobe 9.0.2 `-select_streams v:0 -show_streams -of default=nw=1` on a 64×64 Profile 8.1
    /// clip (x265 PQ tags, `dovi_tool generate` RPU, mkvmerge 82): the head of the stream and its
    /// side data, as printed.
    const PQ_81: &str = "index=0
codec_name=hevc
codec_long_name=H.265 / HEVC (High Efficiency Video Coding)
profile=Main 10
codec_type=video
width=64
height=64
color_transfer=smpte2084
TAG:NUMBER_OF_FRAMES=6
TAG:_STATISTICS_TAGS=BPS DURATION NUMBER_OF_FRAMES NUMBER_OF_BYTES
side_data_type=DOVI configuration record
dv_version_major=1
dv_version_minor=0
dv_profile=8
dv_level=1
rpu_present_flag=1
el_present_flag=0
bl_present_flag=1
dv_bl_signal_compatibility_id=1
dv_md_compression=none
";

    /// The same for the Profile 8.4 clip (x265 HLG tags).
    const HLG_84: &str = "index=0
codec_name=hevc
profile=Main 10
codec_type=video
color_transfer=arib-std-b67
TAG:NUMBER_OF_FRAMES=6
side_data_type=DOVI configuration record
dv_version_major=1
dv_version_minor=0
dv_profile=8
dv_level=1
rpu_present_flag=1
el_present_flag=0
bl_present_flag=1
dv_bl_signal_compatibility_id=4
dv_md_compression=none
";

    /// The PQ clip before the RPU injection: no Dolby Vision side data at all.
    const PLAIN_HEVC: &str = "index=0
codec_name=hevc
profile=Main 10
codec_type=video
color_transfer=smpte2084
TAG:NUMBER_OF_FRAMES=6
";

    fn record(text: &str) -> DvConfigRecord {
        parse(text).expect("a Dolby Vision configuration record")
    }

    /// `text` with the line `key=…` replaced by `key=value`, or dropped when `value` is `None`.
    fn with_field(text: &str, key: &str, value: Option<&str>) -> String {
        let prefix = format!("{key}=");
        assert!(text.contains(&prefix), "{key} is in the fixture");
        text.lines()
            .filter_map(|line| match line.strip_prefix(&prefix) {
                Some(_) => value.map(|value| format!("{prefix}{value}")),
                None => Some(line.to_string()),
            })
            .map(|line| line + "\n")
            .collect()
    }

    #[test]
    fn a_profile_81_record_with_id_1_passes_for_pq() {
        let lines = check(&record(PQ_81), false).unwrap();
        assert_eq!(
            lines,
            [
                "Dolby Vision configuration record: Profile 8, level 1, RPU and base layer present, no enhancement layer.",
                "Dolby Vision compatibility ID 1 (PQ).",
            ]
        );
    }

    #[test]
    fn a_profile_84_record_with_id_4_passes_for_hlg() {
        let lines = check(&record(HLG_84), true).unwrap();
        assert_eq!(lines[1], "Dolby Vision compatibility ID 4 (HLG).");
    }

    #[test]
    fn the_compatibility_id_follows_the_source_transfer() {
        assert_eq!(
            check(&record(HLG_84), false).unwrap_err(),
            ["Dolby Vision compatibility ID is 4; Profile 8.1 needs 1 (PQ)."]
        );
        assert_eq!(
            check(&record(PQ_81), true).unwrap_err(),
            ["Dolby Vision compatibility ID is 1; Profile 8.4 needs 4 (HLG)."]
        );
        // mkvmerge's reading of an HLG stream without its transfer tag.
        let sdr = with_field(HLG_84, "dv_bl_signal_compatibility_id", Some("2"));
        assert_eq!(
            check(&record(&sdr), true).unwrap_err(),
            ["Dolby Vision compatibility ID is 2; Profile 8.4 needs 4 (HLG)."]
        );
    }

    #[test]
    fn each_wrong_field_fails_on_its_own() {
        let cases = [
            (
                "dv_profile",
                "7",
                "dv_profile is 7; mkvdovi writes Profile 8.",
            ),
            (
                "dv_profile",
                "5",
                "dv_profile is 5; mkvdovi writes Profile 8.",
            ),
            ("dv_level", "0", "dv_level is 0; the stream needs a level."),
            (
                "rpu_present_flag",
                "0",
                "rpu_present_flag is 0; the output must carry an RPU.",
            ),
            (
                "bl_present_flag",
                "0",
                "bl_present_flag is 0; the output must carry a base layer.",
            ),
            (
                "el_present_flag",
                "1",
                "el_present_flag is 1; Profile 8 has no enhancement layer.",
            ),
        ];
        for (fixture, hlg_source) in [(PQ_81, false), (HLG_84, true)] {
            for (key, value, problem) in cases {
                let text = with_field(fixture, key, Some(value));
                assert_eq!(
                    check(&record(&text), hlg_source).unwrap_err(),
                    [format!("Dolby Vision configuration record: {problem}")],
                    "{key}={value}"
                );
            }
        }
    }

    #[test]
    fn a_missing_or_unreadable_field_fails() {
        let needs = [
            ("dv_profile", "mkvdovi writes Profile 8"),
            ("dv_level", "the stream needs a level"),
            ("rpu_present_flag", "the output must carry an RPU"),
            ("bl_present_flag", "the output must carry a base layer"),
            ("el_present_flag", "Profile 8 has no enhancement layer"),
            ("dv_bl_signal_compatibility_id", "Profile 8.1 needs 1 (PQ)"),
        ];
        for (key, need) in needs {
            let expected = [format!(
                "Dolby Vision configuration record: no readable {key}; {need}."
            )];
            for value in [None, Some("unknown"), Some("256"), Some("")] {
                let text = with_field(PQ_81, key, value);
                assert_eq!(
                    check(&record(&text), false).unwrap_err(),
                    expected,
                    "{key}={value:?}"
                );
            }
        }
    }

    #[test]
    fn every_problem_is_reported() {
        let text = with_field(PQ_81, "dv_profile", Some("7"));
        let text = with_field(&text, "el_present_flag", Some("1"));
        let text = with_field(&text, "dv_bl_signal_compatibility_id", Some("6"));
        assert_eq!(check(&record(&text), false).unwrap_err().len(), 3);
    }

    #[test]
    fn a_stream_without_the_record_has_none() {
        assert_eq!(parse(PLAIN_HEVC), None);
        assert_eq!(parse(""), None);
        assert_eq!(classify(true, PLAIN_HEVC.as_bytes()), Probe::NoRecord);
        assert_eq!(classify(true, b""), Probe::NoRecord);
    }

    #[test]
    fn the_side_data_entry_alone_is_a_record_with_every_field_missing() {
        let text = format!("{PLAIN_HEVC}side_data_type=DOVI configuration record\n");
        assert_eq!(parse(&text), Some(DvConfigRecord::default()));
        assert_eq!(check(&record(&text), false).unwrap_err().len(), 6);
    }

    #[test]
    fn a_stream_without_the_record_warns_and_passes() {
        let (passed, lines) = report(classify(true, PLAIN_HEVC.as_bytes()), false);
        assert!(
            passed,
            "an ffprobe that reads no record cannot fail the output"
        );
        assert!(
            matches!(lines.as_slice(), [Line::Warn(text)] if text.contains("skipping that check")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_pq_output_with_another_id_names_the_missing_vui_tag() {
        let text = with_field(PQ_81, "dv_bl_signal_compatibility_id", Some("0"));
        let (passed, lines) = report(Probe::Record(record(&text)), false);
        assert!(!passed);
        assert!(
            matches!(lines.as_slice(), [Line::Fail(_), Line::Warn(hint)] if hint.contains("transfer_characteristics")),
            "{lines:?}"
        );
        // A good PQ record and an HLG mismatch get no PQ hint.
        let (passed, lines) = report(Probe::Record(record(PQ_81)), false);
        assert!(
            passed && lines.iter().all(|line| matches!(line, Line::Info(_))),
            "{lines:?}"
        );
        let (passed, lines) = report(Probe::Record(record(PQ_81)), true);
        assert!(
            !passed && lines.iter().all(|line| matches!(line, Line::Fail(_))),
            "{lines:?}"
        );
    }

    #[test]
    fn other_side_data_is_no_record() {
        let text = format!("{PLAIN_HEVC}side_data_type=Display Matrix\nrotation=0\n");
        assert_eq!(parse(&text), None);
    }

    #[test]
    fn a_failed_or_unreadable_ffprobe_run_skips_the_check() {
        assert_eq!(classify(false, PQ_81.as_bytes()), Probe::Unreadable);
        assert_eq!(classify(false, b""), Probe::Unreadable);
        assert_eq!(classify(true, b"dv_profile=\xff\n"), Probe::Unreadable);
        assert_eq!(
            classify(true, PQ_81.as_bytes()),
            Probe::Record(record(PQ_81))
        );
    }
}
