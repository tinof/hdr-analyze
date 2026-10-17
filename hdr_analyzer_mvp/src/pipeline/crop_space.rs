use ffmpeg_next::frame;

use crate::crop::{detect_crop, is_frame_usable_for_crop, CropRect, CROP_EDGE_TOLERANCE};

pub(super) struct CropStabilityMonitor {
    checked_scenes: usize,
    matching_scenes: usize,
    full_frame_scenes: usize,
    skipped_scenes: usize,
}

impl CropStabilityMonitor {
    pub(super) fn new() -> Self {
        Self {
            checked_scenes: 0,
            matching_scenes: 0,
            full_frame_scenes: 0,
            skipped_scenes: 0,
        }
    }

    fn record(
        &mut self,
        committed: CropRect,
        candidate: CropRect,
        frame_width: u32,
        frame_height: u32,
    ) {
        self.checked_scenes += 1;
        if candidate.approximately_matches(committed, CROP_EDGE_TOLERANCE) {
            self.matching_scenes += 1;
        } else if candidate.approximately_matches(
            CropRect::full(frame_width, frame_height),
            CROP_EDGE_TOLERANCE,
        ) {
            self.full_frame_scenes += 1;
        }
    }

    fn record_skipped(&mut self) {
        self.skipped_scenes += 1;
    }

    pub(super) fn report(&self) {
        if self.checked_scenes == 0 {
            println!("Crop stability: no usable scene cuts were sampled.");
            return;
        }
        let stability = self.matching_scenes as f64 * 100.0 / self.checked_scenes as f64;
        let other_scenes = self.checked_scenes - self.matching_scenes - self.full_frame_scenes;
        println!(
            "Crop stability: {:.0}% of {} sampled scene cuts matched the committed crop; {} appeared full-frame; {} used another active area; {} skipped (low signal).",
            stability,
            self.checked_scenes,
            self.full_frame_scenes,
            other_scenes,
            self.skipped_scenes
        );
        if self.matching_scenes != self.checked_scenes {
            println!(
                "Variable active area detected; measurements use one conservative stream-level crop. Per-scene crop application is not enabled."
            );
        }
    }
}

/// Resolve the crop for the current frame, committing a hardened in-stream fallback
/// on the first usable frame when no probe-committed crop exists yet.
pub(super) fn resolve_crop_rect(
    crop_rect_opt: &mut Option<CropRect>,
    crop_monitor: &mut Option<CropStabilityMonitor>,
    analysis_frame: &frame::Video,
) -> CropRect {
    match *crop_rect_opt {
        Some(rect) => rect,
        None if is_frame_usable_for_crop(analysis_frame) => {
            let rect = detect_crop(analysis_frame);
            println!(
                "\nFallback active video area: {}x{} at offset ({}, {})",
                rect.width, rect.height, rect.x, rect.y
            );
            *crop_rect_opt = Some(rect);
            *crop_monitor = Some(CropStabilityMonitor::new());
            rect
        }
        None => CropRect::full(analysis_frame.width(), analysis_frame.height()),
    }
}

/// Sample crop stability at an accepted scene cut. Cuts landing on black/low-signal
/// frames are counted as skipped instead of polluting the variable-AR telemetry.
pub(super) fn sample_scene_cut_crop(
    crop_monitor: &mut Option<CropStabilityMonitor>,
    committed: CropRect,
    analysis_frame: &frame::Video,
) {
    let Some(monitor) = crop_monitor else {
        return;
    };
    if is_frame_usable_for_crop(analysis_frame) {
        monitor.record(
            committed,
            detect_crop(analysis_frame),
            analysis_frame.width(),
            analysis_frame.height(),
        );
    } else {
        monitor.record_skipped();
    }
}

pub(super) fn scale_rect(rect: CropRect, factor: u32) -> CropRect {
    CropRect {
        x: rect.x * factor,
        y: rect.y * factor,
        width: rect.width * factor,
        height: rect.height * factor,
    }
}

/// Map a crop from the CPU path's downscaled analysis space back to full-resolution source
/// coordinates. Edges that touch the analysis frame border snap to the source border so an
/// even-rounded analysis size never invents a 1-2 pixel offset.
pub(super) fn crop_to_full_resolution(
    rect: CropRect,
    downscale: u32,
    analysis_size: (u32, u32),
    full_size: (u32, u32),
) -> CropRect {
    if downscale <= 1 {
        return rect;
    }
    let (analysis_w, analysis_h) = analysis_size;
    let (full_w, full_h) = full_size;
    let x = (rect.x * downscale).min(full_w.saturating_sub(2));
    let y = (rect.y * downscale).min(full_h.saturating_sub(2));
    let right = if rect.x + rect.width >= analysis_w {
        full_w
    } else {
        ((rect.x + rect.width) * downscale).min(full_w)
    };
    let bottom = if rect.y + rect.height >= analysis_h {
        full_h
    } else {
        ((rect.y + rect.height) * downscale).min(full_h)
    };
    CropRect {
        x,
        y,
        width: right.saturating_sub(x).max(2),
        height: bottom.saturating_sub(y).max(2),
    }
}

pub(super) fn shrink_rect(rect: CropRect, factor: u32) -> CropRect {
    CropRect {
        x: rect.x / factor,
        y: rect.y / factor,
        width: (rect.width / factor).max(2),
        height: (rect.height / factor).max(2),
    }
}
