use std::borrow::Cow;
use std::fmt;

use crate::durability::SyncMode;

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
        /// Failure description. `Cow` keeps the library's own static messages
        /// copy-free (borrowed) while letting custom framers/deframers pass
        /// owned, formatted text when they need to.
        message: Cow<'static, str>,
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
        /// `Cow` for the same reason as `InvalidFrame::message`: static reasons
        /// stay copy-free, dynamic ones (verifier output, formatted sizes) are
        /// owned.
        reason: Cow<'static, str>,
    },

    /// The current read attempt reached EOF inside a frame.
    ///
    /// This says nothing about whether a seekable source may grow later;
    /// recovery and live-tailing callers interpret the condition using their
    /// source lifecycle.
    #[error("Unexpected end of file while reading stream")]
    UnexpectedEof,

    /// A poisoned writer or reader rejected the operation.
    ///
    /// An earlier failure left part of a frame on the stream, so its current
    /// position is no longer a known frame boundary. This is a state
    /// rejection, not a new I/O failure: the rejected operation moved no
    /// bytes. Check for the state without provoking it via
    /// `StreamWriter::is_poisoned` / `StreamReader::is_poisoned`; recover by
    /// consuming the stream (`into_inner`), truncating/recovering the torn
    /// tail, and constructing a replacement at a verified offset.
    #[error("stream is poisoned by a failed partial frame; consume, recover, and reconstruct it")]
    Poisoned,

    /// A frame was accepted by the sink, but its durability checkpoint failed.
    ///
    /// `attempted_watermark` includes every complete frame accepted before the
    /// failed checkpoint. Callers must not blindly retry the triggering write:
    /// its bytes are already present even though stable storage was not
    /// confirmed.
    #[error(
        "{mode:?} durability checkpoint through offset {attempted_watermark} failed \
         (previous durable watermark: {previous_watermark:?}): {source}"
    )]
    DurabilityFailed {
        /// Durability operation that failed.
        mode: SyncMode,
        /// Stream offset the checkpoint attempted to make durable.
        attempted_watermark: u64,
        /// Last successfully confirmed durable offset, if any.
        previous_watermark: Option<u64>,
        /// Triggering frame start for an automatic checkpoint.
        frame_start: Option<u64>,
        /// Triggering frame length for an automatic checkpoint.
        wire_len: Option<u64>,
        /// Underlying sink error.
        #[source]
        source: std::io::Error,
    },
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
    ///
    /// Accepts anything `Into<Cow<'static, str>>`: string literals stay
    /// borrowed (no copy, no allocation beyond the boxed kind), and owned
    /// `String`s are taken as-is for callers that need formatted diagnostics.
    #[cold]
    pub fn invalid_frame(message: impl Into<Cow<'static, str>>) -> Self {
        ErrorKind::InvalidFrame {
            message: message.into(),
            declared_len: None,
            buffer_len: None,
            limit: None,
        }
        .into()
    }

    /// Create a new `InvalidFrame` error with contextual details.
    ///
    /// Context fields are optional; pass `Some(..)` where known to improve
    /// diagnostics — prefer them over formatting dynamic values into the
    /// message, since they render on demand at `Display` time. The message
    /// itself is `Cow`: literals stay borrowed and copy-free, owned `String`s
    /// are accepted when a custom framer/deframer genuinely needs one.
    #[cold]
    pub fn invalid_frame_with(
        message: impl Into<Cow<'static, str>>,
        declared_len: Option<usize>,
        buffer_len: Option<usize>,
        limit: Option<usize>,
    ) -> Self {
        ErrorKind::InvalidFrame {
            message: message.into(),
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
    ///
    /// The reason is `Cow`: static reasons stay borrowed and copy-free,
    /// dynamic ones (verifier output, formatted sizes) are owned.
    #[cold]
    pub fn validation_failed(
        validator: &'static str,
        reason: impl Into<Cow<'static, str>>,
    ) -> Self {
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

    /// Creates the fail-stop rejection a poisoned writer/reader returns.
    #[cold]
    pub(crate) fn poisoned() -> Self {
        ErrorKind::Poisoned.into()
    }

    /// Creates an error for a failed durability checkpoint.
    #[cold]
    pub(crate) fn durability_failed(
        mode: SyncMode,
        attempted_watermark: u64,
        previous_watermark: Option<u64>,
        frame_start: Option<u64>,
        wire_len: Option<u64>,
        source: std::io::Error,
    ) -> Self {
        ErrorKind::DurabilityFailed {
            mode,
            attempted_watermark,
            previous_watermark,
            frame_start,
            wire_len,
            source,
        }
        .into()
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

/// The reverse of the `From<std::io::Error>` conversion above: translates a
/// flatstream [`Error`] into a [`std::io::Error`] so application code that
/// surfaces `io::Error` at its boundaries can use `?` on flatstream results.
///
/// The flatstream error becomes the I/O error's inner payload — recoverable via
/// [`std::io::Error::get_ref`] / [`into_inner`](std::io::Error::into_inner), and
/// forwarded by the I/O error's `Display`. An underlying [`ErrorKind::Io`]
/// preserves its source kind, [`ErrorKind::UnexpectedEof`] maps to
/// [`std::io::ErrorKind::UnexpectedEof`], and protocol/validation/durability
/// failures map to [`std::io::ErrorKind::InvalidData`].
impl From<Error> for std::io::Error {
    #[cold]
    fn from(e: Error) -> Self {
        let kind = match e.kind() {
            ErrorKind::Io(source) => source.kind(),
            ErrorKind::UnexpectedEof => std::io::ErrorKind::UnexpectedEof,
            _ => std::io::ErrorKind::InvalidData,
        };
        std::io::Error::new(kind, e)
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

    #[test]
    fn converts_protocol_error_into_invalid_data_preserving_the_error() {
        // App boundaries that surface io::Error can `?` on flatstream results.
        // The flatstream error is preserved as the io::Error's inner payload
        // (downcastable), its Display is forwarded, and protocol failures use
        // standard InvalidData control flow.
        let flat = Error::invalid_frame("bad frame");
        let display = flat.to_string();
        let io: std::io::Error = flat.into();
        assert_eq!(io.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(io.to_string(), display);
        let inner = io.get_ref().expect("inner payload");
        assert!(
            inner.downcast_ref::<Error>().is_some(),
            "payload should be the original flatstream::Error"
        );
    }

    #[test]
    fn converts_underlying_io_error_preserving_standard_kind() {
        let flat = Error::from(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "device refused the read",
        ));
        let io: std::io::Error = flat.into();
        assert_eq!(io.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(io.to_string(), "I/O error: device refused the read");
        assert!(
            io.get_ref()
                .and_then(|inner| inner.downcast_ref::<Error>())
                .is_some(),
            "the original flatstream::Error remains the inner payload"
        );
    }

    #[test]
    fn durability_failure_does_not_masquerade_as_retryable_io() {
        let flat = Error::durability_failed(
            SyncMode::All,
            128,
            Some(64),
            Some(64),
            Some(64),
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "sync denied"),
        );
        let io: std::io::Error = flat.into();

        assert_eq!(io.kind(), std::io::ErrorKind::InvalidData);
        let flat = io
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<Error>())
            .expect("the durability context remains available");
        assert!(matches!(
            flat.kind(),
            ErrorKind::DurabilityFailed {
                attempted_watermark: 128,
                previous_watermark: Some(64),
                source,
                ..
            } if source.kind() == std::io::ErrorKind::PermissionDenied
        ));
    }

    #[test]
    fn poisoned_rejection_is_typed_and_converts_to_invalid_data() {
        // Callers distinguish "the stream is fail-stopped" from a fresh frame
        // error by kind, never by message text.
        let flat = Error::poisoned();
        assert!(matches!(flat.kind(), ErrorKind::Poisoned));
        let io: std::io::Error = flat.into();
        assert_eq!(io.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn unexpected_eof_survives_question_mark_conversion() {
        fn boundary() -> std::result::Result<(), std::io::Error> {
            let flat: Result<()> = Err(Error::unexpected_eof());
            flat?; // uses From<Error> for io::Error
            Ok(())
        }
        let error = boundary().expect_err("the flatstream error must propagate");
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }
}
