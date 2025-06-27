4. Native Rust Implementation (Maximum Performance)
Generated rust
use ffmpeg_next as ffmpeg;
use ndarray::{Array2, ArrayView2};

pub struct NativeHDRSceneDetector {
    // Based on madVR's approach
    histogram_bins: usize, // 256 bins
    rolling_window: usize, // 240 frames
    scene_threshold: f64,
}

impl NativeHDRSceneDetector {
    pub fn process_video(&mut self, path: &str) -> Result<Vec<Scene>> {
        let mut scenes = Vec::new();
        let mut decoder = self.setup_hdr_decoder(path)?;
        
        let mut prev_histogram = Array2::<f32>::zeros((256, 1));
        let mut frame_buffer = VecDeque::new();
        
        while let Some(frame) = decoder.next_frame()? {
            // 1. Extract HDR frame in native format (10-bit)
            let hdr_frame = self.decode_hdr_frame(&frame)?;
            
            // 2. Generate PQ-encoded histogram
            let histogram = self.generate_pq_histogram(&hdr_frame)?;
            
            // 3. Calculate scene change metric
            let diff = self.histogram_difference(&histogram, &prev_histogram);
            
            // 4. Apply temporal smoothing
            frame_buffer.push_back((frame.timestamp(), diff));
            if frame_buffer.len() > self.rolling_window {
                frame_buffer.pop_front();
            }
            
            // 5. Detect scene boundaries
            if self.is_scene_change(&frame_buffer) {
                scenes.push(Scene {
                    start: frame.timestamp(),
                    histogram: histogram.clone(),
                });
            }
            
            prev_histogram = histogram;
        }
        
        Ok(scenes)
    }
    
    fn generate_pq_histogram(&self, frame: &HDRFrame) -> Result<Array2<f32>> {
        // Implement PQ-space histogram generation
        // This matches madVR's approach
    }
}
Use code with caution.
Rust
Recommendation for Your Project
Given that you're building the madVR measurement file format equivalent, I'd recommend:
Start with PySceneDetect bindings for rapid prototyping
Migrate to native Rust implementation for production, implementing:
PQ-encoded histogram generation (matching madVR's 256 bins)
Rolling average scene detection (240 frames default)
Black bar detection before histogram calculation
Temporal coherence checks
Key HDR Scene Detection Considerations
Generated rust
// Critical: Work in PQ space, not gamma space
fn rgb_to_pq_luminance(r: f32, g: f32, b: f32) -> f32 {
    let y = 0.2627 * r + 0.6780 * g + 0.0593 * b; // BT.2020 coefficients
    
    // Apply PQ EOTF
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    
    let y_norm = y / 10000.0; // Normalize to 0-1
    ((c1 + c2 * y_norm.powf(m1)) / (1.0 + c3 * y_norm.powf(m1))).powf(m2)
}
Use code with caution.
Rust
The native Rust approach will give you the most control and best performance for your HDR analysis tool, matching the madVR methodology while avoiding FFmpeg's HDR limitations.


 Native Rust Implementation (Maximum Performance)
rustuse ffmpeg_next as ffmpeg;
use ndarray::{Array2, ArrayView2};

pub struct NativeHDRSceneDetector {
    // Based on madVR's approach
    histogram_bins: usize, // 256 bins
    rolling_window: usize, // 240 frames
    scene_threshold: f64,
}

impl NativeHDRSceneDetector {
    pub fn process_video(&mut self, path: &str) -> Result<Vec<Scene>> {
        let mut scenes = Vec::new();
        let mut decoder = self.setup_hdr_decoder(path)?;
        
        let mut prev_histogram = Array2::<f32>::zeros((256, 1));
        let mut frame_buffer = VecDeque::new();
        
        while let Some(frame) = decoder.next_frame()? {
            // 1. Extract HDR frame in native format (10-bit)
            let hdr_frame = self.decode_hdr_frame(&frame)?;
            
            // 2. Generate PQ-encoded histogram
            let histogram = self.generate_pq_histogram(&hdr_frame)?;
            
            // 3. Calculate scene change metric
            let diff = self.histogram_difference(&histogram, &prev_histogram);
            
            // 4. Apply temporal smoothing
            frame_buffer.push_back((frame.timestamp(), diff));
            if frame_buffer.len() > self.rolling_window {
                frame_buffer.pop_front();
            }
            
            // 5. Detect scene boundaries
            if self.is_scene_change(&frame_buffer) {
                scenes.push(Scene {
                    start: frame.timestamp(),
                    histogram: histogram.clone(),
                });
            }
            
            prev_histogram = histogram;
        }
        
        Ok(scenes)
    }
    
    fn generate_pq_histogram(&self, frame: &HDRFrame) -> Result<Array2<f32>> {
        // Implement PQ-space histogram generation
        // This matches madVR's approach
    }
}
Recommendation for Your Project
Given that you're building the madVR measurement file format equivalent, I'd recommend:

Start with PySceneDetect bindings for rapid prototyping
Migrate to native Rust implementation for production, implementing:

PQ-encoded histogram generation (matching madVR's 256 bins)
Rolling average scene detection (240 frames default)
Black bar detection before histogram calculation
Temporal coherence checks



Key HDR Scene Detection Considerations
rust// Critical: Work in PQ space, not gamma space
fn rgb_to_pq_luminance(r: f32, g: f32, b: f32) -> f32 {
    let y = 0.2627 * r + 0.6780 * g + 0.0593 * b; // BT.2020 coefficients
    
    // Apply PQ EOTF
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    
    let y_norm = y / 10000.0; // Normalize to 0-1
    ((c1 + c2 * y_norm.powf(m1)) / (1.0 + c3 * y_norm.powf(m1))).powf(m2)
}
The native Rust approach will give you the most control and best performance for your HDR analysis tool, matching the madVR methodology while avoiding FFmpeg's HDR limitations.