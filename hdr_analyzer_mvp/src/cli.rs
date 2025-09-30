use clap::Parser;

// --- Command Line Interface ---
#[derive(Parser)]
#[command(name = "hdr_analyzer_mvp")]
#[command(about = "HDR10 to Dolby Vision converter - Phase 1 MVP")]
pub struct Cli {
    /// Path to the input video file (positional)
    #[arg(value_name = "INPUT")]
    pub input_positional: Option<String>,

    /// Path to the input video file (flag-based alternative to positional)
    #[arg(short = 'i', long = "input", value_name = "PATH")]
    pub input_flag: Option<String>,

    /// Path for the output .bin measurement file (optional - auto-generates from input filename if not provided)
    #[arg(short, long)]
    pub output: Option<String>,

    /// DEPRECATED: Optimizer is enabled by default. Use --disable-optimizer to turn off.
    #[arg(long, hide = true)]
    pub enable_optimizer: bool,

    /// (Optional) Enable GPU hardware acceleration.
    /// Examples: "cuda" (for NVIDIA), "vaapi" (for Linux/AMD/Intel), "videotoolbox" (for macOS).
    #[arg(long)]
    pub hwaccel: Option<String>,

    /// madVR measurement file version to write (5 or 6). Default: 5
    #[arg(long, default_value_t = 5)]
    pub madvr_version: u8,

    /// Scene detection threshold (distance metric). Default: 0.3
    #[arg(long, default_value_t = 0.3)]
    pub scene_threshold: f64,

    /// Minimum scene length in frames. Cuts closer than this are dropped. Default: 24
    #[arg(long, default_value_t = 24)]
    pub min_scene_length: u32,

    /// Optional smoothing window (in frames) over the scene-change metric. 0 disables smoothing.
    #[arg(long, default_value_t = 5)]
    pub scene_smoothing: u32,

    /// Optional override for header.target_peak_nits (used for v6). If omitted, defaults to computed maxCLL.
    #[arg(long)]
    pub target_peak_nits: Option<u32>,

    /// Downscale factor for analysis to improve throughput (1=full, 2=half, 4=quarter)
    /// Only affects internal analysis resolution. Output statistics remain comparable.
    #[arg(long, default_value_t = 1)]
    pub downscale: u32,

    /// Disable active-area crop detection (analyze full frame). Useful for diagnostics/validation.
    #[arg(long)]
    pub no_crop: bool,

    /// Disable dynamic optimizer (enabled by default).
    #[arg(long)]
    pub disable_optimizer: bool,

    /// Override the number of Rayon analysis threads (defaults to logical cores).
    #[arg(long)]
    pub analysis_threads: Option<usize>,

    /// Emit per-stage throughput metrics once processing completes.
    #[arg(long)]
    pub profile_performance: bool,

    /// Optimizer profile: conservative, balanced, or aggressive (default: balanced)
    #[arg(long, default_value = "balanced")]
    pub optimizer_profile: String,
}
