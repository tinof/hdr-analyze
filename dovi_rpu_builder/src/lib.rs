//! Dolby Vision Profile 8.1 and 8.4 RPU builders, in-process and without I/O: one complete RPU
//! NAL unit per access unit, byte-identical to what `dovi_tool generate` 2.3.4 writes for the
//! same configuration. Built for the NuvioTV fork, which adds an RPU to each access unit during
//! playback (its steps DV2.1 / DV2.2; JNI stays in the fork), and shared with mkvdovi, which
//! still runs `dovi_tool generate` but takes its `extra.json` from [`RpuParams::extra_json`].
//!
//! - [`RpuParams`]: everything but L1 (profile and composer, CM version and default blocks, L6,
//!   L5, the L1 average floor). The same defaults mkvdovi writes for the same source.
//! - [`RpuBuilder`] → [`ShotRpu`] → [`ShotRpu::write`]: the base once per params, the L1 once per
//!   shot, one write per access unit. [`build`] does all three.
//! - [`Hdr10PlusFrame`] / [`hdr10plus_l1`]: L1 from one HDR10+ payload, the [`SceneKey`] that
//!   tells when `dovi_tool` would start a new scene, and [`Hdr10PlusHold`], which applies that
//!   rule to frames in presentation order.
//! - [`STATIC_84_L1`] and [`static_84_params`]: a static HLG → 8.4 RPU.
//! - [`HlgComposer`]: preset, bt2100 (the default) and the P8 canary.
//!
//! No function here does file I/O or starts a process, and caller input never panics: every
//! refusal is an [`Error`], including the inputs on which the `dolby_vision` crate would panic
//! (composer shapes it cannot write, metadata blocks of the wrong CM version, an HDR10+ payload
//! without the selected peak). [`RpuBuilder::new`] refuses every value a later write would, so a
//! builder that exists writes every shot. The workspace's `panic = "abort"` covers only its own
//! binaries: a consumer's profile decides, and an FFI caller still uses `catch_unwind`. The
//! crate never parses an RPU from outside. Design and byte contract: `docs/RPU_BUILDER.md`.

mod builder;
mod composer;
mod error;
mod hdr10plus;
mod params;
mod static_l1;

pub use builder::{build, RpuBuilder, ShotRpu, L1, RPU_NAL_HEADER};
pub use composer::HlgComposer;
pub use error::{Error, Result};
pub use hdr10plus::{hdr10plus_l1, Hdr10PlusFrame, Hdr10PlusHold, PeakSource, SceneKey};
pub use params::{
    nits_to_pq_code, source_range_81, source_range_84, CmMetadata, CmV40Config, CmVersion,
    ExtraShot, ExtraShots, Level5, Level6, Profile, RpuParams, DEFAULT_MAX_CLL_NITS,
    DEFAULT_MAX_DML_NITS, DEFAULT_MAX_FALL_NITS, DEFAULT_MIN_DML_NITS, DEFAULT_TRIM_TARGETS_NITS,
    MASTERING_MIN_NITS, MASTERING_PEAK_NITS, MAX_TRIM_TARGETS,
};
pub use static_l1::{static_84_params, STATIC_84_L1, STATIC_84_L1_AVG_CM_VERSION};
