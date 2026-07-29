use flatbuffers::FlatBufferBuilder;
use flatstream::{
    read_frame_at, DefaultDeframer, DefaultFramer, ErrorKind, FrameReceipt, StreamReader,
    StreamWriter,
};
use std::io::{self, BufReader, Cursor, Read, Seek, SeekFrom, Write};

fn finish(builder: &mut FlatBufferBuilder, value: &str) -> Vec<u8> {
    builder.reset();
    let value = builder.create_string(value);
    builder.finish(value, None);
    builder.finished_data().to_vec()
}

fn default_stream(values: &[&str]) -> (Vec<u8>, Vec<Vec<u8>>, Vec<FrameReceipt>) {
    let mut wire = Vec::new();
    let mut payloads = Vec::new();
    let mut receipts = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut wire), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        for value in values {
            payloads.push(finish(&mut builder, value));
            receipts.push(writer.write_finished_with_receipt(&mut builder).unwrap());
        }
    }
    (wire, payloads, receipts)
}

#[test]
fn forward_receipts_tile_the_stream_from_a_start_offset() {
    let (wire, expected, original) = default_stream(&["first", "second", "third"]);
    let base = 10_000;
    let mut reader =
        StreamReader::new(Cursor::new(&wire), DefaultDeframer::new()).with_start_offset(base);

    let mut seen = Vec::new();
    reader
        .process_all_with_receipt(|frame| {
            seen.push((frame.payload.to_vec(), frame.receipt));
            Ok(())
        })
        .unwrap();

    assert_eq!(seen.len(), expected.len());
    for i in 0..seen.len() {
        assert_eq!(seen[i].0, expected[i]);
        assert_eq!(seen[i].1.frame_start, base + original[i].frame_start);
        assert_eq!(seen[i].1.wire_len, original[i].wire_len);
        if i > 0 {
            assert_eq!(seen[i - 1].1.end(), seen[i].1.frame_start);
        }
    }
    assert_eq!(reader.bytes_consumed(), base + wire.len() as u64);
}

#[test]
fn read_frame_at_reuses_scratch_and_returns_exact_bounds() {
    let (wire, expected, receipts) = default_stream(&["zero", "one", "two", "three"]);
    let mut source = Cursor::new(&wire);
    let mut scratch = Vec::new();

    // Warm the caller-owned scratch, then prove later point reads reuse it.
    let last = read_frame_at(
        &mut source,
        &DefaultDeframer::new(),
        receipts[3].frame_start,
        &mut scratch,
    )
    .unwrap()
    .unwrap();
    assert_eq!(last.payload, expected[3]);
    assert_eq!(last.receipt, receipts[3]);
    let capacity = scratch.capacity();

    for i in [1usize, 0, 2] {
        let frame = read_frame_at(
            &mut source,
            &DefaultDeframer::new(),
            receipts[i].frame_start,
            &mut scratch,
        )
        .unwrap()
        .unwrap();
        assert_eq!(frame.payload, expected[i]);
        assert_eq!(frame.receipt, receipts[i]);
        assert_eq!(source.position(), receipts[i].end());
        assert_eq!(scratch.capacity(), capacity);
    }

    assert!(read_frame_at(
        &mut source,
        &DefaultDeframer::new(),
        wire.len() as u64,
        &mut scratch,
    )
    .unwrap()
    .is_none());
}

struct SeekCountingCursor<T> {
    inner: Cursor<T>,
    seeks: usize,
}

impl<T: AsRef<[u8]>> Read for SeekCountingCursor<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<T: AsRef<[u8]>> Seek for SeekCountingCursor<T> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.seeks += 1;
        self.inner.seek(position)
    }
}

#[test]
fn read_frame_at_seeks_once_and_counts_frame_reads() {
    let (wire, expected, receipts) = default_stream(&["zero", "target", "two"]);
    let mut source = SeekCountingCursor {
        inner: Cursor::new(&wire),
        seeks: 0,
    };
    let mut scratch = Vec::new();

    let frame = read_frame_at(
        &mut source,
        &DefaultDeframer::new(),
        receipts[1].frame_start,
        &mut scratch,
    )
    .unwrap()
    .unwrap();

    assert_eq!(frame.payload, expected[1]);
    assert_eq!(frame.receipt, receipts[1]);
    assert_eq!(
        source.seeks, 1,
        "point lookup seeks to the frame once; receipt length comes from read accounting"
    );
    assert_eq!(source.inner.position(), receipts[1].end());
}

#[test]
fn an_incomplete_live_file_can_be_retried_from_the_same_offset() {
    let (complete, expected, receipts) = default_stream(&["eventually complete"]);
    let split = complete.len() - 3;
    let mut source = Cursor::new(complete[..split].to_vec());
    let mut scratch = Vec::new();

    let error = read_frame_at(
        &mut source,
        &DefaultDeframer::new(),
        receipts[0].frame_start,
        &mut scratch,
    )
    .unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::UnexpectedEof));

    source.get_mut().extend_from_slice(&complete[split..]);
    let frame = read_frame_at(
        &mut source,
        &DefaultDeframer::new(),
        receipts[0].frame_start,
        &mut scratch,
    )
    .unwrap()
    .unwrap();
    assert_eq!(frame.payload, expected[0]);
    assert_eq!(frame.receipt, receipts[0]);
}

#[cfg(feature = "crc32")]
#[test]
fn checksum_point_lookup_includes_the_checksum_width() {
    use flatstream::{ChecksumDeframer, ChecksumFramer, Crc32};

    let mut wire = Vec::new();
    let mut builder = FlatBufferBuilder::new();
    let expected = finish(&mut builder, "checked");
    let receipt = {
        let mut writer =
            StreamWriter::new(Cursor::new(&mut wire), ChecksumFramer::new(Crc32::new()));
        writer.write_finished_with_receipt(&mut builder).unwrap()
    };

    let mut source = Cursor::new(&wire);
    let mut scratch = Vec::new();
    let frame = read_frame_at(
        &mut source,
        &ChecksumDeframer::new(Crc32::new()),
        receipt.frame_start,
        &mut scratch,
    )
    .unwrap()
    .unwrap();
    assert_eq!(frame.payload, expected);
    assert_eq!(frame.receipt, receipt);
}

struct OneByte<R>(R);

impl<R: Read> Read for OneByte<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let limit = buf.len().min(1);
        self.0.read(&mut buf[..limit])
    }
}

impl<R: Seek> Seek for OneByte<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.seek(position)
    }
}

#[test]
fn forward_position_is_exact_under_one_byte_reads() {
    let (wire, expected, receipts) = default_stream(&["short reads"]);
    let mut reader = StreamReader::new(OneByte(Cursor::new(&wire)), DefaultDeframer::new());
    let frame = reader.read_message_with_receipt().unwrap().unwrap();
    assert_eq!(frame.payload, expected[0]);
    assert_eq!(frame.receipt, receipts[0]);
    assert_eq!(reader.bytes_consumed(), wire.len() as u64);
}

#[test]
fn point_read_receipt_is_exact_under_one_byte_reads() {
    let (wire, expected, receipts) = default_stream(&["zero", "one-byte target", "two"]);
    let mut source = OneByte(Cursor::new(&wire));
    let mut scratch = Vec::new();

    let frame = read_frame_at(
        &mut source,
        &DefaultDeframer::new(),
        receipts[1].frame_start,
        &mut scratch,
    )
    .unwrap()
    .unwrap();

    assert_eq!(frame.payload, expected[1]);
    assert_eq!(frame.receipt, receipts[1]);
}

#[test]
fn point_read_receipt_is_exact_through_retained_bufreader() {
    let (wire, expected, receipts) = default_stream(&["zero", "buffered target", "two"]);
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(&wire).unwrap();
    let mut source = BufReader::new(file);
    let mut scratch = Vec::new();

    let frame = read_frame_at(
        &mut source,
        &DefaultDeframer::new(),
        receipts[1].frame_start,
        &mut scratch,
    )
    .unwrap()
    .unwrap();

    assert_eq!(frame.payload, expected[1]);
    assert_eq!(frame.receipt, receipts[1]);
}
