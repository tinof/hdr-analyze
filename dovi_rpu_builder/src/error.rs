/// Why a builder refused its input. Every refusal is an `Err`: nothing in this crate panics on
/// caller input. This workspace builds with `panic = "abort"`, but a consumer's own profile
/// decides what a panic does, so an FFI caller still wraps each call in
/// `std::panic::catch_unwind` (which catches a panic under `panic = "unwind"`).
///
/// Every variant except `Rpu` is bad input. `Rpu` is an internal failure: `RpuBuilder::new`
/// refuses every value a later write would refuse, so a builder that exists writes every shot.
/// [`Error::code`] gives each variant a stable number for callers across FFI.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The composer's reshaping curves cannot be written (`dovi84_composer::check_mapping_writable`),
    /// or the canary offset is out of range.
    #[error("composer: {0}")]
    InvalidComposer(String),
    /// A metadata block does not belong to the RPU's CM version.
    #[error("metadata block: {0}")]
    BlockLevel(String),
    /// A value the RPU cannot carry: it does not fit the field the `dolby_vision` crate stores it
    /// in, the crate's writer would refuse it (L2 count, L5, L6, L9, L11, an 8.1 source range above
    /// 12 bits), or the 8.1 source range is empty or inverted.
    #[error("value out of range: {0}")]
    OutOfRange(String),
    /// A T.35 payload without the HDR10+ identity (country 0xB5, provider 0x003C, oriented code 1,
    /// application 4 version 1). `hdr10plus_tool` skips such SEI messages rather than failing.
    #[error("not an HDR10+ payload: {0}")]
    NotHdr10Plus(String),
    /// The HDR10+ payload is not a single-window ST 2094-40 application 4 version 1 payload.
    #[error("HDR10+ payload: {0}")]
    Hdr10PlusParse(String),
    /// The HDR10+ payload has no value for the selected peak source.
    #[error("HDR10+ payload has no {0} peak")]
    Hdr10PlusNoPeak(&'static str),
    /// The `dolby_vision` crate refused to build or write the RPU after this crate's checks
    /// passed: an internal failure, not bad input.
    #[error("RPU: {0}")]
    Rpu(String),
}

impl Error {
    /// A stable number per variant, for callers that cannot match on the enum (JNI). A code is
    /// never reused or renumbered; a new variant takes the next number. 7 (`Rpu`) is the only
    /// internal failure.
    pub fn code(&self) -> i32 {
        match self {
            Error::InvalidComposer(_) => 1,
            Error::BlockLevel(_) => 2,
            Error::OutOfRange(_) => 3,
            Error::NotHdr10Plus(_) => 4,
            Error::Hdr10PlusParse(_) => 5,
            Error::Hdr10PlusNoPeak(_) => 6,
            Error::Rpu(_) => 7,
        }
    }

    // `anyhow::Error` does not implement `std::error::Error`, so the crate's errors are kept as
    // their full message, not as a source.
    pub(crate) fn rpu(error: anyhow::Error) -> Self {
        Error::Rpu(format!("{error:#}"))
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_fixed() {
        let errors = [
            Error::InvalidComposer(String::new()),
            Error::BlockLevel(String::new()),
            Error::OutOfRange(String::new()),
            Error::NotHdr10Plus(String::new()),
            Error::Hdr10PlusParse(String::new()),
            Error::Hdr10PlusNoPeak("histogram"),
            Error::Rpu(String::new()),
        ];
        let codes: Vec<i32> = errors.iter().map(Error::code).collect();
        assert_eq!(codes, [1, 2, 3, 4, 5, 6, 7]);
    }
}
