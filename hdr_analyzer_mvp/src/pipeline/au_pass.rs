//! `--au-table-only`: the AU table without measurements (for an imported `RPU.bin`, P15.4).
//! Every frame is still decoded, because the presentation-order check needs the decoder's
//! output order.

use std::path::Path;

use anyhow::{Context, Result};
use ffmpeg_next::{format, frame};

use crate::au_table::{measured_video_stream, write_au_table, AuRecorder, SceneSettings};
use crate::cli::Cli;
use crate::ffmpeg_io::{open_software_decoder, setup_hardware_decoder};

/// The scene detector settings of this run; the sidecar does not record them.
pub(super) fn scene_settings(cli: &Cli) -> SceneSettings {
    SceneSettings {
        detector: "global".to_owned(),
        metric: cli.scene_metric.to_lowercase(),
        threshold: cli.scene_threshold,
        min_length: cli.min_scene_length,
    }
}

pub(super) fn run_au_table_only(
    cli: &Cli,
    input_path: &Path,
    input_context: &mut format::context::Input,
    au_path: &Path,
) -> Result<()> {
    let mut recorder = AuRecorder::new(input_path, input_context)?;
    let stream = measured_video_stream(input_context)?;
    let video_stream_index = stream.index();
    let parameters = stream.parameters();
    let mut decoder = match cli.hwaccel.as_deref() {
        Some(hwaccel) => setup_hardware_decoder(&parameters, hwaccel)?,
        None => open_software_decoder(&parameters)?,
    };
    println!("Writing the AU table only: decoding every frame, measuring nothing...");

    let mut output_pts = Vec::new();
    let mut decoded = frame::Video::empty();
    for (stream, packet) in input_context.packets() {
        if stream.index() != video_stream_index {
            continue;
        }
        recorder.observe_packet(
            packet.pts(),
            packet.position() as i64,
            packet.data().unwrap_or_default(),
        );
        decoder
            .send_packet(&packet)
            .context("Failed to send packet to decoder")?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            output_pts.push(decoded.pts());
        }
    }
    decoder
        .send_eof()
        .context("Failed to send EOF to decoder")?;
    while decoder.receive_frame(&mut decoded).is_ok() {
        output_pts.push(decoded.pts());
    }

    recorder.set_output_pts(output_pts);
    let table = recorder
        .finish(None, scene_settings(cli))
        .context("No AU table written")?;
    write_au_table(au_path, &table)?;
    println!(
        "Wrote AU table: {} ({} access units)",
        au_path.display(),
        table.aus.count
    );
    Ok(())
}
