use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::io::Cursor;

// A minimal example that treats a FlatBuffer root as a string root.
struct StrRoot;

impl<'a> StreamDeserialize<'a> for StrRoot {
    type Root = &'a str;
    fn from_payload(payload: &'a [u8]) -> Result<Self::Root> {
        flatbuffers::root::<&'a str>(payload).map_err(Error::FlatbuffersError)
    }
}

fn main() -> Result<()> {
    // Write a few string roots
    let mut storage = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut storage), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        for i in 0..3 {
            builder.reset();
            let s = builder.create_string(&format!("msg-{i}"));
            builder.finish(s, None);
            writer.write_finished(&mut builder)?;
        }
    }

    // Read back using typed API
    let mut reader = StreamReader::new(Cursor::new(&storage), DefaultDeframer);
    reader.process_typed::<StrRoot, _>(|root| {
        println!("{root}");
        Ok(())
    })?;
    Ok(())
}
