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

fn process_with<D: Deframer>(deframer: D, framed: &[u8], label: &str) -> Result<()> {
    let reader = BufReader::new(Cursor::new(framed));
    let mut stream = StreamReader::new(reader, deframer);
    stream.process_all(|payload| {
        // Use the payload so examples don’t warn
        let _ = payload.len();
        Ok(())
    })?;
    println!("{}: ok", label);
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
        DefaultDeframer.with_validator(NoValidator),
        &framed,
        "NoValidator",
    )?;

    // 3) Read: TableRootValidator (type-agnostic table-root verification)
    process_with(
        DefaultDeframer.with_validator(TableRootValidator::new()),
        &framed,
        "TableRootValidator",
    )?;

    // 4) Read: CompositeValidator (Size + TableRoot)
    {
        let validator = CompositeValidator::new()
            .add(SizeValidator::new(1, 1024 * 1024))
            .add(TableRootValidator::new());
        process_with(
            DefaultDeframer.with_validator(validator),
            &framed,
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
            DefaultDeframer.with_validator(TableRootValidator::new()),
        )
        .process_all(|_| Ok(()))
        .unwrap_err();
        match err {
            Error::ValidationFailed { reason, .. } => {
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
    // let mut stream = StreamReader::new(reader, DefaultDeframer.with_validator(typed));
    // stream.process_all(|_| Ok(()))?;

    // 7) Write path with validation: ValidatingFramer validates before write
    {
        let mut out = Vec::new();
        let framer = DefaultFramer.with_validator(TableRootValidator::new());
        framer.frame_and_write(&mut out, &telemetry)?;
        println!("ValidatingFramer (write path): ok");
    }

    println!("validation_example: done");
    Ok(())
}
