use flatstream::*;

mod harness {
    pub mod faulty_reader;
}
use harness::faulty_reader::{FaultMode, FaultyReader};

#[test]
fn short_reads_are_handled() {
    // Build a valid default-framed message
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, b"hello").unwrap();

    let inner = std::io::Cursor::new(out);
    let faulty = FaultyReader::new(inner, FaultMode::OneByteChunks);
    let mut reader = StreamReader::new(faulty, DefaultDeframer);

    let mut count = 0usize;
    reader
        .process_all(|p| {
            assert_eq!(p, b"hello");
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn interrupted_reads_are_retried() {
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, b"world").unwrap();
    let inner = std::io::Cursor::new(out);
    let faulty = FaultyReader::new(inner, FaultMode::InterruptedEvery(2));
    let mut reader = StreamReader::new(faulty, DefaultDeframer);

    // Our reader doesn't automatically retry on Interrupted; process_all will surface the error.
    // We wrap with a small loop to simulate retry behavior that upper layers might implement.
    loop {
        match reader.process_all(|p| {
            assert_eq!(p, b"world");
            Ok(())
        }) {
            Ok(()) => break,
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            other => panic!("unexpected result: {other:?}"),
        }
    }
}

#[test]
fn premature_eof_yields_unexpected_eof() {
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, b"abcdef").unwrap();
    let inner = std::io::Cursor::new(out);
    let faulty = FaultyReader::new(inner, FaultMode::PrematureEofAt(2));
    let mut reader = StreamReader::new(faulty, DefaultDeframer);

    let result = reader.read_message();
    assert!(matches!(result, Err(Error::UnexpectedEof)));
}
