use dovi84_composer::Composer;
use serde::Deserialize;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use crate::rpu_check::Level5Offsets;

/// Existing measurements files for `input_file`, most specific first. The analyzer's own output
/// (`<stem>_measurements.bin`) leads and the shared `measurements.bin` comes last, so a stale
/// file from another title cannot shadow the valid one.
pub fn measurements_candidates(input_file: &Path) -> Vec<PathBuf> {
    let dir = input_file.parent().unwrap_or(Path::new("."));
    let (Some(stem), Some(name)) = (input_file.file_stem(), input_file.file_name()) else {
        return Vec::new();
    };
    let stem = stem.to_string_lossy();
    let name = name.to_string_lossy();

    let candidates = [
        dir.join(format!("{stem}_measurements.bin")),
        dir.join(format!("{stem}.measurements")),
        dir.join(format!("{name}.measurements")),
        input_file.with_extension("mkv.measurements"),
        dir.join("measurements.bin"),
    ];

    let mut found: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        if candidate.exists() && !found.contains(&candidate) {
            found.push(candidate);
        }
    }
    found
}

/// The most specific existing measurements file for `input_file`.
pub fn find_measurements_file(input_file: &Path) -> Option<PathBuf> {
    measurements_candidates(input_file).into_iter().next()
}

pub fn find_details_file(input_file: &Path) -> Option<PathBuf> {
    let dir = input_file.parent().unwrap_or(Path::new("."));
    let stem = input_file.file_stem()?.to_string_lossy();

    let candidates = [
        dir.join(format!("{}_mkv_Details.txt", stem)),
        dir.join(format!("{}_Details.txt", stem)),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Some(candidate.clone());
        }
    }
    None
}

/// Per-scene L1 statistics from the analyzer's `<measurements>.l1.json` sidecar.
/// Versions 1 to 5 are accepted; version 2 adds provenance and a full-resolution crop,
/// version 3 adds `analysis.luminance_mapping` (`"pq"` or a [`Composer::luminance_mapping`] name),
/// version 4 stores unfiltered averages (same layout), and version 5 records the stream's
/// picture count and the undecodable leading pictures before the first measured frame.
#[derive(Debug, Default, Deserialize)]
pub struct L1Sidecar {
    pub version: u32,
    pub scenes: Vec<L1SidecarScene>,
    #[serde(default)]
    pub analyzer_version: Option<String>,
    #[serde(default)]
    pub source: Option<L1SidecarSource>,
    #[serde(default)]
    pub analysis: Option<L1SidecarAnalysis>,
    #[serde(default)]
    pub crop_space: Option<String>,
    #[serde(default)]
    pub crop: Option<L1SidecarCrop>,
    #[serde(default)]
    pub frames: Option<L1SidecarFrames>,
    /// Content MaxCLL / MaxFALL measured by the analyzer (max-RGB runs of newer version 4
    /// analyzers only).
    #[serde(default)]
    pub light_level: Option<L1SidecarLightLevel>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct L1SidecarLightLevel {
    pub max_cll_nits: u32,
    pub max_fall_nits: u32,
}

/// The sidecar light levels that are safe to put into L6.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MeasuredLightLevels {
    pub max_cll: Option<u32>,
    pub max_fall: Option<u32>,
    /// Why one of the two is withheld.
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct L1SidecarSource {
    pub file_name: String,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    /// Transfer function the analyzer detected, e.g. `"HLG (ARIB STD-B67)"` (version 2+).
    #[serde(default)]
    pub transfer_function: Option<String>,
    /// Coded pictures in the video stream (version 5+).
    #[serde(default)]
    pub stream_frames: Option<u64>,
    /// RASL pictures at the start of an open-GOP cut that no decoder outputs (version 5+).
    /// Measured frame `i` is stream frame `i + leading_skipped_frames`.
    #[serde(default)]
    pub leading_skipped_frames: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct L1SidecarAnalysis {
    pub downscale: u32,
    pub sample_rate: u32,
    pub gpu: bool,
    pub no_crop: bool,
    /// How signal codes became PQ luminance: `"pq"`, or for HLG the
    /// [`Composer::luminance_mapping`] of the Profile 8.4 composer measured through (version 3+).
    #[serde(default)]
    pub luminance_mapping: Option<String>,
}

/// Newest sidecar version this build reads.
pub const L1_SIDECAR_MAX_VERSION: u32 = 5;

/// From this version on the sidecar accounts for every picture of the video stream, including
/// undecodable leading pictures at the start.
pub const L1_SIDECAR_STREAM_FRAMES_VERSION: u32 = 5;

/// From this version on the sidecar averages are unfiltered frame means; older sidecars are
/// reused with an advisory warning.
pub const L1_SIDECAR_UNFILTERED_AVERAGES_VERSION: u32 = 4;

/// Sidecar `analysis.luminance_mapping` of pre-release builds: luma curve only (max-RGB equal to
/// luma), which under-measures saturated highlights, so it is re-analyzed rather than reused.
/// Every current HLG mapping is max-RGB through the full Dolby Vision Profile 8.4 reconstruction
/// (luma curve + chroma MMR + RPU matrix) of one composer, named by
/// [`Composer::luminance_mapping`].
const LUMA_ONLY_DOVI84_MAPPING: &str = "dovi84-v1";

/// Any Dolby Vision 8.4 HLG mapping revision; none of them is valid L1 for a PQ input.
pub(super) fn is_dovi84_family(mapping: Option<&str>) -> bool {
    mapping.is_some_and(|mapping| mapping.starts_with("dovi84-"))
}

impl L1Sidecar {
    /// The version 3 luminance mapping, when present.
    pub fn luminance_mapping(&self) -> Option<&str> {
        self.analysis
            .as_ref()
            .and_then(|analysis| analysis.luminance_mapping.as_deref())
    }

    /// True when the sidecar says it was measured from an HLG source.
    fn source_is_hlg(&self) -> bool {
        self.source
            .as_ref()
            .and_then(|source| source.transfer_function.as_deref())
            .is_some_and(|transfer| transfer.to_uppercase().contains("HLG"))
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct L1SidecarCrop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Default, Deserialize)]
pub struct L1SidecarFrames {
    pub min_pq_12bit: Vec<u16>,
}

impl L1Sidecar {
    /// MaxCLL / MaxFALL usable for L6, or why the sidecar has none. Both need every frame
    /// analyzed (a skipped frame can hide a flash). MaxCLL also needs every pixel: a sampling
    /// stride can miss a small highlight, while a frame average is sound at any stride.
    pub fn measured_light_levels(&self) -> Result<MeasuredLightLevels, String> {
        let Some(levels) = self.light_level else {
            return Err(
                "the measurements carry no content light levels (older analyzer or luma peak domain; delete them to re-analyze)"
                    .into(),
            );
        };
        let (cll, fall) = (levels.max_cll_nits, levels.max_fall_nits);
        if !(1..=10_000).contains(&cll) || !(1..=10_000).contains(&fall) || fall > cll {
            return Err(format!(
                "the sidecar light levels are invalid (MaxCLL {cll}, MaxFALL {fall})"
            ));
        }
        let Some(analysis) = &self.analysis else {
            return Err("the sidecar does not record its sampling".into());
        };
        if analysis.sample_rate != 1 {
            return Err(format!(
                "the analysis skipped frames (sample-rate {})",
                analysis.sample_rate
            ));
        }
        if analysis.downscale != 1 {
            return Ok(MeasuredLightLevels {
                max_cll: None,
                max_fall: Some(fall),
                note: Some(format!(
                    "MaxCLL is not taken from the measurements: analysis at downscale {} can miss small highlights (--analysis-quality accurate measures it).",
                    analysis.downscale
                )),
            });
        }
        Ok(MeasuredLightLevels {
            max_cll: Some(cll),
            max_fall: Some(fall),
            note: None,
        })
    }

    /// Number of frames the sidecar covers (last scene end + 1).
    pub fn frame_count(&self) -> u64 {
        self.scenes
            .iter()
            .map(|scene| scene.end + 1)
            .max()
            .unwrap_or(0)
    }

    /// Undecodable leading pictures in front of the first measured frame (0 before version 5).
    /// The RPU needs this many extra entries at the start: `dovi_tool inject-rpu` gives RPU `n`
    /// to the picture with presentation number `n`, and these pictures come first.
    pub fn leading_frames(&self) -> u64 {
        self.source
            .as_ref()
            .and_then(|source| source.leading_skipped_frames)
            .unwrap_or(0)
    }

    /// Pictures of the video stream the sidecar describes: the measured frames plus the
    /// leading pictures in front of them. This is the length of the RPU.
    pub fn stream_frame_count(&self) -> u64 {
        self.frame_count() + self.leading_frames()
    }

    /// One-line provenance summary for progress output.
    pub fn provenance_summary(&self) -> String {
        let mut parts = vec![format!("sidecar v{}", self.version)];
        if let Some(version) = &self.analyzer_version {
            parts.push(format!("analyzer {version}"));
        }
        if let Some(analysis) = &self.analysis {
            parts.push(format!(
                "downscale {}, sample-rate {}{}",
                analysis.downscale,
                analysis.sample_rate,
                if analysis.gpu { ", GPU" } else { "" }
            ));
        }
        if let Some(crop) = self.crop {
            parts.push(format!(
                "crop {}x{}+{}+{}",
                crop.width, crop.height, crop.x, crop.y
            ));
        }
        parts.push(format!("{} frames", self.frame_count()));
        let leading = self.leading_frames();
        if leading > 0 {
            parts.push(format!(
                "{leading} undecodable leading pictures before them"
            ));
        }
        parts.join(", ")
    }
}

/// L5 active-area offsets derived from the analyzer's committed crop. Requires a version 2
/// sidecar (full-resolution crop plus source dimensions). Returns `None` for a full-frame crop
/// (dovi_tool's zero default already describes it) or when crop detection was disabled.
pub fn level5_from_sidecar(sidecar: &L1Sidecar) -> Option<Level5Offsets> {
    if sidecar.crop_space.as_deref() != Some("full") {
        return None;
    }
    if sidecar
        .analysis
        .as_ref()
        .is_some_and(|analysis| analysis.no_crop)
    {
        return None;
    }
    let source = sidecar.source.as_ref()?;
    let crop = sidecar.crop?;
    let right_edge = crop.x.checked_add(crop.width)?;
    let bottom_edge = crop.y.checked_add(crop.height)?;
    if right_edge > source.width || bottom_edge > source.height {
        return None;
    }
    let clamp = |value: u32| u16::try_from(value).unwrap_or(u16::MAX);
    let offsets = Level5Offsets {
        left: clamp(crop.x),
        right: clamp(source.width - right_edge),
        top: clamp(crop.y),
        bottom: clamp(source.height - bottom_edge),
    };
    (offsets != Level5Offsets::default()).then_some(offsets)
}

/// Why a sidecar cannot be used for source-honest L1.
#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error("no L1 sidecar at {0}")]
    Missing(PathBuf),
    #[error("L1 sidecar {path} is unreadable: {reason}")]
    Unreadable { path: PathBuf, reason: String },
    #[error("unsupported L1 sidecar version {0}")]
    UnsupportedVersion(u32),
    #[error("L1 sidecar is invalid: {0}")]
    Invalid(String),
    #[error("L1 sidecar {0} (re-analysis required)")]
    LuminanceMappingMismatch(String),
}

/// What the caller knows about the input the sidecar must describe. Leave fields `None` for a
/// sidecar that was just produced (the analyzer run is authoritative), or when the analyzed file
/// is an intermediate rather than the input.
#[derive(Debug, Default)]
pub struct SidecarExpectation {
    pub file_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub frames: Option<u64>,
    /// `Some(composer)` when the input is HLG: the sidecar must be version 3+, measured from an
    /// HLG source through that Dolby Vision 8.4 composer (exact name). When `None`, a Dolby
    /// Vision 8.4 mapped sidecar is rejected.
    pub hlg_composer: Option<Composer>,
}

impl SidecarExpectation {
    /// Expect the sidecar to describe `input` exactly (identity + optional frame count).
    pub fn for_input(input: &Path, frames: Option<u64>) -> Self {
        Self {
            file_name: input
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
            size_bytes: fs::metadata(input).ok().map(|meta| meta.len()),
            frames,
            hlg_composer: None,
        }
    }

    /// Set the composer an HLG input is converted with, `None` for other inputs (see
    /// [`SidecarExpectation::hlg_composer`]).
    pub fn with_hlg_composer(mut self, hlg_composer: Option<Composer>) -> Self {
        self.hlg_composer = hlg_composer;
        self
    }
}

#[derive(Debug, Deserialize)]
pub struct L1SidecarScene {
    pub start: u64,
    pub end: u64,
    pub min_pq_12bit: u16,
    /// Retained for sidecar-schema completeness; the RPU average uses the max-RGB mean
    /// (validated against cm v2 shot averages), not the Y-luma mean.
    #[allow(dead_code)]
    pub avg_luma_pq_12bit: u16,
    pub avg_max_rgb_pq_12bit: u16,
    pub max_pq_12bit: u16,
}

/// Path of the L1 sidecar written next to a madVR measurements file.
pub fn l1_sidecar_path(measurements_file: &Path) -> PathBuf {
    let mut sidecar_path = measurements_file.as_os_str().to_owned();
    sidecar_path.push(".l1.json");
    PathBuf::from(sidecar_path)
}

/// Load and validate the L1 sidecar next to a measurements file. Every failure is reported
/// so callers can re-run analysis instead of silently using optimizer-derived L1. On success
/// the sidecar comes back with advisory warnings the caller should print.
pub fn load_l1_sidecar(
    measurements_file: &Path,
    expect: &SidecarExpectation,
) -> Result<(L1Sidecar, Vec<String>), SidecarError> {
    let path = l1_sidecar_path(measurements_file);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(SidecarError::Missing(path));
        }
        Err(error) => {
            return Err(SidecarError::Unreadable {
                path,
                reason: error.to_string(),
            });
        }
    };
    let sidecar: L1Sidecar =
        serde_json::from_reader(std::io::BufReader::new(file)).map_err(|error| {
            SidecarError::Unreadable {
                path: path.clone(),
                reason: error.to_string(),
            }
        })?;
    let advisories = validate_l1_sidecar(&sidecar, expect)?;
    Ok((sidecar, advisories))
}

/// Largest difference between the sidecar's frame count and MediaInfo's input count that still
/// reuses the sidecar. MediaInfo estimates `FrameCount` from duration when an MKV has no
/// statistics tags, so small differences are expected. `--verify` still compares the RPU with the
/// muxed output exactly, which catches real truncation.
pub fn frame_count_tolerance(expected: u64) -> u64 {
    (expected / 1000).max(2)
}

/// Most scenes listed by name in the min <= avg <= max advisory.
const MAX_ORDERING_ADVISORY_SCENES: usize = 5;

/// One advisory for scenes outside min <= avg <= max. The analyzer's average is always the
/// max-RGB mean, while the peak follows --peak-domain and --peak-estimator, so a luma peak domain
/// or a percentile/robust estimator can legitimately put the average above the peak. dovi_tool
/// clamps L1 to spec limits, so this is not a reason to discard measurements.
fn ordering_advisories(sidecar: &L1Sidecar) -> Vec<String> {
    let violations: Vec<String> = sidecar
        .scenes
        .iter()
        .enumerate()
        .filter_map(|(index, scene)| {
            if scene.avg_max_rgb_pq_12bit > scene.max_pq_12bit {
                Some(format!(
                    "scene {index}: avg {} exceeds max {}",
                    scene.avg_max_rgb_pq_12bit, scene.max_pq_12bit
                ))
            } else if scene.min_pq_12bit > scene.avg_max_rgb_pq_12bit {
                Some(format!(
                    "scene {index}: min {} exceeds avg {}",
                    scene.min_pq_12bit, scene.avg_max_rgb_pq_12bit
                ))
            } else {
                None
            }
        })
        .collect();
    if violations.is_empty() {
        return Vec::new();
    }
    let shown = violations[..violations.len().min(MAX_ORDERING_ADVISORY_SCENES)].join("; ");
    let hidden = violations
        .len()
        .saturating_sub(MAX_ORDERING_ADVISORY_SCENES);
    let more = if hidden > 0 {
        format!("; and {hidden} more")
    } else {
        String::new()
    };
    vec![format!(
        "L1 sidecar scenes are outside min <= avg <= max ({shown}{more}). This is expected with a luma peak domain or a percentile/robust peak estimator; dovi_tool clamps these values."
    )]
}

/// Structural and identity checks for a parsed sidecar. Violations that would make the RPU
/// wrong are errors; conditions the conversion tolerates come back as advisory warnings.
pub fn validate_l1_sidecar(
    sidecar: &L1Sidecar,
    expect: &SidecarExpectation,
) -> Result<Vec<String>, SidecarError> {
    if !matches!(sidecar.version, 1..=L1_SIDECAR_MAX_VERSION) {
        return Err(SidecarError::UnsupportedVersion(sidecar.version));
    }
    check_luminance_mapping(sidecar, expect.hlg_composer)?;
    let invalid = |reason: String| Err(SidecarError::Invalid(reason));
    let Some(first) = sidecar.scenes.first() else {
        return invalid("no scenes".into());
    };
    if first.start != 0 {
        return invalid(format!("first scene starts at frame {}", first.start));
    }
    for (index, scene) in sidecar.scenes.iter().enumerate() {
        if scene.end < scene.start {
            return invalid(format!("scene {index} ends before it starts"));
        }
        if let Some(previous) = index.checked_sub(1).map(|i| &sidecar.scenes[i]) {
            if scene.start != previous.end + 1 {
                return invalid(format!(
                    "scene {index} starts at frame {} but the previous scene ended at {}",
                    scene.start, previous.end
                ));
            }
        }
    }
    let mut advisories = ordering_advisories(sidecar);
    if sidecar.version < L1_SIDECAR_UNFILTERED_AVERAGES_VERSION {
        advisories.push(format!(
            "L1 sidecar v{} stores averages that were smoothed over time, so the L1 average of a scene that changes (a fade, a flash) can read wrong. To re-measure, delete the measurements file and run again with an hdr_analyzer_mvp from this version.",
            sidecar.version
        ));
    }
    let covered = sidecar.frame_count();
    if let Some(frames) = &sidecar.frames {
        if frames.min_pq_12bit.len() as u64 != covered {
            return invalid(format!(
                "scenes cover {covered} frames but per-frame data has {}",
                frames.min_pq_12bit.len()
            ));
        }
    }
    let accounted = sidecar.version >= L1_SIDECAR_STREAM_FRAMES_VERSION;
    if accounted {
        let source = sidecar.source.as_ref();
        let (Some(stream), Some(leading)) = (
            source.and_then(|source| source.stream_frames),
            source.and_then(|source| source.leading_skipped_frames),
        ) else {
            return invalid(format!(
                "v{} does not record the stream's picture count",
                sidecar.version
            ));
        };
        if covered + leading != stream {
            return invalid(format!(
                "covers {covered} frames after {leading} leading pictures, but the stream has {stream}"
            ));
        }
    }
    if let Some(expected) = expect.frames {
        let covered = sidecar.stream_frame_count();
        let difference = expected.abs_diff(covered);
        // Before version 5 a sidecar does not say whether the decoder skipped pictures at the
        // start, and a skip shifts every scene against the video. Only an exact match proves
        // that nothing was skipped.
        if !accounted && difference > 0 {
            return invalid(format!(
                "v{} covers {covered} frames but the input video has {expected}, and it predates the record of undecodable leading pictures (sidecar v{L1_SIDECAR_STREAM_FRAMES_VERSION})",
                sidecar.version
            ));
        }
        let tolerance = frame_count_tolerance(expected);
        if difference > tolerance {
            return invalid(format!(
                "covers {covered} frames but the input video has {expected}"
            ));
        }
        if difference > 0 {
            advisories.push(format!(
                "L1 sidecar covers {covered} frames but MediaInfo reports {expected} for the input; within the {tolerance}-frame tolerance, because MediaInfo estimates the count from duration when the MKV has no statistics tags."
            ));
        }
    }
    if let Some(source) = &sidecar.source {
        let name_differs = expect
            .file_name
            .as_ref()
            .is_some_and(|name| *name != source.file_name);
        let size_differs = expect
            .size_bytes
            .is_some_and(|size| size != source.size_bytes);
        if name_differs || size_differs {
            return invalid(format!(
                "produced for '{}' ({} bytes), not this input",
                source.file_name, source.size_bytes
            ));
        }
    }
    Ok(advisories)
}

/// HLG measurements are only valid when taken through the Dolby Vision 8.4 reconstruction of the
/// composer the RPU will carry (sidecar v3+, exact composer name), and no 8.4 mapping is valid
/// for a PQ input.
fn check_luminance_mapping(
    sidecar: &L1Sidecar,
    hlg_composer: Option<Composer>,
) -> Result<(), SidecarError> {
    let mapping = sidecar.luminance_mapping();
    if let Some(composer) = hlg_composer {
        let expected = composer.luminance_mapping();
        if mapping == Some(LUMA_ONLY_DOVI84_MAPPING) {
            return Err(SidecarError::LuminanceMappingMismatch(
                "uses the pre-release luma-only Dolby Vision 8.4 mapping (dovi84-v1), which under-measures saturated highlights"
                    .into(),
            ));
        }
        if let Some(legacy) = mapping.and_then(Composer::from_legacy_luminance_mapping) {
            return Err(SidecarError::LuminanceMappingMismatch(format!(
                "was measured with the {} composer's pre-spec 4:2:0 HLG decode ({}), which reshapes chroma per pixel instead of at chroma resolution; this run needs {expected}",
                legacy.cli_name(),
                mapping.unwrap_or("none")
            )));
        }
        let measured_with = mapping.and_then(Composer::from_luminance_mapping);
        if sidecar.version >= 3 && measured_with.is_none() && is_dovi84_family(mapping) {
            return Err(SidecarError::LuminanceMappingMismatch(format!(
                "names a Dolby Vision 8.4 HLG mapping this mkvdovi does not know ({}); it may come from a newer analyzer",
                mapping.unwrap_or("none")
            )));
        }
        if sidecar.version < 3 || measured_with.is_none() {
            return Err(SidecarError::LuminanceMappingMismatch(format!(
                "v{} (mapping {}) was measured without the Dolby Vision 8.4 HLG mapping",
                sidecar.version,
                mapping.unwrap_or("none")
            )));
        }
        if let Some(other) = measured_with.filter(|&other| other != composer) {
            return Err(SidecarError::LuminanceMappingMismatch(format!(
                "was measured through the {} HLG composer ({}), but this run writes --hlg-composer {} ({expected})",
                other.cli_name(),
                other.luminance_mapping(),
                composer.cli_name()
            )));
        }
        if !sidecar.source_is_hlg() {
            return Err(SidecarError::LuminanceMappingMismatch(
                "does not record an HLG source transfer function".into(),
            ));
        }
    } else if is_dovi84_family(mapping) {
        return Err(SidecarError::LuminanceMappingMismatch(
            "was measured through the Dolby Vision 8.4 HLG mapping, but the input is not HLG"
                .into(),
        ));
    }
    Ok(())
}
