//! C5 — Seekable live-file retry hardening.
//!
//! A follower tailing a journal that another process is still writing will
//! routinely observe a frame that is only partially on disk. The contract that
//! makes this safe is stated in `docs/DESIGN_v2_8.md` and the [`read_frame_at`]
//! rustdoc: an EOF observed *inside* a frame is [`ErrorKind::UnexpectedEof`] —
//! a description of what this read saw, **not** a claim that the file is
//! finalized — and because `read_frame_at` seeks back to the frame start on
//! every call, the follower may simply retry the same absolute offset once more
//! bytes arrive. No `IncompleteFrame` error kind, no partially-consumed reader
//! state to reconcile.
//!
//! These tests pin that on **real files with separate writer and reader
//! handles**, which is the case the in-memory `Cursor` retry test in
//! `tests/positioned_reads.rs` cannot reach: here the reader's `File` has its
//! own kernel file offset and its own view of the file length, and the writer
//! appends out-of-band between the reader's two attempts. Both the default and
//! the CRC-32 framing schemes are covered, because their header widths differ
//! and a torn frame can stop at a different field in each.
//!
//! What is deliberately *not* asserted: that a sequential [`StreamReader`] can
//! resume after a short read. It cannot — its `CountingReader` has already
//! consumed the partial bytes. Point-read retry is safe *because* it is
//! stateless; this file exercises only that primitive.

use flatbuffers::FlatBufferBuilder;
use flatstream::{
    read_frame_at, DefaultDeframer, DefaultFramer, ErrorKind, RetrySafeDeframer, StreamWriter,
};
use std::fs::{File, OpenOptions};
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;
use tempfile::NamedTempFile;

#[cfg(feature = "crc32")]
use flatstream::{ChecksumDeframer, ChecksumFramer, Crc32};

/// A writer handle that appends to an existing file, leaving already-written
/// bytes in place — the shape of a journal a follower tails.
fn append_handle(path: &Path) -> File {
    OpenOptions::new().append(true).open(path).unwrap()
}

/// Frames `values` with `framer` into a single in-memory wire image, returning
/// the bytes and the expected payload of each frame. Building the wire image
/// once and then dribbling it onto disk is what lets a test cut a frame at an
/// arbitrary byte and later complete it byte-for-byte.
fn wire_image<F: flatstream::Framer>(framer: F, values: &[&str]) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut wire = Vec::new();
    let mut payloads = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut wire), framer);
        let mut builder = FlatBufferBuilder::new();
        for value in values {
            builder.reset();
            let off = builder.create_string(value);
            builder.finish(off, None);
            payloads.push(builder.finished_data().to_vec());
            writer.write_finished(&mut builder).unwrap();
        }
    }
    (wire, payloads)
}

/// The heart of C5, parameterized over the framing scheme so default and
/// CRC-32 share one proof: with a `split`-byte prefix of one frame on disk, a
/// point read at offset 0 must report [`ErrorKind::UnexpectedEof`]; after the
/// remainder is appended through a *separate* writer handle, retrying the same
/// offset must return the exact payload and a receipt naming the whole frame.
fn assert_partial_frame_retry<D: RetrySafeDeframer>(wire: &[u8], expected: &[u8], deframer: &D) {
    // A prefix that stops strictly inside the frame — not at a boundary — is
    // the input the contract is about. Cover several cut points, including one
    // inside the header (offset 2, mid-length-prefix) and one mid-payload.
    let cut_points = [2usize, 4, wire.len() / 2, wire.len() - 1];
    for &split in &cut_points {
        assert!(split < wire.len(), "a partial prefix must omit real bytes");

        let temp = NamedTempFile::new().unwrap();
        // Separate handles on the same path: the writer appends, the reader
        // seeks and reads, each with its own kernel file position and its own
        // view of the length — exactly the follower shape.
        let mut writer = append_handle(temp.path());
        let mut reader = File::open(temp.path()).unwrap();

        writer.write_all(&wire[..split]).unwrap();
        writer.flush().unwrap();

        let mut scratch = Vec::new();
        let err = read_frame_at(&mut reader, deframer, 0, &mut scratch)
            .expect_err("a frame cut at an interior byte must not read as complete");
        assert!(
            matches!(err.kind(), ErrorKind::UnexpectedEof),
            "interior EOF must be UnexpectedEof, got {:?} at split {split}",
            err.kind()
        );

        // The remainder arrives out-of-band, as a real appender would deliver
        // it. The reader has done nothing it must now undo.
        writer.write_all(&wire[split..]).unwrap();
        writer.flush().unwrap();

        let frame = read_frame_at(&mut reader, deframer, 0, &mut scratch)
            .unwrap()
            .expect("the completed frame must now read");
        assert_eq!(frame.payload, expected, "payload at split {split}");
        assert_eq!(frame.receipt.frame_start, 0);
        assert_eq!(
            frame.receipt.wire_len,
            wire.len() as u64,
            "receipt must name the whole frame at split {split}"
        );
    }
}

#[test]
fn default_partial_frame_retries_byte_exact_after_append() {
    let (wire, payloads) = wire_image(DefaultFramer, &["eventually complete default frame"]);
    assert_partial_frame_retry(&wire, &payloads[0], &DefaultDeframer::new());
}

#[cfg(feature = "crc32")]
#[test]
fn crc32_partial_frame_retries_byte_exact_after_append() {
    let (wire, payloads) = wire_image(
        ChecksumFramer::new(Crc32::new()),
        &["eventually complete crc32 frame"],
    );
    assert_partial_frame_retry(&wire, &payloads[0], &ChecksumDeframer::new(Crc32::new()));
}

#[test]
fn clean_eof_at_a_frame_boundary_is_none_not_an_error() {
    // The distinction the whole task turns on: EOF at offset == end-of-last-frame
    // is a clean boundary (Ok(None)), while EOF one byte into the *next* frame is
    // UnexpectedEof. A follower uses the former to mean "caught up for now".
    let (wire, _) = wire_image(DefaultFramer, &["first", "second"]);
    let temp = NamedTempFile::new().unwrap();
    let mut writer = append_handle(temp.path());
    writer.write_all(&wire).unwrap();
    writer.flush().unwrap();

    let mut reader = File::open(temp.path()).unwrap();
    let mut scratch = Vec::new();

    // Read both frames forward to discover the exact boundary offset.
    let first = read_frame_at(&mut reader, &DefaultDeframer::new(), 0, &mut scratch)
        .unwrap()
        .unwrap();
    let boundary = first.receipt.end();
    let second = read_frame_at(&mut reader, &DefaultDeframer::new(), boundary, &mut scratch)
        .unwrap()
        .unwrap();
    let end = second.receipt.end();
    assert_eq!(end, wire.len() as u64);

    // At the trailing boundary there is nothing more: a clean tail, not a tear.
    assert!(
        read_frame_at(&mut reader, &DefaultDeframer::new(), end, &mut scratch)
            .unwrap()
            .is_none(),
        "clean EOF at a frame boundary must be Ok(None)"
    );
}

/// A `Read + Seek` source that seeks like the file beneath it but fails every
/// `read` with a chosen non-EOF error. It proves the retry path does not
/// collapse an arbitrary device error into `UnexpectedEof` — only a genuine
/// end-of-input is a torn tail; anything else must propagate intact so a
/// follower does not mistake a failing disk for a frame that will arrive.
struct FailingReads<R> {
    inner: R,
    kind: io::ErrorKind,
}

impl<R: Read> Read for FailingReads<R> {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(self.kind, "injected device error"))
    }
}

impl<R: Seek> Seek for FailingReads<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[test]
fn a_non_eof_device_error_propagates_and_is_not_a_torn_tail() {
    let (wire, _) = wire_image(DefaultFramer, &["intact on disk"]);
    let temp = NamedTempFile::new().unwrap();
    let mut writer = append_handle(temp.path());
    writer.write_all(&wire).unwrap();
    writer.flush().unwrap();

    let mut source = FailingReads {
        inner: File::open(temp.path()).unwrap(),
        kind: io::ErrorKind::PermissionDenied,
    };
    let mut scratch = Vec::new();
    let err = read_frame_at(&mut source, &DefaultDeframer::new(), 0, &mut scratch)
        .expect_err("a device error must not be swallowed");
    match err.into_kind() {
        ErrorKind::Io(e) => assert_eq!(e.kind(), io::ErrorKind::PermissionDenied),
        other => panic!("expected Io(PermissionDenied), got {other:?}"),
    }
}
