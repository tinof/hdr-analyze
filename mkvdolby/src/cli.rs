use clap::{Parser, ValueEnum};

#[derive(Parser, Debug, Clone)]
#[command(author, version, about = "A tool to convert HDR10/HDR10+ files to Dolby Vision.", long_about = None)]
pub struct Args {
    /// One or more input video files. If not provided, processes all .mkv files in the current directory.
    #[arg(required = false)]
    pub input: Vec<String>,

    /// Controls the --hdr10plus-peak-source flag in dovi_tool generate.
    #[arg(long, value_enum, default_value_t = PeakSource::Histogram99)]
    pub peak_source: PeakSource,

    /// Comma-separated list of nits values for the Dolby Vision trim pass (e.g., '100,600,1000').
    #[arg(long, default_value = "100,600,1000")]
    pub trim_targets: String,

    /// Derive target_nits automatically from madVR Details.txt (uses real display peak and maximum target nits).
    /// Enabled by default. Use --no-trim-from-details to disable.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub trim_from_details: bool,

    /// Disable deriving target_nits from Details.txt (Legacy flag support, functionality handled by trim_from_details=false).
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

    /// After muxing, run verification: our verifier on the measurements and DV checks.
    #[arg(long)]
    pub verify: bool,

    /// Enable a brighter Dolby Vision mapping preset for HDR10+ sources.
    /// If --peak-source is set to 'max-scl-luminance' or 'histogram', this switches it to 'histogram99'.
    #[arg(short, long)]
    pub boost: bool,

    /// Experimental boost mode that asks hdr_analyzer_mvp to use a more aggressive optimizer profile.
    #[arg(long)]
    pub boost_experimental: bool,

    /// Content Mapping version for Dolby Vision RPU generation.
    #[arg(long, value_enum, default_value_t = CmVersion::V40)]
    pub cm_version: CmVersion,

    /// Content type for L11 metadata (affects display tone mapping).
    #[arg(long, value_enum, default_value_t = ContentType::Cinema)]
    pub content_type: ContentType,

    /// Enable reference mode for L11 (critical/studio viewing environment).
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub reference_mode: bool,

    /// Source color primaries index for L9 (0=BT.2020, 1=P3-D65, 2=BT.709). Auto-detected if not set.
    #[arg(long)]
    pub source_primaries: Option<u8>,

    /// Optimizer profile for hdr_analyzer_mvp.
    #[arg(long, value_enum, default_value_t = OptimizerProfile::Conservative)]
    pub optimizer_profile: OptimizerProfile,

    /// Do not delete the source file and intermediate files after successful conversion.
    #[arg(long)]
    pub keep_source: bool,

    /// Hardware acceleration hint for analysis and encoding.
    #[arg(long, value_enum, default_value_t = HwAccel::None)]
    pub hwaccel: HwAccel,

    /// Encoder to use for HLG to PQ conversion (libx265 or hevc_videotoolbox).
    #[arg(long, value_enum, default_value_t = Encoder::Libx265)]
    pub encoder: Encoder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HwAccel {
    None,
    Cuda,
}

impl std::fmt::Display for HwAccel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
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
pub enum PeakSource {
    /// Use max-scl from metadata (most conservative; can look dim).
    MaxSclLuminance,
    /// Use the max value from histogram (more conservative).
    Histogram,
    /// (Default) Use the 99th percentile from histogram (good balance of detail vs brightness).
    Histogram99,
}

impl std::fmt::Display for PeakSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Map enum variants to string values expected by dovi_tool CLI
        match self {
            PeakSource::MaxSclLuminance => write!(f, "max-scl-luminance"),
            PeakSource::Histogram => write!(f, "histogram"),
            PeakSource::Histogram99 => write!(f, "histogram99"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum CmVersion {
    /// Content Mapping v2.9 (legacy, L1/L2/L5/L6 only)
    V29,
    /// Content Mapping v4.0 (enhanced tone mapping with L8/L9/L11)
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
    /// Unknown content type
    Unknown = 0,
    /// Film (traditional cinema, 24fps)
    Film = 1,
    /// Live action (sports, concerts)
    Live = 2,
    /// Animation
    Animation = 3,
    /// Cinema generic (default for movies)
    #[default]
    Cinema = 4,
    /// Gaming content
    Gaming = 5,
    /// Graphics/UI
    Graphics = 6,
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
