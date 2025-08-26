use flatstream::{Deframer, Error, Framer, Result, StreamReader, StreamWriter};
use std::io::{Cursor, Read, Write};

/// A custom framer that adds a 2-byte magic number `0xABBA` before each message.
/// Wire format: [2-byte magic | 4-byte length | payload]
///
/// Example purpose: Sketch how to define non-standard headers and how a matching
/// deframer should reject mismatches with an InvalidFrame error.
struct MagicHeaderFramer;

impl Framer for MagicHeaderFramer {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        // Write the magic number
        writer.write_all(&[0xAB, 0xBA])?;

        // Write the length and payload
        let payload_len = payload.len() as u32;
        writer.write_all(&payload_len.to_le_bytes())?;
        writer.write_all(payload)?;
        Ok(())
    }
}

/// A custom deframer that reads and verifies the 2-byte magic number.
struct MagicHeaderDeframer;

impl Deframer for MagicHeaderDeframer {
    fn read_and_deframe<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>> {
        let mut magic_bytes = [0u8; 2];
        match reader.read_exact(&mut magic_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None), // Clean EOF
            Err(e) => return Err(e.into()),
        }

        // Verify the magic number
        if magic_bytes != [0xAB, 0xBA] {
            return Err(Error::invalid_frame_with("Invalid magic number", None, None, None));
        }

        // Read length and payload (similar to DefaultDeframer)
        let mut len_bytes = [0u8; 4];
        reader
            .read_exact(&mut len_bytes)
            .map_err(|_| Error::UnexpectedEof)?;

        let payload_len = u32::from_le_bytes(len_bytes) as usize;
        buffer.resize(payload_len, 0);
        reader
            .read_exact(buffer)
            .map_err(|_| Error::UnexpectedEof)?;

        Ok(Some(()))
    }

    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<()>> {
        // MagicHeaderDeframer expects to consume the magic number before length,
        // so this fast-path is only usable when the caller has already ensured the
        // magic header is present and consumed. In our library, bounded deframer
        // only pre-reads the 4-byte length for standard formats, so we keep the
        // simple payload read here.
        buffer.resize(payload_len, 0);
        reader
            .read_exact(buffer)
            .map_err(|_| Error::UnexpectedEof)?;
        Ok(Some(()))
    }
}

fn main() -> Result<()> {
    println!("=== Custom Framer Example ===\n");
    let mut buffer = Vec::new();

    // 1. Write messages using the custom MagicHeaderFramer
    println!("Writing with MagicHeaderFramer...");
    let mut writer = StreamWriter::new(Cursor::new(&mut buffer), MagicHeaderFramer);
    writer.write(&"message with magic header")?;
    println!("Write successful.\n");

    // 2. Read messages using the custom MagicHeaderDeframer
    println!("Reading with MagicHeaderDeframer...");
    let mut reader = StreamReader::new(Cursor::new(&buffer), MagicHeaderDeframer);
    reader.process_all(|payload| {
        println!(
            "Successfully read: '{}'",
            std::str::from_utf8(payload).unwrap()
        );
        Ok(())
    })?;

    println!("\nCustom framer worked correctly!");
    Ok(())
}
