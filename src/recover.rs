//! Crash recovery: scan a journal, count the intact prefix, report the
//! truncation point.
//!
//! A journal that stopped mid-append — a crash, a kill, a full disk — ends in
//! a torn frame: a partial length header, a partial checksum field, or a
//! partial payload. The reader detects that as `UnexpectedEof` (spec §6);
//! this module turns that one case into the recovery contract the journaling
//! use case needs: a torn tail is a **result** with a safe truncation point.
//!
//! Only `UnexpectedEof` authorizes truncation. Everything else the deframer
//! can report means the file is *not* a cleanly torn journal, and truncating
//! on it would destroy data that a corrupted prefix may still make
//! recoverable — or destroy a perfectly good file read with the wrong
//! configuration:
//!
//! - `ChecksumMismatch` is corruption inside a fully present frame, not a
//!   torn append — or a mismatched checksum configuration;
//! - `InvalidFrame` can mean a wrong format, a configured bound smaller than
//!   the stream's real frames, or a corrupt header;
//! - `ValidationFailed` can mean schema or validator drift, not bad bytes.
//!
//! All of those return `Err` with the stop reason intact; the caller — not
//! this function — decides what a corrupted or misread file deserves. Run
//! recovery with the deframer that matches the wire format (plain, matching
//! checksum, no validators). A prefix-salvage API that truncates *through*
//! corruption would be a separate, explicitly destructive operation; it does
//! not exist today by design.
//!
//! Scope: this contract is designed for **append-only journals whose
//! expected failure mode is a crash during the final write**. It is not a
//! general repair tool for arbitrarily damaged files, and it must not be
//! pointed at sealed container files (a valid container footer is handled by
//! the container layer, never by scanning).

use crate::error::{ErrorKind, Result};
use crate::framing::Deframer;
use std::io::{Read, Seek, SeekFrom};

/// How a recovery scan ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryEnd {
    /// The stream ended exactly on a frame boundary: nothing to truncate.
    CleanEof,
    /// The stream ends in a torn frame (partial header, checksum field, or
    /// payload — the crash-mid-append signature). Truncating to
    /// `last_good_offset` is safe.
    TornTail,
}

/// Outcome of scanning a stream with [`recover`] or [`recover_file`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Number of intact frames before the scan stopped.
    pub frames: u64,
    /// Byte offset one past the last intact frame. For [`recover_file`] this
    /// is an absolute file offset; for [`recover`] it is relative to the
    /// reader's position when the scan began. Zero when no frame is intact.
    pub last_good_offset: u64,
    /// How the scan ended.
    pub end: RecoveryEnd,
}

/// Tracks how many bytes the deframer has consumed. A recovery-compatible
/// deframer must consume exactly one frame and never read ahead; the built-in
/// deframers and their adapters satisfy that contract. The count after a
/// successful frame is therefore precisely that frame's end offset.
struct CountingReader<R> {
    inner: R,
    count: u64,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count += n as u64;
        Ok(n)
    }
}

/// Scans a stream from the reader's **current position** and reports the
/// intact prefix. Offsets in the report are relative to that starting
/// position; for scanning a file from its beginning with absolute offsets,
/// use [`recover_file`].
///
/// The contract, exactly:
///
/// - clean EOF at a frame boundary → `Ok` with [`RecoveryEnd::CleanEof`];
/// - `UnexpectedEof` inside a frame (the crash-mid-append signature) → `Ok`
///   with [`RecoveryEnd::TornTail`] and a safe truncation point;
/// - **anything else** — `ChecksumMismatch`, `InvalidFrame`,
///   `ValidationFailed`, or a genuine device fault — → `Err` with the stop
///   reason intact. Corruption and misconfiguration never authorize
///   truncation.
///
/// A complete but corrupted length header can declare a large in-bounds
/// payload and cause a wasted allocation before EOF is detected (the report
/// stays correct). A genuinely torn 1–3 byte length header is rejected before
/// a length is parsed. When scanning files that may be arbitrarily corrupt,
/// pass a deframer tightened with `with_max_frame_len` to the size your
/// application actually writes.
///
/// Recovery-compatible deframers must consume exactly one frame per call and
/// never read ahead — the offset accounting depends on it. The built-in
/// deframers and their adapters satisfy this; a custom `Deframer` that
/// buffers or reads speculatively would misreport `last_good_offset`.
pub fn recover<R: Read, D: Deframer>(reader: R, deframer: D) -> Result<RecoveryReport> {
    let mut reader = CountingReader {
        inner: reader,
        count: 0,
    };
    let mut buffer = Vec::new();
    let mut report = RecoveryReport {
        frames: 0,
        last_good_offset: 0,
        end: RecoveryEnd::TornTail,
    };
    loop {
        match deframer.read_and_deframe(&mut reader, &mut buffer) {
            Ok(Some(_)) => {
                report.frames += 1;
                report.last_good_offset = reader.count;
            }
            Ok(None) => {
                report.end = RecoveryEnd::CleanEof;
                return Ok(report);
            }
            Err(e) => {
                return match e.kind() {
                    // The crash-mid-append signature — and only it.
                    ErrorKind::UnexpectedEof => {
                        report.end = RecoveryEnd::TornTail;
                        Ok(report)
                    }
                    // Corruption, misconfiguration, and device faults all
                    // surface intact: none of them authorize truncation.
                    _ => Err(e),
                };
            }
        }
    }
}

/// Scans a seekable stream (typically a journal file) from its **beginning**
/// and reports the intact prefix with **absolute** offsets.
///
/// Seeks to offset 0, scans with the same contract as [`recover`], and
/// returns with the cursor positioned at `last_good_offset` — after a
/// [`RecoveryEnd::TornTail`] report the caller truncates to that offset
/// (e.g. `File::set_len`) and can resume appending from the current cursor.
/// If scanning or the final seek returns `Err`, the cursor position is
/// unspecified and the caller must not append through that handle.
///
/// ```
/// use flatstream::{recover_file, DefaultDeframer, DefaultFramer, Framer, RecoveryEnd, Result};
/// use std::io::{Cursor, Seek};
///
/// # fn main() -> Result<()> {
/// // A journal with two intact frames and a torn tail (crash mid-append).
/// let mut journal = Vec::new();
/// DefaultFramer.frame_and_write(&mut journal, b"frame one")?;
/// DefaultFramer.frame_and_write(&mut journal, b"frame two")?;
/// let intact = journal.len() as u64;
/// journal.extend_from_slice(&[7, 0, 0, 0, b'x']); // declares 7 bytes, has 1
///
/// let mut file = Cursor::new(journal);
/// file.seek(std::io::SeekFrom::End(0))?; // recover_file seeks for itself
///
/// let report = recover_file(&mut file, DefaultDeframer::new())?;
/// assert_eq!(report.frames, 2);
/// assert_eq!(report.last_good_offset, intact);
/// assert_eq!(report.end, RecoveryEnd::TornTail);
///
/// // Truncate ONLY on a torn tail, then append from the repaired end.
/// if report.end == RecoveryEnd::TornTail {
///     let offset = usize::try_from(report.last_good_offset)
///         .expect("in-memory journal offset fits usize");
///     file.get_mut().truncate(offset);
/// }
/// assert_eq!(file.stream_position()?, report.last_good_offset);
/// DefaultFramer.frame_and_write(&mut file, b"frame three")?;
/// # Ok(())
/// # }
/// ```
pub fn recover_file<R: Read + Seek, D: Deframer>(
    reader: &mut R,
    deframer: D,
) -> Result<RecoveryReport> {
    reader.seek(SeekFrom::Start(0))?;
    let report = recover(&mut *reader, deframer)?;
    reader.seek(SeekFrom::Start(report.last_good_offset))?;
    Ok(report)
}
