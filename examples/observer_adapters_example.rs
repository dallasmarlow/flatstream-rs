//! Demonstrates zero-copy observers on both write and read paths.
//! Includes optional checksum variants behind feature flags.

use flatstream::framing::{ObserverDeframer, ObserverFramer};
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
use flatstream::Framer; // bring trait into scope where checksum branches call frame_and_write
use flatstream::{DefaultDeframer, DefaultFramer, Result, StreamReader, StreamWriter};
use std::cell::Cell;
use std::io::Cursor;

fn main() -> Result<()> {
    // Observe payloads as they are written
    let write_seen = Cell::new(0usize);
    let framer = ObserverFramer::new(DefaultFramer, |p: &[u8]| {
        // The slice `p` is borrowed from the caller; observers never copy data
        write_seen.set(write_seen.get() + p.len());
    });

    let mut bytes = Vec::new();
    let writer = Cursor::new(&mut bytes);
    let mut stream_writer = StreamWriter::new(writer, framer);
    println!(
        "[writer] Writing two messages with an observer framer to record total payload bytes"
    );
    stream_writer.write(&"alpha")?;
    stream_writer.write(&"beta")?;
    stream_writer.flush()?;
    println!(
        "[writer] ObserverFramer observed a cumulative payload byte count of {}",
        write_seen.get()
    );

    // Observe payloads as they are read
    let read_msgs = Cell::new(0usize);
    let deframer = ObserverDeframer::new(DefaultDeframer, |_p: &[u8]| {
        read_msgs.set(read_msgs.get() + 1);
    });
    let mut stream_reader = StreamReader::new(Cursor::new(&bytes), deframer);
    println!(
        "[reader] Reading messages with an observer deframer to count messages without copying"
    );
    stream_reader.process_all(|_payload| Ok(()))?;
    println!(
        "[reader] ObserverDeframer observed a total of {} message(s)",
        read_msgs.get()
    );

    // Feature-gated checksum observer variants
    #[cfg(feature = "xxhash")]
    {
        use flatstream::framing::{ChecksumDeframer, ChecksumFramer};
        use flatstream::XxHash64;

        let seen = Cell::new(0usize);
        let framer = ObserverFramer::new(ChecksumFramer::new(XxHash64::new()), |p: &[u8]| {
            seen.set(seen.get() + p.len());
        });
        let mut framed_bytes = Vec::new();
        println!(
            "[xxhash] Writing a checksum-protected payload while observing the outgoing slice"
        );
        framer
            .frame_and_write(&mut framed_bytes, b"xxhash payload")
            .unwrap();

        let count = Cell::new(0usize);
        let deframer = ObserverDeframer::new(ChecksumDeframer::new(XxHash64::new()), |_p| {
            count.set(count.get() + 1);
        });
        let mut stream_reader = StreamReader::new(Cursor::new(framed_bytes), deframer);
        println!(
            "[xxhash] Reading back with a checksum-deframer while observing received slices"
        );
        stream_reader.process_all(|_| Ok(()))?;
        println!(
            "[xxhash] Observer summary: observed_bytes={}, observed_messages={}",
            seen.get(),
            count.get()
        );
    }

    #[cfg(feature = "crc32")]
    {
        use flatstream::framing::{ChecksumDeframer, ChecksumFramer};
        use flatstream::Crc32;

        let seen = Cell::new(0usize);
        let framer = ObserverFramer::new(ChecksumFramer::new(Crc32::new()), |p: &[u8]| {
            seen.set(seen.get() + p.len());
        });
        let mut framed_bytes = Vec::new();
        println!(
            "[crc32] Writing a checksum-protected payload while observing the outgoing slice"
        );
        framer
            .frame_and_write(&mut framed_bytes, b"crc32 payload")
            .unwrap();

        let count = Cell::new(0usize);
        let deframer = ObserverDeframer::new(ChecksumDeframer::new(Crc32::new()), |_p| {
            count.set(count.get() + 1);
        });
        let mut stream_reader = StreamReader::new(Cursor::new(framed_bytes), deframer);
        println!(
            "[crc32] Reading back with a checksum-deframer while observing received slices"
        );
        stream_reader.process_all(|_| Ok(()))?;
        println!(
            "[crc32] Observer summary: observed_bytes={}, observed_messages={}",
            seen.get(),
            count.get()
        );
    }

    #[cfg(feature = "crc16")]
    {
        use flatstream::framing::{ChecksumDeframer, ChecksumFramer};
        use flatstream::Crc16;

        let seen = Cell::new(0usize);
        let framer = ObserverFramer::new(ChecksumFramer::new(Crc16::new()), |p: &[u8]| {
            seen.set(seen.get() + p.len());
        });
        let mut framed_bytes = Vec::new();
        println!(
            "[crc16] Writing a checksum-protected payload while observing the outgoing slice"
        );
        framer
            .frame_and_write(&mut framed_bytes, b"crc16 payload")
            .unwrap();

        let count = Cell::new(0usize);
        let deframer = ObserverDeframer::new(ChecksumDeframer::new(Crc16::new()), |_p| {
            count.set(count.get() + 1);
        });
        let mut stream_reader = StreamReader::new(Cursor::new(framed_bytes), deframer);
        println!(
            "[crc16] Reading back with a checksum-deframer while observing received slices"
        );
        stream_reader.process_all(|_| Ok(()))?;
        println!(
            "[crc16] Observer summary: observed_bytes={}, observed_messages={}",
            seen.get(),
            count.get()
        );
    }

    Ok(())
}


