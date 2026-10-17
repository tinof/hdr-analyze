use super::*;
use std::collections::VecDeque;

#[test]
fn average_smoothing_has_no_scene_initialization_bias() {
    let mut ema = None;
    let mut history = VecDeque::new();
    assert_eq!(smooth_average(0.75, &mut ema, &mut history, 0.1, 3), 0.75);
}
