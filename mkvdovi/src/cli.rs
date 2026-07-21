use clap::{Parser, Subcommand, ValueEnum};

#[derive(Subcommand, Debug, Clone)]
pub enum SubCmd {
    /// Inspect Dolby Vision RPU metadata and report suspicious L1 patterns.
    Inspect(InspectArgs),

    /// Output raw NLQ-composited frames to stdout (for piping to an encoder).
    /// Only runs BL+EL compositing — no encoding, no muxing.
    #[command(name = "composite-pipe")]
    CompositePipe(CompositePipeArgs),
}

#[derive(Parser, Debug, Clone)]
pub struct InspectArgs {
    /// Input Dolby Vision file to inspect.
    pub input: String,
}

#[derive(Parser, Debug, Clone)]
pub struct CompositePipeArgs {
    /// Path to the base layer HEVC file.
    #[arg(long)]
    pub bl: String,

    /// Path to the enhancement layer HEVC file.
    #[arg(long)]
    pub el: String,

    /// Path to the RPU binary file.
    #[arg(long)]
    pub rpu: String,

    /// Video width in pixels.
    #[arg(short = 'w', long)]
    pub width: u32,

    /// Video height in pixels.
    #[arg(short = 'H', long)]
    pub height: u32,

    /// Frames per second numerator (e.g., 24000).
    #[arg(long, default_value_t = 24000)]
    pub fps_num: u32,

    /// Frames per second denominator (e.g., 1001).
    #[arg(long, default_value_t = 1001)]
    pub fps_den: u32,
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
    /// Subcommand (e.g., composite-pipe). If omitted, runs the default convert pipeline.
    #[command(subcommand)]
    pub subcmd: Option<SubCmd>,

    /// One or more input video files. If not provided, processes all .mkv files recursively from the current directory.
    #[arg(required = false)]
    pub input: Vec<String>,

    /// Controls the --hdr10plus-peak-source flag in dovi_tool generate.
    #[arg(long, value_enum, default_value_t = PeakSource::Histogram)]
    pub peak_source: PeakSource,

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

    /// CRF to use when converting HLG to PQ (default: 17).
    #[arg(long, default_value_t = 17)]
    pub hlg_crf: u8,

    /// x265 preset to use for HLG->PQ (default: medium).
    #[arg(long, default_value = "medium")]
    pub hlg_preset: String,

    /// Nominal peak luminance for HLG content in cd/m² (default: 1000).
    #[arg(long, default_value_t = 1000)]
    pub hlg_peak_nits: u32,

    /// Quality parameter for Profile 7 FEL re-encoding (default: 18).
    /// Used as CRF for libx265 local encode, or QP for hevc_nvenc (Modal/CUDA).
    #[arg(long, default_value_t = 18)]
    pub fel_crf: u8,

    /// x265 preset to use for local FEL re-encoding (default: medium).
    #[arg(long, default_value = "medium")]
    pub fel_preset: String,

    /// Encoder backend for FEL re-encoding: local (default) or modal (offload to Modal.com).
    #[arg(long, value_enum, default_value_t = FelEncoder::Local)]
    pub fel_encoder: FelEncoder,

    /// NVENC preset for Modal/CUDA encoding (default: p5). Range: p1 (fastest) to p7 (best quality).
    #[arg(long, default_value = "p5")]
    pub fel_nvenc_preset: String,

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

    /// Analysis quality for HDR10/HLG sources.
    /// fast = half-res, every 3rd frame (old default); balanced = half-res, every frame;
    /// accurate = full-res, every frame (slowest but most precise L1).
    #[arg(long, value_enum, default_value_t = AnalysisQuality::Auto)]
    pub analysis_quality: AnalysisQuality,

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

    /// Hardware acceleration hint for analysis and encoding.
    /// auto (default) detects an NVIDIA GPU at startup and uses CUDA when available.
    #[arg(long, value_enum, default_value_t = HwAccel::Auto)]
    pub hwaccel: HwAccel,

    /// Encoder to use for HLG to PQ conversion (libx265 or hevc_videotoolbox).
    #[arg(long, value_enum, default_value_t = Encoder::Libx265)]
    pub encoder: Encoder,

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
    fn composite_pipe_subcommand_precedes_input_vec() {
        let args = Args::try_parse_from([
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
        .unwrap();

        match args.subcmd {
            Some(SubCmd::CompositePipe(pipe_args)) => {
                assert_eq!(pipe_args.bl, "BL.hevc");
                assert_eq!(pipe_args.el, "EL.hevc");
                assert_eq!(pipe_args.rpu, "RPU.bin");
            }
            other => panic!("expected composite-pipe subcommand, got {other:?}"),
        }
        assert!(args.input.is_empty());
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
pub enum Encoder {
    Libx265,
    #[clap(name = "videotoolbox")]
    HevcVideotoolbox,
}

impl std::fmt::Display for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Encoder::Libx265 => write!(f, "libx265"),
            Encoder::HevcVideotoolbox => write!(f, "hevc_videotoolbox"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FelEncoder {
    /// Use local ffmpeg/x265 for FEL re-encoding (default).
    Local,
    /// Offload quality encode to Modal.com (local lossless intermediate → cloud x265).
    Modal,
}

impl std::fmt::Display for FelEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FelEncoder::Local => write!(f, "local"),
            FelEncoder::Modal => write!(f, "modal"),
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
/// auto (default) picks accurate on a CUDA-capable setup, balanced otherwise.
/// Higher quality = more accurate per-scene L1 luminance, at the cost of analysis time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum AnalysisQuality {
    /// Pick automatically: accurate when GPU analysis is active, balanced otherwise.
    #[default]
    Auto,
    /// Half-resolution, every 3rd frame — fastest; may miss brief peak frames.
    Fast,
    /// Half-resolution, every frame — good balance of speed and accuracy.
    Balanced,
    /// Full resolution, every frame — most accurate L1; significantly slower.
    Accurate,
}
