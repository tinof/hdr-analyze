//! The `--mdfix` notice (ROADMAP P11): which authored Dolby Vision levels of the source RPU the
//! rebuild replaces or drops. `dovi_tool generate` regenerates the whole RPU, so the notice can
//! only name what the sampled windows of the source showed; an absence means "not seen".

use dolby_vision::rpu::extension_metadata::blocks::ExtMetadataBlockLevel8;
use dolby_vision::utils::{nits_to_pq_12_bit, pq_to_nits};

use crate::cli::{Args, CmVersion};
use crate::metadata;
use crate::progress;
use crate::rpu_check::AuthoredLevels;

/// L254 that `dovi_tool generate` writes for CM v4.0 (`ExtMetadataBlockLevel254::cmv402_default`).
const GENERATED_L254: (u8, u8) = (0, 2);

/// L9 and L11 of a `--cm-version v29` RPU. mkvdovi writes no `cm_version` then
/// (`metadata/rpu_config.rs`, `generate_extra_json`), so `dovi_tool generate` falls back to CM v4.0
/// with `CmV40DmData::default_safe`: L9 DCI-P3 (index 0) and L11 cinema. ROADMAP lists this as a
/// defect; when v29 output becomes CM v2.9, these lines change with it.
const V29_GENERATED_L9: u8 = 0;
const V29_GENERATED_L11: (u8, u8, bool) = (1, 0, false);
const V29_DEFAULT: &str = "generator default under --cm-version v29";

/// Prints the notice: the first line as a warning, the rest as info lines. Under CM v4.0 the L9 to
/// be written is resolved here as `prepare_configuration` resolves it (MediaInfo unless
/// `--source-primaries`), only when the sample saw an L9.
pub(super) fn print_mdfix_notice(authored: Option<&AuthoredLevels>, args: &Args, input_file: &str) {
    let resolved_primaries = match authored {
        Some(levels) if args.cm_version == CmVersion::V40 && !levels.l9.is_empty() => Some(
            args.source_primaries
                .unwrap_or_else(|| metadata::detect_source_primaries(input_file)),
        ),
        _ => args.source_primaries,
    };
    let mut lines = mdfix_notice(authored, args, resolved_primaries).into_iter();
    if let Some(header) = lines.next() {
        progress::print_warn(&header);
    }
    for line in lines {
        progress::print_info(&format!("  {line}"));
    }
}

/// The notice lines: a header, then one line per authored level the rebuild does not keep.
/// `None`: the source RPU could not be sampled; the header lists what `--mdfix` always does.
/// `resolved_primaries`: the L9 index a CM v4.0 rebuild writes; `None` when it is not known.
pub(super) fn mdfix_notice(
    authored: Option<&AuthoredLevels>,
    args: &Args,
    resolved_primaries: Option<u8>,
) -> Vec<String> {
    let Some(authored) = authored else {
        return vec![format!(
            "--mdfix rebuilds the whole RPU (L1 is measured again). The source RPU could not be \
             sampled, so in general: authored L2 trims are {}, L3 becomes neutral, L4, L8, L10 \
             and L255 are dropped, L5 is reduced to one active area, L6 comes from the container \
             and the measurements, and L9, L11 and L254 are rebuilt.",
            match args.cm_version {
                CmVersion::V40 => "replaced by neutral trims",
                CmVersion::V29 => "dropped",
            }
        )];
    };
    let windows = authored.windows;
    let mut lines = vec![format!(
        "--mdfix rebuilds the whole RPU (L1 is measured again). Authored metadata seen in {windows} \
         sampled window{} of the source RPU that is not kept:",
        if windows == 1 { "" } else { "s" }
    )];
    let v40 = args.cm_version == CmVersion::V40;

    if !authored.l2_trims.is_empty() {
        let source = nits_list(authored.l2_trims.iter().map(|&pq| pq_label(pq)));
        lines.push(if v40 {
            let targets = match args.trim_targets_nits() {
                Ok(targets) => nits_list(targets.iter().map(|nits| nits.to_string())),
                Err(_) => args.trim_targets.clone(),
            };
            format!(
                "L2 trims for {source}: replaced by neutral trims for {targets} (--trim-targets)"
            )
        } else {
            format!("L2 trims for {source}: dropped (no L2 is written under --cm-version v29)")
        });
    }
    if authored.l3 {
        lines.push("L3 offsets: replaced by neutral offsets".to_string());
    }
    if authored.l4 {
        lines.push("L4: dropped".to_string());
    }
    if authored.l5_values.len() > 1 {
        lines.push(format!(
            "L5: {} different active areas; the first one is written for every frame",
            authored.l5_values.len()
        ));
    }
    if authored.l6 {
        lines.push(
            "L6: not taken from the source RPU; the mastering display comes from the container's \
             HDR10 metadata (or defaults), MaxCLL/MaxFALL from the container or the measurements"
                .to_string(),
        );
    }
    if !authored.l8_trims.is_empty() {
        let targets: Vec<String> = authored
            .l8_trims
            .iter()
            .map(|&index| l8_target_label(index, authored))
            .collect();
        lines.push(format!("L8 trims for {}: dropped", targets.join(", ")));
    }
    if !authored.l9.is_empty() {
        let (written, origin) = if !v40 {
            (Some(V29_GENERATED_L9), V29_DEFAULT)
        } else if args.source_primaries.is_some() {
            (resolved_primaries, "--source-primaries")
        } else {
            (resolved_primaries, "detected from the container")
        };
        match written {
            Some(index) => {
                let changed: Vec<u8> = authored
                    .l9
                    .iter()
                    .copied()
                    .filter(|&v| v != index)
                    .collect();
                if !changed.is_empty() {
                    lines.push(format!(
                        "L9 primaries {}: replaced by {} ({origin})",
                        join(changed.iter().map(|&v| primaries_label(v))),
                        primaries_label(index)
                    ));
                }
            }
            None => lines.push(format!(
                "L9 primaries {}: rebuilt from the container's colour metadata",
                join(authored.l9.iter().map(|&v| primaries_label(v)))
            )),
        }
    }
    if !authored.l10_targets.is_empty() {
        let indices = join(authored.l10_targets.keys().map(|index| index.to_string()));
        lines.push(format!("L10 custom target displays {indices}: dropped"));
    }
    let (generated_l11, l11_origin) = if v40 {
        (
            (args.content_type.as_u8(), 0, args.reference_mode),
            "--content-type, --reference-mode",
        )
    } else {
        (V29_GENERATED_L11, V29_DEFAULT)
    };
    let changed_l11: Vec<(u8, u8, bool)> = authored
        .l11
        .iter()
        .copied()
        .filter(|&value| value != generated_l11)
        .collect();
    if !changed_l11.is_empty() {
        lines.push(format!(
            "L11 {}: replaced by {} ({l11_origin})",
            join(changed_l11.iter().map(|&value| l11_label(value))),
            l11_label(generated_l11)
        ));
    }
    let changed_l254: Vec<(u8, u8)> = authored
        .l254
        .iter()
        .copied()
        .filter(|&value| value != GENERATED_L254)
        .collect();
    if !changed_l254.is_empty() {
        lines.push(format!(
            "L254 {}: replaced by {}",
            join(changed_l254.iter().map(|&value| l254_label(value))),
            l254_label(GENERATED_L254)
        ));
    }
    if authored.l255 {
        lines.push("L255: dropped".to_string());
    }
    if lines.len() == 1 {
        lines[0] = format!(
            "--mdfix rebuilds the whole RPU (L1 is measured again). No other authored metadata \
             that the rebuild drops was seen in {windows} sampled window{} of the source RPU.",
            if windows == 1 { "" } else { "s" }
        );
    }
    lines
}

fn join(items: impl Iterator<Item = String>) -> String {
    items.collect::<Vec<_>>().join(", ")
}

fn nits_list(items: impl Iterator<Item = String>) -> String {
    format!("{} nits", join(items))
}

/// Nits of a 12-bit PQ code, as the roundest number of nits that encodes to the same code
/// (2081 -> "100", 3079 -> "1000").
fn pq_label(pq: u16) -> String {
    let nits = pq_to_nits(f64::from(pq) / 4095.0);
    [100.0, 50.0, 10.0, 1.0]
        .iter()
        .map(|step| (nits / step).round() * step)
        .find(|&rounded| nits_to_pq_12_bit(rounded) == pq)
        .map(|rounded| format!("{rounded:.0}"))
        .unwrap_or_else(|| format!("{nits:.1}"))
}

/// An L8 trim target: a custom display by the peak of the sampled L10 that defines it (L10 may
/// use any non-preset index, including 24 and 25, which `trim_target_nits` reads as 300 nits);
/// otherwise a display the crate knows by its nits, else by its index alone. `trim_target_nits`
/// reads 100 for any index it does not know, and index 1 is the 100-nit display.
fn l8_target_label(index: u8, authored: &AuthoredLevels) -> String {
    if let Some(&pq) = authored.l10_targets.get(&index) {
        return format!(
            "custom target display {index} ({} nits, from L10)",
            pq_label(pq)
        );
    }
    let known_nits = ExtMetadataBlockLevel8 {
        target_display_index: index,
        ..Default::default()
    }
    .trim_target_nits();
    if index == 1 || known_nits != 100 {
        format!("{known_nits} nits")
    } else {
        format!("target display index {index}")
    }
}

fn primaries_label(index: u8) -> String {
    match index {
        0 => "0 (P3-D65)".to_string(),
        1 => "1 (BT.709)".to_string(),
        2 => "2 (BT.2020)".to_string(),
        other => other.to_string(),
    }
}

fn l11_label((content_type, whitepoint, reference_mode): (u8, u8, bool)) -> String {
    format!(
        "(content type {content_type}, whitepoint {whitepoint}, reference mode {})",
        if reference_mode { "on" } else { "off" }
    )
}

fn l254_label((dm_mode, dm_version_index): (u8, u8)) -> String {
    format!("(DM mode {dm_mode}, version index {dm_version_index})")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use clap::Parser;

    use super::*;
    use crate::rpu_check::Level5Offsets;

    fn args(flags: &[&str]) -> Args {
        let mut argv = vec!["mkvdovi"];
        argv.extend_from_slice(flags);
        Args::try_parse_from(argv).unwrap()
    }

    /// A Profile 7 MEL (CM v2.9) source like the retail cuts: non-neutral L2 at 100 nits, L4,
    /// one L5, L6.
    fn mel_v29() -> AuthoredLevels {
        AuthoredLevels {
            windows: 3,
            l2_trims: BTreeSet::from([2081]),
            l4: true,
            l5_values: BTreeSet::from([Level5Offsets::default()]),
            l6: true,
            ..Default::default()
        }
    }

    /// A Profile 8.1 CM v4.0 source like r5: L2 at three targets, L3, L4, L8 at index 1, L9,
    /// L11 and L254 as the generator writes them by default.
    fn p81_v40() -> AuthoredLevels {
        AuthoredLevels {
            windows: 3,
            l2_trims: BTreeSet::from([2081, 2851, 3079]),
            l3: true,
            l4: true,
            l5_values: BTreeSet::from([Level5Offsets {
                top: 276,
                bottom: 276,
                ..Default::default()
            }]),
            l6: true,
            l8_trims: BTreeSet::from([1]),
            l9: BTreeSet::from([2]),
            l11: BTreeSet::from([(1, 0, false)]),
            l254: BTreeSet::from([GENERATED_L254]),
            ..Default::default()
        }
    }

    /// The notice with L9 resolved only from `--source-primaries`, as without MediaInfo.
    fn notice(authored: Option<&AuthoredLevels>, args: &Args) -> Vec<String> {
        mdfix_notice(authored, args, args.source_primaries)
    }

    fn line<'a>(lines: &'a [String], level: &str) -> Option<&'a String> {
        lines[1..]
            .iter()
            .find(|l| l.starts_with(&format!("{level} ")) || l.starts_with(&format!("{level}:")))
    }

    #[test]
    fn pq_labels_are_round_nits() {
        assert_eq!(pq_label(2081), "100");
        assert_eq!(pq_label(2851), "600");
        assert_eq!(pq_label(3079), "1000");
    }

    #[test]
    fn mel_source_names_replaced_l2_and_dropped_l4() {
        let lines = notice(Some(&mel_v29()), &args(&["--mdfix"]));
        assert!(lines[0].contains("3 sampled windows"), "{lines:?}");
        assert_eq!(
            line(&lines, "L2").unwrap(),
            "L2 trims for 100 nits: replaced by neutral trims for 100, 600, 1000 nits (--trim-targets)"
        );
        assert_eq!(line(&lines, "L4").unwrap(), "L4: dropped");
        assert!(line(&lines, "L6").is_some());
        for absent in ["L3", "L5", "L8", "L9", "L10", "L11", "L254", "L255"] {
            assert!(line(&lines, absent).is_none(), "{absent}: {lines:?}");
        }
    }

    #[test]
    fn trim_targets_follow_the_option() {
        let lines = notice(Some(&mel_v29()), &args(&["--trim-targets", "100,300"]));
        assert!(line(&lines, "L2")
            .unwrap()
            .ends_with("neutral trims for 100, 300 nits (--trim-targets)"));
    }

    #[test]
    fn cm_v40_source_names_l3_l8_and_keeps_equal_l9_l11_l254_quiet() {
        let lines = notice(Some(&p81_v40()), &args(&["--mdfix"]));
        assert!(line(&lines, "L2")
            .unwrap()
            .starts_with("L2 trims for 100, 600, 1000 nits: replaced"));
        assert_eq!(
            line(&lines, "L3").unwrap(),
            "L3 offsets: replaced by neutral offsets"
        );
        assert_eq!(
            line(&lines, "L8").unwrap(),
            "L8 trims for 100 nits: dropped"
        );
        // L9 unresolved (no MediaInfo here): it cannot be compared.
        assert!(line(&lines, "L9")
            .unwrap()
            .contains("rebuilt from the container"));
        assert!(line(&lines, "L11").is_none(), "{lines:?}");
        assert!(line(&lines, "L254").is_none(), "{lines:?}");
    }

    #[test]
    fn detected_primaries_name_l9_only_when_they_differ() {
        let authored = p81_v40();
        let lines = mdfix_notice(Some(&authored), &args(&["--mdfix"]), Some(2));
        assert!(line(&lines, "L9").is_none(), "{lines:?}");
        let lines = mdfix_notice(Some(&authored), &args(&["--mdfix"]), Some(0));
        assert_eq!(
            line(&lines, "L9").unwrap(),
            "L9 primaries 2 (BT.2020): replaced by 0 (P3-D65) (detected from the container)"
        );
    }

    #[test]
    fn l11_reference_mode_and_whitepoint_differences_are_named() {
        let mut authored = p81_v40();
        authored.l11.insert((1, 0, true));
        authored.l11.insert((1, 3, false));
        let lines = notice(Some(&authored), &args(&[]));
        let l11 = line(&lines, "L11").expect("a mixed sample still names L11");
        assert!(l11.contains("reference mode on"), "{l11}");
        assert!(l11.contains("whitepoint 3"), "{l11}");
        assert!(!l11.starts_with("L11 (content type 1, whitepoint 0, reference mode off)"));
        assert!(l11.ends_with(
            "replaced by (content type 1, whitepoint 0, reference mode off) (--content-type, --reference-mode)"
        ));

        // The options decide what is written: a matching --reference-mode keeps it quiet.
        authored.l11 = BTreeSet::from([(1, 0, true)]);
        let lines = notice(Some(&authored), &args(&["--reference-mode", "true"]));
        assert!(line(&lines, "L11").is_none(), "{lines:?}");
    }

    #[test]
    fn explicit_source_primaries_name_l9_only_when_they_differ() {
        let authored = p81_v40();
        let lines = notice(Some(&authored), &args(&["--source-primaries", "2"]));
        assert!(line(&lines, "L9").is_none(), "{lines:?}");
        let lines = notice(Some(&authored), &args(&["--source-primaries", "0"]));
        assert_eq!(
            line(&lines, "L9").unwrap(),
            "L9 primaries 2 (BT.2020): replaced by 0 (P3-D65) (--source-primaries)"
        );
    }

    #[test]
    fn authored_l254_is_named_only_when_it_differs() {
        let mut authored = p81_v40();
        authored.l254.insert((1, 1));
        let lines = notice(Some(&authored), &args(&[]));
        assert_eq!(
            line(&lines, "L254").unwrap(),
            "L254 (DM mode 1, version index 1): replaced by (DM mode 0, version index 2)"
        );
    }

    #[test]
    fn cm_v29_output_drops_l2_and_names_the_generator_defaults() {
        let v29 = args(&["--cm-version", "v29"]);
        let mut authored = p81_v40();
        authored.l9 = BTreeSet::from([2]);
        authored.l11 = BTreeSet::from([(1, 0, true)]);
        authored.l254 = BTreeSet::from([(1, 1)]);
        let lines = notice(Some(&authored), &v29);
        assert_eq!(
            line(&lines, "L2").unwrap(),
            "L2 trims for 100, 600, 1000 nits: dropped (no L2 is written under --cm-version v29)"
        );
        assert_eq!(
            line(&lines, "L9").unwrap(),
            "L9 primaries 2 (BT.2020): replaced by 0 (P3-D65) (generator default under --cm-version v29)"
        );
        assert_eq!(
            line(&lines, "L11").unwrap(),
            "L11 (content type 1, whitepoint 0, reference mode on): replaced by (content type 1, whitepoint 0, reference mode off) (generator default under --cm-version v29)"
        );
        assert_eq!(
            line(&lines, "L254").unwrap(),
            "L254 (DM mode 1, version index 1): replaced by (DM mode 0, version index 2)"
        );

        // Source values equal to what the generator writes are kept: no line.
        authored.l9 = BTreeSet::from([V29_GENERATED_L9]);
        authored.l11 = BTreeSet::from([V29_GENERATED_L11]);
        authored.l254 = BTreeSet::from([GENERATED_L254]);
        let lines = notice(Some(&authored), &v29);
        for kept in ["L9", "L11", "L254"] {
            assert!(line(&lines, kept).is_none(), "{kept}: {lines:?}");
        }
    }

    #[test]
    fn l5_is_named_only_when_the_sample_saw_several_areas() {
        let mut authored = mel_v29();
        assert!(line(&notice(Some(&authored), &args(&[])), "L5").is_none());
        authored.l5_values.insert(Level5Offsets {
            top: 140,
            bottom: 140,
            ..Default::default()
        });
        assert_eq!(
            line(&notice(Some(&authored), &args(&[])), "L5").unwrap(),
            "L5: 2 different active areas; the first one is written for every frame"
        );
    }

    #[test]
    fn l8_custom_targets_never_read_as_100_nits() {
        let mut authored = AuthoredLevels {
            windows: 1,
            l8_trims: BTreeSet::from([1, 5]),
            ..Default::default()
        };
        let lines = notice(Some(&authored), &args(&[]));
        assert_eq!(
            line(&lines, "L8").unwrap(),
            "L8 trims for 100 nits, target display index 5: dropped"
        );
        authored.l10_targets = BTreeMap::from([(5, 2851)]);
        let lines = notice(Some(&authored), &args(&[]));
        assert_eq!(
            line(&lines, "L8").unwrap(),
            "L8 trims for 100 nits, custom target display 5 (600 nits, from L10): dropped"
        );
        assert_eq!(
            line(&lines, "L10").unwrap(),
            "L10 custom target displays 5: dropped"
        );
        // An L10 at index 24 overrides the crate's 300-nit lookup for that index.
        authored.l8_trims = BTreeSet::from([24]);
        authored.l10_targets = BTreeMap::from([(24, 3079)]);
        let lines = notice(Some(&authored), &args(&[]));
        assert_eq!(
            line(&lines, "L8").unwrap(),
            "L8 trims for custom target display 24 (1000 nits, from L10): dropped"
        );
        authored.l10_targets.clear();
        let lines = notice(Some(&authored), &args(&[]));
        assert_eq!(
            line(&lines, "L8").unwrap(),
            "L8 trims for 300 nits: dropped"
        );
    }

    #[test]
    fn clean_sample_says_nothing_else_is_dropped() {
        let authored = AuthoredLevels {
            windows: 1,
            l5_values: BTreeSet::from([Level5Offsets::default()]),
            ..Default::default()
        };
        let lines = notice(Some(&authored), &args(&[]));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("No other authored metadata"), "{lines:?}");
        assert!(lines[0].contains("1 sampled window of"), "{lines:?}");
    }

    #[test]
    fn failed_sample_gives_the_general_notice() {
        let lines = notice(None, &args(&[]));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("could not be sampled"));
        assert!(lines[0].contains("L2 trims are replaced by neutral trims"));
        assert!(lines[0].contains("L4, L8, L10 and L255 are dropped"));
        let lines = notice(None, &args(&["--cm-version", "v29"]));
        assert!(lines[0].contains("L2 trims are dropped"));
    }
}
