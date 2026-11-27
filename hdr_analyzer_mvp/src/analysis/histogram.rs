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

/// The 256-bin luminance histogram of the madVR v5 `.bin` (ROADMAP E8.1): bins 0 to 63 split PQ
/// [0, PQ(100 nits)) evenly, bins 64 to 255 split [PQ(100 nits), 1]. The CPU writer
/// (`analysis::frame`), the CUDA bin LUT (`analysis::gpu`) and every reader below share it, so a
/// reader returns the PQ of the bin a value was written to; before E8.1 the readers took bin `b`
/// as PQ `b / 255`, which read 1000 nits as about 305.
#[derive(Clone, Copy, Debug)]
pub struct LumBins {
    sdr_peak_pq: f64,
    sdr_step: f64,
    hdr_step: f64,
}

impl LumBins {
    /// Bins below PQ(100 nits).
    pub const SDR_BINS: usize = 64;
    /// Bins from PQ(100 nits) to PQ 1.
    pub const HDR_BINS: usize = 192;
    /// All bins.
    pub const BINS: usize = Self::SDR_BINS + Self::HDR_BINS;

    pub fn new() -> Self {
        let sdr_peak_pq = nits_to_pq(100.0);
        Self {
            sdr_peak_pq,
            sdr_step: sdr_peak_pq / Self::SDR_BINS as f64,
            hdr_step: (1.0 - sdr_peak_pq) / Self::HDR_BINS as f64,
        }
    }

    /// The bin of a PQ value, clamped to the last bin. The arithmetic is the writers' since
    /// v5, operation for operation, so the written `.bin` stays byte-identical.
    pub fn bin(&self, pq: f64) -> usize {
        let bin = if pq < self.sdr_peak_pq {
            (pq / self.sdr_step).floor() as usize
        } else {
            Self::SDR_BINS + ((pq - self.sdr_peak_pq) / self.hdr_step).floor() as usize
        };
        bin.min(Self::BINS - 1)
    }

    /// The PQ value at the lower edge of `bin`, the base of [`LumBins::center_pq`]. Not a
    /// reader: the product of a bin index and a step can land a hair below the writer's
    /// floor, so only the centre round-trips through [`LumBins::bin`].
    fn lower_pq(&self, bin: usize) -> f64 {
        if bin < Self::SDR_BINS {
            bin as f64 * self.sdr_step
        } else {
            self.sdr_peak_pq + (bin - Self::SDR_BINS) as f64 * self.hdr_step
        }
    }

    /// The PQ value at the centre of `bin`: what a reader takes for the pixels counted in it.
    pub fn center_pq(&self, bin: usize) -> f64 {
        let width = if bin < Self::SDR_BINS {
            self.sdr_step
        } else {
            self.hdr_step
        };
        self.lower_pq(bin) + width / 2.0
    }
}

impl Default for LumBins {
    fn default() -> Self {
        Self::new()
    }
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
    let bins = LumBins::new();
    let mut weighted_sum = 0.0;
    let mut total_weight = 0.0;

    for (bin_index, &weight) in histogram.iter().enumerate() {
        if weight > 0.0 {
            let pq_value = bins.center_pq(bin_index);

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
    let bins = LumBins::new();
    let mut cumulative_percentage = 0.0;

    // Start from the highest bin and work backwards
    for (bin_index, &percentage) in histogram.iter().enumerate().rev() {
        cumulative_percentage += percentage;

        // When we reach 1% (99th percentile), this is our highlight knee
        if cumulative_percentage >= 1.0 {
            return pq_to_nits(bins.center_pq(bin_index));
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
    let bins = LumBins::new();
    let target_cumulative = 100.0 - percentile; // Convert to upper tail
    let mut cumulative_percentage = 0.0;

    // Start from the highest bin and work backwards
    for (bin_index, &percentage) in histogram.iter().enumerate().rev() {
        cumulative_percentage += percentage;

        if cumulative_percentage >= target_cumulative {
            return bins.center_pq(bin_index);
        }
    }

    // Fallback: return max bin with data
    for (bin_index, &percentage) in histogram.iter().enumerate().rev() {
        if percentage > 0.0 {
            return bins.center_pq(bin_index);
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
    // A percentile is read at its bin's centre, which can lie above every pixel of the frame
    // (up to half a bin), so it never exceeds the direct maximum (ROADMAP E8.1).
    match peak_source {
        "histogram99" => compute_histogram_percentile_pq(histogram, 99.0).min(direct_max_pq),
        "histogram999" => compute_histogram_percentile_pq(histogram, 99.9).min(direct_max_pq),
        _ => direct_max_pq, // "max" or unknown defaults to direct max
    }
}

/// The source [`select_peak_pq`] applies for `peak_source`, which treats an unknown name as
/// the direct maximum.
pub fn applied_peak_source(peak_source: &str) -> &'static str {
    match peak_source {
        "histogram99" => "histogram99",
        "histogram999" => "histogram999",
        _ => "max",
    }
}

/// Apply exponential moving average (EMA) smoothing to histogram bins.
///
/// This reduces frame-to-frame noise in the histogram while preserving temporal trends.
/// Uses the formula: `smoothed[i] = beta * current[i] + (1-beta) * ema_state[i]`
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
/// Bins of the hue histogram over the full hue circle.
pub const HUE_BINS: usize = 31;
/// [`hue_lut`] entry of a chroma pair too close to neutral to have a hue.
pub const HUE_SKIP: u8 = u8::MAX;

/// The hue bin of a 10-bit chroma pair, or [`HUE_SKIP`] when its magnitude is under 10 codes:
/// the angle in f64 degrees, as the analyzer has always binned it on the CPU.
pub fn hue_bin(u_code: u16, v_code: u16) -> u8 {
    // Center around 512 (neutral chroma in 10-bit limited range)
    let u_centered = i32::from(u_code) - 512;
    let v_centered = i32::from(v_code) - 512;

    // Skip near-zero chroma (grayscale/low saturation pixels don't contribute to hue)
    let chroma_magnitude = f64::from(u_centered * u_centered + v_centered * v_centered).sqrt();
    if chroma_magnitude < 10.0 {
        return HUE_SKIP;
    }

    // Compute hue angle in degrees [0, 360)
    let hue_radians = f64::from(v_centered).atan2(f64::from(u_centered));
    let hue_degrees = (hue_radians.to_degrees() + 360.0) % 360.0;

    // Map to bin [0, 30]
    let bin = ((hue_degrees * HUE_BINS as f64) / 360.0).floor() as usize;
    bin.min(HUE_BINS - 1) as u8
}

/// [`hue_bin`] of every 10-bit chroma pair, indexed `(u_code << 10) | v_code` (1 MiB). The CPU and
/// the CUDA kernel both read this one table, so they bin every pixel alike (ROADMAP E12.3): the
/// kernel's own f32 `atan2f` put 2 of the 2^20 pairs one bin off.
pub fn hue_lut() -> &'static [u8] {
    static LUT: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        (0..1u32 << 20)
            .map(|index| hue_bin((index >> 10) as u16, (index & 1023) as u16))
            .collect()
    })
}

pub fn compute_hue_histogram(frame: &frame::Video, crop_rect: &CropRect) -> Vec<f64> {
    let lut = hue_lut();
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

            let bin = lut[(usize::from(u_code) << 10) | usize::from(v_code)];
            if bin == HUE_SKIP {
                continue;
            }

            hue_histogram[usize::from(bin)] += 1.0;
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
    fn applied_peak_source_names_what_select_peak_pq_does() {
        let mut histogram = vec![0.0; 256];
        histogram[100] = 99.5;
        histogram[200] = 0.5;
        let direct = 0.9;
        let select = |source| select_peak_pq(&histogram, direct, source);
        // The three sources must give different peaks for the comparison below to mean anything.
        assert_eq!(select("max"), direct);
        // The centre of bins 100 and 200 in the v5 layout (ROADMAP E8.1): 64 bins below
        // PQ(100 nits), 192 equal bins above it.
        let sdr_peak_pq = nits_to_pq(100.0);
        let hdr_step = (1.0 - sdr_peak_pq) / 192.0;
        let centre = |bin: f64| sdr_peak_pq + (bin - 64.0 + 0.5) * hdr_step;
        assert!((select("histogram99") - centre(100.0)).abs() < 1e-12);
        assert!((select("histogram999") - centre(200.0)).abs() < 1e-12);

        for (source, applied) in [
            ("max", "max"),
            ("histogram99", "histogram99"),
            ("histogram999", "histogram999"),
            ("Histogram99", "max"),
            ("bogus", "max"),
        ] {
            assert_eq!(applied_peak_source(source), applied, "{source}");
            assert_eq!(select(source), select(applied), "{source}");
        }
    }

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

    #[test]
    fn every_bin_reads_back_as_itself() {
        let bins = LumBins::new();
        let sdr_peak_pq = nits_to_pq(100.0);
        assert_eq!(bins.bin(0.0), 0);
        assert_eq!(
            bins.bin(sdr_peak_pq),
            64,
            "PQ(100 nits) opens the first HDR bin"
        );
        assert_eq!(bins.bin(1.0), 255);
        assert_eq!(bins.bin(2.0), 255, "above PQ 1 clamps to the last bin");
        let mut previous = -1.0;
        for bin in 0..LumBins::BINS {
            let centre = bins.center_pq(bin);
            assert_eq!(bins.bin(centre), bin, "bin {bin} centre {centre}");
            assert!(centre > previous, "bin {bin} centres must rise");
            previous = centre;
        }
    }

    #[test]
    fn readers_take_a_bin_at_the_pq_it_was_written_with() {
        // 1 % of the pixels at 1000 nits over a 10-nit picture. Before E8.1 the readers took
        // bin b as PQ b / 255 and read this as about 305 nits.
        let bins = LumBins::new();
        let mut histogram = vec![0.0; 256];
        histogram[bins.bin(nits_to_pq(10.0))] = 99.0;
        histogram[bins.bin(nits_to_pq(1000.0))] = 1.0;
        let knee = find_highlight_knee_nits(&histogram);
        assert!((knee - 1000.0).abs() < 30.0, "knee {knee} nits");
        let p99 = pq_to_nits(compute_histogram_percentile_pq(&histogram, 99.0));
        assert!((p99 - 1000.0).abs() < 30.0, "p99 {p99} nits");
    }

    #[test]
    fn a_percentile_peak_never_exceeds_the_brightest_pixel() {
        // A flat frame at exactly PQ(100 nits) opens bin 64, whose centre lies half a bin above
        // every pixel: the peak stays at the direct maximum.
        let bins = LumBins::new();
        let flat = nits_to_pq(100.0);
        let mut histogram = vec![0.0; 256];
        histogram[bins.bin(flat)] = 100.0;
        assert!(bins.center_pq(64) > flat);
        assert_eq!(select_peak_pq(&histogram, flat, "histogram99"), flat);
        assert_eq!(select_peak_pq(&histogram, flat, "histogram999"), flat);
        // Below the direct maximum the percentile itself applies.
        let direct = nits_to_pq(1000.0);
        let read = select_peak_pq(&histogram, direct, "histogram99");
        assert_eq!(read, bins.center_pq(64));
        assert!(read < direct);
    }
}
