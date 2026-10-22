//! Synthetic HDR10+ sequences for the fixtures: payloads encoded with the `hdr10plus` crate and
//! the JSON `hdr10plus_tool extract` would write for them (`metadata_json::generate_json`).
//! Each sequence exercises one side of the scene rule `dovi_tool` inherits from
//! `hdr10plus_tool` (see `dovi_rpu_builder::hdr10plus`).

use std::fs;
use std::path::Path;

use anyhow::Result;
use hdr10plus::metadata::{
    BezierCurve, DistributionMaxRgb, Hdr10PlusMetadata, Hdr10PlusMetadataEncOpts,
};
use hdr10plus::metadata_json::generate_json;

use crate::{write_json, write_payloads};

const PERCENTAGES: [u8; 10] = [1, 5, 10, 25, 50, 75, 90, 95, 98, 99];

/// One profile A frame: MaxSCL, average maxRGB and the ten percentile values, in the
/// payload's units (tenths of a nit as `dovi_tool` reads them).
fn frame(
    maxscl: [u32; 3],
    average_maxrgb: u32,
    percentiles: [u32; 10],
    fraction_bright_pixels: u16,
) -> Hdr10PlusMetadata {
    Hdr10PlusMetadata {
        itu_t_t35_country_code: 0xB5,
        itu_t_t35_terminal_provider_code: 0x003C,
        itu_t_t35_terminal_provider_oriented_code: 0x0001,
        application_identifier: 4,
        application_version: 1,
        num_windows: 1,
        maxscl,
        average_maxrgb,
        num_distribution_maxrgb_percentiles: 10,
        distribution_maxrgb: PERCENTAGES
            .iter()
            .zip(percentiles)
            .map(|(&percentage, percentile)| DistributionMaxRgb {
                percentage,
                percentile,
            })
            .collect(),
        fraction_bright_pixels,
        ..Default::default()
    }
}

fn profile_b(mut metadata: Hdr10PlusMetadata, anchors: [u16; 9]) -> Hdr10PlusMetadata {
    metadata.targeted_system_display_maximum_luminance = 400;
    metadata.tone_mapping_flag = true;
    metadata.bezier_curve = Some(BezierCurve {
        knee_point_x: 100,
        knee_point_y: 200,
        num_bezier_curve_anchors: 9,
        bezier_curve_anchors: anchors.to_vec(),
    });
    metadata
}

/// Profile A: a held frame, a `fraction_bright_pixels`-only change (no new scene), a luminance
/// change, a change that rounds to the same L1 (new scene, same L1), and two frames below every
/// floor that clamp to the same L1 (new scene, same L1). The 98% percentile is above the 99%
/// one, so `histogram` and `histogram99` differ.
fn profile_a_sequence() -> Vec<Hdr10PlusMetadata> {
    let bright = [500, 1200, 2500, 4000, 5200, 6500, 8000, 9000, 9800, 9600];
    let mid = [100, 400, 900, 1500, 2600, 3300, 4100, 4600, 5000, 4800];
    let dark = [5, 20, 40, 80, 150, 250, 380, 450, 600, 550];
    let darker = [5, 20, 40, 80, 150, 250, 380, 450, 700, 650];
    vec![
        frame([9000, 8500, 7000], 1500, bright, 100),
        frame([9000, 8500, 7000], 1500, bright, 100),
        frame([9000, 8500, 7000], 1500, bright, 200),
        frame([5000, 4000, 3000], 800, mid, 200),
        frame([5001, 4000, 3000], 801, mid, 200),
        frame([5001, 4000, 3000], 801, mid, 200),
        frame([600, 500, 400], 10, dark, 10),
        frame([650, 500, 400], 20, darker, 10),
        frame([650, 500, 400], 20, darker, 10),
    ]
}

/// Profile B: a Bezier-only change (new scene, same L1), then a luminance change.
fn profile_b_sequence() -> Vec<Hdr10PlusMetadata> {
    let percentiles = [300, 900, 1800, 3000, 4200, 5600, 7000, 7900, 8600, 8800];
    let anchors_1 = [100, 200, 300, 400, 500, 600, 700, 800, 900];
    let anchors_2 = [110, 210, 310, 410, 510, 610, 710, 810, 910];
    let base = frame([8000, 7000, 6000], 1200, percentiles, 50);
    let other = frame([6000, 5000, 4000], 700, percentiles, 50);
    vec![
        profile_b(base.clone(), anchors_1),
        profile_b(base.clone(), anchors_1),
        profile_b(base.clone(), anchors_2),
        profile_b(base, anchors_2),
        profile_b(other, anchors_2),
    ]
}

pub fn write(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    for (name, sequence) in [
        ("profile_a", profile_a_sequence()),
        ("profile_b", profile_b_sequence()),
    ] {
        let payloads = sequence
            .iter()
            .map(|metadata| metadata.encode_with_opts(&Hdr10PlusMetadataEncOpts::default()))
            .collect::<Result<Vec<_>>>()?;
        // Re-parsed, so the crate sets each frame's profile as `hdr10plus_tool` sees it.
        let parsed = payloads
            .iter()
            .map(|payload| Hdr10PlusMetadata::parse(payload))
            .collect::<Result<Vec<_>>>()?;
        let refs: Vec<&Hdr10PlusMetadata> = parsed.iter().collect();
        write_payloads(&dir.join(format!("{name}.payloads")), &payloads)?;
        write_json(
            &dir.join(format!("{name}.json")),
            &generate_json(&refs, "hdr10plus_tool", "1.7.2"),
        )?;
    }
    Ok(())
}
