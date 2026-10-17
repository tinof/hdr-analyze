use super::*;

fn frame(bin: usize) -> MadVRFrame {
    let mut lum_histogram = vec![0.0; 256];
    lum_histogram[bin] = 100.0;
    MadVRFrame {
        lum_histogram,
        ..Default::default()
    }
}

/// With `--sample-rate 3` the frames in between hold a copy of the last analyzed frame.
/// The shot changes at source frame 40; the first analyzed frame of the new shot is 42.
#[test]
fn sampled_analysis_cuts_at_the_first_analyzed_frame_of_a_shot() {
    let frames: Vec<MadVRFrame> = (0..90)
        .map(|index| frame(if index - index % 3 < 40 { 30 } else { 180 }))
        .collect();
    let (cuts, frame_indices, series) = detect_scene_cuts(&frames, 3, 3.0, 12);
    assert_eq!(cuts, vec![42]);
    assert_eq!(frame_indices.len(), 30);
    assert_eq!(series.diff.len(), 30);

    // Every frame analyzed: the copies make the picture change at 42 as well.
    let (cuts, _, _) = detect_scene_cuts(&frames, 1, 3.0, 12);
    assert_eq!(cuts, vec![42]);
}
