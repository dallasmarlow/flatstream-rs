use std::fmt;

/// Custom error type for the flatstream-rs library.
///
/// The payload lives behind a `Box`, so `Error` is pointer-sized and the hot
/// paths' `Result`s stay register-friendly — the single allocation happens on
/// the (cold) error path, where an error is about to be formatted or matched
/// anyway. Inspect the failure with [`kind`](Self::kind).
#[derive(thiserror::Error)]
#[error(transparent)]
pub struct Error(Box<ErrorKind>);

/// The failure categories. Obtained from [`Error::kind`].
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// Underlying I/O errors from std::io operations.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Checksum mismatch error when calculated checksum doesn't match stored checksum.
    #[error("Checksum mismatch: expected {expected}, got {calculated}")]
    ChecksumMismatch { expected: u64, calculated: u64 },

    /// Invalid frame error for malformed frames (e.g., oversized length, policy limits).
    ///
    /// Optional context fields help diagnose issues quickly; they are rendered
    /// by `Display` on demand rather than pre-formatted at construction.
    #[error(
        "Invalid frame: {message}{}",
        InvalidFrameContext(declared_len, buffer_len, limit)
    )]
    InvalidFrame {
        message: &'static str,
        /// Declared payload length (from header), if known
        declared_len: Option<usize>,
        /// Available bytes or current buffer length, if relevant
        buffer_len: Option<usize>,
        /// Configured limit (e.g., frame bounds), if relevant
        limit: Option<usize>,
    },

    /// FlatBuffers-specific deserialization issues.
    #[error("FlatBuffers error: {0}")]
    FlatbuffersError(#[from] flatbuffers::InvalidFlatbuffer),

    /// Validation failure from a configured validator.
    ///
    /// `validator` is typically the value returned by the validator's `.name()` method.
    #[error("Validation failed (validator: {validator}): {reason}")]
    ValidationFailed {
        validator: &'static str,
        reason: String,
    },

    /// Unexpected end of file while reading stream data.
    #[error("Unexpected end of file while reading stream")]
    UnexpectedEof,
}

/// Renders `InvalidFrame`'s optional context as ` (declared_len=…, …)` — on
/// demand at `Display` time, so constructing the error formats nothing.
struct InvalidFrameContext<'a>(&'a Option<usize>, &'a Option<usize>, &'a Option<usize>);

impl fmt::Display for InvalidFrameContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut sep = " (";
        for (name, value) in [
            ("declared_len", self.0),
            ("buffer_len", self.1),
            ("limit", self.2),
        ] {
            if let Some(v) = value {
                write!(f, "{sep}{name}={v}")?;
                sep = ", ";
            }
        }
        if sep == ", " {
            write!(f, ")")?;
        }
        Ok(())
    }
}

impl Error {
    /// Returns the kind of failure this error represents.
    #[inline]
    pub fn kind(&self) -> &ErrorKind {
        &self.0
    }

    /// Consumes the error, returning its kind.
    pub fn into_kind(self) -> ErrorKind {
        *self.0
    }

    /// Create a new `InvalidFrame` error with a descriptive message.
    #[cold]
    pub fn invalid_frame(message: &'static str) -> Self {
        ErrorKind::InvalidFrame {
            message,
            declared_len: None,
            buffer_len: None,
            limit: None,
        }
        .into()
    }

    /// Create a new `InvalidFrame` error with contextual details.
    ///
    /// Context fields are optional; pass `Some(..)` where known to improve diagnostics.
    #[cold]
    pub fn invalid_frame_with(
        message: &'static str,
        declared_len: Option<usize>,
        buffer_len: Option<usize>,
        limit: Option<usize>,
    ) -> Self {
        ErrorKind::InvalidFrame {
            message,
            declared_len,
            buffer_len,
            limit,
        }
        .into()
    }

    /// Create a new `ChecksumMismatch` error with expected and calculated values.
    #[cold]
    pub fn checksum_mismatch(expected: u64, calculated: u64) -> Self {
        ErrorKind::ChecksumMismatch {
            expected,
            calculated,
        }
        .into()
    }

    /// Create a new `ValidationFailed` error for the named validator.
    #[cold]
    pub fn validation_failed(validator: &'static str, reason: impl Into<String>) -> Self {
        ErrorKind::ValidationFailed {
            validator,
            reason: reason.into(),
        }
        .into()
    }

    /// Create a new `UnexpectedEof` error (EOF inside a frame).
    #[cold]
    pub fn unexpected_eof() -> Self {
        ErrorKind::UnexpectedEof.into()
    }
}

impl From<ErrorKind> for Error {
    #[cold]
    fn from(kind: ErrorKind) -> Self {
        Self(Box::new(kind))
    }
}

impl From<std::io::Error> for Error {
    #[cold]
    fn from(e: std::io::Error) -> Self {
        ErrorKind::Io(e).into()
    }
}

impl From<flatbuffers::InvalidFlatbuffer> for Error {
    #[cold]
    fn from(e: flatbuffers::InvalidFlatbuffer) -> Self {
        ErrorKind::FlatbuffersError(e).into()
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

/// Result type alias for the library operations.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_is_pointer_sized() {
        // The entire point of the boxed-kind pattern: hot-path Results stay
        // register-friendly. A change that fattens Error is a regression.
        assert_eq!(std::mem::size_of::<Error>(), std::mem::size_of::<usize>());
        assert_eq!(
            std::mem::size_of::<Result<()>>(),
            std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn invalid_frame_context_renders_on_demand() {
        let plain = Error::invalid_frame("bad frame");
        assert_eq!(plain.to_string(), "Invalid frame: bad frame");

        let full = Error::invalid_frame_with("too large", Some(10), None, Some(4));
        assert_eq!(
            full.to_string(),
            "Invalid frame: too large (declared_len=10, limit=4)"
        );
    }

    #[test]
    fn source_chain_preserved() {
        // `Error` is transparent over `ErrorKind`, whose #[from] fields are
        // the source chain — recovery code walks to the underlying io::Error.
        let err = Error::from(std::io::Error::other("disk fault"));
        let source = std::error::Error::source(&err).expect("io source");
        assert_eq!(source.to_string(), "disk fault");
    }
}
