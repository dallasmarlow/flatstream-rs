//! Defines the framing and deframing strategies for the byte stream.

use crate::checksum::Checksum;
use crate::error::{Error, Result};
use std::io::{Read, Write};

//--- Framer Trait and Implementations ---

/// A trait that defines how a raw payload is framed and written to a stream.
///
/// Purpose: Separate wire-format concerns (headers/checksums) from I/O and serialization.
/// Implementations are small strategy objects composed into `StreamWriter`.
pub trait Framer {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()>;
}

/// The default framing strategy: `[4-byte length | payload]`
///
/// When to use: Highest throughput baseline when you don't need integrity checks.
pub struct DefaultFramer;

impl Framer for DefaultFramer {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        // Enforce 32-bit length header contract to avoid truncation on cast
        if payload.len() > u32::MAX as usize {
            return Err(Error::invalid_frame_with(
                "payload length exceeds 32-bit header limit",
                Some(payload.len()),
                None,
                Some(u32::MAX as usize),
            ));
        }
        let payload_len = payload.len() as u32;
        writer.write_all(&payload_len.to_le_bytes())?;
        writer.write_all(payload)?;
        Ok(())
    }
}

/// A framing strategy that includes a checksum: `[4-byte length | 8-byte checksum | payload]`
///
/// When to use: Integrity validation at read-time and/or independent message corruption detection.
pub struct ChecksumFramer<C: Checksum> {
    checksum_alg: C,
}

impl<C: Checksum> ChecksumFramer<C> {
    pub fn new(checksum_alg: C) -> Self {
        Self { checksum_alg }
    }
}

impl<C: Checksum> Framer for ChecksumFramer<C> {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        // Enforce 32-bit length header contract to avoid truncation on cast
        if payload.len() > u32::MAX as usize {
            return Err(Error::invalid_frame_with(
                "payload length exceeds 32-bit header limit",
                Some(payload.len()),
                None,
                Some(u32::MAX as usize),
            ));
        }
        let payload_len = payload.len() as u32;
        let checksum = self.checksum_alg.calculate(payload);
        let checksum_size = self.checksum_alg.size();

        writer.write_all(&payload_len.to_le_bytes())?;

        // Write only the required number of bytes for the checksum
        match checksum_size {
            0 => {
                // No checksum bytes to write
            }
            2 => {
                // Write 2 bytes for CRC16
                writer.write_all(&(checksum as u16).to_le_bytes())?;
            }
            4 => {
                // Write 4 bytes for CRC32
                writer.write_all(&(checksum as u32).to_le_bytes())?;
            }
            8 => {
                // Write 8 bytes for XXHash64
                writer.write_all(&checksum.to_le_bytes())?;
            }
            _ => {
                // For any other size, write the full u64 and truncate if needed
                writer.write_all(&checksum.to_le_bytes())?;
            }
        }

        writer.write_all(payload)?;
        Ok(())
    }
}

//--- Deframer Trait and Implementations ---

/// A trait that defines how a message is deframed and read from a stream.
///
/// Purpose: Parse a framed stream into payload slices, validating headers and (optionally) checksums.
pub trait Deframer {
    /// Returns Ok(Some(())) on success, Ok(None) on clean EOF.
    fn read_and_deframe<R: Read>(&self, reader: &mut R, buffer: &mut Vec<u8>)
        -> Result<Option<()>>;

    /// Fast-path: called when the 4-byte little-endian payload length has already been read.
    /// Implementations must read any additional header fields (e.g., checksum), then the payload.
    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<()>>;
}

/// The default deframing strategy.
///
/// When to use: Safe, straightforward parser. Resizes and zeroes the buffer.
#[derive(Clone, Copy, Default)]
pub struct DefaultDeframer;

impl DefaultDeframer {
    // Intentionally no constructor; use `DefaultDeframer` unit value directly or `DefaultDeframer::default()`.
}

impl Deframer for DefaultDeframer {
    fn read_and_deframe<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>> {
        let mut len_bytes = [0u8; 4];
        match reader.read_exact(&mut len_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None), // Clean EOF
            Err(e) => return Err(e.into()),
        }

        let payload_len = u32::from_le_bytes(len_bytes) as usize;
        buffer.resize(payload_len, 0);
        reader.read_exact(buffer).map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => Error::UnexpectedEof,
            _ => e.into(),
        })?;

        Ok(Some(()))
    }

    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<()>> {
        buffer.resize(payload_len, 0);
        reader.read_exact(buffer).map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => Error::UnexpectedEof,
            _ => e.into(),
        })?;
        Ok(Some(()))
    }
}

/// A deframing strategy that verifies a checksum.
///
/// When to use: Reads streams written with a matching `ChecksumFramer<C>`.
#[derive(Clone, Copy)]
pub struct ChecksumDeframer<C: Checksum> {
    checksum_alg: C,
}

impl<C: Checksum> ChecksumDeframer<C> {
    pub fn new(checksum_alg: C) -> Self {
        Self { checksum_alg }
    }
}

impl<C: Checksum> Deframer for ChecksumDeframer<C> {
    fn read_and_deframe<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>> {
        // This directly addresses Lesson 3: Distinguish between clean and unexpected EOF.
        let mut len_bytes = [0u8; 4];
        match reader.read_exact(&mut len_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let payload_len = u32::from_le_bytes(len_bytes) as usize;
        let checksum_size = self.checksum_alg.size();

        // Read checksum bytes based on the checksum size
        let expected_checksum = match checksum_size {
            0 => {
                // No checksum to read
                0
            }
            2 => {
                // Read 2 bytes for CRC16
                let mut checksum_bytes = [0u8; 2];
                reader
                    .read_exact(&mut checksum_bytes)
                    .map_err(|_| Error::UnexpectedEof)?;
                u16::from_le_bytes(checksum_bytes) as u64
            }
            4 => {
                // Read 4 bytes for CRC32
                let mut checksum_bytes = [0u8; 4];
                reader
                    .read_exact(&mut checksum_bytes)
                    .map_err(|_| Error::UnexpectedEof)?;
                u32::from_le_bytes(checksum_bytes) as u64
            }
            8 => {
                // Read 8 bytes for XXHash64
                let mut checksum_bytes = [0u8; 8];
                reader
                    .read_exact(&mut checksum_bytes)
                    .map_err(|_| Error::UnexpectedEof)?;
                u64::from_le_bytes(checksum_bytes)
            }
            _ => {
                // For any other size, read 8 bytes (backward compatibility)
                let mut checksum_bytes = [0u8; 8];
                reader
                    .read_exact(&mut checksum_bytes)
                    .map_err(|_| Error::UnexpectedEof)?;
                u64::from_le_bytes(checksum_bytes)
            }
        };

        buffer.resize(payload_len, 0);
        reader.read_exact(buffer).map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => Error::UnexpectedEof,
            _ => e.into(),
        })?;

        self.checksum_alg.verify(expected_checksum, buffer)?;

        Ok(Some(()))
    }

    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<()>> {
        let checksum_size = self.checksum_alg.size();

        let expected_checksum = match checksum_size {
            0 => 0,
            2 => {
                let mut checksum_bytes = [0u8; 2];
                reader
                    .read_exact(&mut checksum_bytes)
                    .map_err(|_| Error::UnexpectedEof)?;
                u16::from_le_bytes(checksum_bytes) as u64
            }
            4 => {
                let mut checksum_bytes = [0u8; 4];
                reader
                    .read_exact(&mut checksum_bytes)
                    .map_err(|_| Error::UnexpectedEof)?;
                u32::from_le_bytes(checksum_bytes) as u64
            }
            8 => {
                let mut checksum_bytes = [0u8; 8];
                reader
                    .read_exact(&mut checksum_bytes)
                    .map_err(|_| Error::UnexpectedEof)?;
                u64::from_le_bytes(checksum_bytes)
            }
            _ => {
                let mut checksum_bytes = [0u8; 8];
                reader
                    .read_exact(&mut checksum_bytes)
                    .map_err(|_| Error::UnexpectedEof)?;
                u64::from_le_bytes(checksum_bytes)
            }
        };

        buffer.resize(payload_len, 0);
        reader.read_exact(buffer).map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => Error::UnexpectedEof,
            _ => e.into(),
        })?;

        self.checksum_alg.verify(expected_checksum, buffer)?;
        Ok(Some(()))
    }
}

/// A high-performance deframer that uses an `unsafe` block to avoid unnecessary buffer zeroing.
///
/// Safety: Only use with trusted data sources (e.g., files you just wrote). Avoids buffer
/// initialization to remove zeroing cost; ensures capacity via `reserve` and sets length with `unsafe`.
#[derive(Clone, Copy, Default)]
pub struct UnsafeDeframer;

impl UnsafeDeframer {
    // Intentionally no constructor; use `UnsafeDeframer` unit value directly.
}

// Implementation for the unsafe version
impl Deframer for UnsafeDeframer {
    fn read_and_deframe<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>> {
        let mut len_bytes = [0u8; 4];
        match reader.read_exact(&mut len_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let payload_len = u32::from_le_bytes(len_bytes) as usize;

        buffer.clear();
        if buffer.capacity() < payload_len {
            let additional = payload_len.saturating_sub(buffer.len());
            if additional > 0 {
                buffer.reserve(additional);
            }
        }

        unsafe {
            buffer.set_len(payload_len);
        }

        reader.read_exact(buffer).map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => Error::UnexpectedEof,
            _ => e.into(),
        })?;
        Ok(Some(()))
    }

    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<()>> {
        // Only grow the buffer if current capacity is insufficient.
        if buffer.capacity() < payload_len {
            let additional = payload_len.saturating_sub(buffer.len());
            if additional > 0 {
                buffer.reserve(additional);
            }
        }
        unsafe {
            buffer.set_len(payload_len);
        }
        reader.read_exact(buffer).map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => Error::UnexpectedEof,
            _ => e.into(),
        })?;
        Ok(Some(()))
    }
}

/// Deframer using the safe `Read::take` method.
///
/// When to use: Alternative safe implementation; performance may vary with reader type.
#[derive(Clone, Copy, Default)]
pub struct SafeTakeDeframer;

impl SafeTakeDeframer {
    // Intentionally no constructor; use `SafeTakeDeframer` unit value directly.
}

// Implementation for the safe version
impl Deframer for SafeTakeDeframer {
    fn read_and_deframe<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>> {
        let mut len_bytes = [0u8; 4];
        match reader.read_exact(&mut len_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let payload_len = u32::from_le_bytes(len_bytes) as usize;

        buffer.clear();
        buffer.reserve(payload_len);

        reader.take(payload_len as u64).read_to_end(buffer)?;

        if buffer.len() != payload_len {
            return Err(Error::UnexpectedEof);
        }

        Ok(Some(()))
    }

    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<()>> {
        buffer.clear();
        buffer.reserve(payload_len);
        reader.take(payload_len as u64).read_to_end(buffer)?;
        if buffer.len() != payload_len {
            return Err(Error::UnexpectedEof);
        }
        Ok(Some(()))
    }
}

/// A composable adapter that enforces a maximum frame length for any deframer
///
/// Failure semantics: Returns `Error::InvalidFrame` with context (declared_len/limit) when exceeded.
/// that begins by reading a 4-byte little-endian payload length.
pub struct BoundedDeframer<D: Deframer> {
    inner: D,
    max: usize,
}

impl<D: Deframer> BoundedDeframer<D> {
    pub fn new(inner: D, max: usize) -> Self {
        Self { inner, max }
    }
}

// (no shim needed)

impl<D: Deframer> Deframer for BoundedDeframer<D> {
    fn read_and_deframe<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>> {
        let mut len_bytes = [0u8; 4];
        match reader.read_exact(&mut len_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let payload_len = u32::from_le_bytes(len_bytes) as usize;
        if payload_len > self.max {
            return Err(Error::invalid_frame_with(
                "frame length exceeds configured limit",
                Some(payload_len),
                None,
                Some(self.max),
            ));
        }

        self.inner.read_after_length(reader, buffer, payload_len)
    }

    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<()>> {
        if payload_len > self.max {
            return Err(Error::invalid_frame_with(
                "frame length exceeds configured limit",
                Some(payload_len),
                None,
                Some(self.max),
            ));
        }
        self.inner.read_after_length(reader, buffer, payload_len)
    }
}

/// Backward compatibility alias
#[doc(hidden)]
#[deprecated(since = "0.2.7", note = "Please use `BoundedDeframer` instead")]
pub type MaxFrameLen<D> = BoundedDeframer<D>;

/// A composable adapter that enforces a maximum payload length for any framer.
///
/// Failure semantics: Returns `Error::InvalidFrame` with context (payload len/limit) when exceeded.
pub struct BoundedFramer<F: Framer> {
    inner: F,
    max_len: usize,
}

impl<F: Framer> BoundedFramer<F> {
    pub fn new(inner: F, max_len: usize) -> Self {
        Self { inner, max_len }
    }
}

impl<F: Framer> Framer for BoundedFramer<F> {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        if payload.len() > self.max_len {
            return Err(Error::invalid_frame_with(
                "payload length exceeds configured limit",
                Some(payload.len()),
                None,
                Some(self.max_len),
            ));
        }
        self.inner.frame_and_write(writer, payload)
    }
}

//--- Observer Adapters ---

/// An adapter that allows observing payloads on the write path without copying or mutating.
///
/// Callback timing: Invoked exactly once per frame, before delegating inner framing.
pub struct ObserverFramer<F: Framer, C: Fn(&[u8])> {
    inner: F,
    callback: C,
}

impl<F: Framer, C: Fn(&[u8])> ObserverFramer<F, C> {
    pub fn new(inner: F, callback: C) -> Self {
        Self { inner, callback }
    }
}

impl<F: Framer, C: Fn(&[u8])> Framer for ObserverFramer<F, C> {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        (self.callback)(payload);
        self.inner.frame_and_write(writer, payload)
    }
}

/// An adapter that allows observing payloads on the read path without copying or mutating.
///
/// Callback timing: Invoked exactly once per frame, after inner deframing succeeds.
pub struct ObserverDeframer<D: Deframer, C: Fn(&[u8])> {
    inner: D,
    callback: C,
}

impl<D: Deframer, C: Fn(&[u8])> ObserverDeframer<D, C> {
    pub fn new(inner: D, callback: C) -> Self {
        Self { inner, callback }
    }
}

impl<D: Deframer, C: Fn(&[u8])> Deframer for ObserverDeframer<D, C> {
    fn read_and_deframe<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>> {
        match self.inner.read_and_deframe(reader, buffer)? {
            Some(()) => {
                (self.callback)(buffer);
                Ok(Some(()))
            }
            None => Ok(None),
        }
    }

    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<()>> {
        match self.inner.read_after_length(reader, buffer, payload_len)? {
            Some(()) => {
                (self.callback)(buffer);
                Ok(Some(()))
            }
            None => Ok(None),
        }
    }
}

//--- Fluent Extension Traits ---

/// Extension methods for framers to enable fluent composition without importing adapter types.
pub trait FramerExt: Framer + Sized {
    /// Enforce a maximum payload length.
    fn bounded(self, max: usize) -> BoundedFramer<Self> {
        BoundedFramer::new(self, max)
    }

    /// Observe payloads on the write path without copying. Useful for metrics/logging.
    fn observed<C: Fn(&[u8])>(self, callback: C) -> ObserverFramer<Self, C> {
        ObserverFramer::new(self, callback)
    }
}

impl<T: Framer> FramerExt for T {}

/// Extension methods for deframers to enable fluent composition without importing adapter types.
pub trait DeframerExt: Deframer + Sized {
    /// Enforce a maximum payload length.
    fn bounded(self, max: usize) -> BoundedDeframer<Self> {
        BoundedDeframer::new(self, max)
    }

    /// Observe payloads on the read path without copying. Useful for metrics/logging.
    fn observed<C: Fn(&[u8])>(self, callback: C) -> ObserverDeframer<Self, C> {
        ObserverDeframer::new(self, callback)
    }
}

impl<T: Deframer> DeframerExt for T {}
