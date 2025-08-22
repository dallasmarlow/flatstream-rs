// To build this example:
// 1) Install flatc
// 2) Run: flatc --rust -o examples/generated examples/schemas/telemetry.fbs
// 3) cargo run --example typed_reading_flatc_example --features flatc_example

#![cfg(feature = "flatc_example")]

use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::io::Cursor;

// Generated module path from step (2). Do not check generated code into the repo.
mod generated {
    pub mod telemetry_generated {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/generated/telemetry_generated.rs"
        ));
    }
}

use generated::telemetry_generated::telemetry::{self, TelemetryEvent, TelemetryEventArgs};

struct TelemetryEventRoot;

impl<'a> StreamDeserialize<'a> for TelemetryEventRoot {
    type Root = TelemetryEvent<'a>;
    fn from_payload(payload: &'a [u8]) -> Result<Self::Root> {
        flatbuffers::root::<TelemetryEvent<'a>>(payload).map_err(Error::FlatbuffersError)
    }
}

fn main() -> Result<()> {
    // Write a few TelemetryEvent messages
    let mut storage = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut storage), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        for i in 0..3u64 {
            builder.reset();
            let msg = builder.create_string(&format!("event-{i}"));
            let event = TelemetryEvent::create(
                &mut builder,
                &TelemetryEventArgs {
                    message: Some(msg),
                    timestamp: i,
                },
            );
            builder.finish(event, None);
            writer.write_finished(&mut builder)?;
        }
    }

    // Read with typed API
    let mut reader = StreamReader::new(Cursor::new(&storage), DefaultDeframer);
    let mut seen = 0u64;
    reader.process_typed::<TelemetryEventRoot, _>(|event| {
        assert!(event.message().unwrap().starts_with("event-"));
        seen += 1;
        Ok(())
    })?;
    assert_eq!(seen, 3);
    Ok(())
}
