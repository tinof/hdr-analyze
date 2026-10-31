//! `--hdr10plus-l1 hybrid`: Profile 8.1 L1 for an HDR10+ input from both the HDR10+ metadata and
//! the analyzer's measurement. The scene list is the union of the HDR10+ scene starts and the
//! analyzer's cuts; each piece takes its peak from HDR10+ and its average and minimum from the
//! measured frames (ROADMAP P9, rule chosen in `docs/ROADMAP_LOG.md` 2026-10-13).
//!
//! The result is the analyzer sidecar with its scenes replaced by the pieces, in measured frames,
//! so `generate_extra_json` and `--verify` treat it like any measured sidecar: both add the
//! undecodable leading pictures themselves.

use std::fs::File;
use std::path::Path;

use dolby_vision::rpu::extension_metadata::blocks::level1::L1_MAX_PQ_MIN_VALUE;
use dolby_vision::utils::nits_to_pq_12_bit;
use serde_json::Value;

use super::hdr10plus::hdr10plus_peak_nits;
use crate::cli::PeakSource;
use crate::metadata::{
    L1Sidecar, L1SidecarScene, L1_SIDECAR_FRAME_PEAK_VERSION, L1_SIDECAR_STREAM_FRAMES_VERSION,
};
use crate::progress;

/// Fallback lines printed one by one before the rest is summarized.
const MAX_FALLBACK_LINES: usize = 10;

/// More pieces than this many per analyzer scene means very short HDR10+ scenes, where L1 can
/// change every few frames.
const DENSE_PIECES_PER_SCENE: usize = 3;

/// Why a piece's peak comes from the pixels instead of the HDR10+ metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FallbackReason {
    /// The selected `--peak-source` statistic is absent from the HDR10+ frame.
    Missing,
    /// The statistic is 0 nits, as MaxSCL is on every frame of HDR10+ Profile A discs.
    Zero,
    /// The statistic is outside the PQ range.
    Malformed,
    /// The HDR10+ peak, after the generator's 100-nit floor, does not exceed the measured
    /// average. `dovi_tool` would then lower the average to `max - 1`.
    BelowAverage,
}

impl FallbackReason {
    fn describe(self) -> &'static str {
        match self {
            Self::Missing => "no HDR10+ peak statistic",
            Self::Zero => "HDR10+ peak is 0",
            Self::Malformed => "HDR10+ peak outside the PQ range",
            Self::BelowAverage => "HDR10+ peak not above the measured average",
        }
    }
}

/// A piece whose peak is the maximum of its own measured frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Fallback {
    /// First and last stream frame of the piece (the first piece also covers leading pictures).
    pub(super) first_frame: u64,
    pub(super) last_frame: u64,
    pub(super) reason: FallbackReason,
}

/// What the hybrid did, for the progress output.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct HybridReport {
    pub(super) pieces: usize,
    pub(super) hdr10plus_scenes: usize,
    pub(super) analyzer_scenes: usize,
    pub(super) fallbacks: Vec<Fallback>,
}

impl HybridReport {
    pub(super) fn print(&self, peak_source: PeakSource) {
        let count = |reason| {
            self.fallbacks
                .iter()
                .filter(|fallback| fallback.reason == reason)
                .count()
        };
        progress::print_info(&format!(
            "HDR10+ hybrid L1: {} pieces ({} HDR10+ scenes ∪ {} analyzer cuts); peak from HDR10+ ({peak_source}) on {}; pixel fallback on {} (missing {}, zero {}, malformed {}, below average {}). Average and minimum are measured. --verify reports the hybrid peak as the measured one.",
            self.pieces,
            self.hdr10plus_scenes,
            self.analyzer_scenes,
            self.pieces - self.fallbacks.len(),
            self.fallbacks.len(),
            count(FallbackReason::Missing),
            count(FallbackReason::Zero),
            count(FallbackReason::Malformed),
            count(FallbackReason::BelowAverage),
        ));
        for fallback in self.fallbacks.iter().take(MAX_FALLBACK_LINES) {
            progress::print_warn(&format!(
                "HDR10+ hybrid: frames {}-{} take the measured peak of their own frames ({}).",
                fallback.first_frame,
                fallback.last_frame,
                fallback.reason.describe()
            ));
        }
        if self.fallbacks.len() > MAX_FALLBACK_LINES {
            progress::print_warn(&format!(
                "HDR10+ hybrid: ... and {} more pieces with their own measured peak.",
                self.fallbacks.len() - MAX_FALLBACK_LINES
            ));
        }
        if self.pieces > DENSE_PIECES_PER_SCENE * self.analyzer_scenes.max(1) {
            progress::print_warn(&format!(
                "HDR10+ hybrid: {} pieces for {} analyzer scenes; the HDR10+ scenes are short, so L1 can change every few frames.",
                self.pieces, self.analyzer_scenes
            ));
        }
    }
}

/// Whether a sidecar can carry the hybrid's average, minimum and fallback peak: every frame
/// measured (sample rate 1), per-frame arrays for all of them, a max-RGB peak domain, the `max`
/// estimator and the analyzer's `max` peak source, so a frame peak is a direct maximum, a PQ
/// mapping, the stream frame count (version 5) for the HDR10+ match, and the per-frame peak and
/// peak source (version 6) for the fallback.
///
/// A direct maximum can still miss a small highlight under `--downscale 2`, which is what `auto`
/// resolves to on the CPU, or `--pre-denoise median3`, which the sidecar reader does not see.
pub(super) fn check_measurements(sidecar: &L1Sidecar) -> Result<(), String> {
    if sidecar.version < L1_SIDECAR_STREAM_FRAMES_VERSION
        || sidecar
            .source
            .as_ref()
            .and_then(|source| source.stream_frames)
            .is_none()
    {
        return Err(format!(
            "sidecar v{} does not record the stream frame count (needs v{L1_SIDECAR_STREAM_FRAMES_VERSION})",
            sidecar.version
        ));
    }
    if sidecar.version < L1_SIDECAR_FRAME_PEAK_VERSION {
        return Err(format!(
            "sidecar v{} has no per-frame peak and no analyzer peak source (needs v{L1_SIDECAR_FRAME_PEAK_VERSION}, from an hdr_analyzer_mvp as new as mkvdovi)",
            sidecar.version
        ));
    }
    let Some(analysis) = &sidecar.analysis else {
        return Err("the sidecar does not record its sampling".into());
    };
    if analysis.sample_rate != 1 {
        return Err(format!(
            "the analysis skipped frames (sample-rate {})",
            analysis.sample_rate
        ));
    }
    if sidecar.luminance_mapping() != Some("pq") {
        return Err(format!(
            "the luminance mapping is {}, not pq",
            sidecar.luminance_mapping().unwrap_or("not recorded")
        ));
    }
    if let Some(domain) = sidecar.peak_domain.as_deref().filter(|&d| d != "max-rgb") {
        return Err(format!("the peak domain is {domain}, not max-rgb"));
    }
    if let Some(estimator) = sidecar.peak_estimator.as_deref().filter(|&e| e != "max") {
        return Err(format!("the peak estimator is {estimator}, not max"));
    }
    match sidecar.peak_source.as_deref() {
        Some("max") => {}
        source => {
            return Err(format!(
                "the analyzer's peak source is {}, not max",
                source.unwrap_or("not recorded")
            ))
        }
    }
    let frames = sidecar.frame_count();
    let Some(per_frame) = &sidecar.frames else {
        return Err("the sidecar has no per-frame values".into());
    };
    for (name, len) in [
        ("minimum", per_frame.min_pq_12bit.len()),
        ("max-RGB average", per_frame.avg_max_rgb_pq_12bit.len()),
        ("maximum", per_frame.max_pq_12bit.len()),
    ] {
        if len as u64 != frames {
            return Err(format!(
                "scenes cover {frames} frames but the per-frame {name} has {len}"
            ));
        }
    }
    Ok(())
}

/// Reads the extracted HDR10+ metadata and returns the sidecar with the hybrid pieces as its
/// scenes.
pub(super) fn apply_file(
    sidecar: L1Sidecar,
    hdr10plus_json: &Path,
    peak_source: PeakSource,
) -> Result<(L1Sidecar, HybridReport), String> {
    let metadata: Value = File::open(hdr10plus_json)
        .map_err(|error| error.to_string())
        .and_then(|file| serde_json::from_reader(file).map_err(|error| error.to_string()))
        .map_err(|error| {
            format!(
                "cannot read the HDR10+ metadata '{}': {error}",
                hdr10plus_json.display()
            )
        })?;
    apply(sidecar, &metadata, peak_source)
}

/// [`apply_file`] on parsed HDR10+ metadata.
pub(super) fn apply(
    mut sidecar: L1Sidecar,
    metadata: &Value,
    peak_source: PeakSource,
) -> Result<(L1Sidecar, HybridReport), String> {
    check_measurements(&sidecar)?;
    let (scene_info, starts) = hdr10plus_scenes(metadata, sidecar.stream_frame_count())?;
    let peaks: Vec<Option<f64>> = starts
        .iter()
        .map(|&start| hdr10plus_peak_nits(&scene_info[start as usize], peak_source))
        .collect();
    let frames = sidecar.frames.as_ref().ok_or("no per-frame values")?;
    let (pieces, report) = build_pieces(
        &sidecar.scenes,
        &frames.avg_max_rgb_pq_12bit,
        &frames.min_pq_12bit,
        &frames.max_pq_12bit,
        sidecar.leading_frames(),
        &starts,
        &peaks,
    );
    sidecar.scenes = pieces;
    Ok((sidecar, report))
}

/// The HDR10+ frames and scene starts (stream frames), after the structural checks the hybrid
/// relies on. `dovi_tool` tolerates a list that starts after frame 0 by shifting it; the hybrid
/// maps HDR10+ frame `n` to picture `n` and refuses that instead.
fn hdr10plus_scenes(
    metadata: &Value,
    stream_frames: u64,
) -> Result<(&Vec<Value>, Vec<u64>), String> {
    let scene_info = metadata
        .get("SceneInfo")
        .and_then(Value::as_array)
        .ok_or("the HDR10+ metadata has no SceneInfo")?;
    let starts = metadata
        .pointer("/SceneInfoSummary/SceneFirstFrameIndex")
        .and_then(Value::as_array)
        .ok_or("the HDR10+ metadata has no SceneInfoSummary.SceneFirstFrameIndex")?
        .iter()
        .map(|start| {
            start
                .as_u64()
                .ok_or("an HDR10+ scene start is not an integer")
        })
        .collect::<Result<Vec<u64>, _>>()?;
    match starts.first() {
        None => return Err("the HDR10+ metadata lists no scenes".into()),
        Some(&first) if first != 0 => {
            return Err(format!(
                "the first HDR10+ scene starts at frame {first}, not 0"
            ))
        }
        Some(_) => {}
    }
    let frames = scene_info.len() as u64;
    if let Some(pair) = starts.windows(2).find(|pair| pair[1] <= pair[0]) {
        return Err(format!(
            "HDR10+ scene starts {} and {} are not increasing",
            pair[0], pair[1]
        ));
    }
    if let Some(&last) = starts.last().filter(|&&last| last >= frames) {
        return Err(format!(
            "an HDR10+ scene starts at frame {last}, past the {frames} frames of metadata"
        ));
    }
    if let Some(lengths) = metadata
        .pointer("/SceneInfoSummary/SceneFrameNumbers")
        .and_then(Value::as_array)
    {
        let lengths: Vec<u64> = lengths.iter().filter_map(Value::as_u64).collect();
        let ends = starts.iter().skip(1).copied().chain([frames]);
        let consistent = lengths.len() == starts.len()
            && starts
                .iter()
                .zip(ends)
                .zip(&lengths)
                .all(|((start, end), length)| end - start == *length);
        if !consistent {
            return Err("HDR10+ SceneFrameNumbers do not match the scene starts".into());
        }
    }
    if frames != stream_frames {
        return Err(format!(
            "the HDR10+ metadata has {frames} frames but the video stream has {stream_frames}"
        ));
    }
    Ok((scene_info, starts))
}

/// The union pieces, in measured frames.
///
/// `scenes` are the analyzer's scenes (contiguous from frame 0, validated); `avg`, `min` and
/// `peak` are per measured frame; `leading` is the count of undecodable pictures before
/// measured frame 0; `hdr10plus_starts` are stream frames with the selected peak of each scene's
/// first frame in `hdr10plus_peaks` (nits).
///
/// A piece's average is the mean of its frame codes rounded half away from zero and its minimum
/// the smallest frame code (the pre-registered rule). Its peak is the HDR10+ peak of the scene
/// that holds its first stream frame, rounded to whole nits and converted as `dovi_tool` does,
/// or, per [`FallbackReason`], the largest of its own frame peaks.
pub(super) fn build_pieces(
    scenes: &[L1SidecarScene],
    avg: &[u16],
    min: &[u16],
    peak: &[u16],
    leading: u64,
    hdr10plus_starts: &[u64],
    hdr10plus_peaks: &[Option<f64>],
) -> (Vec<L1SidecarScene>, HybridReport) {
    let frames = scenes.last().map_or(0, |scene| scene.end + 1);
    let mut cuts: Vec<u64> = scenes.iter().map(|scene| scene.start).collect();
    // HDR10+ starts at or before the first measured picture fold into the first piece.
    cuts.extend(
        hdr10plus_starts
            .iter()
            .filter(|&&start| start > leading)
            .map(|&start| start - leading),
    );
    cuts.push(0);
    cuts.sort_unstable();
    cuts.dedup();

    let mut report = HybridReport {
        pieces: cuts.len(),
        hdr10plus_scenes: hdr10plus_starts.len(),
        analyzer_scenes: scenes.len(),
        fallbacks: Vec::new(),
    };
    let mut pieces = Vec::with_capacity(cuts.len());
    for (index, &start) in cuts.iter().enumerate() {
        let end = cuts.get(index + 1).map_or(frames, |&next| next) - 1;
        let range = start as usize..=end as usize;
        let piece_avg = mean_code(&avg[range.clone()]);
        let piece_min = min[range.clone()].iter().copied().min().unwrap_or(0);

        // Every HDR10+ start past the leading pictures is a cut, so the piece lies inside one
        // HDR10+ scene.
        let hdr10plus = hdr10plus_starts.partition_point(|&s| s <= start + leading) - 1;
        let max = match hdr10plus_peak_code(hdr10plus_peaks[hdr10plus], piece_avg) {
            Ok(code) => code,
            Err(reason) => {
                report.fallbacks.push(Fallback {
                    first_frame: if index == 0 { 0 } else { start + leading },
                    last_frame: end + leading,
                    reason,
                });
                peak[range].iter().copied().max().unwrap_or(0)
            }
        };
        pieces.push(L1SidecarScene {
            start,
            end,
            min_pq_12bit: piece_min,
            // Not read downstream; the RPU average is the max-RGB mean.
            avg_luma_pq_12bit: 0,
            avg_max_rgb_pq_12bit: piece_avg,
            max_pq_12bit: max,
        });
    }
    (pieces, report)
}

/// The 12-bit PQ code `dovi_tool generate --hdr10plus-json` writes for an HDR10+ peak (whole
/// nits first, `generator.rs` in dovi_tool 2.3.4), or why the piece falls back.
fn hdr10plus_peak_code(peak_nits: Option<f64>, piece_avg: u16) -> Result<u16, FallbackReason> {
    let nits = peak_nits.ok_or(FallbackReason::Missing)?;
    if !nits.is_finite() || !(0.0..=10_000.0).contains(&nits) {
        return Err(FallbackReason::Malformed);
    }
    let nits = nits.round();
    if nits == 0.0 {
        return Err(FallbackReason::Zero);
    }
    let code = nits_to_pq_12_bit(nits);
    if code.max(L1_MAX_PQ_MIN_VALUE) <= piece_avg {
        return Err(FallbackReason::BelowAverage);
    }
    Ok(code)
}

/// Mean of 12-bit codes rounded half away from zero, in integers.
fn mean_code(codes: &[u16]) -> u16 {
    let n = codes.len() as u64;
    if n == 0 {
        return 0;
    }
    let sum: u64 = codes.iter().map(|&code| u64::from(code)).sum();
    ((2 * sum + n) / (2 * n)) as u16
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::metadata::{
        validate_l1_sidecar, L1SidecarAnalysis, L1SidecarCrop, L1SidecarFrames, L1SidecarSource,
        SidecarExpectation,
    };

    fn scene(start: u64, end: u64, max: u16) -> L1SidecarScene {
        L1SidecarScene {
            start,
            end,
            min_pq_12bit: 0,
            avg_luma_pq_12bit: 0,
            avg_max_rgb_pq_12bit: 0,
            max_pq_12bit: max,
        }
    }

    /// Fixture F: 10 measured frames, analyzer scenes [0-5] and [6-9].
    fn fixture_scenes() -> Vec<L1SidecarScene> {
        vec![scene(0, 5, 2500), scene(6, 9, 2600)]
    }
    const AVG: [u16; 10] = [800, 801, 800, 801, 900, 901, 1000, 1000, 1001, 1002];
    const MIN: [u16; 10] = [5, 3, 7, 9, 20, 2, 11, 11, 11, 10];
    /// Frame peaks whose maximum per scene is the scene's peak in [`fixture_scenes`].
    const MAX: [u16; 10] = [2300, 2290, 2305, 2310, 2500, 2420, 2600, 2550, 2510, 2580];

    fn spans(pieces: &[L1SidecarScene]) -> Vec<(u64, u64)> {
        pieces
            .iter()
            .map(|piece| (piece.start, piece.end))
            .collect()
    }

    fn maxima(pieces: &[L1SidecarScene]) -> Vec<u16> {
        pieces.iter().map(|piece| piece.max_pq_12bit).collect()
    }

    #[test]
    fn pieces_are_the_union_with_measured_average_and_minimum() {
        let (pieces, report) = build_pieces(
            &fixture_scenes(),
            &AVG,
            &MIN,
            &MAX,
            0,
            &[0, 4],
            &[Some(100.0), Some(1000.0)],
        );
        assert_eq!(spans(&pieces), [(0, 3), (4, 5), (6, 9)]);
        // 800.5, 900.5 and 1000.75 round half away from zero.
        let averages: Vec<u16> = pieces.iter().map(|p| p.avg_max_rgb_pq_12bit).collect();
        assert_eq!(averages, [801, 901, 1001]);
        let minima: Vec<u16> = pieces.iter().map(|p| p.min_pq_12bit).collect();
        assert_eq!(minima, [3, 2, 10]);
        assert_eq!(maxima(&pieces), [2081, 3079, 3079]);
        assert_eq!(
            report,
            HybridReport {
                pieces: 3,
                hdr10plus_scenes: 2,
                analyzer_scenes: 2,
                fallbacks: Vec::new(),
            }
        );
    }

    #[test]
    fn hdr10plus_peaks_are_rounded_to_whole_nits_like_dovi_tool() {
        // The plain conversion of 130.4 nits is 2191; dovi_tool writes PQ(130) = 2189.
        assert_eq!(hdr10plus_peak_code(Some(130.4), 0), Ok(2189));
        assert_eq!(hdr10plus_peak_code(Some(116.5), 0), Ok(2145));
        assert_eq!(hdr10plus_peak_code(Some(116.7), 0), Ok(2145));
    }

    #[test]
    fn missing_zero_and_malformed_peaks_take_the_measured_maximum_of_the_piece() {
        for (peaks, reason) in [
            ([None, None], FallbackReason::Missing),
            ([Some(0.0), Some(0.4)], FallbackReason::Zero),
            ([Some(10_000.1), Some(f64::NAN)], FallbackReason::Malformed),
        ] {
            let (pieces, report) =
                build_pieces(&fixture_scenes(), &AVG, &MIN, &MAX, 0, &[0, 4], &peaks);
            // Piece 0 peaks on its last frame, piece 1 on its first: neither is the 2500 of the
            // analyzer scene that holds piece 0.
            assert_eq!(maxima(&pieces), [2310, 2500, 2600], "{reason:?}");
            let reasons: Vec<_> = report.fallbacks.iter().map(|f| f.reason).collect();
            assert_eq!(reasons, [reason; 3]);
            let frames: Vec<_> = report
                .fallbacks
                .iter()
                .map(|f| (f.first_frame, f.last_frame))
                .collect();
            assert_eq!(frames, [(0, 3), (4, 5), (6, 9)]);
        }
    }

    #[test]
    fn the_fallback_peak_covers_exactly_the_frames_of_the_piece() {
        let none = [None, None];
        // A cut at 8 splits the second analyzer scene: 2600 stays with piece (6, 7).
        let (pieces, _) = build_pieces(&fixture_scenes(), &AVG, &MIN, &MAX, 0, &[0, 8], &none);
        assert_eq!(spans(&pieces), [(0, 5), (6, 7), (8, 9)]);
        assert_eq!(maxima(&pieces), [2500, 2600, 2580]);

        // A piece that is a whole analyzer scene keeps that scene's peak.
        let (pieces, _) = build_pieces(&fixture_scenes(), &AVG, &MIN, &MAX, 0, &[0], &[None]);
        let scene_peaks: Vec<u16> = fixture_scenes().iter().map(|s| s.max_pq_12bit).collect();
        assert_eq!(maxima(&pieces), scene_peaks);
    }

    #[test]
    fn the_fallback_peak_reads_measured_frames_past_the_leading_pictures() {
        // Two leading pictures: the cut at stream frame 6 is measured frame 4. Reading the peaks
        // in stream frames would give piece (0, 3) the 2500 of measured frame 4.
        let (pieces, report) = build_pieces(
            &fixture_scenes(),
            &AVG,
            &MIN,
            &MAX,
            2,
            &[0, 2, 6],
            &[None, None, None],
        );
        assert_eq!(spans(&pieces), [(0, 3), (4, 5), (6, 9)]);
        assert_eq!(maxima(&pieces), [2310, 2500, 2600]);
        let frames: Vec<_> = report
            .fallbacks
            .iter()
            .map(|f| (f.first_frame, f.last_frame))
            .collect();
        assert_eq!(frames, [(0, 5), (6, 7), (8, 11)]);
    }

    #[test]
    fn a_peak_that_would_lower_the_measured_average_falls_back() {
        // 2081 is the generator's floor for the maximum: an HDR10+ peak of 1 nit is written as
        // 2081 and still lies above an average of 801, so only the floored value counts.
        let (pieces, report) = build_pieces(
            &fixture_scenes(),
            &AVG,
            &MIN,
            &MAX,
            0,
            &[0, 4],
            &[Some(1.0), Some(1000.0)],
        );
        assert_eq!(maxima(&pieces), [614, 3079, 3079]);
        assert!(report.fallbacks.is_empty());

        let bright = [2100, 2100, 2100, 2100, 2081, 2081, 2081, 2081, 2081, 2081];
        let (pieces, report) = build_pieces(
            &fixture_scenes(),
            &bright,
            &MIN,
            &MAX,
            0,
            &[0, 4],
            &[Some(100.0), Some(100.0)],
        );
        // Piece 0: average 2100 above the floored peak 2081. Pieces 1 and 2: average 2081 equal
        // to it, which the generator would also lower to 2080.
        assert_eq!(maxima(&pieces), [2310, 2500, 2600]);
        assert!(report
            .fallbacks
            .iter()
            .all(|f| f.reason == FallbackReason::BelowAverage));
        assert_eq!(report.fallbacks.len(), 3);
    }

    #[test]
    fn hdr10plus_starts_are_shifted_past_the_leading_pictures() {
        // Two leading pictures: HDR10+ starts 0 and 2 fold into the first piece, which takes the
        // peak of the scene holding stream frame 2; start 6 is measured frame 4.
        let (pieces, report) = build_pieces(
            &fixture_scenes(),
            &AVG,
            &MIN,
            &MAX,
            2,
            &[0, 2, 6],
            &[None, Some(100.0), Some(1000.0)],
        );
        assert_eq!(spans(&pieces), [(0, 3), (4, 5), (6, 9)]);
        assert_eq!(maxima(&pieces), [2081, 3079, 3079]);
        assert!(report.fallbacks.is_empty());
    }

    fn metadata(starts: &[u64], frames: usize, peak_tenths: u64) -> Value {
        let frame = json!({
            "LuminanceParameters": {
                "AverageRGB": 100,
                "MaxScl": [0, 0, 0],
                "LuminanceDistributions": {
                    "DistributionIndex": [1, 5, 10, 25, 50, 75, 90, 95, 99],
                    "DistributionValues": [10, 20, 30, 40, 50, 60, 70, 80, peak_tenths],
                },
            },
        });
        let lengths: Vec<u64> = starts
            .iter()
            .zip(starts.iter().skip(1).copied().chain([frames as u64]))
            .map(|(start, end)| end - start)
            .collect();
        json!({
            "SceneInfo": vec![frame; frames],
            "SceneInfoSummary": {
                "SceneFirstFrameIndex": starts,
                "SceneFrameNumbers": lengths,
            },
        })
    }

    fn fixture_sidecar(leading: u64) -> L1Sidecar {
        L1Sidecar {
            version: 6,
            scenes: fixture_scenes(),
            source: Some(L1SidecarSource {
                file_name: "clip.mkv".into(),
                size_bytes: 1,
                width: 1920,
                height: 1080,
                transfer_function: None,
                stream_frames: Some(10 + leading),
                leading_skipped_frames: Some(leading),
            }),
            analysis: Some(L1SidecarAnalysis {
                downscale: 1,
                sample_rate: 1,
                gpu: false,
                no_crop: false,
                luminance_mapping: Some("pq".into()),
            }),
            crop: Some(L1SidecarCrop {
                x: 0,
                y: 140,
                width: 1920,
                height: 800,
            }),
            frames: Some(L1SidecarFrames {
                min_pq_12bit: MIN.to_vec(),
                avg_max_rgb_pq_12bit: AVG.to_vec(),
                max_pq_12bit: MAX.to_vec(),
            }),
            peak_domain: Some("max-rgb".into()),
            peak_estimator: Some("max".into()),
            peak_source: Some("max".into()),
            ..Default::default()
        }
    }

    #[test]
    fn apply_replaces_only_the_scenes_and_stays_a_valid_sidecar() {
        let sidecar = fixture_sidecar(0);
        assert_eq!(check_measurements(&sidecar), Ok(()));
        let (hybrid, report) = apply(
            sidecar,
            &metadata(&[0, 4], 10, 10_000),
            PeakSource::Histogram,
        )
        .unwrap();
        assert_eq!(spans(&hybrid.scenes), [(0, 3), (4, 5), (6, 9)]);
        assert_eq!(maxima(&hybrid.scenes), [3079, 3079, 3079]);
        assert_eq!(report.pieces, 3);
        assert!(hybrid.crop.is_some());
        assert_eq!(hybrid.frames.as_ref().unwrap().min_pq_12bit, MIN);
        assert_eq!(hybrid.stream_frame_count(), 10);
        assert!(validate_l1_sidecar(&hybrid, &SidecarExpectation::default()).is_ok());

        // MaxScl is 0 on every frame (HDR10+ Profile A): every piece falls back.
        let (hybrid, report) = apply(
            fixture_sidecar(0),
            &metadata(&[0, 4], 10, 10_000),
            PeakSource::MaxScl,
        )
        .unwrap();
        assert_eq!(maxima(&hybrid.scenes), [2310, 2500, 2600]);
        assert_eq!(report.fallbacks.len(), 3);
    }

    #[test]
    fn hybrid_shots_cover_the_leading_pictures_in_extra_json() {
        let (hybrid, _) = apply(
            fixture_sidecar(2),
            &metadata(&[0, 2, 6], 12, 10_000),
            PeakSource::Histogram,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extra.json");
        let static_meta = std::collections::HashMap::from([
            ("min_dml".to_string(), 0.005),
            ("max_dml".to_string(), 1000.0),
            ("max_cll".to_string(), 1000.0),
            ("max_fall".to_string(), 400.0),
        ]);
        crate::metadata::generate_extra_json(
            &path,
            "8.1",
            &static_meta,
            &[],
            None,
            None,
            Some(&hybrid),
        )
        .unwrap();
        let extra: Value = serde_json::from_reader(File::open(&path).unwrap()).unwrap();
        assert_eq!(extra["length"], 12);
        let shots: Vec<(u64, u64)> = extra["shots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|shot| {
                (
                    shot["start"].as_u64().unwrap(),
                    shot["duration"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(shots, [(0, 6), (6, 2), (8, 4)]);
    }

    #[test]
    fn malformed_hdr10plus_scene_lists_are_refused() {
        let refused = |metadata: Value, needle: &str| {
            let error = apply(fixture_sidecar(0), &metadata, PeakSource::Histogram).unwrap_err();
            assert!(error.contains(needle), "{error}");
        };
        refused(json!({}), "no SceneInfo");
        let mut no_summary = metadata(&[0, 4], 10, 10_000);
        no_summary
            .as_object_mut()
            .unwrap()
            .remove("SceneInfoSummary");
        refused(no_summary, "SceneFirstFrameIndex");
        refused(metadata(&[], 10, 10_000), "no scenes");
        refused(metadata(&[3, 4], 10, 10_000), "starts at frame 3, not 0");
        let mut decreasing = metadata(&[0, 4], 10, 10_000);
        decreasing["SceneInfoSummary"]["SceneFirstFrameIndex"] = json!([0, 6, 4]);
        refused(decreasing, "not increasing");
        let mut past_end = metadata(&[0, 4], 10, 10_000);
        past_end["SceneInfoSummary"]["SceneFirstFrameIndex"] = json!([0, 10]);
        refused(past_end, "past the 10 frames");
        let mut lengths = metadata(&[0, 4], 10, 10_000);
        lengths["SceneInfoSummary"]["SceneFrameNumbers"] = json!([5, 5]);
        refused(lengths, "SceneFrameNumbers");
        // A length that would overflow `start + length` is refused, not a panic.
        let mut huge = metadata(&[0, 4], 10, 10_000);
        huge["SceneInfoSummary"]["SceneFrameNumbers"] = json!([4, u64::MAX]);
        refused(huge, "SceneFrameNumbers");
        let mut not_integer = metadata(&[0, 4], 10, 10_000);
        not_integer["SceneInfoSummary"]["SceneFirstFrameIndex"] = json!([0, "4"]);
        refused(not_integer, "not an integer");
        refused(
            metadata(&[0, 4], 11, 10_000),
            "11 frames but the video stream has 10",
        );
    }

    #[test]
    fn measurements_that_cannot_carry_the_hybrid_are_named() {
        let rejected = |change: fn(&mut L1Sidecar), needle: &str| {
            let mut sidecar = fixture_sidecar(0);
            change(&mut sidecar);
            let error = check_measurements(&sidecar).unwrap_err();
            assert!(error.contains(needle), "{error}");
        };
        rejected(|s| s.version = 4, "stream frame count");
        rejected(|s| s.version = 5, "needs v6");
        rejected(
            |s| s.analysis.as_mut().unwrap().sample_rate = 3,
            "sample-rate 3",
        );
        rejected(
            |s| s.analysis.as_mut().unwrap().luminance_mapping = Some("dovi84-v3".into()),
            "dovi84-v3",
        );
        rejected(
            |s| s.peak_domain = Some("luma".into()),
            "peak domain is luma",
        );
        rejected(
            |s| s.peak_estimator = Some("robust".into()),
            "peak estimator is robust",
        );
        rejected(
            |s| s.peak_estimator = Some("percentile".into()),
            "peak estimator is percentile",
        );
        rejected(
            |s| s.peak_source = Some("histogram99".into()),
            "peak source is histogram99",
        );
        rejected(
            |s| s.peak_source = Some("histogram999".into()),
            "peak source is histogram999",
        );
        rejected(|s| s.peak_source = None, "peak source is not recorded");
        rejected(|s| s.frames = None, "no per-frame values");
        rejected(
            |s| s.frames.as_mut().unwrap().avg_max_rgb_pq_12bit.clear(),
            "per-frame max-RGB average has 0",
        );
        rejected(
            |s| {
                s.frames.as_mut().unwrap().min_pq_12bit.pop();
            },
            "per-frame minimum has 9",
        );
        rejected(
            |s| {
                s.frames.as_mut().unwrap().max_pq_12bit.pop();
            },
            "per-frame maximum has 9",
        );
    }
}
