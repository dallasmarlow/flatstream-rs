#![no_main]
use flatstream::{DefaultDeframer, StreamReader};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

// The deframers default to the wire format's ~4 GiB ceiling, so untrusted input
// is read through an explicitly tightened bound (the recommended setup).
// Arbitrary bytes must never panic, hang, or allocate past that bound; every
// yielded payload respects it.
const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

fuzz_target!(|data: &[u8]| {
    let deframer = DefaultDeframer::new().with_max_frame_len(MAX_FRAME_LEN);
    let mut reader = StreamReader::new(Cursor::new(data), deframer);
    let _ = reader.process_all(|payload| {
        assert!(payload.len() <= MAX_FRAME_LEN);
        Ok(())
    });
});
