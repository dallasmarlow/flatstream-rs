use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::io::Cursor;

fn build_empty_table_vec() -> Vec<u8> {
    let mut b = FlatBufferBuilder::new();
    let start = b.start_table();
    let root = b.end_table(start);
    b.finish(root, None);
    b.finished_data().to_vec()
}

#[test]
fn validating_deframer_rejects_structurally_invalid_payload_after_deframe() {
    // Create a framed message with an invalid FlatBuffer payload: bytes that are not a table
    let payload = b"not a flatbuffer table".to_vec();
    let mut framed = Vec::new();
    DefaultFramer
        .frame_and_write(&mut framed, &payload)
        .unwrap();

    let deframer = DefaultDeframer.with_validator(StructuralValidator::new());
    let mut reader = StreamReader::new(Cursor::new(framed), deframer);
    let err = reader.read_message().unwrap_err();
    match err {
        Error::ValidationFailed { validator, .. } => assert_eq!(validator, "StructuralValidator"),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

#[test]
fn validating_deframer_accepts_valid() {
    let buf = build_empty_table_vec();
    // Frame one message
    let mut framed = Vec::new();
    {
        let framer = DefaultFramer;
        framer.frame_and_write(&mut framed, &buf).unwrap();
    }

    let deframer = DefaultDeframer.with_validator(StructuralValidator::new());
    let mut reader = StreamReader::new(Cursor::new(framed), deframer);
    let first = reader.read_message().unwrap();
    assert!(first.is_some());
}

#[cfg(feature = "xxhash")]
#[test]
fn validating_deframer_with_checksum_propagates_checksum_error() {
    // Build valid table
    let buf = build_empty_table_vec();
    // Frame with checksum
    let mut framed = Vec::new();
    {
        let framer = ChecksumFramer::new(XxHash64::new());
        framer.frame_and_write(&mut framed, &buf).unwrap();
    }
    // Corrupt payload byte
    if framed.len() > 12 {
        // 4 len + 8 checksum
        framed[12] ^= 1;
    }

    let deframer =
        ChecksumDeframer::new(XxHash64::new()).with_validator(StructuralValidator::new());
    let mut reader = StreamReader::new(Cursor::new(framed), deframer);
    let err = reader.read_message().unwrap_err();
    match err {
        Error::ChecksumMismatch { .. } => {}
        other => panic!("expected ChecksumMismatch, got {other:?}"),
    }
}

#[test]
fn fluent_api_compiles_and_runs() {
    let buf = build_empty_table_vec();
    let mut framed = Vec::new();
    DefaultFramer.frame_and_write(&mut framed, &buf).unwrap();

    let deframer = DefaultDeframer.bounded(1024 * 1024).with_validator(
        CompositeValidator::new()
            .add(SizeValidator::new(1, 1024 * 1024))
            .add(StructuralValidator::new()),
    );
    let mut reader = StreamReader::new(Cursor::new(framed), deframer);
    assert!(reader.read_message().unwrap().is_some());
}

#[test]
fn validating_framer_rejects_invalid_before_write() {
    // Payload that will fail StructuralValidator
    let payload = b"not a flatbuffer table".to_vec();
    let framer = DefaultFramer.with_validator(StructuralValidator::new());

    let mut sink = Vec::new();
    let err = framer.frame_and_write(&mut sink, &payload).unwrap_err();
    match err {
        Error::ValidationFailed { validator, .. } => assert_eq!(validator, "StructuralValidator"),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}
