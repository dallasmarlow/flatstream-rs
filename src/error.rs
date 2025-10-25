use thiserror::Error;

/// Custom error types for the flatstream-rs library.
#[derive(Error, Debug)]
pub enum Error {
    /// Underlying I/O errors from std::io operations.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Checksum mismatch error when calculated checksum doesn't match stored checksum.
    #[error("Checksum mismatch: expected {expected}, got {calculated}")]
    ChecksumMismatch { expected: u64, calculated: u64 },

    /// Invalid frame error for malformed frames (e.g., oversized length, policy limits).
    ///
    /// Optional context fields help diagnose issues quickly while keeping errors lightweight.
    #[error("Invalid frame: {message}{context}")]
    InvalidFrame {
        message: String,
        /// Declared payload length (from header), if known
        declared_len: Option<usize>,
        /// Available bytes or current buffer length, if relevant
        buffer_len: Option<usize>,
        /// Configured limit (e.g., bounded adapters), if relevant
        limit: Option<usize>,
        /// Pre-rendered human-readable context string
        context: String,
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

impl Error {
    /// Create a new `InvalidFrame` error with a descriptive message.
    pub fn invalid_frame(message: impl Into<String>) -> Self {
        Self::InvalidFrame {
            message: message.into(),
            declared_len: None,
            buffer_len: None,
            limit: None,
            context: String::new(),
        }
    }

    /// Create a new `InvalidFrame` error with contextual details.
    ///
    /// Context fields are optional; pass `Some(..)` where known to improve diagnostics.
    pub fn invalid_frame_with(
        message: impl Into<String>,
        declared_len: Option<usize>,
        buffer_len: Option<usize>,
        limit: Option<usize>,
    ) -> Self {
        let mut ctx = String::new();
        if declared_len.is_some() || buffer_len.is_some() || limit.is_some() {
            ctx.push_str(" (");
            if let Some(v) = declared_len {
                ctx.push_str(&format!("declared_len={v}"));
            }
            if let Some(v) = buffer_len {
                if !ctx.ends_with('(') {
                    ctx.push_str(", ");
                }
                ctx.push_str(&format!("buffer_len={v}"));
            }
            if let Some(v) = limit {
                if !ctx.ends_with('(') {
                    ctx.push_str(", ");
                }
                ctx.push_str(&format!("limit={v}"));
            }
            ctx.push(')');
        }
        Self::InvalidFrame {
            message: message.into(),
            declared_len,
            buffer_len,
            limit,
            context: ctx,
        }
    }

    /// Create a new `ChecksumMismatch` error with expected and calculated values.
    pub fn checksum_mismatch(expected: u64, calculated: u64) -> Self {
        Self::ChecksumMismatch {
            expected,
            calculated,
        }
    }
}

/// Result type alias for the library operations.
pub type Result<T> = std::result::Result<T, Error>;
