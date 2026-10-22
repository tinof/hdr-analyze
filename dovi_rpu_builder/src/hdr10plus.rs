//! HDR10+ (SMPTE ST 2094-40) metadata of one access unit → L1, as `dovi_tool generate
//! --hdr10plus-json` computes it.
//!
//! Ported from dovi_tool 2.3.4 `src/dovi/generator.rs` (the L1 arithmetic) and the `hdr10plus`
//! crate's `compute_scene_information` (the scene comparison), both Copyright (c) quietvoid,
//! MIT License; the full copyright and permission notices are in `NOTICE` at the repository root.
//!
//! # Input
//!
//! `payload` is the payload of one `user_data_registered_itu_t_t35` SEI message as an RBSP
//! (emulation prevention bytes removed), starting with the country code `0xB5`: the bytes
//! `hdr10plus_tool extract` reads. A payload without the HDR10+ T.35 identity is
//! [`Error::NotHdr10Plus`] (`hdr10plus_tool` skips such messages); the rest must pass the checks
//! `hdr10plus_tool` runs by default (`Hdr10PlusMetadata::validate`), which accept only
//! single-window application 4 version 1 payloads.
//!
//! # Scenes and holding L1
//!
//! `dovi_tool` takes L1 from the first frame of each scene `hdr10plus_tool` found, and that tool
//! starts a scene when a frame's metadata differs from the previous frame's in presentation
//! order (`compute_scene_information` in the `hdr10plus` crate's `metadata_json`). [`SceneKey`]
//! is exactly what it compares. To reproduce `dovi_tool`, keep the previous presentation-order
//! frame's key; when the key changes, take this frame's [`Hdr10PlusFrame::l1`] and write it with
//! `scene_refresh: true`, otherwise keep the held L1 and write `scene_refresh: false`. A scene can
//! start with the same L1 as the previous one (only the Bezier curve changed, or different
//! values that round or clamp to the same codes); the scene refresh still goes out. The caller
//! reorders decode order to presentation order and decides what an access unit without a
//! payload gets: this module gives it no L1. `hdr10plus_tool` (and FFmpeg) give such an access
//! unit the previous payload in decode order, before reordering, so that is what `dovi_tool`
//! sees. [`Hdr10PlusHold`] applies the rule above to frames in presentation order.
//!
//! Not covered: `--hdr10plus-l1 hybrid` (the analyzer's scene cuts and averages), and HDR10+
//! together with a reused measurement sidecar.

use dolby_vision::rpu::extension_metadata::blocks::ExtMetadataBlockLevel1;
use dolby_vision::utils::nits_to_pq_12_bit;
use hdr10plus::metadata::{Hdr10PlusMetadata, PeakBrightnessSource, VariablePeakBrightness};

use crate::builder::L1;
use crate::error::{Error, Result};
use crate::params::CmVersion;

/// ITU-T T.35 identity of an HDR10+ payload: country (United States), terminal provider
/// (Samsung), provider-oriented code, ST 2094-40 application and version.
const COUNTRY_CODE: u8 = 0xB5;
const PROVIDER_CODE: u16 = 0x003C;
const PROVIDER_ORIENTED_CODE: u16 = 0x0001;
const APPLICATION_IDENTIFIER: u8 = 4;
const APPLICATION_VERSION: u8 = 1;

/// Where the L1 maximum comes from (`dovi_tool generate --hdr10plus-peak-source`, mkvdovi
/// `--hdr10plus-peak-source`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PeakSource {
    /// The highest value of the maxRGB percentile distribution (mkvdovi's default).
    #[default]
    Histogram,
    /// The last percentile of the distribution, usually 99.98%.
    Histogram99,
    /// The largest MaxSCL component.
    MaxScl,
    /// The luminance of the MaxSCL components (BT.2020 weights).
    MaxSclLuminance,
}

impl PeakSource {
    fn to_hdr10plus(self) -> PeakBrightnessSource {
        match self {
            PeakSource::Histogram => PeakBrightnessSource::Histogram,
            PeakSource::Histogram99 => PeakBrightnessSource::Histogram99,
            PeakSource::MaxScl => PeakBrightnessSource::MaxScl,
            PeakSource::MaxSclLuminance => PeakBrightnessSource::MaxSclLuminance,
        }
    }

    fn name(self) -> &'static str {
        match self {
            PeakSource::Histogram => "histogram",
            PeakSource::Histogram99 => "histogram99",
            PeakSource::MaxScl => "max-scl",
            PeakSource::MaxSclLuminance => "max-scl-luminance",
        }
    }
}

/// The fields `hdr10plus_tool` compares between consecutive frames to start a scene:
/// `LuminanceParameters` (AverageRGB, the percentile distribution, MaxSCL), `NumberOfWindows`,
/// `TargetedSystemDisplayMaximumLuminance` and, for profile B, `BezierCurveData`. Not compared:
/// `fraction_bright_pixels`, the mastering and targeted actual-peak data, saturation weight.
///
/// `hdr10plus_tool` compares the Bezier curve only when every frame of the file is profile B;
/// this key compares it whenever the frame itself is profile B (the crate's `set_profile`). The
/// two differ only for a stream that mixes profiles.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SceneKey {
    average_maxrgb: u32,
    distribution: Vec<(u8, u32)>,
    maxscl: [u32; 3],
    num_windows: u8,
    targeted_system_display_maximum_luminance: u32,
    bezier_curve: Option<(u16, u16, Vec<u16>)>,
}

/// The T.35 identity in the first seven bytes, checked before parsing as `hdr10plus_tool` picks
/// its SEI messages (`st2094_40_sei_msg`); `Hdr10PlusMetadata::parse` and `validate` do not look
/// at the country and provider fields.
fn check_identity(payload: &[u8]) -> Result<()> {
    let [country, p0, p1, o0, o1, application, version, ..] = *payload else {
        return Err(Error::NotHdr10Plus(format!(
            "{} bytes, shorter than the 7-byte T.35 header",
            payload.len()
        )));
    };
    let identity = [
        ("country code", u32::from(country), u32::from(COUNTRY_CODE)),
        (
            "terminal provider code",
            u32::from(u16::from_be_bytes([p0, p1])),
            u32::from(PROVIDER_CODE),
        ),
        (
            "terminal provider oriented code",
            u32::from(u16::from_be_bytes([o0, o1])),
            u32::from(PROVIDER_ORIENTED_CODE),
        ),
        (
            "application identifier",
            u32::from(application),
            u32::from(APPLICATION_IDENTIFIER),
        ),
        (
            "application version",
            u32::from(version),
            u32::from(APPLICATION_VERSION),
        ),
    ];
    for (field, actual, expected) in identity {
        if actual != expected {
            return Err(Error::NotHdr10Plus(format!(
                "{field} {actual:#x}, expected {expected:#x}"
            )));
        }
    }
    Ok(())
}

/// One parsed and checked HDR10+ payload.
#[derive(Clone, Debug)]
pub struct Hdr10PlusFrame {
    metadata: Hdr10PlusMetadata,
}

impl Hdr10PlusFrame {
    /// Parse and check one payload (see the module docs for which bytes).
    pub fn parse(payload: &[u8]) -> Result<Self> {
        check_identity(payload)?;
        let metadata = Hdr10PlusMetadata::parse(payload)
            .map_err(|error| Error::Hdr10PlusParse(format!("{error:#}")))?;
        // Refuses everything but one window, among others: `parse` would overwrite MaxSCL and the
        // average per window but append every window's percentiles.
        metadata
            .validate()
            .map_err(|error| Error::Hdr10PlusParse(format!("{error:#}")))?;
        Ok(Self { metadata })
    }

    /// The scene-comparison key of this frame ([`SceneKey`]).
    pub fn scene_key(&self) -> SceneKey {
        let m = &self.metadata;
        SceneKey {
            average_maxrgb: m.average_maxrgb,
            distribution: m
                .distribution_maxrgb
                .iter()
                .map(|entry| (entry.percentage, entry.percentile))
                .collect(),
            maxscl: m.maxscl,
            num_windows: m.num_windows,
            targeted_system_display_maximum_luminance: m.targeted_system_display_maximum_luminance,
            bezier_curve: m
                .bezier_curve
                .as_ref()
                .filter(|_| m.profile == "B")
                .map(|curve| {
                    (
                        curve.knee_point_x,
                        curve.knee_point_y,
                        curve.bezier_curve_anchors.clone(),
                    )
                }),
        }
    }

    /// L1 of this frame as `dovi_tool` computes it for a scene's first frame: min 0, max the
    /// selected peak and avg the average maxRGB, both in nits rounded to whole nits, then
    /// converted to 12-bit PQ and clamped with `avg_cm_version`'s floor
    /// (`ExtMetadataBlockLevel1::from_stats_cm_version`). Use the same `avg_cm_version` as the
    /// RPU's [`RpuParams::l1_avg_pq_cm_version`](crate::RpuParams): for HDR10+ mkvdovi leaves
    /// it at the CM version.
    pub fn l1(&self, peak_source: PeakSource, avg_cm_version: CmVersion) -> Result<L1> {
        let avg_nits = f64::from(self.metadata.average_maxrgb) / 10.0;
        // `dovi_tool` unwraps this and panics.
        let max_nits = self
            .metadata
            .peak_brightness_nits(peak_source.to_hdr10plus())
            .ok_or(Error::Hdr10PlusNoPeak(peak_source.name()))?;
        let max_pq = nits_to_pq_12_bit(max_nits.round());
        let avg_pq = nits_to_pq_12_bit(avg_nits.round());
        let block = ExtMetadataBlockLevel1::from_stats_cm_version(
            0,
            max_pq,
            avg_pq,
            avg_cm_version.to_crate(),
        );
        Ok(L1::new(block.min_pq, block.avg_pq, block.max_pq))
    }
}

/// The scene rule of the module docs over frames in presentation order: returns each frame's L1
/// (held from the scene's first frame) and whether the frame starts a scene.
#[derive(Clone, Debug)]
pub struct Hdr10PlusHold {
    peak_source: PeakSource,
    avg_cm_version: CmVersion,
    held: Option<(SceneKey, L1)>,
}

impl Hdr10PlusHold {
    pub fn new(peak_source: PeakSource, avg_cm_version: CmVersion) -> Self {
        Self {
            peak_source,
            avg_cm_version,
            held: None,
        }
    }

    /// The next frame in presentation order: `(L1, scene_refresh)`. Call [`reset`](Self::reset)
    /// after a seek, so the next frame starts a scene.
    pub fn next(&mut self, frame: &Hdr10PlusFrame) -> Result<(L1, bool)> {
        let key = frame.scene_key();
        if let Some((held_key, l1)) = &self.held {
            if *held_key == key {
                return Ok((*l1, false));
            }
        }
        let l1 = frame.l1(self.peak_source, self.avg_cm_version)?;
        self.held = Some((key, l1));
        Ok((l1, true))
    }

    pub fn reset(&mut self) {
        self.held = None;
    }
}

/// L1 of one HDR10+ payload: [`Hdr10PlusFrame::parse`] then [`Hdr10PlusFrame::l1`].
pub fn hdr10plus_l1(
    payload: &[u8],
    peak_source: PeakSource,
    avg_cm_version: CmVersion,
) -> Result<L1> {
    Hdr10PlusFrame::parse(payload)?.l1(peak_source, avg_cm_version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hdr10plus::metadata::{
        BezierCurve, DistributionMaxRgb, Hdr10PlusMetadataEncOpts, ProcessingWindow,
    };

    const PERCENTAGES: [u8; 10] = [1, 5, 10, 25, 50, 75, 90, 95, 98, 99];

    fn metadata(average_maxrgb: u32, maxscl: [u32; 3], top: u32) -> Hdr10PlusMetadata {
        let mut percentiles = [100, 200, 300, 400, 500, 600, 700, 800, 900, top];
        percentiles.sort_unstable();
        Hdr10PlusMetadata {
            itu_t_t35_country_code: COUNTRY_CODE,
            itu_t_t35_terminal_provider_code: PROVIDER_CODE,
            itu_t_t35_terminal_provider_oriented_code: PROVIDER_ORIENTED_CODE,
            application_identifier: APPLICATION_IDENTIFIER,
            application_version: APPLICATION_VERSION,
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
            ..Default::default()
        }
    }

    fn encode(metadata: &Hdr10PlusMetadata, validate: bool) -> Vec<u8> {
        metadata
            .encode_with_opts(&Hdr10PlusMetadataEncOpts {
                validate,
                ..Default::default()
            })
            .unwrap()
    }

    fn frame(metadata: &Hdr10PlusMetadata) -> Hdr10PlusFrame {
        Hdr10PlusFrame::parse(&encode(metadata, true)).unwrap()
    }

    fn profile_b(mut m: Hdr10PlusMetadata, anchor: u16) -> Hdr10PlusMetadata {
        m.targeted_system_display_maximum_luminance = 400;
        m.tone_mapping_flag = true;
        m.bezier_curve = Some(BezierCurve {
            knee_point_x: 10,
            knee_point_y: 20,
            num_bezier_curve_anchors: 2,
            bezier_curve_anchors: vec![anchor, 500],
        });
        m
    }

    #[test]
    fn l1_is_dovi_tools_arithmetic() {
        // avg 100.0 nits, histogram peak 1000.0 nits, MaxSCL [1000, 900, 800] nits.
        let m = metadata(1000, [10000, 9000, 8000], 10000);
        let f = frame(&m);
        let l1 = f.l1(PeakSource::Histogram, CmVersion::V29).unwrap();
        assert_eq!(
            l1,
            L1::new(0, nits_to_pq_12_bit(100.0), nits_to_pq_12_bit(1000.0))
        );
        assert_eq!((l1.avg_pq, l1.max_pq), (2081, 3079));
        let luminance = (0.2627 * 10000.0 + 0.678 * 9000.0 + 0.0593 * 8000.0) / 10.0;
        assert_eq!(
            f.l1(PeakSource::MaxSclLuminance, CmVersion::V29)
                .unwrap()
                .max_pq,
            nits_to_pq_12_bit(f64::round(luminance))
        );
        assert_eq!(
            f.l1(PeakSource::MaxScl, CmVersion::V29).unwrap().max_pq,
            3079
        );
    }

    #[test]
    fn nits_are_rounded_before_the_pq_conversion() {
        let pq = |avg| {
            frame(&metadata(avg, [10000, 9000, 8000], 10000))
                .l1(PeakSource::Histogram, CmVersion::V29)
                .unwrap()
                .avg_pq
        };
        assert_eq!(pq(1004), nits_to_pq_12_bit(100.0));
        assert_eq!(pq(1005), nits_to_pq_12_bit(101.0));
    }

    #[test]
    fn the_avg_floor_is_the_requested_cm_versions() {
        // 5 nits: above the V29 floor (819, about 2.4 nits), below the V40 floor (1229).
        let f = frame(&metadata(50, [10000, 9000, 8000], 10000));
        let v29 = f.l1(PeakSource::Histogram, CmVersion::V29).unwrap().avg_pq;
        assert_eq!(v29, nits_to_pq_12_bit(5.0));
        assert!(v29 > 819 && v29 < 1229);
        assert_eq!(
            f.l1(PeakSource::Histogram, CmVersion::V40).unwrap().avg_pq,
            1229
        );
    }

    #[test]
    fn histogram_and_histogram99_read_different_percentiles() {
        let mut m = metadata(1000, [10000, 9000, 8000], 10000);
        // The 98% percentile above the 99% one: histogram takes the maximum, histogram99 the last.
        m.distribution_maxrgb[8].percentile = 9000;
        m.distribution_maxrgb[9].percentile = 6000;
        let f = frame(&m);
        assert_eq!(
            f.l1(PeakSource::Histogram, CmVersion::V29).unwrap().max_pq,
            nits_to_pq_12_bit(900.0)
        );
        assert_eq!(
            f.l1(PeakSource::Histogram99, CmVersion::V29)
                .unwrap()
                .max_pq,
            nits_to_pq_12_bit(600.0)
        );
    }

    #[test]
    fn each_identity_field_is_checked() {
        let good = encode(&metadata(1000, [1, 2, 3], 10000), true);
        assert!(Hdr10PlusFrame::parse(&good).is_ok());
        for (byte, value) in [(0, 0x26), (2, 0x3D), (4, 0x02), (5, 0x05), (6, 0x00)] {
            let mut bad = good.clone();
            bad[byte] = value;
            assert!(
                matches!(Hdr10PlusFrame::parse(&bad), Err(Error::NotHdr10Plus(_))),
                "byte {byte} = {value:#x} accepted"
            );
        }
        assert!(matches!(
            Hdr10PlusFrame::parse(&good[..6]),
            Err(Error::NotHdr10Plus(_))
        ));
    }

    #[test]
    fn malformed_payloads_are_refused() {
        let good = encode(&metadata(1000, [1, 2, 3], 10000), true);
        assert!(matches!(
            Hdr10PlusFrame::parse(&good[..10]),
            Err(Error::Hdr10PlusParse(_))
        ));
        let mut two_windows = metadata(1000, [1, 2, 3], 10000);
        two_windows.num_windows = 2;
        two_windows.processing_windows = Some(vec![ProcessingWindow::default()]);
        assert!(matches!(
            Hdr10PlusFrame::parse(&encode(&two_windows, false)),
            Err(Error::Hdr10PlusParse(_))
        ));
        let mut nine_percent = metadata(1000, [1, 2, 3], 10000);
        nine_percent.distribution_maxrgb[0].percentage = 2;
        assert!(Hdr10PlusFrame::parse(&encode(&nine_percent, false)).is_err());
    }

    #[test]
    fn a_missing_peak_is_an_error() {
        let mut m = metadata(1000, [1, 2, 3], 10000);
        m.distribution_maxrgb.clear();
        // Not reachable through `parse` (validation needs 9 or 10 percentiles); where dovi_tool
        // would panic, this is an error.
        let f = Hdr10PlusFrame { metadata: m };
        assert!(matches!(
            f.l1(PeakSource::Histogram, CmVersion::V29),
            Err(Error::Hdr10PlusNoPeak("histogram"))
        ));
        assert!(f.l1(PeakSource::MaxScl, CmVersion::V29).is_ok());
    }

    #[test]
    fn the_scene_key_compares_what_hdr10plus_tool_compares() {
        let base = metadata(1000, [10000, 9000, 8000], 10000);
        let key = frame(&base).scene_key();

        let mut bright_pixels = base.clone();
        bright_pixels.fraction_bright_pixels = 300;
        assert_eq!(
            frame(&bright_pixels).scene_key(),
            key,
            "fraction_bright_pixels"
        );

        let mut average = base.clone();
        average.average_maxrgb += 1;
        let mut maxscl = base.clone();
        maxscl.maxscl[2] += 1;
        let mut percentile = base.clone();
        percentile.distribution_maxrgb[3].percentile += 1;
        for (what, changed) in [
            ("average", average),
            ("maxscl", maxscl),
            ("percentile", percentile),
        ] {
            assert_ne!(frame(&changed).scene_key(), key, "{what}");
        }

        let b = frame(&profile_b(base.clone(), 100)).scene_key();
        assert_ne!(
            frame(&profile_b(base.clone(), 101)).scene_key(),
            b,
            "Bezier anchors"
        );
        let mut target = profile_b(base.clone(), 100);
        target.targeted_system_display_maximum_luminance = 500;
        assert_ne!(frame(&target).scene_key(), b, "targeted display maximum");
        assert_ne!(b, key, "profile A against B");
    }

    #[test]
    fn hold_keeps_l1_within_a_scene_and_restarts_after_reset() {
        let a = frame(&metadata(1000, [10000, 9000, 8000], 10000));
        let mut same_l1 = metadata(1001, [10000, 9000, 8000], 10000);
        same_l1.fraction_bright_pixels = 0;
        let b = frame(&same_l1);
        let mut hold = Hdr10PlusHold::new(PeakSource::Histogram, CmVersion::V40);
        let (l1, refresh) = hold.next(&a).unwrap();
        assert!(refresh);
        assert_eq!(hold.next(&a).unwrap(), (l1, false));
        // 100.1 nits rounds like 100.0: a new scene with the same L1.
        assert_eq!(hold.next(&b).unwrap(), (l1, true));
        hold.reset();
        assert_eq!(hold.next(&b).unwrap(), (l1, true));
    }
}
