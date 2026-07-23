#![no_main]
use flatstream::{
    ChecksumDeframer, ChecksumFramer, Framer, StreamReader, XxHash64, DEFAULT_MAX_FRAME_LEN,
};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

// Two invariants per input: arbitrary bytes must never panic the checksummed
// deframer, and any payload framed with the matching checksum must read back
// byte-identical.
fuzz_target!(|data: &[u8]| {
    let deframer = ChecksumDeframer::new(XxHash64::new());
    let mut reader = StreamReader::new(Cursor::new(data), deframer);
    let _ = reader.process_all(|_| Ok(()));

    if data.len() <= DEFAULT_MAX_FRAME_LEN {
        let mut framed = Vec::new();
        ChecksumFramer::new(XxHash64::new())
            .frame_and_write(&mut framed, data)
            .unwrap();
        let mut reader =
            StreamReader::new(Cursor::new(&framed), ChecksumDeframer::new(XxHash64::new()));
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
});
