#![no_main]
use flatstream::checksum::Checksum;
use flatstream::{
    ChecksumDeframer, ChecksumFramer, Crc16, Crc32, Framer, StreamReader, XxHash64,
    DEFAULT_MAX_FRAME_LEN,
};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fn exercise_checksum<C: Checksum + Copy>(data: &[u8], checksum: C) {
    // Arbitrary bytes must never panic, and any yielded payload must respect
    // the configured allocation bound.
    let deframer = ChecksumDeframer::new(checksum);
    let mut reader = StreamReader::new(Cursor::new(data), deframer);
    let _ = reader.process_all(|payload| {
        assert!(payload.len() <= DEFAULT_MAX_FRAME_LEN);
        Ok(())
    });

    // Any payload framed with the matching algorithm must read back exactly.
    if data.len() <= DEFAULT_MAX_FRAME_LEN {
        let mut framed = Vec::new();
        ChecksumFramer::new(checksum)
            .frame_and_write(&mut framed, data)
            .unwrap();
        let mut reader = StreamReader::new(Cursor::new(&framed), ChecksumDeframer::new(checksum));
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
