//! Defines the framing and deframing strategies for the byte stream.

use crate::checksum::Checksum;
use crate::error::{Error, Result};
use crate::validation::Validator;
use std::io::{Read, Write};

/// Default maximum accepted payload length for the core deframers: 16 MiB.
///
/// A length header is attacker-controlled input (a torn or corrupt frame is the
/// *expected* input for a journal after a crash), so the deframers refuse to
/// allocate for frames above this bound unless explicitly raised. 16 MiB
/// comfortably covers telemetry payloads and occasional large dumps while
/// capping what a corrupt 4-byte header can demand.
pub const DEFAULT_MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

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

/// A framing strategy that includes a checksum:
/// `[4-byte length | C::SIZE-byte checksum | payload]` — the checksum field's
/// width is the algorithm's associated `SIZE` (8 for XXH3-64, 4 for CRC-32,
/// 2 for CRC-16), not a fixed 8 bytes.
///
/// When to use: Integrity validation at read-time and/or independent message corruption detection.
pub struct ChecksumFramer<C: Checksum> {
    checksum_alg: C,
}

impl<C: Checksum> ChecksumFramer<C> {
    pub fn new(checksum_alg: C) -> Self {
        const {
            assert!(
                C::SIZE <= 8,
                "checksum wider than the u64 the trait works in"
            )
        };
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

        // Assemble the full header ([4-byte length | checksum bytes]) in a
        // 12-byte stack scratch and issue a single write_all — halves the call
        // count on this path versus writing length and checksum separately.
        // The bytes on the wire are identical (wire-format corpus tests).
        // `C::SIZE` is an associated const, so the header length and the
        // serialization width constant-fold by construction.
        //
        // On "copying" here: only header *metadata* is materialized — integers
        // must become little-endian bytes somewhere, and previously each
        // `to_le_bytes()` produced the same stack bytes before its own
        // write_all. At these fixed sizes `copy_from_slice` compiles to plain
        // register stores (no memcpy call). Payload bytes are never copied;
        // the zero-copy guarantee concerns the payload, not the 4–12 header
        // bytes.
        let mut header = [0u8; 12];
        header[..4].copy_from_slice(&payload_len.to_le_bytes());
        let checksum_field: &mut [u8; 8] = (&mut header[4..12]).try_into().unwrap();
        self.checksum_alg.write_bytes(checksum, checksum_field);

        writer.write_all(&header[..4 + C::SIZE])?;
        writer.write_all(payload)?;
        Ok(())
    }
}

//--- Deframer Trait and Implementations ---

/// Fills `header` from `reader`, distinguishing the EOF cases the wire spec
/// separates (`WIRE_FORMAT_SPEC.md` §6): zero bytes then EOF is a clean
/// frame boundary (`Ok(None)`); a torn header (some but not all bytes) is
/// `ErrorKind::UnexpectedEof`; all other I/O errors propagate intact as
/// `ErrorKind::Io`. `Interrupted` reads are retried, matching `read_exact`.
///
/// Shape matters here: the boundary probe is a one-byte `read_exact` — a
/// one-byte request cannot be torn, so its `UnexpectedEof` means exactly
/// "zero bytes available", i.e. a clean boundary. Both calls are `read_exact`
/// with lengths statically known at every monomorphized call site, so they
/// compile to plain loads; reading the whole header through one dynamic
/// `read` call costs a real memcpy per frame — measured at +100% on the
/// tight read loops.
#[inline(always)]
fn read_header<R: Read>(reader: &mut R, header: &mut [u8]) -> Result<Option<()>> {
    match reader.read_exact(&mut header[..1]) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    reader
        .read_exact(&mut header[1..])
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => Error::unexpected_eof(),
            _ => e.into(),
        })?;
    Ok(Some(()))
}

/// Rejects a declared payload length that exceeds the configured bound —
/// before any allocation is sized from it.
#[inline(always)]
fn check_frame_len(payload_len: usize, max: usize) -> Result<()> {
    if payload_len > max {
        return Err(Error::invalid_frame_with(
            "frame length exceeds configured limit",
            Some(payload_len),
            None,
            Some(max),
        ));
    }
    Ok(())
}

/// Reads `payload_len` bytes into the front of `buffer`, growing (and
/// zero-initializing) it only when the high-water mark rises. A partial
/// payload is `ErrorKind::UnexpectedEof`; other I/O errors propagate intact.
#[inline(always)]
fn read_payload<R: Read>(reader: &mut R, buffer: &mut Vec<u8>, payload_len: usize) -> Result<()> {
    if payload_len > buffer.len() {
        buffer.resize(payload_len, 0);
    }
    reader
        .read_exact(&mut buffer[..payload_len])
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => Error::unexpected_eof(),
            _ => e.into(),
        })
}

/// A trait that defines how a message is deframed and read from a stream.
///
/// Purpose: Parse a framed stream into payload lengths, validating headers and
/// (optionally) checksums. On success `buffer[..n]` holds the payload; the
/// buffer is a high-water mark — implementations grow it as needed (zeroing
/// only the growth) and never shrink it, so steady-state reads touch memory
/// exactly once, in `read_exact`.
pub trait Deframer {
    /// Reads one frame. Returns `Ok(Some(n))` with the payload length on
    /// success (payload in `buffer[..n]`), `Ok(None)` on clean EOF at a frame
    /// boundary; EOF anywhere inside a frame is `ErrorKind::UnexpectedEof`.
    ///
    /// The provided implementation reads the 4-byte little-endian length
    /// header and delegates to [`read_after_length`](Self::read_after_length).
    #[inline]
    fn read_and_deframe<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<usize>> {
        let mut len_bytes = [0u8; 4];
        match read_header(reader, &mut len_bytes)? {
            Some(()) => {
                self.read_after_length(reader, buffer, u32::from_le_bytes(len_bytes) as usize)
            }
            None => Ok(None),
        }
    }

    /// Continues a read whose 4-byte little-endian payload length has already
    /// been parsed. Implementations must bound `payload_len` before sizing any
    /// allocation from it, then read any additional header fields (e.g.
    /// checksum) and the payload.
    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<usize>>;
}

/// The default deframing strategy for `[4-byte length | payload]` streams.
///
/// When to use: Safe, allocation-bounded parser for almost all cases. Frames
/// declaring more than `max_frame_len` bytes (default
/// [`DEFAULT_MAX_FRAME_LEN`], 16 MiB) are rejected *before* any allocation;
/// raise the bound with [`with_max_frame_len`](Self::with_max_frame_len), or
/// opt out entirely with [`unbounded`](Self::unbounded) for fully trusted
/// streams.
#[derive(Clone, Copy)]
pub struct DefaultDeframer {
    max_frame_len: usize,
}

impl DefaultDeframer {
    pub fn new() -> Self {
        Self {
            max_frame_len: DEFAULT_MAX_FRAME_LEN,
        }
    }

    /// Sets the maximum accepted payload length (enforced before allocation).
    pub fn with_max_frame_len(mut self, max: usize) -> Self {
        self.max_frame_len = max;
        self
    }

    /// Accepts any length a 32-bit header can declare. This is the explicit
    /// opt-out of hostile-input protection: a corrupt header can demand a
    /// ~4 GiB allocation, so reserve it for streams you fully trust.
    pub fn unbounded() -> Self {
        Self {
            max_frame_len: u32::MAX as usize,
        }
    }
}

impl Default for DefaultDeframer {
    fn default() -> Self {
        Self::new()
    }
}

impl Deframer for DefaultDeframer {
    #[inline]
    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<usize>> {
        check_frame_len(payload_len, self.max_frame_len)?;
        read_payload(reader, buffer, payload_len)?;
        Ok(Some(payload_len))
    }
}

/// A deframing strategy that verifies a checksum.
///
/// When to use: Reads streams written with a matching `ChecksumFramer<C>`.
/// Applies the same `max_frame_len` bound as [`DefaultDeframer`] (default
/// [`DEFAULT_MAX_FRAME_LEN`]).
#[derive(Clone, Copy)]
pub struct ChecksumDeframer<C: Checksum> {
    checksum_alg: C,
    max_frame_len: usize,
}

impl<C: Checksum> ChecksumDeframer<C> {
    pub fn new(checksum_alg: C) -> Self {
        const {
            assert!(
                C::SIZE <= 8,
                "checksum wider than the u64 the trait works in"
            )
        };
        Self {
            checksum_alg,
            max_frame_len: DEFAULT_MAX_FRAME_LEN,
        }
    }

    /// Sets the maximum accepted payload length (enforced before allocation).
    pub fn with_max_frame_len(mut self, max: usize) -> Self {
        self.max_frame_len = max;
        self
    }

    /// Accepts any length a 32-bit header can declare. This is the explicit
    /// opt-out of hostile-input protection: a corrupt header can demand a
    /// ~4 GiB allocation, so reserve it for streams you fully trust.
    pub fn unbounded(checksum_alg: C) -> Self {
        Self {
            checksum_alg,
            max_frame_len: u32::MAX as usize,
        }
    }
}

impl<C: Checksum> Deframer for ChecksumDeframer<C> {
    fn read_and_deframe<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<usize>> {
        // Read `[len | checksum]` as one header (write-path twin of the
        // ChecksumFramer's single-write_all assembly). Safe only because
        // `read_header` distinguishes a clean frame boundary (zero bytes)
        // from a torn header — a plain `read_exact` over the merged header
        // could not tell those apart (spec §6). `C::SIZE` keeps the header
        // length statically known, so the reads compile to plain loads.
        let mut header = [0u8; 12];
        match read_header(reader, &mut header[..4 + C::SIZE])? {
            Some(()) => {}
            None => return Ok(None),
        }
        let payload_len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        check_frame_len(payload_len, self.max_frame_len)?;
        let expected = self.checksum_alg.read_bytes(&header[4..4 + C::SIZE]);

        read_payload(reader, buffer, payload_len)?;
        self.checksum_alg.verify(expected, &buffer[..payload_len])?;
        Ok(Some(payload_len))
    }

    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<usize>> {
        check_frame_len(payload_len, self.max_frame_len)?;

        // The length header is already consumed, so EOF here is inside the
        // frame: a torn checksum field maps to UnexpectedEof, every other
        // I/O error propagates intact (recovery logic needs the kind).
        let mut checksum_bytes = [0u8; 8];
        reader
            .read_exact(&mut checksum_bytes[..C::SIZE])
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::UnexpectedEof => Error::unexpected_eof(),
                _ => e.into(),
            })?;
        let expected = self.checksum_alg.read_bytes(&checksum_bytes);

        read_payload(reader, buffer, payload_len)?;
        self.checksum_alg.verify(expected, &buffer[..payload_len])?;
        Ok(Some(payload_len))
    }
}

/// A composable adapter that enforces a maximum payload length for any framer.
///
/// Failure semantics: Returns `ErrorKind::InvalidFrame` with context (payload len/limit) when exceeded.
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

//--- Validation Adapters ---

/// A composable adapter that adds validation to any `Framer`.
#[derive(Debug, Clone)]
pub struct ValidatingFramer<F: Framer, V: Validator> {
    inner: F,
    validator: V,
}

impl<F: Framer, V: Validator> ValidatingFramer<F, V> {
    #[inline]
    pub fn new(inner: F, validator: V) -> Self {
        Self { inner, validator }
    }
}

impl<F: Framer, V: Validator> Framer for ValidatingFramer<F, V> {
    #[inline]
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        // Validate before writing to ensure malformed data never hits the wire
        self.validator.validate(payload)?;
        self.inner.frame_and_write(writer, payload)
    }
}

/// A composable adapter that adds validation to any `Deframer`.
#[derive(Debug, Clone)]
pub struct ValidatingDeframer<D: Deframer, V: Validator> {
    inner: D,
    validator: V,
}

impl<D: Deframer, V: Validator> ValidatingDeframer<D, V> {
    #[inline]
    pub fn new(inner: D, validator: V) -> Self {
        Self { inner, validator }
    }
}

impl<D: Deframer, V: Validator> Deframer for ValidatingDeframer<D, V> {
    #[inline]
    fn read_and_deframe<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<usize>> {
        match self.inner.read_and_deframe(reader, buffer)? {
            Some(n) => {
                self.validator.validate(&buffer[..n])?;
                Ok(Some(n))
            }
            None => Ok(None),
        }
    }

    #[inline]
    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<usize>> {
        match self.inner.read_after_length(reader, buffer, payload_len)? {
            Some(n) => {
                self.validator.validate(&buffer[..n])?;
                Ok(Some(n))
            }
            None => Ok(None),
        }
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
    ) -> Result<Option<usize>> {
        match self.inner.read_and_deframe(reader, buffer)? {
            Some(n) => {
                (self.callback)(&buffer[..n]);
                Ok(Some(n))
            }
            None => Ok(None),
        }
    }

    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<usize>> {
        match self.inner.read_after_length(reader, buffer, payload_len)? {
            Some(n) => {
                (self.callback)(&buffer[..n]);
                Ok(Some(n))
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

    /// Adds a validation layer to this framer.
    #[inline]
    fn with_validator<V: Validator>(self, validator: V) -> ValidatingFramer<Self, V> {
        ValidatingFramer::new(self, validator)
    }
}

impl<T: Framer> FramerExt for T {}

/// Extension methods for deframers to enable fluent composition without importing adapter types.
pub trait DeframerExt: Deframer + Sized {
    /// Observe payloads on the read path without copying. Useful for metrics/logging.
    fn observed<C: Fn(&[u8])>(self, callback: C) -> ObserverDeframer<Self, C> {
        ObserverDeframer::new(self, callback)
    }

    /// Adds a validation layer to this deframer.
    #[inline]
    fn with_validator<V: Validator>(self, validator: V) -> ValidatingDeframer<Self, V> {
        ValidatingDeframer::new(self, validator)
    }
}

impl<T: Deframer> DeframerExt for T {}
