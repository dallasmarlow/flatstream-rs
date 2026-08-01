use flatbuffers::FlatBufferBuilder;
use flatstream::framing::{DeframerExt, FramerExt};
use flatstream::*;
use std::io::{BufReader, Cursor};

// Use a generated schema so we validate a non-empty table as well
#[allow(clippy::extra_unused_lifetimes, mismatched_lifetime_syntaxes)]
#[path = "generated/telemetry_generated.rs"]
mod telemetry_generated;

fn build_telemetry_event() -> Vec<u8> {
    let mut b = FlatBufferBuilder::new();
    let msg = b.create_string("hello");
    let mut tb = telemetry_generated::telemetry::TelemetryEventBuilder::new(&mut b);
    tb.add_message(msg);
    tb.add_timestamp(123);
    let root = tb.finish();
    b.finish(root, None);
    b.finished_data().to_vec()
}

fn write_framed(payload: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, payload)?;
    Ok(out)
}

/// Reads `framed` under `deframer` and asserts the stream round-trips to
/// exactly `expected`.
///
/// Collecting the payloads matters: a validator that silently accepted zero
/// messages would satisfy a bare `process_all(|_| Ok(()))`, so the count is
/// half the assertion.
fn process_with<D: Deframer>(
    deframer: D,
    framed: &[u8],
    expected: &[u8],
    label: &str,
) -> Result<()> {
    let reader = BufReader::new(Cursor::new(framed));
    let mut stream = StreamReader::new(reader, deframer);

    let mut seen: Vec<Vec<u8>> = Vec::new();
    stream.process_all(|payload| {
        seen.push(payload.to_vec());
        Ok(())
    })?;

    assert_eq!(seen.len(), 1, "{label}: expected exactly one message");
    assert_eq!(
        seen[0], expected,
        "{label}: payload must survive validation byte-for-byte"
    );
    println!("{label}: ok ({} bytes round-tripped)", seen[0].len());
    Ok(())
}

fn main() -> Result<()> {
    println!("validation_example: starting");
    // 1) Write: DefaultFramer (no validation on write)
    // Use a non-empty telemetry event for realism
    let telemetry = build_telemetry_event();
    let framed = write_framed(&telemetry)?;

    // 2) Read: NoValidator (zero-cost)
    process_with(
        DefaultDeframer::new().with_validator(NoValidator),
        &framed,
        &telemetry,
        "NoValidator",
    )?;

    // 3) Read: TableRootValidator (type-agnostic table-root verification)
    process_with(
        DefaultDeframer::new().with_validator(TableRootValidator::new()),
        &framed,
        &telemetry,
        "TableRootValidator",
    )?;

    // 4) Read: CompositeValidator (Size + TableRoot)
    {
        let validator = CompositeValidator::new()
            .add(SizeValidator::new(1, 1024 * 1024))
            .add(TableRootValidator::new());
        process_with(
            DefaultDeframer::new().with_validator(validator),
            &framed,
            &telemetry,
            "CompositeValidator (Size + TableRoot)",
        )?;
    }

    // 5) Demonstrate failure: table-root validator rejects invalid payload
    {
        let invalid_payload = b"not a flatbuffer table".to_vec();
        let mut invalid_framed = Vec::new();
        DefaultFramer.frame_and_write(&mut invalid_framed, &invalid_payload)?;

        let err = StreamReader::new(
            BufReader::new(Cursor::new(&invalid_framed)),
            DefaultDeframer::new().with_validator(TableRootValidator::new()),
        )
        .process_all(|_| Ok(()))
        .unwrap_err();
        match err.into_kind() {
            ErrorKind::ValidationFailed { reason, .. } => {
                println!(
                    "TableRootValidator: expected failure observed: {}",
                    reason.trim()
                );
            }
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
    }

    // 6) Optional: TypedValidator via function pointer
    // If you have generated code, pass its root verifier function:
    // let typed = TypedValidator::from_verify(|opts, payload| my_schema::root_as_event_with_opts(opts, payload).map(|_| ()));
    // let reader = BufReader::new(Cursor::new(&framed));
    // let mut stream = StreamReader::new(reader, DefaultDeframer::new().with_validator(typed));
    // stream.process_all(|_| Ok(()))?;

    // 7) Write path with validation: ValidatingFramer validates before write
    {
        let framer = DefaultFramer.with_validator(TableRootValidator::new());

        // A valid payload must produce byte-identical output to the plain
        // framer: validation is a gate, not a transform.
        let mut out = Vec::new();
        framer.frame_and_write(&mut out, &telemetry)?;
        assert_eq!(
            out, framed,
            "ValidatingFramer must not alter the bytes of a payload it accepts"
        );

        // An invalid payload must be rejected *before* anything reaches the
        // sink — a partially-written frame would corrupt the stream.
        let mut rejected = Vec::new();
        let err = framer
            .frame_and_write(&mut rejected, b"not a flatbuffer table")
            .unwrap_err();
        assert!(
            matches!(err.kind(), ErrorKind::ValidationFailed { .. }),
            "expected ValidationFailed on the write path, got {:?}",
            err.kind()
        );
        assert!(
            rejected.is_empty(),
            "a rejected payload must leave the sink untouched, found {} bytes",
            rejected.len()
        );
        println!("ValidatingFramer (write path): accepts valid, rejects invalid, writes nothing on reject");
    }

    println!("validation_example: done");
    Ok(())
}
