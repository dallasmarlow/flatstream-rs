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

    /// Invalid frame error for malformed frames (e.g., unexpected EOF during length read).
    #[error("Invalid frame: {message}")]
    InvalidFrame { message: String },

    /// FlatBuffers-specific deserialization issues.
    #[error("FlatBuffers error: {0}")]
    FlatbuffersError(#[from] flatbuffers::InvalidFlatbuffer),

    /// Unexpected end of file while reading stream data.
    #[error("Unexpected end of file while reading stream")]
    UnexpectedEof,
}

impl Error {
    /// Create a new `InvalidFrame` error with a descriptive message.
    pub fn invalid_frame(message: impl Into<String>) -> Self {
        Self::InvalidFrame {
            message: message.into(),
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
