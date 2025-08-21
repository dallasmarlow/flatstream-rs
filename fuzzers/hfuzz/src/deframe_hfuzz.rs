use flatstream::{DefaultDeframer, Deframer, StreamReader};
use honggfuzz::fuzz;
use std::io::Cursor;

fn main() {
    loop {
        fuzz!(|data: &[u8]| {
            let mut reader = StreamReader::new(Cursor::new(data), DefaultDeframer);
            let _ = reader.process_all(|_| Ok(()));
        });
    }
}

