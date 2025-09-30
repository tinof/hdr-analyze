use crate::crop::CropRect;
use ffmpeg_next::frame;

// --- Constants for PQ Conversion ---
const ST2084_Y_MAX: f64 = 10000.0;
const ST2084_M1: f64 = 2610.0 / 16384.0;
const ST2084_M2: f64 = (2523.0 / 4096.0) * 128.0;
const ST2084_C1: f64 = 3424.0 / 4096.0;
const ST2084_C2: f64 = (2413.0 / 4096.0) * 32.0;
const ST2084_C3: f64 = (2392.0 / 4096.0) * 32.0;

/* --- Formulas --- */
#[inline]
pub fn nits_to_pq(nits: f64) -> f64 {
    let y = (nits / ST2084_Y_MAX).max(0.0);
    ((ST2084_C1 + ST2084_C2 * y.powf(ST2084_M1)) / (1.0 + ST2084_C3 * y.powf(ST2084_M1)))
        .powf(ST2084_M2)
}

/// Calculate average PQ from histogram data.
///
/// The histogram represents PQ values directly, with each bin corresponding to a PQ range.
/// This function computes a weighted average where each bin's contribution is proportional
/// to the percentage of pixels it contains.
///
/// # Arguments
/// * `histogram` - Array of 256 values representing pixel percentages for each PQ bin
///
/// # Returns
/// Weighted average PQ value in range [0.0, 1.0]
#[allow(dead_code)]
pub fn calculate_avg_pq_from_histogram(histogram: &[f64]) -> f64 {
    let mut weighted_sum = 0.0;
    let mut total_weight = 0.0;

    for (bin_index, &weight) in histogram.iter().enumerate() {
        if weight > 0.0 {
            // Convert bin index back to PQ value
            // Each bin represents a PQ range from 0.0 to 1.0
            let pq_value = (bin_index as f64) / 255.0;

            weighted_sum += pq_value * weight;
            total_weight += weight;
        }
    }

    if total_weight > 0.0 {
        weighted_sum / total_weight
    } else {
        0.0
    }
}

/// Convert PQ value back to nits (inverse PQ function).
///
/// This function implements the inverse ST.2084 EOTF to convert PQ code values
/// back to absolute luminance values in nits.
///
/// # Arguments
/// * `pq` - PQ value in range [0.0, 1.0]
///
/// # Returns
/// Luminance value in nits (cd/m²)
pub fn pq_to_nits(pq: f64) -> f64 {
    if pq <= 0.0 {
        return 0.0;
    }

    let y = ((pq.powf(1.0 / ST2084_M2) - ST2084_C1).max(0.0)
        / (ST2084_C2 - ST2084_C3 * pq.powf(1.0 / ST2084_M2)))
    .powf(1.0 / ST2084_M1);
    y * ST2084_Y_MAX
}

/// Find the 99th percentile (highlight knee) from the luminance histogram.
///
/// This function identifies the luminance level below which 99% of pixels fall,
/// which is useful for determining appropriate tone mapping targets while
/// preserving highlight detail.
///
/// # Arguments
/// * `histogram` - Array of 256 values representing pixel percentages for each PQ bin
///
/// # Returns
/// Luminance value in nits representing the 99th percentile
pub fn find_highlight_knee_nits(histogram: &[f64]) -> f64 {
    let mut cumulative_percentage = 0.0;

    // Start from the highest bin and work backwards
    for (bin_index, &percentage) in histogram.iter().enumerate().rev() {
        cumulative_percentage += percentage;

        // When we reach 1% (99th percentile), this is our highlight knee
        if cumulative_percentage >= 1.0 {
            // Convert bin index back to approximate nits value
            let pq_value = (bin_index as f64) / 255.0;
            return pq_to_nits(pq_value);
        }
    }

    // Fallback if no significant highlights found
    1000.0
}

/// Compute a percentile value from the histogram, returning PQ code.
///
/// This function finds the PQ value at which the specified percentile of pixels fall below.
/// Used for robust peak detection that's less sensitive to noise than direct max.
///
/// # Arguments
/// * `histogram` - Array of 256 values representing pixel percentages for each PQ bin
/// * `percentile` - Target percentile (0.0-100.0), e.g., 99.0 for P99, 99.9 for P99.9
///
/// # Returns
/// PQ value (0.0-1.0) at the requested percentile
pub fn compute_histogram_percentile_pq(histogram: &[f64], percentile: f64) -> f64 {
    let target_cumulative = 100.0 - percentile; // Convert to upper tail
    let mut cumulative_percentage = 0.0;

    // Start from the highest bin and work backwards
    for (bin_index, &percentage) in histogram.iter().enumerate().rev() {
        cumulative_percentage += percentage;

        if cumulative_percentage >= target_cumulative {
            // Convert bin index back to PQ value
            return (bin_index as f64) / 255.0;
        }
    }

    // Fallback: return max bin with data
    for (bin_index, &percentage) in histogram.iter().enumerate().rev() {
        if percentage > 0.0 {
            return (bin_index as f64) / 255.0;
        }
    }

    1.0 // Ultimate fallback
}

/// Select peak PQ based on the specified peak source method.
///
/// # Arguments
/// * `histogram` - Luminance histogram
/// * `direct_max_pq` - Peak PQ from direct frame analysis
/// * `peak_source` - Method to use: "max", "histogram99", or "histogram999"
///
/// # Returns
/// Peak PQ value selected by the specified method
pub fn select_peak_pq(histogram: &[f64], direct_max_pq: f64, peak_source: &str) -> f64 {
    match peak_source {
        "histogram99" => compute_histogram_percentile_pq(histogram, 99.0),
        "histogram999" => compute_histogram_percentile_pq(histogram, 99.9),
        _ => direct_max_pq, // "max" or unknown defaults to direct max
    }
}

/// Apply exponential moving average (EMA) smoothing to histogram bins.
///
/// This reduces frame-to-frame noise in the histogram while preserving temporal trends.
/// Uses the formula: smoothed[i] = beta * current[i] + (1-beta) * ema_state[i]
///
/// # Arguments
/// * `histogram` - Current frame's histogram (will be smoothed in-place)
/// * `ema_state` - EMA state from previous frame (updated in-place)
/// * `beta` - EMA coefficient (0.0-1.0). Lower = more smoothing. 0 disables.
///
/// # Notes
/// The histogram is renormalized after smoothing to maintain sum ≈ 100.0
pub fn apply_histogram_ema(histogram: &mut [f64], ema_state: &mut [f64], beta: f64) {
    if beta <= 0.0 || beta > 1.0 {
        return; // No smoothing
    }

    for i in 0..histogram.len().min(ema_state.len()) {
        ema_state[i] = beta * histogram[i] + (1.0 - beta) * ema_state[i];
        histogram[i] = ema_state[i];
    }

    // Renormalize to maintain sum ≈ 100.0
    let sum: f64 = histogram.iter().sum();
    if sum > 0.0 {
        let scale = 100.0 / sum;
        for bin in histogram.iter_mut() {
            *bin *= scale;
        }
    }
}

/// Apply temporal median filter to histogram bins.
///
/// This further reduces noise by taking the median of the last N frames for each bin.
/// More aggressive than EMA but can introduce slight lag.
///
/// # Arguments
/// * `histogram` - Current frame's histogram (will be replaced with median)
/// * `history` - Ring buffer of recent histograms (oldest to newest)
/// * `window_size` - Number of frames to use for median (e.g., 3)
///
/// # Notes
/// Modifies histogram in-place. History should contain at least 1 histogram.
pub fn apply_histogram_temporal_median(histogram: &mut [f64], history: &[Vec<f64>]) {
    if history.is_empty() {
        return;
    }

    let hist_len = histogram.len();
    for bin_idx in 0..hist_len {
        // Collect values for this bin across all frames in history + current
        let mut values: Vec<f64> = history.iter().map(|h| h[bin_idx]).collect();
        values.push(histogram[bin_idx]);

        // Compute median
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = values.len() / 2;
        histogram[bin_idx] = if values.len() % 2 == 0 {
            (values[mid - 1] + values[mid]) / 2.0
        } else {
            values[mid]
        };
    }

    // Renormalize
    let sum: f64 = histogram.iter().sum();
    if sum > 0.0 {
        let scale = 100.0 / sum;
        for bin in histogram.iter_mut() {
            *bin *= scale;
        }
    }
}

/// Compute hue histogram (31 bins) from chroma planes (U and V).
///
/// This function generates a hue distribution histogram by analyzing the color
/// information in the U (Cb) and V (Cr) chroma planes. The hue angle is computed
/// from the chroma components and quantized into 31 bins covering the full 360-degree
/// hue circle.
///
/// # Arguments
/// * `frame` - Native FFmpeg video frame in YUV420P10LE format
/// * `crop_rect` - Active area to analyze (chroma is subsampled 2x2)
///
/// # Returns
/// Vector of 31 f64 values representing percentage distribution across hue bins
pub fn compute_hue_histogram(frame: &frame::Video, crop_rect: &CropRect) -> Vec<f64> {
    const HUE_BINS: usize = 31;
    let mut hue_histogram = vec![0.0; HUE_BINS];

    // U and V planes (4:2:0 subsampled, so dimensions are halved)
    let u_plane = frame.data(1);
    let v_plane = frame.data(2);
    let u_stride = frame.stride(1);
    let v_stride = frame.stride(2);

    // Chroma coordinates (subsampled 2x2, so divide by 2)
    let cx_start = (crop_rect.x / 2) as usize;
    let cy_start = (crop_rect.y / 2) as usize;
    let cx_end = cx_start + (crop_rect.width / 2) as usize;
    let cy_end = cy_start + (crop_rect.height / 2) as usize;

    let mut total_pixels = 0u64;

    for cy in cy_start..cy_end {
        let u_row_base = cy * u_stride + cx_start * 2;
        let v_row_base = cy * v_stride + cx_start * 2;

        if u_row_base >= u_plane.len() || v_row_base >= v_plane.len() {
            continue;
        }

        let u_row_end = (u_row_base + (cx_end - cx_start) * 2).min(u_plane.len());
        let v_row_end = (v_row_base + (cx_end - cx_start) * 2).min(v_plane.len());

        let u_row = &u_plane[u_row_base..u_row_end];
        let v_row = &v_plane[v_row_base..v_row_end];

        for (u_px, v_px) in u_row.chunks_exact(2).zip(v_row.chunks_exact(2)) {
            // Read 10-bit chroma values (0..1023 in limited range, nominal 64-960)
            let u_code = u16::from_le_bytes([u_px[0], u_px[1]]) & 0x03FF;
            let v_code = u16::from_le_bytes([v_px[0], v_px[1]]) & 0x03FF;

            // Center around 512 (neutral chroma in 10-bit limited range)
            let u_centered = (u_code as i32) - 512;
            let v_centered = (v_code as i32) - 512;

            // Skip near-zero chroma (grayscale/low saturation pixels don't contribute to hue)
            let chroma_magnitude =
                ((u_centered * u_centered + v_centered * v_centered) as f64).sqrt();
            if chroma_magnitude < 10.0 {
                continue;
            }

            // Compute hue angle in degrees [0, 360)
            let hue_radians = (v_centered as f64).atan2(u_centered as f64);
            let hue_degrees = (hue_radians.to_degrees() + 360.0) % 360.0;

            // Map to bin [0, 30]
            let bin = ((hue_degrees * HUE_BINS as f64) / 360.0).floor() as usize;
            let bin = bin.min(HUE_BINS - 1);

            hue_histogram[bin] += 1.0;
            total_pixels += 1;
        }
    }

    // Normalize to percentages
    if total_pixels > 0 {
        let total = total_pixels as f64;
        for bin in &mut hue_histogram {
            *bin = (*bin / total) * 100.0;
        }
    }

    hue_histogram
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pq_nits_round_trip() {
        // Test PQ <-> nits conversion round-trip for common values
        let test_values = vec![0.0, 10.0, 100.0, 1000.0, 4000.0, 10000.0];
        for nits in test_values {
            let pq = nits_to_pq(nits);
            let back_to_nits = pq_to_nits(pq);
            let error = (nits - back_to_nits).abs();
            assert!(
                error < 0.1 || error / nits.max(1.0) < 0.001,
                "Round-trip failed: {} -> {} -> {}, error: {}",
                nits,
                pq,
                back_to_nits,
                error
            );
        }
    }

    #[test]
    fn test_histogram_binning_v5_boundaries() {
        // Test v5 histogram bin boundaries
        let sdr_peak_pq = nits_to_pq(100.0);
        let sdr_step = sdr_peak_pq / 64.0;
        let hdr_step = (1.0 - sdr_peak_pq) / 192.0;

        // Bin 0 should be near PQ 0
        assert!(sdr_step > 0.0 && sdr_step < 0.01);

        // Bin 64 should be at SDR peak (100 nits)
        let bin_64_pq = sdr_peak_pq;
        let bin_64_nits = pq_to_nits(bin_64_pq);
        assert!((bin_64_nits - 100.0).abs() < 1.0);

        // Bin 255 should be at PQ 1.0 (10000 nits)
        let bin_255_pq = sdr_peak_pq + 192.0 * hdr_step;
        assert!((bin_255_pq - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_find_highlight_knee() {
        // Test highlight knee detection with synthetic histogram
        let mut histogram = vec![0.0; 256];
        // 98% in lower bins, 2% in high bins
        histogram[100] = 98.0;
        histogram[250] = 1.5;
        histogram[255] = 0.5;

        let knee_nits = find_highlight_knee_nits(&histogram);
        // Should find the knee around bins 250-255 (high nits)
        assert!(
            knee_nits > 1000.0,
            "Knee should be in high range, got {} nits",
            knee_nits
        );
    }
}
