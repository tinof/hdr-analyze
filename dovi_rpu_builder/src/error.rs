/// Why a builder refused its input. Every refusal is an `Err`: nothing in this crate panics on
/// caller input, because the fork calls it through FFI under `panic = "abort"`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The composer's reshaping curves cannot be written (`dovi84_composer::check_mapping_writable`),
    /// or the canary offset is out of range.
    #[error("composer: {0}")]
    InvalidComposer(String),
    /// A metadata block does not belong to the RPU's CM version.
    #[error("metadata block: {0}")]
    BlockLevel(String),
    /// A value does not fit the field the `dolby_vision` crate stores it in.
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
    /// The `dolby_vision` crate refused to build or write the RPU.
    #[error("RPU: {0}")]
    Rpu(String),
}

impl Error {
    // `anyhow::Error` does not implement `std::error::Error`, so the crate's errors are kept as
    // their full message, not as a source.
    pub(crate) fn rpu(error: anyhow::Error) -> Self {
        Error::Rpu(format!("{error:#}"))
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
