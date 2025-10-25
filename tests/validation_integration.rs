use flatbuffers::FlatBufferBuilder;
use flatstream::framing::{DeframerExt, FramerExt};
use flatstream::*;
use std::io::Cursor;

// Tests-only import of generated schemas used to validate TypedValidator behavior.
// These imports do not affect the core library; clippy is suppressed only for
// these generated files to avoid noise in test builds.
#[allow(clippy::extra_unused_lifetimes, mismatched_lifetime_syntaxes)]
#[path = "../examples/generated/lobster_message_generated.rs"]
mod lobster_generated;
#[allow(clippy::extra_unused_lifetimes, mismatched_lifetime_syntaxes)]
#[path = "../examples/generated/telemetry_generated.rs"]
mod telemetry_generated;

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
    // Use process_all to validate that the pipeline accepts the typed message
    reader.process_all(|_| Ok(())).unwrap();
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

// --- TypedValidator tests ---

#[test]
fn typed_validator_accepts_matching_root() {
    // Build a minimal TelemetryEvent buffer using the generated API
    let mut b = FlatBufferBuilder::new();
    let msg = b.create_string("");
    let mut tb = telemetry_generated::telemetry::TelemetryEventBuilder::new(&mut b);
    tb.add_message(msg);
    tb.add_timestamp(0);
    let root = tb.finish();
    b.finish(root, None);
    let buf = b.finished_data().to_vec();

    let validator = TypedValidator::from_verify(|opts, payload| {
        telemetry_generated::telemetry::root_as_telemetry_event_with_opts(opts, payload).map(|_| ())
    });
    let deframer = DefaultDeframer.with_validator(validator);
    let mut framed = Vec::new();
    DefaultFramer.frame_and_write(&mut framed, &buf).unwrap();
    let mut reader = StreamReader::new(Cursor::new(framed), deframer);
    assert!(reader.read_message().unwrap().is_some());
}

#[test]
fn typed_validator_rejects_wrong_type() {
    // Build a valid Lobster MessageEvent and attempt to validate as TelemetryEvent (wrong schema)
    let mut b = FlatBufferBuilder::new();
    {
        use lobster_generated::flatstream::lobster as lob;
        let mut mb = lob::MessageEventBuilder::new(&mut b);
        mb.add_timestamp(0.0);
        mb.add_event_type(1);
        mb.add_order_id(42);
        mb.add_size(1);
        mb.add_price(100);
        mb.add_direction(1);
        let root = mb.finish();
        b.finish(root, None);
    }
    let buf = b.finished_data().to_vec();
    let validator = TypedValidator::from_verify(|opts, payload| {
        telemetry_generated::telemetry::root_as_telemetry_event_with_opts(opts, payload).map(|_| ())
    });
    let deframer = DefaultDeframer.with_validator(validator);
    let mut framed = Vec::new();
    DefaultFramer.frame_and_write(&mut framed, &buf).unwrap();
    let mut reader = StreamReader::new(Cursor::new(framed), deframer);
    let err = reader.process_all(|_| Ok(())).unwrap_err();
    match err {
        Error::ValidationFailed { validator, .. } => assert_eq!(validator, "TypedValidator"),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

#[test]
fn process_all_and_messages_propagate_validation_failed() {
    // Frame structurally invalid payload so validation runs and fails
    let payload = b"not a flatbuffer table".to_vec();
    let mut framed = Vec::new();
    DefaultFramer
        .frame_and_write(&mut framed, &payload)
        .unwrap();

    let deframer = DefaultDeframer.with_validator(StructuralValidator::new());
    let mut reader = StreamReader::new(Cursor::new(framed.clone()), deframer);

    let err = reader.process_all(|_| Ok(())).unwrap_err();
    assert!(matches!(err, Error::ValidationFailed { .. }));

    let mut reader = StreamReader::new(
        Cursor::new(framed),
        DefaultDeframer.with_validator(StructuralValidator::new()),
    );
    let mut iter = reader.messages();
    let err = iter.next().unwrap_err();
    assert!(matches!(err, Error::ValidationFailed { .. }));
}
