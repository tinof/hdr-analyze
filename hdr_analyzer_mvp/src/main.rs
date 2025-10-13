use anyhow::Result;
// Native FFmpeg imports

mod crop;

mod analysis;
mod cli;
mod ffmpeg_io;
mod optimizer;
mod pipeline;
mod writer;

use clap::Parser;
use cli::Cli;

use ffmpeg_io::get_native_video_info;
use pipeline::run;

fn main() -> Result<()> {
    let cli = Cli::parse();

    let input_path = match (&cli.input_positional, &cli.input_flag) {
        (Some(pos), None) => pos.clone(),
        (None, Some(flag)) => flag.clone(),
        (Some(_), Some(_)) => {
            return Err(anyhow::anyhow!(
                "Cannot specify input both as positional argument and via -i/--input flag"
            ));
        }
        (None, None) => {
            return Err(anyhow::anyhow!(
                "Input file required: provide as positional argument or via -i/--input flag"
            ));
        }
    };

    if let Some(threads) = cli.analysis_threads {
        if threads == 0 {
            return Err(anyhow::anyhow!("--analysis-threads must be at least 1"));
        }
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global()
            .map_err(|err| anyhow::anyhow!("Failed to configure Rayon thread pool: {err}"))?;
    }

    if cli.hlg_peak_nits <= 0.0 {
        return Err(anyhow::anyhow!("--hlg-peak-nits must be greater than 0"));
    }

    println!(
        "HDR Analyzer MVP (Native Pipeline) - Starting analysis of: {}",
        input_path
    );

    let (video_info, input_context) = get_native_video_info(&input_path)?;
    println!(
        "Video resolution: {}x{}",
        video_info.width, video_info.height
    );
    if let Some(frames) = video_info.total_frames {
        println!("Total frames: {}", frames);
    }

    run(&cli, &video_info, input_context)?;

    println!("Native analysis complete!");
    Ok(())
}
