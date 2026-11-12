use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Parser, Subcommand, ValueEnum};
use dovi84_composer::Composer;

#[derive(Subcommand, Debug, Clone)]
pub enum SubCmd {
    /// Inspect Dolby Vision RPU metadata and report suspicious L1 patterns.
    Inspect(InspectArgs),
    /// Write the playback manifest (`<stem>.dvmanifest.json`) of an HDR10 or HLG MKV: its
    /// measured per-shot L1 and the AU table a player checks it against
    /// (docs/PLAYBACK_MANIFEST.md). Never muxes, converts or deletes anything.
    Manifest(ManifestArgs),
}

/// `mkvdovi manifest`. The flags after `--work-dir` are the conversion's flags that change the
/// measured L1 or the RPU, with the same defaults, so the manifest describes the RPUs a
/// conversion with the same flags writes.
#[derive(Parser, Debug, Clone)]
pub struct ManifestArgs {
    /// Input MKV (HDR10 or HLG, one HEVC video track).
    pub input: String,

    /// Manifest file to write. Default: `<work dir>/<input stem>.dvmanifest.json`.
    #[arg(long)]
    pub out: Option<std::path::PathBuf>,

    /// Directory for the measurements (`<stem>_measurements.bin`, its `.l1.json` and `.au.json`)
    /// and the analyzer log; created when missing, never removed. Default: the input's directory.
    /// Use it when the input is on a read-only mount.
    #[arg(long)]
    pub work_dir: Option<std::path::PathBuf>,

    /// Take the shots and the RPU parameters from this `RPU.bin` (display order, as
    /// `dovi_tool generate` writes it and `dovi_tool extract-rpu` returns it) instead of
    /// measuring. Accepted only when the manifest rebuilds every RPU of the file byte for byte;
    /// the first picture that differs is named. Measures nothing (the analyzer writes only the
    /// AU table, `<stem>.rpu-import.au.json`), so the flags that shape the RPU or the
    /// measurement cannot be given with it; `--hlg-composer` names the composer the file must
    /// carry. The coverage class is `approximate`.
    #[arg(
        long,
        value_name = "RPU.bin",
        conflicts_with_all = [
            "cm_version",
            "content_type",
            "reference_mode",
            "source_primaries",
            "trim_targets",
            "analysis_quality",
        ]
    )]
    pub rpu: Option<std::path::PathBuf>,

    /// As for conversion.
    #[arg(
        long,
        value_parser = PossibleValuesParser::new(Composer::ALL.map(Composer::cli_name))
            .map(|name| Composer::from_cli_name(&name).expect("restricted to Composer::ALL")),
        default_value = Composer::Bt2100V1.cli_name()
    )]
    pub hlg_composer: Composer,

    /// As for conversion.
    #[arg(long, value_enum, default_value_t = CmVersion::V40)]
    pub cm_version: CmVersion,

    /// As for conversion.
    #[arg(long, value_enum, default_value_t = ContentType::Movies)]
    pub content_type: ContentType,

    /// As for conversion.
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    pub reference_mode: bool,

    /// As for conversion.
    #[arg(long)]
    pub source_primaries: Option<u8>,

    /// As for conversion.
    #[arg(long, default_value = "100,600,1000")]
    pub trim_targets: String,

    /// As for conversion. The coverage class is `full` only for accurate (the default).
    #[arg(long, value_enum, default_value_t = AnalysisQuality::Auto)]
    pub analysis_quality: AnalysisQuality,

    /// As for conversion.
    #[arg(long, value_enum, default_value_t = HwAccel::Auto)]
    pub hwaccel: HwAccel,

    /// Verbose mode: show raw command output.
    #[arg(short, long)]
    pub verbose: bool,

    /// Quiet mode: minimal output.
    #[arg(short, long)]
    pub quiet: bool,
}

impl ManifestArgs {
    /// The conversion arguments these settings stand for: the conversion's defaults with this
    /// command's flags, so the pipeline phases shared with `convert_file` read them unchanged.
    pub fn conversion_args(&self) -> Args {
        let mut args = Args::try_parse_from(["mkvdovi"]).expect("the defaults parse");
        args.input = vec![self.input.clone()];
        args.hlg_composer = self.hlg_composer;
        args.cm_version = self.cm_version;
        args.content_type = self.content_type;
        args.reference_mode = self.reference_mode;
        args.source_primaries = self.source_primaries;
        args.trim_targets = self.trim_targets.clone();
        args.analysis_quality = self.analysis_quality;
        args.hwaccel = self.hwaccel;
        args.verbose = self.verbose;
        args.quiet = self.quiet;
        // dovi_tool takes no part in a manifest; `raw` keeps `resolve_auto_settings` quiet about it.
        args.dovi_input = DoviInput::Raw;
        // Never deleted: the manifest command has no deletion step, and this says so twice.
        args.keep_source = true;
        args
    }
}

#[derive(Parser, Debug, Clone)]
pub struct InspectArgs {
    /// Input Dolby Vision file to inspect.
    pub input: String,
}

#[derive(Parser, Debug, Clone)]
#[command(
    author,
    version,
    about = "A tool to convert HDR10/HDR10+ files to Dolby Vision.",
    long_about = None,
    subcommand_precedence_over_arg = true
)]
pub struct Args {
    /// Subcommand (inspect). If omitted, runs the default convert pipeline.
    #[command(subcommand)]
    pub subcmd: Option<SubCmd>,

    /// One or more input video files. If not provided, processes all .mkv files recursively from the current directory.
    #[arg(required = false)]
    pub input: Vec<String>,

    /// Controls the --hdr10plus-peak-source flag in dovi_tool generate, and the HDR10+ peak of
    /// the hybrid L1 (--hdr10plus-l1).
    #[arg(long, value_enum, default_value_t = PeakSource::Histogram)]
    pub peak_source: PeakSource,

    /// L1 for HDR10+ inputs. hybrid (default) = the analyzer also runs: scenes = union of the
    /// HDR10+ scene starts and the analyzer's cuts; peak from HDR10+ (--peak-source), or the
    /// measured peak of the piece's own frames where the HDR10+ peak is missing, 0 or not above
    /// the measured average (each one is reported); average, minimum and L5 from the measured
    /// pixels. Needs every frame analyzed. hdr10plus = dovi_tool takes L1 from the HDR10+ metadata
    /// (first frame of each HDR10+ scene; minimum 0, average rounded to whole nits, no L5), with
    /// no analysis. Without this flag, a file that cannot take hybrid (--analysis-quality fast,
    /// --legacy-madvr-l1, an older or missing analyzer, unusable HDR10+ metadata, a failed
    /// analysis) uses hdr10plus with a warning; `--hdr10plus-l1 hybrid` refuses it instead and
    /// cannot be combined with --legacy-madvr-l1. Frames without their own HDR10+ payload (which
    /// hdr10plus_tool fills from the previous frame) warn; `--hdr10plus-l1 hybrid` refuses them.
    /// Other inputs ignore it.
    #[arg(long, value_enum)]
    pub hdr10plus_l1: Option<Hdr10PlusL1>,

    /// Comma-separated list of nits values for the Dolby Vision trim pass (e.g., '100,600,1000').
    #[arg(long, default_value = "100,600,1000")]
    pub trim_targets: String,

    /// Legacy no-op retained for CLI compatibility.
    #[arg(long, hide = true, default_value_t = true, action = clap::ArgAction::Set)]
    pub trim_from_details: bool,

    /// Legacy no-op retained for CLI compatibility.
    #[arg(long, hide = true, action = clap::ArgAction::SetFalse, overrides_with = "trim_from_details")]
    pub no_trim_from_details: bool,

    /// Drop chapters in the output file (kept by default).
    #[arg(long)]
    pub drop_chapters: bool,

    /// Drop global tags in the output file (kept by default).
    #[arg(long)]
    pub drop_tags: bool,

    /// After muxing, run verification: our verifier on the measurements and DV checks.
    #[arg(long)]
    pub verify: bool,

    /// Rebuild unreliable Dolby Vision metadata from fresh base-layer measurements.
    #[arg(long)]
    pub mdfix: bool,

    /// Enable a brighter Dolby Vision mapping preset for HDR10+ sources.
    /// If another --peak-source is selected, this switches it to 'histogram99'.
    #[arg(short, long)]
    pub boost: bool,

    /// Experimental boost mode that asks hdr_analyzer_mvp to use a more aggressive optimizer profile.
    #[arg(long)]
    pub boost_experimental: bool,

    /// Content Mapping version for Dolby Vision RPU generation.
    #[arg(long, value_enum, default_value_t = CmVersion::V40)]
    pub cm_version: CmVersion,

    /// Content type for L11 metadata (affects display tone mapping).
    #[arg(long, value_enum, default_value_t = ContentType::Movies)]
    pub content_type: ContentType,

    /// Enable reference mode for L11 (critical/studio viewing environment).
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    pub reference_mode: bool,

    /// Source color primaries index for L9 (0=P3-D65, 1=BT.709, 2=BT.2020). Auto-detected if not set.
    #[arg(long)]
    pub source_primaries: Option<u8>,

    /// Optimizer profile for hdr_analyzer_mvp.
    #[arg(long, value_enum, default_value_t = OptimizerProfile::Conservative)]
    pub optimizer_profile: OptimizerProfile,

    /// Analysis quality for HDR10, HLG and HDR10+ sources (HDR10+ unless --hdr10plus-l1 hdr10plus).
    /// auto = accurate; fast = half-res, every 3rd frame; balanced = half-res, every frame
    /// (about two to three times faster on the CPU, but misreads small highlights and drops the
    /// measured MaxCLL); accurate = full-res, every frame (most precise L1; the only preset where
    /// CPU and CUDA give identical output). With GPU analysis all three run at the same speed.
    #[arg(long, value_enum, default_value_t = AnalysisQuality::Auto)]
    pub analysis_quality: AnalysisQuality,

    /// Dolby Vision Profile 8.4 composer (reshaping curves) written into the RPU of HLG inputs.
    /// bt2100 (default) = fitted to the BT.2100 / BT.2408 1000-nit HLG-to-PQ conversion with
    /// neutrals kept neutral; preset = the dolby_vision crate's Profile84 preset that dovi_tool
    /// generates (decodes neutral greys slightly blue). A playback test on an LG OLED C9
    /// confirmed that the display applies the composer; if a display renders bt2100 output
    /// wrongly, convert with preset.
    #[arg(
        long,
        value_parser = PossibleValuesParser::new(Composer::ALL.map(Composer::cli_name))
            .map(|name| Composer::from_cli_name(&name).expect("restricted to Composer::ALL")),
        default_value = Composer::Bt2100V1.cli_name()
    )]
    pub hlg_composer: Composer,

    /// Compatibility escape hatch: build L1 from the madVR measurements file with dovi_tool's
    /// --use-custom-targets instead of the analyzer's measured L1 sidecar. L1 max then comes
    /// from optimizer targets and L1 average is a placeholder. Only for reproducing old output.
    #[arg(long)]
    pub legacy_madvr_l1: bool,

    /// Keep the source file after successful conversion (by default it is deleted).
    #[arg(long)]
    pub keep_source: bool,

    /// Force a clean re-run: discard any leftover temp directory from an interrupted
    /// conversion instead of resuming from it. By default, a leftover temp dir is reused so
    /// completed steps (analysis, RPU, extracted base layer) are not redone.
    #[arg(long)]
    pub no_resume: bool,

    /// Warn when the current step's output file stops growing for this many seconds
    /// (0 disables). Helps tell a stalled tool apart from merely slow storage.
    #[arg(long, default_value_t = 300)]
    pub stall_timeout: u64,

    /// Hardware acceleration for the hdr_analyzer_mvp pass (GPU decode and analysis).
    /// auto (default) detects an NVIDIA GPU at startup and uses CUDA when available.
    #[arg(long, value_enum, default_value_t = HwAccel::Auto)]
    pub hwaccel: HwAccel,

    /// How Dolby Vision inputs are fed to dovi_tool for remove/convert/demux.
    /// auto (default) passes the MKV directly when dovi_tool is 2.3.4+ (skipping a full-size
    /// ffmpeg HEVC extraction), else extracts first. raw always extracts; mkv always tries the
    /// container first. A failed direct read falls back to extraction automatically.
    #[arg(long, value_enum, default_value_t = DoviInput::Auto)]
    pub dovi_input: DoviInput,

    /// Verbose mode: show raw command output (useful for debugging).
    #[arg(short, long)]
    pub verbose: bool,

    /// Quiet mode: minimal output (only errors and final result).
    #[arg(short, long)]
    pub quiet: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn hwaccel_and_analysis_quality_default_to_auto() {
        let args = Args::try_parse_from(["mkvdovi"]).unwrap();
        assert_eq!(args.hwaccel, HwAccel::Auto);
        assert_eq!(args.analysis_quality, AnalysisQuality::Auto);
        assert_eq!(args.dovi_input, DoviInput::Auto);
    }

    #[test]
    fn trim_targets_parse_as_nits_in_order() {
        let args = Args::try_parse_from(["mkvdovi"]).unwrap();
        assert_eq!(args.trim_targets_nits().unwrap(), vec![100, 600, 1000]);
        let args = Args::try_parse_from(["mkvdovi", "--trim-targets", "600, 100"]).unwrap();
        assert_eq!(args.trim_targets_nits().unwrap(), vec![600, 100]);
        let args = Args::try_parse_from(["mkvdovi", "--trim-targets", "100,x"]).unwrap();
        assert!(args.trim_targets_nits().is_err());
    }

    #[test]
    fn hlg_composer_defaults_to_bt2100_and_parses_the_preset() {
        let args = Args::try_parse_from(["mkvdovi"]).unwrap();
        assert_eq!(args.hlg_composer, Composer::Bt2100V1);
        let args = Args::try_parse_from(["mkvdovi", "--hlg-composer", "bt2100"]).unwrap();
        assert_eq!(args.hlg_composer, Composer::Bt2100V1);
        let args = Args::try_parse_from(["mkvdovi", "--hlg-composer", "preset"]).unwrap();
        assert_eq!(args.hlg_composer, Composer::Preset);
        assert!(Args::try_parse_from(["mkvdovi", "--hlg-composer", "dovi84-bt2100-v1"]).is_err());
    }

    #[test]
    fn hdr10plus_l1_defaults_to_hdr10plus_and_hybrid_excludes_legacy_madvr_l1() {
        let args = Args::try_parse_from(["mkvdovi"]).unwrap();
        assert_eq!(args.hdr10plus_l1, None);
        let args = Args::try_parse_from(["mkvdovi", "--hdr10plus-l1", "hybrid"]).unwrap();
        assert_eq!(args.hdr10plus_l1, Some(Hdr10PlusL1::Hybrid));
        let args = Args::try_parse_from(["mkvdovi", "--hdr10plus-l1", "hdr10plus"]).unwrap();
        assert_eq!(args.hdr10plus_l1, Some(Hdr10PlusL1::Hdr10plus));
        // Only an explicit hybrid conflicts with --legacy-madvr-l1, not the default.
        for flags in [
            &["--legacy-madvr-l1"][..],
            &["--hdr10plus-l1", "hdr10plus", "--legacy-madvr-l1"],
        ] {
            let args = Args::try_parse_from(["mkvdovi"].iter().chain(flags)).unwrap();
            assert_eq!(args.flag_conflict(), None, "{flags:?}");
        }
        let args =
            Args::try_parse_from(["mkvdovi", "--hdr10plus-l1", "hybrid", "--legacy-madvr-l1"])
                .unwrap();
        assert!(args.flag_conflict().unwrap().contains("--legacy-madvr-l1"));
        assert!(Args::try_parse_from(["mkvdovi", "--hdr10plus-l1", "pixels"]).is_err());
    }

    #[test]
    fn explicit_dovi_input_values_parse() {
        for (value, expected) in [("raw", DoviInput::Raw), ("mkv", DoviInput::Mkv)] {
            let args = Args::try_parse_from(["mkvdovi", "--dovi-input", value]).unwrap();
            assert_eq!(args.dovi_input, expected);
            assert_eq!(args.dovi_input.to_string(), value);
        }
    }

    #[test]
    fn explicit_hwaccel_and_quality_values_parse() {
        let args = Args::try_parse_from([
            "mkvdovi",
            "--hwaccel",
            "none",
            "--analysis-quality",
            "balanced",
        ])
        .unwrap();
        assert_eq!(args.hwaccel, HwAccel::None);
        assert_eq!(args.analysis_quality, AnalysisQuality::Balanced);
    }

    #[test]
    fn inspect_subcommand_precedes_input_vec() {
        let args = Args::try_parse_from(["mkvdovi", "inspect", "movie.mkv"]).unwrap();

        match args.subcmd {
            Some(SubCmd::Inspect(inspect_args)) => assert_eq!(inspect_args.input, "movie.mkv"),
            other => panic!("expected inspect subcommand, got {other:?}"),
        }
        assert!(args.input.is_empty());
    }

    #[test]
    fn manifest_subcommand_parses_its_paths_and_flags() {
        let args = Args::try_parse_from([
            "mkvdovi",
            "manifest",
            "movie.mkv",
            "--out",
            "m.json",
            "--work-dir",
            "/tmp/w",
            "--hlg-composer",
            "preset",
            "--cm-version",
            "v29",
        ])
        .unwrap();
        let Some(SubCmd::Manifest(manifest)) = args.subcmd else {
            panic!("expected the manifest subcommand");
        };
        assert_eq!(manifest.input, "movie.mkv");
        assert_eq!(
            manifest.out.as_deref(),
            Some(std::path::Path::new("m.json"))
        );
        assert_eq!(
            manifest.work_dir.as_deref(),
            Some(std::path::Path::new("/tmp/w"))
        );
        let converted = manifest.conversion_args();
        assert_eq!(converted.hlg_composer, Composer::Preset);
        assert_eq!(converted.cm_version, CmVersion::V29);
        assert!(args.input.is_empty());
        // Conversion-only flags are not manifest flags.
        for flag in ["--mdfix", "--legacy-madvr-l1", "--verify", "--keep-source"] {
            assert!(
                Args::try_parse_from(["mkvdovi", "manifest", "movie.mkv", flag]).is_err(),
                "{flag}"
            );
        }
    }

    #[test]
    fn manifest_rpu_refuses_the_flags_it_reads_from_the_file() {
        let parse = |extra: &[&str]| {
            let mut argv = vec!["mkvdovi", "manifest", "movie.mkv", "--rpu", "RPU.bin"];
            argv.extend(extra);
            Args::try_parse_from(argv)
        };
        let Some(SubCmd::Manifest(manifest)) = parse(&[]).unwrap().subcmd else {
            panic!("expected the manifest subcommand");
        };
        assert_eq!(
            manifest.rpu.as_deref(),
            Some(std::path::Path::new("RPU.bin"))
        );
        // The composer the file must carry, the decoder and the paths stay.
        for allowed in [
            &["--hlg-composer", "preset"][..],
            &["--hwaccel", "none"],
            &["--out", "m.json", "--work-dir", "/tmp/w"],
        ] {
            assert!(parse(allowed).is_ok(), "{allowed:?}");
        }
        for refused in [
            &["--cm-version", "v29"][..],
            &["--content-type", "game"],
            &["--reference-mode", "true"],
            &["--source-primaries", "0"],
            &["--trim-targets", "100"],
            &["--analysis-quality", "fast"],
        ] {
            assert!(parse(refused).is_err(), "{refused:?}");
            // Without --rpu they are manifest flags.
            let mut argv = vec!["mkvdovi", "manifest", "movie.mkv"];
            argv.extend(refused);
            assert!(Args::try_parse_from(argv).is_ok(), "{refused:?}");
        }
    }

    #[test]
    fn manifest_defaults_are_the_conversion_defaults() {
        let Some(SubCmd::Manifest(manifest)) =
            Args::try_parse_from(["mkvdovi", "manifest", "a.mkv"])
                .unwrap()
                .subcmd
        else {
            panic!("expected the manifest subcommand");
        };
        let ours = manifest.conversion_args();
        let convert = Args::try_parse_from(["mkvdovi", "a.mkv"]).unwrap();
        assert_eq!(ours.hlg_composer, convert.hlg_composer);
        assert_eq!(ours.cm_version, convert.cm_version);
        assert_eq!(ours.content_type, convert.content_type);
        assert_eq!(ours.reference_mode, convert.reference_mode);
        assert_eq!(ours.source_primaries, convert.source_primaries);
        assert_eq!(ours.trim_targets, convert.trim_targets);
        assert_eq!(ours.analysis_quality, convert.analysis_quality);
        assert_eq!(ours.hwaccel, convert.hwaccel);
        assert_eq!(ours.input, convert.input);
        // Settings the manifest does not offer keep the conversion's defaults.
        assert_eq!(ours.optimizer_profile, convert.optimizer_profile);
        assert_eq!(ours.peak_source, convert.peak_source);
        assert!(!ours.boost && !ours.boost_experimental && !ours.mdfix && !ours.legacy_madvr_l1);
        assert_eq!(ours.hdr10plus_l1, None);
    }

    #[test]
    fn removed_fel_encode_interface_is_rejected() {
        // The FEL compositor and every encode path were removed; clap must not accept their
        // flags or the composite-pipe subcommand's arguments any more.
        for flag in [
            ["--fel-crf", "18"],
            ["--fel-preset", "medium"],
            ["--fel-encoder", "local"],
            ["--fel-nvenc-preset", "p5"],
            ["--encoder", "libx265"],
        ] {
            let result = Args::try_parse_from(["mkvdovi", flag[0], flag[1], "movie.mkv"]);
            assert!(result.is_err(), "{} must be rejected", flag[0]);
        }
        assert!(Args::try_parse_from([
            "mkvdovi",
            "composite-pipe",
            "--bl",
            "BL.hevc",
            "--el",
            "EL.hevc",
            "--rpu",
            "RPU.bin",
            "-w",
            "3840",
            "-H",
            "2160",
        ])
        .is_err());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HwAccel {
    /// Detect the best backend for this machine (CUDA when an NVIDIA GPU is present).
    Auto,
    None,
    Cuda,
}

impl std::fmt::Display for HwAccel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HwAccel::Auto => write!(f, "auto"),
            HwAccel::None => write!(f, "none"),
            HwAccel::Cuda => write!(f, "cuda"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DoviInput {
    Auto,
    Raw,
    Mkv,
}

impl std::fmt::Display for DoviInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DoviInput::Auto => write!(f, "auto"),
            DoviInput::Raw => write!(f, "raw"),
            DoviInput::Mkv => write!(f, "mkv"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PeakSource {
    /// (Default) Use the max value from histogram metadata.
    Histogram,
    /// Brighter mapping: use the last histogram percentile, usually 99.98% brightness.
    Histogram99,
    /// Use the max value from the max-scl components.
    MaxScl,
    /// Use luminance calculated from max-scl metadata (most conservative; can look dim).
    MaxSclLuminance,
}

impl Args {
    /// `--trim-targets` as nits values, in the order given.
    pub fn trim_targets_nits(&self) -> Result<Vec<u32>, std::num::ParseIntError> {
        self.trim_targets
            .split(',')
            .map(|s| s.trim().parse::<u32>())
            .collect()
    }

    /// A flag combination refused before any file is processed. Only an explicit hybrid conflicts:
    /// the default leaves --legacy-madvr-l1 on the HDR10+ metadata path (`resolve_hdr10plus_mode`).
    pub fn flag_conflict(&self) -> Option<&'static str> {
        (self.hdr10plus_l1 == Some(Hdr10PlusL1::Hybrid) && self.legacy_madvr_l1).then_some(
            "--hdr10plus-l1 hybrid builds L1 from the measured L1 sidecar, which --legacy-madvr-l1 replaces; use one or the other.",
        )
    }
}

/// Where the L1 of an HDR10+ input comes from (`--hdr10plus-l1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Hdr10PlusL1 {
    /// dovi_tool generate --hdr10plus-json: L1 from the HDR10+ metadata alone.
    Hdr10plus,
    /// Union of HDR10+ scenes and analyzer cuts; HDR10+ peak, measured average and minimum.
    Hybrid,
}

impl std::fmt::Display for PeakSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Map enum variants to string values expected by dovi_tool CLI
        match self {
            PeakSource::MaxSclLuminance => write!(f, "max-scl-luminance"),
            PeakSource::MaxScl => write!(f, "max-scl"),
            PeakSource::Histogram => write!(f, "histogram"),
            PeakSource::Histogram99 => write!(f, "histogram99"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum CmVersion {
    /// Content Mapping v2.9 (legacy, L1/L2/L5/L6 only)
    V29,
    /// Content Mapping v4.0 (adds L9/L11 and dovi_tool defaults; authored workflows may add L8)
    #[default]
    V40,
}

impl std::fmt::Display for CmVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CmVersion::V29 => write!(f, "V29"),
            CmVersion::V40 => write!(f, "V40"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum ContentType {
    /// Default Dolby Vision playback behavior
    Default = 0,
    /// Movies: minimizes post-processing to preserve artistic intent
    #[default]
    #[value(alias = "cinema", alias = "film")]
    Movies = 1,
    /// Game: minimizes input latency
    #[value(alias = "gaming")]
    Game = 2,
    /// Sport: enables frame rate conversion for high-motion content
    Sport = 3,
    /// User-generated content: enables compensating post-processing
    UserGeneratedContent = 4,
}

impl ContentType {
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OptimizerProfile {
    Conservative,
    Balanced,
    Aggressive,
}

impl std::fmt::Display for OptimizerProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OptimizerProfile::Conservative => write!(f, "conservative"),
            OptimizerProfile::Balanced => write!(f, "balanced"),
            OptimizerProfile::Aggressive => write!(f, "aggressive"),
        }
    }
}

/// Controls the resolution and frame-sampling rate of the hdr_analyzer_mvp pass.
/// auto (default) picks accurate with or without GPU analysis.
/// Higher quality = more accurate per-scene L1 luminance, at the cost of CPU analysis time.
/// GPU analysis is limited by NVDEC decoding, so there every preset runs at the same speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum AnalysisQuality {
    /// Accurate on every host, so a file gets the same L1 with or without GPU analysis
    /// (exception: an analyzer without `+cuda` under `--hwaccel cuda`, ROADMAP E12.2).
    #[default]
    Auto,
    /// Half-resolution, every 3rd frame — fastest on the CPU; may miss brief peak frames.
    Fast,
    /// Half-resolution, every frame — faster on the CPU; misreads small highlights (high and
    /// low) and drops the measured MaxCLL.
    Balanced,
    /// Full resolution, every frame — most accurate L1; the default, slower on the CPU.
    Accurate,
}
