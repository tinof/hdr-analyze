mod format;
mod probe;
mod rpu_config;
mod sidecar;
mod static_metadata;

pub use self::format::*;
pub use self::probe::*;
pub use self::rpu_config::*;
pub use self::sidecar::*;
pub use self::static_metadata::*;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum HdrFormat {
    Hdr10Plus,
    Hdr10WithMeasurements,
    Hdr10Unsupported,
    Hlg,
    DolbyVisionMel,
    DolbyVisionFel,
    DolbyVisionP8,
    Unsupported,
}

impl HdrFormat {
    #[allow(dead_code)]
    pub fn name(&self) -> &'static str {
        match self {
            HdrFormat::Hdr10Plus => "HDR10+",
            HdrFormat::Hdr10WithMeasurements => "HDR10 (with measurements)",
            HdrFormat::Hdr10Unsupported => "HDR10 (no measurements)",
            HdrFormat::Hlg => "HLG",
            HdrFormat::DolbyVisionMel => "Dolby Vision Profile 7 MEL",
            HdrFormat::DolbyVisionFel => "Dolby Vision Profile 7 FEL",
            HdrFormat::DolbyVisionP8 => "Dolby Vision Profile 8",
            HdrFormat::Unsupported => "Unsupported",
        }
    }
}

#[cfg(test)]
mod tests;
