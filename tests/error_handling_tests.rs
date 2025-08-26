use flatstream::*;
use std::io::{self, Cursor, Write};

struct TestMessage(&'static str);

impl StreamSerialize for TestMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<A>,
    ) -> Result<()> {
        let offset = builder.create_string(self.0);
        builder.finish(offset, None);
        Ok(())
    }
}

struct FailingWriter {
    written: usize,
    fail_after: usize,
}

impl Write for FailingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written >= self.fail_after {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Simulated I/O error",
            ));
        }
        let remaining = self.fail_after - self.written;
        let n = remaining.min(buf.len());
        self.written += n;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn io_error_propagates() {
    // Purpose: A write error from the underlying writer must surface as Error::Io
    // from StreamWriter::write.
    let failing_writer = FailingWriter {
        written: 0,
        fail_after: 10,
    };
    let mut writer = StreamWriter::new(failing_writer, DefaultFramer);
    let msg = TestMessage("This message will fail to write completely");
    match writer.write(&msg) {
        Ok(_) => panic!("expected I/O error"),
        Err(Error::Io(e)) => {
            assert_eq!(e.kind(), io::ErrorKind::BrokenPipe);
        }
        Err(e) => panic!("wrong error type: {e:?}"),
    }
}

#[test]
fn invalid_frame_detected_or_eof() {
    // Purpose: A wildly large length prefix should produce InvalidFrame or UnexpectedEof
    // when attempting to read the declared payload.
    let mut buffer = Vec::new();
    let huge_length: u32 = 100_000_000;
    buffer.extend_from_slice(&huge_length.to_le_bytes());
    buffer.extend_from_slice(b"some data");
    let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
    match reader.read_message() {
        Ok(_) => panic!("expected error"),
        Err(Error::InvalidFrame { .. }) | Err(Error::UnexpectedEof) => {}
        Err(e) => panic!("wrong error type: {e:?}"),
    }
}

#[test]
fn unexpected_eof_detected() {
    // Purpose: Declared length with missing payload bytes should yield UnexpectedEof.
    let mut buffer = Vec::new();
    let length: u32 = 100;
    buffer.extend_from_slice(&length.to_le_bytes());
    let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
    match reader.read_message() {
        Ok(_) => panic!("expected UnexpectedEof"),
        Err(Error::UnexpectedEof) => {}
        Err(e) => panic!("wrong error type: {e:?}"),
    }
}

#[test]
fn clean_eof_ok() {
    // Purpose: After reading exactly the written messages, subsequent reads should
    // return Ok(None) (clean EOF), and process_all should iterate the same count.
    let mut buffer = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        writer.write(&TestMessage("m1")).unwrap();
        writer.write(&TestMessage("m2")).unwrap();
    }
    let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
    let mut count = 0;
    loop {
        match reader.read_message() {
            Ok(Some(_)) => count += 1,
            Ok(None) => break,
            Err(e) => panic!("unexpected error: {e:?}"),
        }
    }
    assert_eq!(count, 2);

    let mut reader2 = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
    let mut count2 = 0;
    reader2
        .process_all(|_| {
            count2 += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count2, 2);
}

#[cfg(feature = "xxhash")]
#[test]
fn checksum_mismatch_detected() {
    // Purpose: Corrupting the checksum bytes should cause ChecksumMismatch on read.
    let mut buffer = Vec::new();
    {
        let framer = ChecksumFramer::new(XxHash64::new());
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        writer.write(&TestMessage("valid")).unwrap();
    }
    // corrupt one byte in the checksum field (positions 4..12)
    if buffer.len() >= 6 {
        buffer[5] ^= 0xFF;
    }
    let deframer = ChecksumDeframer::new(XxHash64::new());
    let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
    match reader.read_message() {
        Ok(_) => panic!("expected checksum mismatch"),
        Err(Error::ChecksumMismatch { .. }) => {}
        Err(e) => panic!("wrong error type: {e:?}"),
    }
}
