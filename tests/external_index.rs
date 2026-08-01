//! B1 — The external "offset → frame" index recipe, as an executable contract.
//!
//! This is the pattern every index-building consumer needs, and the reason
//! `FrameReceipt` exists (`docs/DESIGN_v2_8.md` §2): record where each frame
//! landed at write time, then seek straight to it later, without ever
//! reimplementing `4 + checksum_width + payload_len` wire arithmetic in
//! application code.
//!
//! `examples/external_index.rs` demonstrates the pattern on an in-memory
//! `Cursor`. These tests pin the properties that make it *safe* — on real
//! files, with checksums, across an appending reopen, and after a crash —
//! because those are the cases where getting the arithmetic wrong silently
//! yields an index that points into the middle of frames.

use flatbuffers::FlatBufferBuilder;
use flatstream::{
    recover_file, DefaultDeframer, DefaultFramer, FrameReceipt, RecoveryEnd, StreamReader,
    StreamWriter,
};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom};
use tempfile::NamedTempFile;

#[cfg(feature = "crc32")]
use flatstream::{ChecksumDeframer, ChecksumFramer, Crc32};

/// Serializes `s` as a string root and returns the exact payload bytes that
/// must appear on the wire.
fn finish(builder: &mut FlatBufferBuilder, s: &str) -> Vec<u8> {
    builder.reset();
    let off = builder.create_string(s);
    builder.finish(off, None);
    builder.finished_data().to_vec()
}

fn records() -> Vec<String> {
    // Deliberately varied lengths: a constant-size record would let a wrong
    // stride pass every contiguity check.
    (0..64)
        .map(|i| format!("record {i} {}", "x".repeat(i % 23)))
        .collect()
}

/// Reads exactly one frame whose first byte is at `offset`.
fn read_frame_at_path<D: flatstream::RetrySafeDeframer>(
    path: &std::path::Path,
    offset: u64,
    deframer: D,
) -> Vec<u8> {
    let mut file = File::open(path).unwrap();
    let mut scratch = Vec::new();
    flatstream::read_frame_at(&mut file, &deframer, offset, &mut scratch)
        .unwrap()
        .expect("a frame begins at an indexed offset")
        .payload
        .to_vec()
}

#[test]
fn index_tiles_the_stream_with_no_gaps_or_overlaps() {
    // The load-bearing property. If receipts ever disagreed with the bytes
    // actually emitted, the index would still *look* plausible — every entry
    // would have an offset and a length — but would drift, and the drift would
    // only surface as a corrupt read much later. So: assert that consecutive
    // frames abut exactly, that the entries cover every byte of the file with
    // none left over, and that `bytes_written()` agrees with the file size.
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path().to_path_buf();
    let payloads = records();

    let mut index: Vec<FrameReceipt> = Vec::new();
    let mut expected_payloads: Vec<Vec<u8>> = Vec::new();
    {
        let file = File::create(&path).unwrap();
        let mut writer = StreamWriter::new(BufWriter::new(file), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        for r in &payloads {
            expected_payloads.push(finish(&mut builder, r));
            index.push(writer.write_finished_with_receipt(&mut builder).unwrap());
        }
        writer.flush().unwrap();
        assert_eq!(
            writer.bytes_written(),
            index.last().unwrap().frame_start + index.last().unwrap().wire_len,
            "bytes_written must equal the end of the last frame"
        );
    }

    let file_len = std::fs::metadata(&path).unwrap().len();

    // Contiguity: frame n+1 starts exactly where frame n ended.
    let mut cursor = 0u64;
    for (i, receipt) in index.iter().enumerate() {
        assert_eq!(
            receipt.frame_start, cursor,
            "gap or overlap before frame {i}"
        );
        assert_eq!(
            receipt.wire_len,
            (4 + expected_payloads[i].len()) as u64,
            "wire_len must be the 4-byte prefix plus the payload for frame {i}"
        );
        cursor += receipt.wire_len;
    }
    // Coverage: the tiling accounts for the whole file, no trailing slack.
    assert_eq!(
        cursor, file_len,
        "index must cover every byte of the stream"
    );
}

#[test]
fn seeking_to_an_indexed_offset_returns_the_exact_payload() {
    // Random access, deliberately out of order, and deliberately including the
    // first and last frames — an off-by-one in the index shows up at the edges
    // first.
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path().to_path_buf();
    let payloads = records();

    let mut index = Vec::new();
    let mut expected = Vec::new();
    {
        let file = File::create(&path).unwrap();
        let mut writer = StreamWriter::new(BufWriter::new(file), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        for r in &payloads {
            expected.push(finish(&mut builder, r));
            index.push(writer.write_finished_with_receipt(&mut builder).unwrap());
        }
        writer.flush().unwrap();
    }

    for i in [index.len() - 1, 0, 7, 42, 1, index.len() - 2] {
        let got = read_frame_at_path(&path, index[i].frame_start, DefaultDeframer::new());
        assert_eq!(
            got, expected[i],
            "frame {i} did not round-trip via its offset"
        );
        // And the payload is a real FlatBuffer root, not just matching bytes.
        assert_eq!(flatbuffers::root::<&str>(&got).unwrap(), payloads[i]);
    }
}

#[test]
fn a_receipt_names_exactly_the_bytes_of_its_own_frame() {
    // Stronger than contiguity: slice the raw file at [frame_start,
    // frame_start + wire_len) and assert those bytes *are* the frame — the
    // declared length prefix plus the payload and nothing else. This is what
    // lets a consumer copy a frame out of a journal without parsing it.
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path().to_path_buf();
    let payloads = records();

    let mut index = Vec::new();
    let mut expected = Vec::new();
    {
        let file = File::create(&path).unwrap();
        let mut writer = StreamWriter::new(BufWriter::new(file), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        for r in &payloads {
            expected.push(finish(&mut builder, r));
            index.push(writer.write_finished_with_receipt(&mut builder).unwrap());
        }
        writer.flush().unwrap();
    }

    let mut raw = Vec::new();
    File::open(&path).unwrap().read_to_end(&mut raw).unwrap();

    for (i, receipt) in index.iter().enumerate() {
        let start = receipt.frame_start as usize;
        let end = start + receipt.wire_len as usize;
        let frame = &raw[start..end];
        let declared = u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize;
        assert_eq!(declared, expected[i].len());
        assert_eq!(&frame[4..], &expected[i][..]);
    }
}

#[cfg(feature = "crc32")]
#[test]
fn wire_len_accounts_for_the_checksum_field() {
    // A consumer that hardcoded `4 + payload_len` would be wrong by exactly
    // `Crc32::SIZE` here, and would still see a "working" index for the first
    // frame only. The receipt has to track the framer's real output.
    use flatstream::checksum::Checksum;

    let temp = NamedTempFile::new().unwrap();
    let path = temp.path().to_path_buf();
    let payloads = records();

    let mut index = Vec::new();
    let mut expected = Vec::new();
    {
        let file = File::create(&path).unwrap();
        let mut writer = StreamWriter::new(BufWriter::new(file), ChecksumFramer::new(Crc32::new()));
        let mut builder = FlatBufferBuilder::new();
        for r in &payloads {
            expected.push(finish(&mut builder, r));
            index.push(writer.write_finished_with_receipt(&mut builder).unwrap());
        }
        writer.flush().unwrap();
    }

    let mut cursor = 0u64;
    for (i, receipt) in index.iter().enumerate() {
        assert_eq!(receipt.frame_start, cursor);
        assert_eq!(
            receipt.wire_len,
            (4 + Crc32::SIZE + expected[i].len()) as u64
        );
        cursor += receipt.wire_len;
    }
    assert_eq!(cursor, std::fs::metadata(&path).unwrap().len());

    // Random access still verifies the checksum on the way out.
    let got = read_frame_at_path(
        &path,
        index[9].frame_start,
        ChecksumDeframer::new(Crc32::new()),
    );
    assert_eq!(got, expected[9]);
}

#[test]
fn with_start_offset_yields_absolute_offsets_when_appending() {
    // The reopen case: a journal already holds frames, recovery leaves the
    // cursor at the append point, and a fresh writer is wrapped around it.
    // Without `with_start_offset` the new writer's receipts would restart at
    // zero and silently collide with the existing index.
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path().to_path_buf();

    let mut index = Vec::new();
    let mut expected = Vec::new();
    let mut builder = FlatBufferBuilder::new();

    // --- session 1 -------------------------------------------------------
    {
        let file = File::create(&path).unwrap();
        let mut writer = StreamWriter::new(BufWriter::new(file), DefaultFramer);
        for r in ["alpha", "beta", "gamma"] {
            expected.push(finish(&mut builder, r));
            index.push(writer.write_finished_with_receipt(&mut builder).unwrap());
        }
        writer.flush().unwrap();
    }
    let after_session_1 = std::fs::metadata(&path).unwrap().len();

    // --- session 2: reopen, recover, append ------------------------------
    {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let report = recover_file(&mut file, DefaultDeframer::new()).unwrap();
        assert_eq!(report.end, RecoveryEnd::CleanEof);
        assert_eq!(report.last_good_offset, after_session_1);
        // `recover_file` leaves the cursor at the append point; tell the
        // writer where that is so its receipts stay absolute.
        let mut writer = StreamWriter::new(BufWriter::new(file), DefaultFramer)
            .with_start_offset(report.last_good_offset)
            .unwrap();
        assert_eq!(writer.bytes_written(), after_session_1);
        for r in ["delta", "epsilon"] {
            expected.push(finish(&mut builder, r));
            index.push(writer.write_finished_with_receipt(&mut builder).unwrap());
        }
        writer.flush().unwrap();
    }

    // One index, two sessions, still contiguous and still absolute.
    let mut cursor = 0u64;
    for (i, receipt) in index.iter().enumerate() {
        assert_eq!(receipt.frame_start, cursor, "entry {i}");
        cursor += receipt.wire_len;
    }
    assert_eq!(cursor, std::fs::metadata(&path).unwrap().len());
    assert_eq!(index[3].frame_start, after_session_1);

    // Every entry — from both sessions — still random-accesses correctly.
    for (i, receipt) in index.iter().enumerate() {
        assert_eq!(
            read_frame_at_path(&path, receipt.frame_start, DefaultDeframer::new()),
            expected[i]
        );
    }
}

#[test]
fn index_entries_below_the_recovery_point_survive_a_torn_tail() {
    // The crash case a journal must survive. After a torn append, every index
    // entry that ends at or before `last_good_offset` must still resolve; the
    // entry describing the torn frame must not. This is what makes it safe for
    // an application to persist its index lazily.
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path().to_path_buf();

    let mut index = Vec::new();
    let mut expected = Vec::new();
    {
        let file = File::create(&path).unwrap();
        let mut writer = StreamWriter::new(BufWriter::new(file), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        for r in &records() {
            expected.push(finish(&mut builder, r));
            index.push(writer.write_finished_with_receipt(&mut builder).unwrap());
        }
        writer.flush().unwrap();
    }

    // Simulate a crash midway through the final frame.
    let full_len = std::fs::metadata(&path).unwrap().len();
    let last = index.last().unwrap();
    let torn_len = last.frame_start + last.wire_len / 2;
    assert!(torn_len < full_len);
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(torn_len)
        .unwrap();

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let report = recover_file(&mut file, DefaultDeframer::new()).unwrap();
    assert_eq!(report.end, RecoveryEnd::TornTail);
    // Recovery's truncation point must be exactly the last intact frame's end,
    // which the index independently predicted.
    assert_eq!(report.last_good_offset, last.frame_start);
    assert_eq!(report.frames as usize, index.len() - 1);
    drop(file);

    // Every entry the index recorded below the recovery point still resolves.
    for (i, receipt) in index.iter().enumerate() {
        if receipt.frame_start + receipt.wire_len <= report.last_good_offset {
            assert_eq!(
                read_frame_at_path(&path, receipt.frame_start, DefaultDeframer::new()),
                expected[i],
                "entry {i} should have survived the torn tail"
            );
        }
    }

    // And the torn frame's entry is correctly identified as unusable: a reader
    // seeked there hits EOF inside the frame rather than returning garbage.
    let mut f = File::open(&path).unwrap();
    f.seek(SeekFrom::Start(last.frame_start)).unwrap();
    let mut reader = StreamReader::new(f, DefaultDeframer::new());
    assert!(
        reader.read_message().is_err(),
        "the torn frame must not read back as a valid message"
    );
}
