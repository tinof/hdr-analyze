use dolby_vision::rpu::rpu_data_mapping::RpuDataMapping;
use dovi84_composer::{canary_mapping, check_mapping_writable, Composer, DEFAULT_CANARY_OFFSET};

use crate::error::{Error, Result};

/// The Profile 8.4 composer (reshaping curves) an RPU carries.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum HlgComposer {
    /// The `dolby_vision` crate's `Profile84` preset, which `dovi_tool generate` writes.
    Preset,
    /// The BT.2100-fitted composer, mkvdovi's default (`--hlg-composer bt2100`).
    #[default]
    Bt2100,
    /// The P8 playback canary: `Bt2100` with both chroma MMR constants shifted by `offset`
    /// (normalized; within [-1, 1], not rounding to zero), as `fit_hlg_composer canary-rpu`
    /// writes it. A device that applies the RPU's composer shows the picture tinted (green for a
    /// negative offset). Diagnostic only: mkvdovi never writes it.
    Canary { offset: f64 },
}

impl HlgComposer {
    /// The canary at its default offset, −1/16 (about −64 10-bit codes): green.
    pub fn canary() -> Self {
        HlgComposer::Canary {
            offset: DEFAULT_CANARY_OFFSET,
        }
    }

    /// The composer's reshaping curves, checked to be writable by the `dolby_vision` crate.
    pub fn rpu_data_mapping(self) -> Result<RpuDataMapping> {
        let mapping = match self {
            HlgComposer::Preset => Composer::Preset.rpu_data_mapping(),
            HlgComposer::Bt2100 => Composer::Bt2100V1.rpu_data_mapping(),
            HlgComposer::Canary { offset } => canary_mapping(offset)
                .map_err(|error| Error::InvalidComposer(format!("{error:#}")))?,
        };
        check_mapping_writable(&mapping)
            .map_err(|error| Error::InvalidComposer(format!("{error:#}")))?;
        Ok(mapping)
    }
}

impl From<Composer> for HlgComposer {
    fn from(composer: Composer) -> Self {
        match composer {
            Composer::Preset => HlgComposer::Preset,
            Composer::Bt2100V1 => HlgComposer::Bt2100,
        }
    }
}
