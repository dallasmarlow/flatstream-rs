//! Verification of error handling behavior as documented
//! Tests all error types: Io, ChecksumMismatch, InvalidFrame, UnexpectedEof

use flatstream_rs::*;
use std::io::{self, Cursor, Read, Write};

struct TestMessage(String);

impl StreamSerialize for TestMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<A>,
    ) -> Result<()> {
        let offset = builder.create_string(&self.0);
        builder.finish(offset, None);
        Ok(())
    }
}

fn main() {
    println!("=== Error Handling Verification ===\n");
    
    println!("1. I/O Error Handling:");
    test_io_error();
    
    println!("\n2. Checksum Mismatch Detection:");
    #[cfg(feature = "xxhash")]
    test_checksum_mismatch();
    #[cfg(not(feature = "xxhash"))]
    println!("   (Skipped - requires 'xxhash' feature)");
    
    println!("\n3. Invalid Frame Detection:");
    test_invalid_frame();
    
    println!("\n4. Unexpected EOF Handling:");
    test_unexpected_eof();
    
    println!("\n5. Clean EOF Handling:");
    test_clean_eof();
}

// Custom writer that fails after N bytes
struct FailingWriter {
    written: usize,
    fail_after: usize,
}

impl Write for FailingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written >= self.fail_after {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "Simulated I/O error"));
        }
        self.written += buf.len();
        Ok(buf.len())
    }
    
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn test_io_error() {
    let failing_writer = FailingWriter {
        written: 0,
        fail_after: 10, // Fail after 10 bytes
    };
    
    let mut writer = StreamWriter::new(failing_writer, DefaultFramer);
    let msg = TestMessage("This message will fail to write completely".to_string());
    
    match writer.write(&msg) {
        Ok(_) => println!("   ✗ Expected I/O error, but write succeeded"),
        Err(Error::Io(e)) => {
            println!("   ✓ I/O error correctly propagated: {}", e);
            println!("   ✓ Error kind: {:?}", e.kind());
        }
        Err(e) => println!("   ✗ Wrong error type: {:?}", e),
    }
}

#[cfg(feature = "xxhash")]
fn test_checksum_mismatch() {
    // Create a valid message with checksum
    let mut buffer = Vec::new();
    {
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        writer.write(&TestMessage("Valid message".to_string())).unwrap();
    }
    
    // Corrupt the checksum bytes (bytes 4-11)
    println!("   Original checksum bytes: {:02X} {:02X} {:02X} {:02X}...", 
        buffer[4], buffer[5], buffer[6], buffer[7]);
    
    buffer[5] ^= 0xFF; // Flip bits in checksum
    println!("   Corrupted checksum bytes: {:02X} {:02X} {:02X} {:02X}...", 
        buffer[4], buffer[5], buffer[6], buffer[7]);
    
    // Try to read the corrupted message
    let checksum = XxHash64::new();
    let deframer = ChecksumDeframer::new(checksum);
    let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
    
    match reader.read_message() {
        Ok(_) => println!("   ✗ Expected checksum mismatch, but read succeeded"),
        Err(Error::ChecksumMismatch { expected, calculated }) => {
            println!("   ✓ Checksum mismatch detected!");
            println!("   ✓ Expected: 0x{:016X}", expected);
            println!("   ✓ Calculated: 0x{:016X}", calculated);
        }
        Err(e) => println!("   ✗ Wrong error type: {:?}", e),
    }
}

fn test_invalid_frame() {
    // Create a buffer with an invalid length field
    let mut buffer = Vec::new();
    
    // Write an impossibly large length (100MB)
    let huge_length: u32 = 100_000_000;
    buffer.extend_from_slice(&huge_length.to_le_bytes());
    buffer.extend_from_slice(b"some data");
    
    let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
    
    match reader.read_message() {
        Ok(_) => println!("   ✗ Expected invalid frame error, but read succeeded"),
        Err(Error::InvalidFrame { message }) => {
            println!("   ✓ Invalid frame detected: {}", message);
        }
        Err(Error::UnexpectedEof) => {
            println!("   ✓ Detected as unexpected EOF (frame larger than available data)");
        }
        Err(e) => println!("   ✗ Wrong error type: {:?}", e),
    }
}

fn test_unexpected_eof() {
    // Create a partial message (length field but no payload)
    let mut buffer = Vec::new();
    
    // Write length of 100 bytes
    let length: u32 = 100;
    buffer.extend_from_slice(&length.to_le_bytes());
    // But don't write the payload!
    
    let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
    
    match reader.read_message() {
        Ok(_) => println!("   ✗ Expected unexpected EOF, but read succeeded"),
        Err(Error::UnexpectedEof) => {
            println!("   ✓ Unexpected EOF correctly detected");
            println!("   ✓ Stream ended while expecting 100 bytes of payload");
        }
        Err(e) => println!("   ✗ Wrong error type: {:?}", e),
    }
}

fn test_clean_eof() {
    // Create some valid messages
    let mut buffer = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        writer.write(&TestMessage("Message 1".to_string())).unwrap();
        writer.write(&TestMessage("Message 2".to_string())).unwrap();
    }
    
    let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
    
    // Read all messages
    let mut count = 0;
    loop {
        match reader.read_message() {
            Ok(Some(payload)) => {
                count += 1;
                println!("   Read message {}: {} bytes", count, payload.len());
            }
            Ok(None) => {
                println!("   ✓ Clean EOF detected after {} messages", count);
                println!("   ✓ read_message() returned Ok(None) as documented");
                break;
            }
            Err(e) => {
                println!("   ✗ Unexpected error: {:?}", e);
                break;
            }
        }
    }
    
    // Also test with process_all
    let mut reader2 = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
    let mut count2 = 0;
    match reader2.process_all(|_payload| {
        count2 += 1;
        Ok(())
    }) {
        Ok(()) => {
            println!("   ✓ process_all() completed normally on EOF");
            println!("   ✓ Processed {} messages total", count2);
        }
        Err(e) => println!("   ✗ process_all() error: {:?}", e),
    }
} 