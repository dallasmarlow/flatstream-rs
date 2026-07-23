#![no_main]
use flatstream::{DefaultDeframer, StreamReader, DEFAULT_MAX_FRAME_LEN};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

// Arbitrary bytes must never panic, hang, or allocate past the frame bound;
// every yielded payload respects the default limit.
fuzz_target!(|data: &[u8]| {
    let mut reader = StreamReader::new(Cursor::new(data), DefaultDeframer::new());
    let _ = reader.process_all(|payload| {
        assert!(payload.len() <= DEFAULT_MAX_FRAME_LEN);
        Ok(())
    });
});
