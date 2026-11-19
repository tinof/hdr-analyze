//! The three curves and one frame's metadata on one display. The curve maths is libplacebo's
//! (`lp`, `tm_shim.c`); this module names the curves, checks the numbers before they reach C,
//! and converts between PQ and displayed nits.

use anyhow::{bail, ensure, Result};
use clap::ValueEnum;

#[cfg(test)]
use crate::lp::CurveParams;
use crate::lp::{self, ToneMap};

/// A libplacebo 7.387.0 tone-mapping curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Curve {
    /// libplacebo's single-pivot hyperbolic spline (its default curve).
    Spline,
    /// libplacebo's `st2094-10`: a three-point rational curve "inspired by SMPTE ST2094-10,
    /// with some modifications", not the standard's annex.
    #[value(name = "st2094-10")]
    St2094_10,
    /// libplacebo's ITU-R BT.2390 EETF with its black-point gain, knee offset 0.5 (ITU).
    Bt2390,
}

impl Curve {
    pub const ALL: [Self; 3] = [Self::Spline, Self::St2094_10, Self::Bt2390];

    /// The name `pl_find_tone_map_function` knows.
    pub fn name(self) -> &'static str {
        match self {
            Self::Spline => "spline",
            Self::St2094_10 => "st2094-10",
            Self::Bt2390 => "bt2390",
        }
    }

    /// The label every report prints.
    pub fn label(self) -> &'static str {
        match self {
            Self::Spline => "spline (libplacebo)",
            Self::St2094_10 => "st2094-10 (libplacebo form)",
            Self::Bt2390 => "bt2390 (libplacebo form, knee offset 0.5)",
        }
    }
}

/// The display a run simulates and the source black every frame shares.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    pub source_black_nits: f64,
    pub panel_black_nits: f64,
    pub panel_peak_nits: f64,
}

impl Display {
    /// Blacks must lie below the panel peak and below 100 nits, the least source peak
    /// libplacebo infers, so every curve has a range to map; the peak at most 10,000 nits.
    pub fn validate(&self) -> Result<()> {
        let Self {
            source_black_nits: source,
            panel_black_nits: black,
            panel_peak_nits: peak,
        } = *self;
        ensure!(
            peak.is_finite() && peak > 0.0 && peak <= 10_000.0,
            "panel peak {peak} nits is outside (0, 10000]"
        );
        let ceiling = peak.min(100.0);
        for (name, value) in [("source black", source), ("panel black", black)] {
            ensure!(
                value.is_finite() && (0.0..ceiling).contains(&value),
                "{name} {value} nits must be at least 0 and below {ceiling} nits \
                 (the panel peak and 100 nits)"
            );
        }
        Ok(())
    }
}

/// One frame's L1 max and avg as 12-bit PQ codes (fractions allowed); avg 0 or less: absent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metadata {
    pub max_code: f64,
    pub avg_code: f64,
}

impl Metadata {
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [("L1 max", self.max_code), ("L1 avg", self.avg_code)] {
            if !(value.is_finite() && (0.0..=4095.0).contains(&value)) {
                bail!("{name} {value} is not a 12-bit PQ code (0 to 4095)");
            }
        }
        Ok(())
    }
}

/// A curve set up for one frame's metadata on one display, as libplacebo's renderer does.
#[derive(Debug)]
pub struct Mapping {
    tone_map: ToneMap,
}

impl Mapping {
    pub fn new(curve: Curve, display: &Display, metadata: &Metadata) -> Result<Self> {
        display.validate()?;
        metadata.validate()?;
        let avg = if metadata.avg_code > 0.0 {
            metadata.avg_code / 4095.0
        } else {
            0.0
        };
        let tone_map = ToneMap::new(
            curve.name(),
            display.source_black_nits as f32,
            (metadata.max_code / 4095.0) as f32,
            avg as f32,
            display.panel_black_nits as f32,
            display.panel_peak_nits as f32,
        )?;
        Ok(Self { tone_map })
    }

    /// Displayed nits of source PQ `x` (0..1).
    pub fn displayed_nits(&self, x: f64) -> f64 {
        f64::from(lp::pq_to_nits(self.tone_map.sample(x as f32)))
    }

    /// Source PQ above which every value displays the same (the display peak on a no-op).
    pub fn clip_ceiling(&self) -> f64 {
        f64::from(self.tone_map.clip_ceiling())
    }

    pub fn is_noop(&self) -> bool {
        self.tone_map.is_noop()
    }

    #[cfg(test)]
    pub fn params(&self) -> CurveParams {
        self.tone_map.params()
    }
}

/// libplacebo's inverse PQ EOTF (f32): nits to PQ.
pub fn nits_to_pq(nits: f64) -> f64 {
    f64::from(lp::nits_to_pq(nits as f32))
}

#[cfg(test)]
#[path = "curves_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "gate_tests.rs"]
mod gate_tests;
