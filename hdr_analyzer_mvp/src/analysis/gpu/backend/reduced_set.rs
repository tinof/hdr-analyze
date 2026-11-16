//! `--peak-estimator st2094-10` on the GPU: the launch of `reduced_set_extrema` (kernels.cu).

use anyhow::{anyhow, Result};
use cudarc::driver::{LaunchConfig, PushKernelArg};

use super::{GpuAnalyzer, Planes, BLOCK_THREADS};
use crate::crop::CropRect;

impl GpuAnalyzer {
    /// Clear the reduced-set result and launch `reduced_set_extrema` on the analysis stream, so
    /// it is ordered after the uploads and before the download.
    pub(super) fn launch_reduced_set(
        &mut self,
        planes: &Planes,
        crop_rect: &CropRect,
    ) -> Result<()> {
        self.stream
            .memset_zeros(&mut self.extrema)
            .map_err(|err| anyhow!("failed to clear the reduced-set result: {err:?}"))?;
        let (crop_x, crop_y) = (crop_rect.x as i32, crop_rect.y as i32);
        let (crop_width, crop_height) = (crop_rect.width as i32, crop_rect.height as i32);
        let blocks = crop_rect.width.div_ceil(2) * crop_rect.height.div_ceil(2);
        let cfg = LaunchConfig {
            grid_dim: (
                blocks.div_ceil(BLOCK_THREADS).clamp(1, self.max_blocks),
                1,
                1,
            ),
            block_dim: (BLOCK_THREADS, 1, 1),
            shared_mem_bytes: 0,
        };
        let mut launch = self.stream.launch_builder(&self.reduced_set_kernel);
        launch
            .arg(&planes.y)
            .arg(&planes.u)
            .arg(&planes.v)
            .arg(&self.eotf_lut)
            .arg(&mut self.extrema)
            .arg(&planes.width)
            .arg(&planes.height)
            .arg(&planes.y_stride)
            .arg(&planes.u_stride)
            .arg(&planes.v_stride)
            .arg(&crop_x)
            .arg(&crop_y)
            .arg(&crop_width)
            .arg(&crop_height)
            .arg(&planes.layout);
        // SAFETY: the plane addresses are live device buffers (uploaded on this stream or an
        // NVDEC surface the caller keeps referenced), the table holds EOTF_LUT_INTERVALS + 1
        // values and `extrema` two words, as the kernel reads and writes them.
        unsafe { launch.launch(cfg) }
            .map_err(|err| anyhow!("CUDA reduced-set launch failed: {err:?}"))?;
        Ok(())
    }
}
