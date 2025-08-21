#![no_main]
use flatstream::{DefaultDeframer, Deframer, StreamReader};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let deframer = DefaultDeframer;
    let mut reader = StreamReader::new(Cursor::new(data), deframer);
    let _ = reader.process_all(|_| Ok(()));
});

