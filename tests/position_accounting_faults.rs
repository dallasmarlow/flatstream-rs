//! C6 — Position-accounting fault semantics.
//!
//! Two distinct accounting mechanisms sit behind the [`FrameReceipt`]s these
//! tests check. On the sequential [`StreamReader`] path, `bytes_consumed` and
//! every receipt rest on one invariant: the reader's internal `CountingReader`
//! counts exactly the bytes the source *returns*, no more and no less. The
//! stateless [`read_frame_at`] path installs the same counting boundary after
//! its initial seek. Both must agree with the writer's recorded receipts
//! frame-for-frame. These tests pin what that means at the edges the happy-path
//! suites never reach:
//!
//! - (a) a **custom deframer that reads its payload with `read_vectored`** is
//!   accounted just as precisely as one using `read`: on the sequential path
//!   because the counting wrapper forwards and tallies `read_vectored` too, and
//!   on the `read_frame_at` path because its per-call wrapper counts the same
//!   vectored reads. Its receipts are byte-for-byte the ones a `DefaultFramer`
//!   write recorded on both paths.
//! - (b) bytes a frame **successfully consumed before a torn tail**
//!   (`UnexpectedEof`) still advance `bytes_consumed`; a mid-frame EOF does not
//!   roll the counter back to the frame start.
//! - (c) a genuine **device error** contributes nothing to the counter: only
//!   the bytes actually returned before it are counted, and the failing read
//!   itself adds zero.
//! - (d) a nonzero [`with_start_offset`](StreamReader::with_start_offset)
//!   **composes with an installed static memory policy**: receipts stay
//!   base-relative and exact across a buffer reclamation that shrinks the
//!   internal buffer mid-stream.
//!
//! Every case uses a deterministic in-memory source (a `Cursor`) or a local
//! `Read`/`Seek` wrapper — no tempfiles that could race, no sleeps, and no
//! benchmark claims. All framing is `DefaultFramer`, so these tests run in the
//! default gate without a checksum feature; the wire format the custom deframer
//! reads is the ordinary length-prefixed one.

use flatbuffers::FlatBufferBuilder;
use flatstream::{
    read_frame_at, DefaultDeframer, DefaultFramer, Deframer, Error, ErrorKind, FrameReceipt,
    Result, RetrySafeDeframer, SizeThresholdPolicy, StreamReader, StreamWriter,
};
use std::io::{self, Cursor, IoSliceMut, Read};

/// Serializes each value as a FlatBuffer string root and returns the wire image
/// plus the per-frame payloads and the receipts the writer recorded. The
/// receipts are the ground truth every reader in this file is checked against.
fn default_stream(values: &[&str]) -> (Vec<u8>, Vec<Vec<u8>>, Vec<FrameReceipt>) {
    let mut wire = Vec::new();
    let mut payloads = Vec::new();
    let mut receipts = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut wire), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        for value in values {
            builder.reset();
            let off = builder.create_string(value);
            builder.finish(off, None);
            payloads.push(builder.finished_data().to_vec());
            receipts.push(writer.write_finished_with_receipt(&mut builder).unwrap());
        }
    }
    (wire, payloads, receipts)
}

// --- (a) A custom deframer that reads via `read_vectored`. ---

/// A `[4-byte length | payload]` deframer whose payload read is driven through
/// [`Read::read_vectored`] rather than a single `read_exact`. It splits the
/// destination into two `IoSliceMut` halves so the vectored path genuinely
/// runs, and it is otherwise wire-compatible with [`DefaultFramer`]. The header
/// is still parsed by the trait's provided `read_and_deframe`, so only the
/// payload read differs from the default — which is the whole point: the byte
/// counter must tally `read_vectored` returns exactly as it does `read`.
struct VectoredDeframer;

impl Deframer for VectoredDeframer {
    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<usize>> {
        if payload_len > buffer.len() {
            buffer.resize(payload_len, 0);
        }
        let mut filled = 0;
        while filled < payload_len {
            // Split the outstanding tail into two slices so the read is truly
            // vectored; a 1-byte remainder leaves an empty head, which the
            // std fallback simply skips.
            let remaining = &mut buffer[filled..payload_len];
            let mid = remaining.len() / 2;
            let (head, tail) = remaining.split_at_mut(mid);
            let mut slices = [IoSliceMut::new(head), IoSliceMut::new(tail)];
            match reader.read_vectored(&mut slices) {
                // Bytes were outstanding, yet the source returned none: an EOF
                // inside the frame, which is a torn tail, not a real fault.
                Ok(0) => return Err(Error::unexpected_eof()),
                Ok(n) => filled += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Err(Error::unexpected_eof())
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Some(payload_len))
    }
}

impl RetrySafeDeframer for VectoredDeframer {}

#[test]
fn vectored_custom_deframer_produces_exact_receipt_bounds() {
    let (wire, expected, receipts) = default_stream(&["alpha", "second frame", "three"]);

    // Sequential reader path: every receipt must equal the writer's, proving
    // `read_vectored` returns are counted with the same precision as `read`.
    let mut reader = StreamReader::new(Cursor::new(&wire), VectoredDeframer);
    let mut seen = 0usize;
    reader
        .process_all_with_receipt(|frame| {
            assert_eq!(frame.payload, &expected[seen][..], "payload {seen}");
            assert_eq!(frame.receipt, receipts[seen], "receipt {seen}");
            seen += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(seen, expected.len());
    assert_eq!(reader.bytes_consumed(), wire.len() as u64);

    // Point-read path: the per-call counting wrapper must produce the same
    // receipt for a deframer it never special-cases.
    let mut source = Cursor::new(&wire);
    let mut scratch = Vec::new();
    for i in 0..expected.len() {
        let frame = read_frame_at(
            &mut source,
            &VectoredDeframer,
            receipts[i].frame_start,
            &mut scratch,
        )
        .unwrap()
        .unwrap();
        assert_eq!(frame.payload, &expected[i][..]);
        assert_eq!(frame.receipt, receipts[i]);
    }
}

// --- (b) Bytes consumed before an UnexpectedEof still advance the counter. ---

#[test]
fn bytes_consumed_before_a_torn_tail_are_retained() {
    let (wire, _expected, receipts) = default_stream(&["first", "second frame torn here"]);
    let boundary = receipts[0].end();

    // Truncate mid-second-frame: the whole 4-byte length header plus two
    // payload bytes are present, so the failed read consumes them before it
    // hits EOF. It must land strictly inside the second frame.
    let trunc_len = (boundary as usize) + 4 + 2;
    assert!(
        (trunc_len as u64) < receipts[1].end(),
        "truncation must fall inside the second frame"
    );

    let mut reader = StreamReader::new(Cursor::new(&wire[..trunc_len]), DefaultDeframer::new());

    let first = reader.read_message_with_receipt().unwrap().unwrap();
    assert_eq!(first.receipt, receipts[0]);
    assert_eq!(reader.bytes_consumed(), boundary);
    assert!(!reader.is_poisoned());

    let err = reader
        .read_message()
        .expect_err("a mid-frame truncation must surface as an error");
    assert!(matches!(err.kind(), ErrorKind::UnexpectedEof));
    assert!(
        reader.is_poisoned(),
        "a failed read that consumed frame bytes must fail-stop the reader"
    );

    // The counter reflects the header and payload bytes the failed attempt
    // did consume — it is not rolled back to the second frame's start.
    assert!(
        reader.bytes_consumed() > boundary,
        "consumed bytes before the tear must be retained"
    );
    assert_eq!(
        reader.bytes_consumed(),
        trunc_len as u64,
        "every available byte of the torn frame was consumed and counted"
    );

    let poisoned = reader
        .read_message()
        .expect_err("a sequential reader must not continue from the middle of a frame");
    assert!(matches!(poisoned.kind(), ErrorKind::Poisoned));
    assert_eq!(reader.bytes_consumed(), trunc_len as u64);
}

// --- (c) A device error counts only the bytes actually returned. ---

/// A source that returns bytes normally until `budget` bytes have been handed
/// out, then fails every subsequent `read` with a non-EOF device error. The
/// failing read returns `Err` (zero bytes), so a correct counter must never
/// attribute anything to it.
struct DeviceErrorAfter<R> {
    inner: R,
    budget: usize,
}

impl<R: Read> Read for DeviceErrorAfter<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.budget == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "device gone",
            ));
        }
        let cap = buf.len().min(self.budget);
        let n = self.inner.read(&mut buf[..cap])?;
        self.budget -= n;
        Ok(n)
    }
}

#[test]
fn a_device_error_counts_only_bytes_actually_returned() {
    let (wire, _expected, receipts) = default_stream(&["first", "second frame interrupted"]);
    let boundary = receipts[0].end();

    // Allow the whole first frame plus five bytes of the second, then fail.
    let budget = (boundary as usize) + 5;
    assert!(
        (budget as u64) < receipts[1].end(),
        "the error must strike inside the second frame"
    );

    let source = DeviceErrorAfter {
        inner: Cursor::new(&wire),
        budget,
    };
    let mut reader = StreamReader::new(source, DefaultDeframer::new());

    let first = reader.read_message_with_receipt().unwrap().unwrap();
    assert_eq!(first.receipt, receipts[0]);
    assert_eq!(reader.bytes_consumed(), boundary);

    let err = reader
        .read_message()
        .expect_err("the injected device error must surface");
    assert!(
        matches!(err.kind(), ErrorKind::Io(e) if e.kind() == io::ErrorKind::PermissionDenied),
        "a device error must propagate intact, not become UnexpectedEof: {:?}",
        err.kind()
    );

    // Exactly the returned bytes are counted; the failing read added nothing.
    assert_eq!(
        reader.bytes_consumed(),
        budget as u64,
        "the counter equals bytes actually returned before the error"
    );
    assert!(
        reader.is_poisoned(),
        "a device error after partial-frame consumption must fail-stop the reader"
    );
}

// --- (d) A nonzero start offset composes with a static memory policy. ---

#[test]
fn start_offset_composes_with_a_reclaiming_memory_policy() {
    // A large frame grows the internal buffer and arms the policy; a run of
    // small frames then triggers a reclamation that shrinks the buffer back to
    // the policy baseline. Receipts must stay base-relative and exact across
    // that shrink. `Vec::shrink_to` may retain allocator-specific excess
    // capacity, so only the direction and minimum baseline are portable.
    let big = "b".repeat(2000);
    let values = [big.as_str(), "s1", "s2", "s3", "s4"];
    let (wire, expected, original) = default_stream(&values);

    let base = 4096u64;
    let policy = SizeThresholdPolicy::new(100, 100, 2).with_baseline(64);
    let mut reader = StreamReader::new(Cursor::new(&wire), DefaultDeframer::new())
        .with_memory_policy(policy)
        .with_start_offset(base)
        .unwrap();

    let mut caps = Vec::new();
    let mut idx = 0usize;
    // A `while let` cannot express this: the `ReadFrame` borrows the reader, so
    // the `reader.buffer_capacity()` call below is only legal once that borrow
    // ends — which is exactly what copying the payload/receipt out in this match
    // arm does. The clippy `while_let_loop` suggestion would not compile here.
    #[allow(clippy::while_let_loop)]
    loop {
        let (payload, receipt) = match reader.read_message_with_receipt().unwrap() {
            Some(frame) => (frame.payload.to_vec(), frame.receipt),
            None => break,
        };
        assert_eq!(payload, expected[idx], "payload {idx}");
        // Receipts are shifted by the start offset but otherwise identical to
        // what the writer recorded — the offset composes with the policy.
        assert_eq!(
            receipt.frame_start,
            base + original[idx].frame_start,
            "start {idx}"
        );
        assert_eq!(receipt.wire_len, original[idx].wire_len, "wire_len {idx}");
        if idx > 0 {
            // Even across the reclamation the frames still tile contiguously.
            assert_eq!(receipt.frame_start, base + original[idx].frame_start);
        }
        caps.push(reader.buffer_capacity());
        idx += 1;
    }
    assert_eq!(idx, expected.len());
    assert_eq!(reader.bytes_consumed(), base + wire.len() as u64);

    // The large frame grew the buffer well past the 64-byte baseline; the
    // reclamation scheduled after the second small frame shrinks it at the
    // start of the third small read. The exact receipts above held *through*
    // that shrink.
    assert!(
        caps[0] >= 1024,
        "the large frame grew the buffer: {}",
        caps[0]
    );
    assert!(
        caps[3] < caps[0],
        "the buffer was reclaimed before the fourth read (caps: {caps:?})"
    );
    assert!(
        caps[3] >= 64,
        "Vec::shrink_to keeps at least the requested baseline (caps: {caps:?})"
    );
}
