//! Statically dispatched durability checkpoints with a durable watermark.

use flatstream::{DefaultFramer, StreamWriter, SyncEveryNFrames, SyncMode};
use std::io::BufWriter;
use std::num::NonZeroU64;

fn main() -> flatstream::Result<()> {
    let file = tempfile::tempfile()?;
    let policy = SyncEveryNFrames::new(NonZeroU64::new(2).unwrap(), SyncMode::Data);
    let mut writer =
        StreamWriter::new(BufWriter::new(file), DefaultFramer).with_sync_policy(policy);

    let first = writer.write_with_receipt(&"first")?;
    assert_eq!(writer.durable_watermark(), None);

    let second = writer.write_with_receipt(&"second")?;
    assert_eq!(second.frame_start, first.end());
    assert_eq!(
        writer.durable_watermark(),
        Some(second.end()),
        "the two-frame policy must checkpoint exactly at frame two"
    );

    let third = writer.write_with_receipt(&"third")?;
    assert_eq!(
        writer.durable_watermark(),
        Some(second.end()),
        "one pending frame must not advance the durable watermark"
    );

    let forced = writer.sync_all()?;
    assert_eq!(forced, third.end());
    assert_eq!(writer.durable_watermark(), Some(third.end()));
    let file = writer
        .into_inner()
        .into_inner()
        .map_err(|error| error.into_error())?;
    assert_eq!(file.metadata()?.len(), third.end());

    println!(
        "three frames written; automatic checkpoint at {}, forced checkpoint at {}",
        second.end(),
        third.end()
    );
    Ok(())
}
