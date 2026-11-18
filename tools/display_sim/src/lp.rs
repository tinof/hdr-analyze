//! The FFI boundary to `tm_shim.c`, the only unsafe code of the tool: scalar arguments, one
//! owned context per curve setting.

use std::ffi::{c_char, c_float, c_int, CStr, CString};
use std::ptr::NonNull;

use anyhow::{bail, Result};

/// `struct tm_ctx` of the shim; only ever behind a pointer.
#[repr(C)]
struct TmCtx {
    _opaque: [u8; 0],
}

extern "C" {
    fn tm_new(
        curve: *const c_char,
        src_black_nits: c_float,
        src_max_pq: c_float,
        src_avg_pq: c_float,
        panel_black_nits: c_float,
        panel_peak_nits: c_float,
    ) -> *mut TmCtx;
    fn tm_free(ctx: *mut TmCtx);
    fn tm_sample(ctx: *const TmCtx, x: c_float) -> c_float;
    fn tm_clip_ceiling(ctx: *const TmCtx) -> c_float;
    fn tm_is_noop(ctx: *const TmCtx) -> c_int;
    #[cfg(test)]
    fn tm_params(ctx: *const TmCtx, out: *mut c_float);
    fn tm_nits_to_pq(nits: c_float) -> c_float;
    fn tm_pq_to_nits(pq: c_float) -> c_float;
    fn tm_version() -> *const c_char;
    fn tm_fix_version() -> c_int;
}

/// The parameters libplacebo's curve runs with, after the renderer's steps (PQ, 0..1).
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveParams {
    pub input_min: f32,
    pub input_max: f32,
    pub input_avg: f32,
    pub output_min: f32,
    pub output_max: f32,
    pub knee_offset: f32,
}

/// One libplacebo curve set up for one source and display (`tm_new`). It owns its context.
#[derive(Debug)]
pub struct ToneMap {
    ctx: NonNull<TmCtx>,
}

// SAFETY: the context is a heap block that only this value points to; the shim keeps no
// global state, so moving it to another thread is sound. It is not `Sync`: nothing here
// needs shared access.
unsafe impl Send for ToneMap {}

impl ToneMap {
    /// The caller has validated the numbers (`crate::curves::Setup::validate`): finite, blacks
    /// below peaks, nits within PQ's range, so no libplacebo assertion can fire.
    pub fn new(
        curve: &str,
        src_black_nits: f32,
        src_max_pq: f32,
        src_avg_pq: f32,
        panel_black_nits: f32,
        panel_peak_nits: f32,
    ) -> Result<Self> {
        let name = CString::new(curve)?;
        // SAFETY: `name` is a valid NUL-terminated string for the duration of the call; the
        // shim copies nothing from it but looks it up in libplacebo's static table.
        let ctx = unsafe {
            tm_new(
                name.as_ptr(),
                src_black_nits,
                src_max_pq,
                src_avg_pq,
                panel_black_nits,
                panel_peak_nits,
            )
        };
        match NonNull::new(ctx) {
            Some(ctx) => Ok(Self { ctx }),
            None => bail!("libplacebo has no tone-mapping curve '{curve}' (or out of memory)"),
        }
    }

    /// Displayed PQ of source PQ `x`.
    pub fn sample(&self, x: f32) -> f32 {
        // SAFETY: `ctx` came from `tm_new` and lives until `drop`.
        unsafe { tm_sample(self.ctx.as_ptr(), x) }
    }

    /// Source PQ above which every value displays the same.
    pub fn clip_ceiling(&self) -> f32 {
        // SAFETY: as in `sample`.
        unsafe { tm_clip_ceiling(self.ctx.as_ptr()) }
    }

    /// The renderer would skip the curve: the display shows the source, clipped to its range.
    pub fn is_noop(&self) -> bool {
        // SAFETY: as in `sample`.
        unsafe { tm_is_noop(self.ctx.as_ptr()) != 0 }
    }

    #[cfg(test)]
    pub fn params(&self) -> CurveParams {
        let mut out = [0.0f32; 6];
        // SAFETY: as in `sample`; `out` holds the six floats the shim writes.
        unsafe { tm_params(self.ctx.as_ptr(), out.as_mut_ptr()) };
        CurveParams {
            input_min: out[0],
            input_max: out[1],
            input_avg: out[2],
            output_min: out[3],
            output_max: out[4],
            knee_offset: out[5],
        }
    }
}

impl Drop for ToneMap {
    fn drop(&mut self) {
        // SAFETY: `ctx` came from `tm_new` and is freed exactly once, here.
        unsafe { tm_free(self.ctx.as_ptr()) }
    }
}

/// libplacebo's PQ EOTF (`pl_hdr_rescale`, f32): normalized PQ to nits.
pub fn pq_to_nits(pq: f32) -> f32 {
    // SAFETY: a pure function of its scalar argument.
    unsafe { tm_pq_to_nits(pq) }
}

/// libplacebo's inverse PQ EOTF: nits to normalized PQ.
pub fn nits_to_pq(nits: f32) -> f32 {
    // SAFETY: a pure function of its scalar argument.
    unsafe { tm_nits_to_pq(nits) }
}

/// The linked library's `pl_version()` (git describe) for the report.
pub fn version() -> String {
    // SAFETY: `pl_version` returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(tm_version()) }
        .to_string_lossy()
        .into_owned()
}

/// Refuses a library other than 7.387.0 at run time; the shim checks major and API version
/// when it compiles, this checks the fix version.
pub fn check_version() -> Result<()> {
    // SAFETY: a pure function without arguments.
    let fix = unsafe { tm_fix_version() };
    if fix != 0 {
        bail!(
            "linked libplacebo {} is 7.387.{fix}, not 7.387.0",
            version()
        );
    }
    Ok(())
}
