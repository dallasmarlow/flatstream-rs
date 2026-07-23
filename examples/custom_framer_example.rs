use flatstream::{Deframer, Error, Framer, Result, StreamReader, StreamWriter};
use std::io::{Cursor, Read, Write};

/// A custom deframer must pick its own allocation bound: the core deframers
/// default to the wire format's ~4 GiB ceiling, and this one bypasses them
/// entirely. 16 MiB is a sensible ceiling for this example's payloads.
const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// A custom framer that adds a 2-byte magic number `0xABBA` before each message.
/// Wire format: [2-byte magic | 4-byte length | payload]
///
/// Example purpose: Sketch how to define non-standard headers and how a matching
/// deframer should reject mismatches with an InvalidFrame error.
struct MagicHeaderFramer;

impl Framer for MagicHeaderFramer {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        // The length field is u32 on the wire: reject anything that would
        // truncate rather than silently casting (reference behavior for
        // custom framers).
        let payload_len = u32::try_from(payload.len()).map_err(|_| {
            Error::invalid_frame_with(
                "payload exceeds u32 length field",
                Some(payload.len()),
                None,
                Some(u32::MAX as usize),
            )
        })?;

        // Write the magic number, then the length and payload
        writer.write_all(&[0xAB, 0xBA])?;
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
    ) -> Result<Option<usize>> {
        // The magic precedes the standard header, so this deframer overrides
        // the provided implementation: consume and verify the magic and length
        // as one merged header (like the core's `[len | checksum]` read), then
        // delegate the payload read.
        //
        // The 1-byte probe mirrors the core deframers: a 1-byte request cannot
        // be torn, so UnexpectedEof here means zero bytes at a frame boundary —
        // clean EOF. EOF anywhere later inside the header is a torn frame, and
        // non-EOF I/O errors propagate with their kind intact.
        let mut header = [0u8; 6]; // [magic: 2 | length: 4]
        match reader.read_exact(&mut header[..1]) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        reader
            .read_exact(&mut header[1..])
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::UnexpectedEof => Error::unexpected_eof(),
                _ => e.into(),
            })?;

        // Verify the magic number
        if header[..2] != [0xAB, 0xBA] {
            return Err(Error::invalid_frame("Invalid magic number"));
        }

        let payload_len = u32::from_le_bytes(header[2..6].try_into().unwrap()) as usize;
        self.read_after_length(reader, buffer, payload_len)
    }

    fn read_after_length<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
        payload_len: usize,
    ) -> Result<Option<usize>> {
        // Bound before allocating: a custom deframer bypasses the core
        // deframers' built-in check entirely, so it must enforce its own limit
        // — never size an allocation from an unvalidated length field.
        if payload_len > MAX_FRAME_LEN {
            return Err(Error::invalid_frame_with(
                "frame length exceeds configured limit",
                Some(payload_len),
                None,
                Some(MAX_FRAME_LEN),
            ));
        }
        // High-water-mark buffer: grow (zeroing only the growth) and read the
        // payload into the front; the reader yields `buffer[..payload_len]`.
        if payload_len > buffer.len() {
            buffer.resize(payload_len, 0);
        }
        reader
            .read_exact(&mut buffer[..payload_len])
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::UnexpectedEof => Error::unexpected_eof(),
                _ => e.into(),
            })?;
        Ok(Some(payload_len))
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
    let mut messages = Vec::new();
    reader.process_all(|payload| {
        // The payload is a FlatBuffer, not raw text — decode the string root
        // instead of printing the serialized bytes.
        let message = flatbuffers::root::<&str>(payload).map_err(Error::from)?;
        messages.push(message.to_string());
        Ok(())
    })?;
    assert_eq!(messages, ["message with magic header"]);
    println!("Successfully read back: {messages:?}");

    println!("\nCustom framer worked correctly!");
    Ok(())
}
