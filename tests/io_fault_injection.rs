use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::io::Write;

mod harness {
    pub mod faulty_reader;
}
use harness::faulty_reader::{FaultMode, FaultyReader};

/// Accepts `fail_after` bytes, then fails every write with BrokenPipe.
struct FailingWriter {
    written: usize,
    fail_after: usize,
}

impl Write for FailingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.written >= self.fail_after {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Simulated I/O error",
            ));
        }
        let remaining = self.fail_after - self.written;
        let n = remaining.min(buf.len());
        self.written += n;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn write_io_error_propagates_with_kind() {
    // Purpose: A write error from the underlying writer must surface as
    // ErrorKind::Io with the original io::ErrorKind preserved.
    let failing_writer = FailingWriter {
        written: 0,
        fail_after: 10,
    };
    let mut writer = StreamWriter::new(failing_writer, DefaultFramer);
    let mut b = FlatBufferBuilder::new();
    let s = b.create_string("This message will fail to write completely");
    b.finish(s, None);
    match writer.write_finished(&mut b).unwrap_err().into_kind() {
        ErrorKind::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe),
        other => panic!("wrong error type: {other:?}"),
    }
}

#[test]
fn short_reads_are_handled() {
    // Purpose: Reader should handle an underlying reader that returns very small chunks.
    // Build a valid default-framed message
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, b"hello").unwrap();

    let inner = std::io::Cursor::new(out);
    let faulty = FaultyReader::new(inner, FaultMode::OneByteChunks);
    let mut reader = StreamReader::new(faulty, DefaultDeframer::new());

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
fn interrupted_reads_are_transparent() {
    // Purpose: Simulate EINTR on every second read call. The deframers read via
    // `read_exact`, which retries `ErrorKind::Interrupted` internally (std
    // contract), so interruption never surfaces to the caller at all — the
    // stream reads through successfully on the first attempt. (An earlier
    // version of this test wrapped process_all in a caller-side retry loop
    // "because errors surface"; that premise was false and the loop was dead
    // code — this assertion is the true contract.)
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, b"world").unwrap();
    let inner = std::io::Cursor::new(out);
    let faulty = FaultyReader::new(inner, FaultMode::InterruptedEvery(2));
    let mut reader = StreamReader::new(faulty, DefaultDeframer::new());

    let mut count = 0;
    reader
        .process_all(|p| {
            assert_eq!(p, b"world");
            count += 1;
            Ok(())
        })
        .expect("Interrupted is absorbed by read_exact, never surfaced");
    assert_eq!(count, 1);
}

#[test]
fn premature_eof_yields_unexpected_eof() {
    // Purpose: A reader that stops mid-frame should produce UnexpectedEof on read_message.
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, b"abcdef").unwrap();
    let inner = std::io::Cursor::new(out);
    let faulty = FaultyReader::new(inner, FaultMode::PrematureEofAt(2));
    let mut reader = StreamReader::new(faulty, DefaultDeframer::new());

    let err = reader.read_message().unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::UnexpectedEof));
}
