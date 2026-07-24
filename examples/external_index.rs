// Example purpose: Build an external "offset -> frame" index using FrameReceipt,
// then random-access individual frames by seeking a reader to a recorded offset.
// This is the pattern a terminal scrollback journal (or any sparse index) needs,
// and it replaces the fragile `bytes += 8 + payload_len` arithmetic a consumer
// would otherwise have to keep in lockstep with the wire layout.
//!
//! Demonstrates `write_with_receipt` / `FrameReceipt` for index construction and
//! offset-addressed reads with today's `Read + Seek` API. A first-class
//! `read_frame_at` is deferred to the v3 range-parser work; this shows the
//! supported pattern in the meantime.

use flatstream::{
    DefaultDeframer, DefaultFramer, FrameReceipt, Result, StreamReader, StreamWriter,
};
use std::io::{Cursor, Seek, SeekFrom};

/// One index entry: the application's record id and where its frame lives.
struct IndexEntry {
    id: u32,
    receipt: FrameReceipt,
}

fn main() -> Result<()> {
    let records = [
        (10u32, "first record"),
        (20, "second record — a bit longer"),
        (30, "third"),
        (40, "fourth record payload"),
    ];

    // --- Write pass: capture a receipt per frame into an external index. ---
    println!(
        "[write] framing {} records, recording an offset index",
        records.len()
    );
    let mut wire: Vec<u8> = Vec::new();
    let mut index: Vec<IndexEntry> = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut wire), DefaultFramer);
        for (id, text) in records {
            // The receipt tells us exactly where this frame landed — no
            // knowledge of the 8-byte header or wire layout in this code.
            let receipt = writer.write_with_receipt(&text)?;
            index.push(IndexEntry { id, receipt });
        }
        writer.flush()?;

        // `bytes_written()` is the running stream offset: it must equal the
        // total bytes produced.
        assert_eq!(writer.bytes_written() as usize, wire.len());
    }

    // The index must tile the stream: each frame begins exactly where the
    // previous one ended, and together they cover every byte.
    let mut expected_start = 0u64;
    for entry in &index {
        assert_eq!(entry.receipt.frame_start, expected_start);
        expected_start += entry.receipt.wire_len;
    }
    assert_eq!(expected_start as usize, wire.len());
    println!(
        "[index] {} entries, contiguous and covering {} bytes",
        index.len(),
        wire.len()
    );

    // --- Random access: fetch frames out of order by seeking to the offset. ---
    // Fetch id 30 (the 3rd record) and id 10 (the 1st), in that order.
    for wanted in [30u32, 10] {
        let entry = index
            .iter()
            .find(|e| e.id == wanted)
            .expect("record present in index");

        let payload = read_frame_at(&wire, entry.receipt.frame_start)?;
        let text = flatbuffers::root::<&str>(&payload).expect("string root");

        let (_, expected) = records
            .iter()
            .copied()
            .find(|(id, _)| *id == wanted)
            .unwrap();
        assert_eq!(text, expected);
        println!(
            "[fetch] id {wanted} @ offset {} -> {text:?}",
            entry.receipt.frame_start
        );
    }

    println!("[ok] external index built from receipts; random access verified");
    Ok(())
}

/// Read a single frame whose first byte is at `offset`, using the current
/// `Read + Seek` API: seek, wrap a fresh reader, pull one message.
///
/// This allocates a reader (and its buffer) per call — the cost the future
/// `read_frame_at` will remove by reusing caller-supplied scratch. Returned as
/// an owned `Vec` so it outlives the transient reader.
fn read_frame_at(wire: &[u8], offset: u64) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(wire);
    cursor.seek(SeekFrom::Start(offset))?;
    let mut reader = StreamReader::new(cursor, DefaultDeframer::new());
    let payload = reader
        .read_message()?
        .expect("a frame begins at the indexed offset");
    Ok(payload.to_vec())
}
