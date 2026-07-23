#![no_main]
use flatstream::checksum::Checksum;
use flatstream::{
    ChecksumDeframer, ChecksumFramer, Crc16, Crc32, Framer, StreamReader, XxHash64,
};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

// The deframers default to the wire format's ~4 GiB ceiling, so untrusted input
// is read through an explicitly tightened bound (the recommended setup).
const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

fn exercise_checksum<C: Checksum + Copy>(data: &[u8], checksum: C) {
    // Arbitrary bytes must never panic, and any yielded payload must respect the
    // configured allocation bound.
    let deframer = ChecksumDeframer::new(checksum).with_max_frame_len(MAX_FRAME_LEN);
    let mut reader = StreamReader::new(Cursor::new(data), deframer);
    let _ = reader.process_all(|payload| {
        assert!(payload.len() <= MAX_FRAME_LEN);
        Ok(())
    });

    // Any payload framed with the matching algorithm must read back exactly.
    if data.len() <= MAX_FRAME_LEN {
        let mut framed = Vec::new();
        ChecksumFramer::new(checksum)
            .frame_and_write(&mut framed, data)
            .unwrap();
        let deframer = ChecksumDeframer::new(checksum).with_max_frame_len(MAX_FRAME_LEN);
        let mut reader = StreamReader::new(Cursor::new(&framed), deframer);
        let mut seen = false;
        reader
            .process_all(|payload| {
                assert_eq!(payload, data);
                seen = true;
                Ok(())
            })
            .unwrap();
        assert!(seen);
    }
}

// Exercise every built-in checksum width and implementation for each input.
fuzz_target!(|data: &[u8]| {
    exercise_checksum(data, XxHash64::new());
    exercise_checksum(data, Crc32::new());
    exercise_checksum(data, Crc16::new());
});
