use flatbuffers::FlatBufferBuilder;
use flatstream::framing::{DeframerExt, FramerExt};
use flatstream::*;
use std::io::{BufReader, BufWriter, Cursor};

fn build_empty_table() -> Vec<u8> {
    let mut b = FlatBufferBuilder::new();
    let start = b.start_table();
    let root = b.end_table(start);
    b.finish(root, None);
    b.finished_data().to_vec()
}

fn main() -> Result<()> {
    println!("validation_example: starting");
    // 1) Write: DefaultFramer (no validation on write)
    let mut framed = Vec::new();
    {
        let writer = BufWriter::new(Cursor::new(&mut framed));
        let mut stream = StreamWriter::new(writer, DefaultFramer);
        // Build a minimal FlatBuffer table so structural validation will succeed later.
        let mut b = FlatBufferBuilder::new();
        let start = b.start_table();
        let root = b.end_table(start);
        b.finish(root, None);
        stream.write_finished(&mut b)?;
        stream.flush()?;
    }

    // 2) Read: NoValidator (zero-cost)
    {
        let reader = BufReader::new(Cursor::new(&framed));
        let mut stream = StreamReader::new(reader, DefaultDeframer.with_validator(NoValidator));
        stream.process_all(|_payload| Ok(()))?;
        println!("NoValidator: ok");
    }

    // 3) Read: StructuralValidator (type-agnostic structural verification)
    {
        let reader = BufReader::new(Cursor::new(&framed));
        let mut stream = StreamReader::new(
            reader,
            DefaultDeframer.with_validator(StructuralValidator::new()),
        );
        stream.process_all(|_payload| Ok(()))?;
        println!("StructuralValidator: ok");
    }

    // 4) Read: CompositeValidator (Size + Structural)
    {
        let validator = CompositeValidator::new()
            .add(SizeValidator::new(1, 1024 * 1024))
            .add(StructuralValidator::new());
        let reader = BufReader::new(Cursor::new(&framed));
        let mut stream = StreamReader::new(reader, DefaultDeframer.with_validator(validator));
        stream.process_all(|_payload| Ok(()))?;
        println!("CompositeValidator (Size + Structural): ok");
    }

    // 5) Demonstrate failure: structural validator rejects invalid payload
    {
        let invalid_payload = b"not a flatbuffer table".to_vec();
        let mut invalid_framed = Vec::new();
        DefaultFramer.frame_and_write(&mut invalid_framed, &invalid_payload)?;

        let reader = BufReader::new(Cursor::new(&invalid_framed));
        let mut stream = StreamReader::new(
            reader,
            DefaultDeframer.with_validator(StructuralValidator::new()),
        );
        let err = stream.process_all(|_| Ok(())).unwrap_err();
        match err {
            Error::ValidationFailed { reason, .. } => {
                println!(
                    "StructuralValidator: expected failure observed: {}",
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
        let framer = DefaultFramer.with_validator(StructuralValidator::new());
        let valid = build_empty_table();
        framer.frame_and_write(&mut out, &valid)?;
        println!("ValidatingFramer (write path): ok");
    }

    println!("validation_example: done");
    Ok(())
}
