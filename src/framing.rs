//! Defines the framing and deframing strategies for the byte stream.

use crate::checksum::Checksum;
use crate::error::{Error, Result};
use crate::validation::Validator;
use std::io::{IoSlice, Read, Write};

/// Default maximum accepted payload length for the core deframers: the
/// FlatBuffers maximum buffer size (2 GiB), so every valid FlatBuffer reads
/// out of the box.
///
/// Bound to the runtime's own limit by construction
/// (`flatbuffers::FLATBUFFERS_MAX_BUFFER_SIZE`): FlatBuffers' signed 32-bit
/// offsets cap a buffer at 2 GiB, so nothing standard is excluded. A length
/// header is nonetheless attacker-controlled input (a torn or corrupt frame
/// is the *expected* input for a journal after a crash), so tighten the
/// accepted length to your workload's real ceiling with
/// [`with_max_frame_len`](DefaultDeframer::with_max_frame_len): a single
/// integer compare rejects an oversized declared length before any allocation
/// is sized from it. Raw or custom non-FlatBuffer framing may deliberately
/// raise the bound, up to [`MAX_WIRE_FRAME_LEN`].
pub const DEFAULT_MAX_FRAME_LEN: usize = flatbuffers::FLATBUFFERS_MAX_BUFFER_SIZE;

/// The absolute framing ceiling: the largest payload length the wire format's
/// 4-byte `u32` length prefix can express (~4 GiB).
///
/// This bounds one frame's payload, never a file — a stream may hold any
/// number of maximum-size frames, and file-level offsets (e.g. a container's
/// index) are `u64` values independent of this constant. Standard FlatBuffers
/// stop at [`DEFAULT_MAX_FRAME_LEN`] (2 GiB); the range above it is available
/// only to explicitly configured raw/custom formats via
/// [`with_max_frame_len`](DefaultDeframer::with_max_frame_len).
pub const MAX_WIRE_FRAME_LEN: usize = u32::MAX as usize;

/// Writes every byte of `slices`, in order, with a single `write_vectored`
/// call per attempt — the header and the payload reach the sink together
/// instead of as two separate `write_all`s. On a sink that implements
/// `writev` (`File`, `TcpStream`) this halves the syscalls per frame; on one
/// that does not, the standard-library fallback writes the first non-empty
/// slice per call, so the behavior degrades to exactly the two-call shape it
/// replaces.
///
/// Hand-rolled because [`Write::write_all_vectored`] is still unstable on the
/// MSRV (rust-lang/rust#70436). Three properties this loop must have, each of
/// which has burned someone:
///
/// - **`writev` is not atomic across slices.** A call may accept any prefix of
///   the total — including a partial slice — so the loop re-slices with
///   [`IoSlice::advance_slices`] and continues. No all-or-nothing guarantee is
///   claimed or relied on.
/// - **`Ok(0)` with bytes outstanding means the sink stopped accepting**, and
///   must become `WriteZero` rather than an infinite loop.
/// - **Leading empty slices are dropped before the loop**, so a zero-length
///   payload cannot make an `Ok(0)` look like a stall.
///
/// `Interrupted` is retried, matching [`Write::write_all`].
///
/// The single-call case is peeled out of the loop deliberately: every sink
/// that matters (`File`, `TcpStream`, `BufWriter`, `Vec`) accepts a whole
/// small frame in one call, and routing that case through
/// `IoSlice::advance_slices` — which rescans the slice list and asserts —
/// costs measurably more than the vectored write saves on an already-buffered
/// sink. Measured: `docs/benchmark/FINDINGS_VECTORED_FRAMING.md` §F2.
#[inline]
fn write_all_vectored<W: Write>(writer: &mut W, slices: &mut [IoSlice<'_>]) -> Result<()> {
    let total: usize = slices.iter().map(|s| s.len()).sum();
    match writer.write_vectored(slices) {
        Ok(n) if n == total => Ok(()),
        Ok(n) => write_remainder(writer, slices, n, total),
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
            write_remainder(writer, slices, 0, total)
        }
        Err(e) => Err(e.into()),
    }
}

/// The partial-write continuation: `written` bytes of `total` have been
/// accepted, so re-slice and keep going. Outlined and `#[cold]` because a sink
/// that splits a frame is the exception, and keeping it out of line leaves the
/// common path in §`write_all_vectored` small enough to inline.
#[cold]
#[inline(never)]
fn write_remainder<W: Write>(
    writer: &mut W,
    slices: &mut [IoSlice<'_>],
    written: usize,
    total: usize,
) -> Result<()> {
    let mut rest = slices;
    let mut done = written;
    // `advance_slices` also drops leading empty slices, so a zero-length
    // payload cannot leave a stray empty slice that reads as a stall.
    IoSlice::advance_slices(&mut rest, done);
    while done < total {
        match writer.write_vectored(rest) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "sink accepted no bytes while a frame was outstanding",
                )
                .into())
            }
            Ok(n) => {
                done += n;
                IoSlice::advance_slices(&mut rest, n);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

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
        // One vectored call puts `[len][payload]` on the wire together. The
        // bytes are identical to the two-`write_all` form this replaces
        // (pinned by the wire-format corpus tests); only the call count
        // changes.
        let len_bytes = payload_len.to_le_bytes();
        let mut bufs = [IoSlice::new(&len_bytes), IoSlice::new(payload)];
        write_all_vectored(writer, &mut bufs)
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
        // 12-byte stack scratch, then hand header and payload to the sink in
        // one vectored call — one call per frame instead of the three this
        // path would otherwise need. The bytes on the wire are identical
        // (wire-format corpus tests). `C::SIZE` is an associated const, so the
        // header length and the serialization width constant-fold by
        // construction.
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

        let mut bufs = [IoSlice::new(&header[..4 + C::SIZE]), IoSlice::new(payload)];
        write_all_vectored(writer, &mut bufs)
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
///
/// Implementations must consume exactly one frame per successful call and
/// must not read ahead into the next frame. `recover()` relies on this
/// contract to report the exact end offset of the last intact frame.
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
/// When to use: The general-purpose parser for almost all cases. By default it
/// accepts up to the FlatBuffers maximum buffer size
/// ([`DEFAULT_MAX_FRAME_LEN`], 2 GiB) — every valid FlatBuffer, nothing more.
/// When reading from an untrusted source, tighten the accepted length with
/// [`with_max_frame_len`](Self::with_max_frame_len) so a corrupt header can't
/// demand a huge allocation; raw non-FlatBuffer framing may raise it up to
/// [`MAX_WIRE_FRAME_LEN`].
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
    ///
    /// Consumes and returns `self`, so the result must be used. Dropping it
    /// leaves the default bound in force — a silent no-op on the one knob that
    /// stands between a corrupt length header and a huge allocation:
    ///
    /// ```compile_fail
    /// #![deny(unused_must_use)]
    /// use flatstream::DefaultDeframer;
    /// let deframer = DefaultDeframer::new();
    /// deframer.with_max_frame_len(1024); // bound discarded; does not compile
    /// ```
    #[must_use]
    pub fn with_max_frame_len(mut self, max: usize) -> Self {
        self.max_frame_len = max;
        self
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
/// Applies the same length policy as [`DefaultDeframer`]: the FlatBuffers
/// maximum ([`DEFAULT_MAX_FRAME_LEN`], 2 GiB) by default, tightened for
/// untrusted input with [`with_max_frame_len`](Self::with_max_frame_len).
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
    #[must_use]
    pub fn with_max_frame_len(mut self, max: usize) -> Self {
        self.max_frame_len = max;
        self
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
    #[must_use]
    fn bounded(self, max: usize) -> BoundedFramer<Self> {
        BoundedFramer::new(self, max)
    }

    /// Observe payloads on the write path without copying. Useful for metrics/logging.
    #[must_use]
    fn observed<C: Fn(&[u8])>(self, callback: C) -> ObserverFramer<Self, C> {
        ObserverFramer::new(self, callback)
    }

    /// Adds a validation layer to this framer.
    #[must_use]
    #[inline]
    fn with_validator<V: Validator>(self, validator: V) -> ValidatingFramer<Self, V> {
        ValidatingFramer::new(self, validator)
    }
}

impl<T: Framer> FramerExt for T {}

/// Extension methods for deframers to enable fluent composition without importing adapter types.
pub trait DeframerExt: Deframer + Sized {
    /// Observe payloads on the read path without copying. Useful for metrics/logging.
    #[must_use]
    fn observed<C: Fn(&[u8])>(self, callback: C) -> ObserverDeframer<Self, C> {
        ObserverDeframer::new(self, callback)
    }

    /// Adds a validation layer to this deframer.
    #[must_use]
    #[inline]
    fn with_validator<V: Validator>(self, validator: V) -> ValidatingDeframer<Self, V> {
        ValidatingDeframer::new(self, validator)
    }
}

impl<T: Deframer> DeframerExt for T {}

#[cfg(test)]
mod vectored_tests {
    use super::*;

    /// The canonical bytes a default frame must occupy, computed independently
    /// of the framer.
    fn expected_default(payload: &[u8]) -> Vec<u8> {
        let mut v = (payload.len() as u32).to_le_bytes().to_vec();
        v.extend_from_slice(payload);
        v
    }

    /// A sink that never overrides `write_vectored` and accepts **one byte per
    /// call**. It therefore exercises the standard library's vectored
    /// fallback (first non-empty slice only) *and* the partial-write loop at
    /// its most hostile: every frame takes `4 + payload.len()` calls.
    #[derive(Default)]
    struct OneByteAtATime {
        written: Vec<u8>,
        calls: usize,
    }

    impl Write for OneByteAtATime {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.calls += 1;
            if buf.is_empty() {
                return Ok(0);
            }
            self.written.push(buf[0]);
            Ok(1)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A sink that *does* implement `write_vectored` but accepts at most
    /// `limit` bytes per call, consuming across slice boundaries. This is the
    /// case the fallback sink cannot reach: it forces
    /// `IoSlice::advance_slices` to re-slice a *partially consumed* slice.
    struct PartialVectored {
        written: Vec<u8>,
        limit: usize,
        calls: usize,
    }

    impl Write for PartialVectored {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.limit);
            self.written.extend_from_slice(&buf[..n]);
            Ok(n)
        }
        fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> std::io::Result<usize> {
            self.calls += 1;
            let mut budget = self.limit;
            let mut total = 0;
            for buf in bufs {
                if budget == 0 {
                    break;
                }
                let n = buf.len().min(budget);
                self.written.extend_from_slice(&buf[..n]);
                budget -= n;
                total += n;
            }
            Ok(total)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A sink that accepts nothing. The loop must surface `WriteZero` rather
    /// than spin forever.
    struct Stalled;

    impl Write for Stalled {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Ok(0)
        }
        fn write_vectored(&mut self, _bufs: &[IoSlice<'_>]) -> std::io::Result<usize> {
            Ok(0)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn vectored_output_is_byte_exact_on_a_dribbling_sink() {
        // The whole point of the partial-write loop: the bytes must be
        // identical no matter how the sink chops the write up.
        for payload in [b"".as_slice(), b"x", b"hello frame", &[7u8; 300]] {
            let mut sink = OneByteAtATime::default();
            DefaultFramer.frame_and_write(&mut sink, payload).unwrap();
            assert_eq!(
                sink.written,
                expected_default(payload),
                "payload len {}",
                payload.len()
            );
            // One call per byte, plus the call that reports the empty tail
            // slice as consumed for a zero-length payload.
            assert!(sink.calls >= 4 + payload.len());
        }
    }

    #[test]
    fn advance_slices_re_slices_a_partially_consumed_slice() {
        // A 3-byte budget lands mid-header on the first call and mid-payload
        // later, so the loop must resume inside a slice, not just drop whole
        // ones. If `advance_slices` were mishandled, the output would be
        // duplicated or truncated here rather than merely slow.
        let payload: Vec<u8> = (0..=200u8).collect();
        let mut sink = PartialVectored {
            written: Vec::new(),
            limit: 3,
            calls: 0,
        };
        DefaultFramer.frame_and_write(&mut sink, &payload).unwrap();
        assert_eq!(sink.written, expected_default(&payload));
        // 205 bytes at 3 per call: the frame genuinely spanned many calls.
        assert_eq!(sink.calls, (4 + payload.len()).div_ceil(3));
    }

    #[test]
    fn one_vectored_call_suffices_when_the_sink_takes_everything() {
        // The win E1 exists for: a sink that accepts the whole frame gets
        // exactly one call for header + payload.
        let payload = [9u8; 512];
        let mut sink = PartialVectored {
            written: Vec::new(),
            limit: usize::MAX,
            calls: 0,
        };
        DefaultFramer.frame_and_write(&mut sink, &payload).unwrap();
        assert_eq!(sink.calls, 1);
        assert_eq!(sink.written, expected_default(&payload));
    }

    #[test]
    fn a_non_vectoring_sink_costs_the_same_two_calls_it_did_before() {
        // The regression E1 could plausibly have introduced. Most user-written
        // `Write` impls never override `write_vectored`, and
        // `is_write_vectored()` is unstable on the MSRV (rust-lang/rust#69941),
        // so we cannot detect them and route around the vectored path.
        //
        // We do not need to. The provided `write_vectored` forwards the first
        // non-empty slice to `write`, and the partial-write loop supplies the
        // rest — so such a sink sees exactly the header-then-payload pair of
        // calls it saw before this change, with the same bytes. No detection,
        // no regression, no `cfg` branch.
        #[derive(Default)]
        struct AcceptsEverythingUnvectored {
            written: Vec<u8>,
            calls: usize,
        }

        impl Write for AcceptsEverythingUnvectored {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.calls += 1;
                self.written.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let payload = [3u8; 256];
        let mut sink = AcceptsEverythingUnvectored::default();
        DefaultFramer.frame_and_write(&mut sink, &payload).unwrap();

        assert_eq!(sink.written, expected_default(&payload));
        assert_eq!(sink.calls, 2, "header and payload, exactly as before E1");
    }

    #[cfg(feature = "xxhash")]
    #[test]
    fn checksummed_frame_is_one_call_and_byte_exact() {
        use crate::checksum::Checksum;
        use crate::XxHash64;

        let payload = b"checksummed payload".as_slice();
        let mut sink = PartialVectored {
            written: Vec::new(),
            limit: usize::MAX,
            calls: 0,
        };
        ChecksumFramer::new(XxHash64::new())
            .frame_and_write(&mut sink, payload)
            .unwrap();

        let mut expected = (payload.len() as u32).to_le_bytes().to_vec();
        expected.extend_from_slice(&XxHash64::new().calculate(payload).to_le_bytes());
        expected.extend_from_slice(payload);
        assert_eq!(sink.written, expected);
        // [len|checksum] and payload travel together: one call, not three.
        assert_eq!(sink.calls, 1);
    }

    #[cfg(feature = "xxhash")]
    #[test]
    fn checksummed_frame_survives_a_dribbling_sink() {
        use crate::checksum::Checksum;
        use crate::XxHash64;

        let payload = b"checksummed payload".as_slice();
        let mut sink = PartialVectored {
            written: Vec::new(),
            limit: 5,
            calls: 0,
        };
        ChecksumFramer::new(XxHash64::new())
            .frame_and_write(&mut sink, payload)
            .unwrap();

        let mut expected = (payload.len() as u32).to_le_bytes().to_vec();
        expected.extend_from_slice(&XxHash64::new().calculate(payload).to_le_bytes());
        expected.extend_from_slice(payload);
        assert_eq!(sink.written, expected);
    }

    #[test]
    fn a_stalled_sink_becomes_write_zero_not_a_hang() {
        let err = DefaultFramer
            .frame_and_write(&mut Stalled, b"payload")
            .unwrap_err();
        match err.into_kind() {
            crate::error::ErrorKind::Io(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::WriteZero)
            }
            other => panic!("expected Io(WriteZero), got {other:?}"),
        }
    }

    #[test]
    fn empty_payload_terminates() {
        // A zero-length payload leaves a trailing empty slice; the loop must
        // recognize the frame as complete instead of reading `Ok(0)` as a
        // stall. Regression guard for the `advance_slices(.., 0)` prologue.
        let mut sink = PartialVectored {
            written: Vec::new(),
            limit: usize::MAX,
            calls: 0,
        };
        DefaultFramer.frame_and_write(&mut sink, b"").unwrap();
        assert_eq!(sink.written, vec![0, 0, 0, 0]);
    }
}
