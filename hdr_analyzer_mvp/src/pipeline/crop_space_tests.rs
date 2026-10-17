use super::*;

#[test]
fn cpu_crop_maps_back_to_full_resolution() {
    let letterbox = CropRect {
        x: 0,
        y: 140,
        width: 1920,
        height: 800,
    };
    let full = crop_to_full_resolution(letterbox, 2, (1920, 1080), (3840, 2160));
    assert_eq!(
        (full.x, full.y, full.width, full.height),
        (0, 280, 3840, 1600)
    );
    let unscaled = crop_to_full_resolution(letterbox, 1, (1920, 1080), (1920, 1080));
    assert_eq!(unscaled.height, 800);
}

#[test]
fn edge_touching_crop_snaps_to_odd_source_border() {
    let full_frame = CropRect::full(958, 538);
    let mapped = crop_to_full_resolution(full_frame, 2, (958, 538), (1917, 1077));
    assert_eq!(
        (mapped.x, mapped.y, mapped.width, mapped.height),
        (0, 0, 1917, 1077)
    );
}
