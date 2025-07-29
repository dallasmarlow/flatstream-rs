//! Defines the framing and deframing strategies for the byte stream.

use crate::checksum::Checksum;
use crate::error::{Error, Result};
use std::io::{Read, Write};

//--- Framer Trait and Implementations ---

/// A trait that defines how a raw payload is framed and written to a stream.
pub trait Framer {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()>;
}

/// The default framing strategy: `[4-byte length | payload]`
pub struct DefaultFramer;

impl Framer for DefaultFramer {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        let payload_len = payload.len() as u32;
        writer.write_all(&payload_len.to_le_bytes())?;
        writer.write_all(payload)?;
        Ok(())
    }
}

/// A framing strategy that includes a checksum: `[4-byte length | 8-byte checksum | payload]`
pub struct ChecksumFramer<C: Checksum> {
    checksum_alg: C,
}

impl<C: Checksum> ChecksumFramer<C> {
    pub fn new(checksum_alg: C) -> Self {
        Self { checksum_alg }
    }
}

impl<C: Checksum> Framer for ChecksumFramer<C> {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        let payload_len = payload.len() as u32;
        let checksum = self.checksum_alg.calculate(payload);
        writer.write_all(&payload_len.to_le_bytes())?;
        writer.write_all(&checksum.to_le_bytes())?;
        writer.write_all(payload)?;
        Ok(())
    }
}

//--- Deframer Trait and Implementations ---

/// A trait that defines how a message is deframed and read from a stream.
pub trait Deframer {
    /// Returns Ok(Some(())) on success, Ok(None) on clean EOF.
    fn read_and_deframe<R: Read>(&self, reader: &mut R, buffer: &mut Vec<u8>)
        -> Result<Option<()>>;
}

/// The default deframing strategy.
pub struct DefaultDeframer;

impl Deframer for DefaultDeframer {
    fn read_and_deframe<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>> {
        let mut len_bytes = [0u8; 4];
        match reader.read_exact(&mut len_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None), // Clean EOF
            Err(e) => return Err(e.into()),
        }

        let payload_len = u32::from_le_bytes(len_bytes) as usize;
        buffer.resize(payload_len, 0);
        reader
            .read_exact(buffer)
            .map_err(|_| Error::UnexpectedEof)?;

        Ok(Some(()))
    }
}

/// A deframing strategy that verifies a checksum.
pub struct ChecksumDeframer<C: Checksum> {
    checksum_alg: C,
}

impl<C: Checksum> ChecksumDeframer<C> {
    pub fn new(checksum_alg: C) -> Self {
        Self { checksum_alg }
    }
}

impl<C: Checksum> Deframer for ChecksumDeframer<C> {
    fn read_and_deframe<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>> {
        // This directly addresses Lesson 3: Distinguish between clean and unexpected EOF.
        let mut len_bytes = [0u8; 4];
        match reader.read_exact(&mut len_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let payload_len = u32::from_le_bytes(len_bytes) as usize;

        let mut checksum_bytes = [0u8; 8];
        reader
            .read_exact(&mut checksum_bytes)
            .map_err(|_| Error::UnexpectedEof)?;
        let expected_checksum = u64::from_le_bytes(checksum_bytes);

        buffer.resize(payload_len, 0);
        reader
            .read_exact(buffer)
            .map_err(|_| Error::UnexpectedEof)?;

        self.checksum_alg.verify(expected_checksum, buffer)?;

        Ok(Some(()))
    }
}
