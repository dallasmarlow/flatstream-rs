use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::io::Cursor;

struct StrRoot;

impl<'a> StreamDeserialize<'a> for StrRoot {
    type Root = &'a str;

    fn from_payload(payload: &'a [u8]) -> Result<Self::Root> {
        flatbuffers::root::<&'a str>(payload).map_err(Error::FlatbuffersError)
    }
}

fn build_string_messages(count: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let framer = DefaultFramer;
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), framer);
    let mut builder = FlatBufferBuilder::new();
    for i in 0..count {
        builder.reset();
        let s = builder.create_string(&format!("message {i}"));
        builder.finish(s, None);
        writer.write_finished(&mut builder).unwrap();
    }
    buf
}

#[test]
fn typed_read_default_deframer() {
    // Purpose: Typed processing with DefaultDeframer yields expected roots and counts.
    let data = build_string_messages(3);
    let mut reader = StreamReader::new(Cursor::new(&data), DefaultDeframer);
    let mut count = 0;
    reader
        .process_typed::<StrRoot, _>(|root| {
            assert!(root.starts_with("message "));
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 3);
}

#[test]
fn typed_messages_iterator_default() {
    // Purpose: The typed iterator yields the same number of roots with expected content.
    let data = build_string_messages(3);
    let mut reader = StreamReader::new(Cursor::new(&data), DefaultDeframer);
    let mut it = reader.typed_messages::<StrRoot>();
    let mut count = 0;
    while let Some(root) = it.next().unwrap() {
        assert!(root.starts_with("message "));
        count += 1;
    }
    assert_eq!(count, 3);
}

#[test]
fn process_typed_with_payload_passes_both() {
    // Purpose: process_typed_with_payload passes both the typed root and the raw payload slice.
    let data = build_string_messages(1);
    let mut reader = StreamReader::new(Cursor::new(&data), DefaultDeframer);
    let mut saw = false;
    reader
        .process_typed_with_payload::<StrRoot, _>(|root, payload| {
            assert_eq!(root, flatbuffers::root::<&str>(payload).unwrap());
            saw = true;
            Ok(())
        })
        .unwrap();
    assert!(saw);
}

#[cfg(feature = "unsafe_typed")]
#[test]
fn process_typed_unchecked_skips_verification() {
    // Purpose: Unchecked typed processing skips verification but still iterates the same count.
    let data = build_string_messages(2);
    let mut reader = StreamReader::new(Cursor::new(&data), DefaultDeframer);
    let mut count = 0;
    reader
        .process_typed_unchecked::<StrRoot, _>(|_root| {
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn typed_read_safe_take_deframer() {
    // Purpose: Typed reading works equivalently with SafeTakeDeframer.
    let data = build_string_messages(3);
    let mut reader = StreamReader::new(Cursor::new(&data), SafeTakeDeframer);
    let mut count = 0;
    reader
        .process_typed::<StrRoot, _>(|_| {
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 3);
}

#[test]
fn typed_read_unsafe_deframer() {
    // Purpose: Typed reading works equivalently with UnsafeDeframer (trusted data).
    let data = build_string_messages(3);
    let mut reader = StreamReader::new(Cursor::new(&data), UnsafeDeframer);
    let mut count = 0;
    reader
        .process_typed::<StrRoot, _>(|_| {
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 3);
}

#[cfg(feature = "xxhash")]
#[test]
fn typed_read_checksum_xxhash64() {
    // Purpose: Typed reading works with checksum framing/deframing (XXHash64).
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), ChecksumFramer::new(XxHash64::new()));
    let mut builder = FlatBufferBuilder::new();
    for i in 0..3 {
        builder.reset();
        let s = builder.create_string(&format!("message {i}"));
        builder.finish(s, None);
        writer.write_finished(&mut builder).unwrap();
    }
    let mut reader = StreamReader::new(Cursor::new(&buf), ChecksumDeframer::new(XxHash64::new()));
    let mut count = 0;
    reader
        .process_typed::<StrRoot, _>(|_| {
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 3);
}

#[cfg(feature = "crc32")]
#[test]
fn typed_read_checksum_crc32() {
    // Purpose: Typed reading works with checksum framing/deframing (CRC32).
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), ChecksumFramer::new(Crc32::new()));
    let mut builder = FlatBufferBuilder::new();
    for i in 0..3 {
        builder.reset();
        let s = builder.create_string(&format!("message {i}"));
        builder.finish(s, None);
        writer.write_finished(&mut builder).unwrap();
    }
    let mut reader = StreamReader::new(Cursor::new(&buf), ChecksumDeframer::new(Crc32::new()));
    let mut count = 0;
    reader
        .process_typed::<StrRoot, _>(|_| {
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 3);
}

#[cfg(feature = "crc16")]
#[test]
fn typed_read_checksum_crc16() {
    // Purpose: Typed reading works with checksum framing/deframing (CRC16).
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), ChecksumFramer::new(Crc16::new()));
    let mut builder = FlatBufferBuilder::new();
    for i in 0..3 {
        builder.reset();
        let s = builder.create_string(&format!("message {i}"));
        builder.finish(s, None);
        writer.write_finished(&mut builder).unwrap();
    }
    let mut reader = StreamReader::new(Cursor::new(&buf), ChecksumDeframer::new(Crc16::new()));
    let mut count = 0;
    reader
        .process_typed::<StrRoot, _>(|_| {
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 3);
}

#[cfg(feature = "xxhash")]
#[test]
fn checksum_mismatch_propagates_error_typed() {
    // Purpose: When payload is corrupted, typed processing returns a checksum mismatch error.
    // Write valid frame with checksum
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), ChecksumFramer::new(XxHash64::new()));
    let mut builder = FlatBufferBuilder::new();
    let s = builder.create_string("hello");
    builder.finish(s, None);
    writer.write_finished(&mut builder).unwrap();

    // Corrupt one byte in payload area if long enough
    if buf.len() > 20 {
        buf[20] ^= 1;
    }

    let mut reader = StreamReader::new(Cursor::new(&buf), ChecksumDeframer::new(XxHash64::new()));
    let err = reader.process_typed::<StrRoot, _>(|_| Ok(())).unwrap_err();
    match err {
        Error::ChecksumMismatch { .. } => {}
        other => panic!("expected checksum mismatch, got {other:?}"),
    }
}

#[test]
fn processor_error_propagates_and_stops_typed() {
    // Purpose: An error in the typed processor closure propagates and stops iteration.
    let data = build_string_messages(5);
    let mut reader = StreamReader::new(Cursor::new(&data), DefaultDeframer);
    let mut count = 0usize;
    let err = reader
        .process_typed::<StrRoot, _>(|_| {
            count += 1;
            if count == 3 {
                return Err(Error::Io(std::io::Error::other("stop")));
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(count, 3);
    match err {
        Error::Io(e) => assert_eq!(e.to_string(), "stop"),
        _ => panic!("unexpected error"),
    }
}

#[test]
fn from_payload_invalid_and_empty() {
    // Purpose: from_payload surfaces FlatbuffersError for empty or invalid payloads.
    // Empty
    let empty: &[u8] = &[];
    let err = StrRoot::from_payload(empty).unwrap_err();
    match err {
        Error::FlatbuffersError(_) => {}
        _ => panic!("expected flatbuffers error"),
    }

    // Random bytes
    let bad = [1u8, 2, 3, 4, 5, 6, 7];
    let err = StrRoot::from_payload(&bad).unwrap_err();
    match err {
        Error::FlatbuffersError(_) => {}
        _ => panic!("expected flatbuffers error"),
    }
}
