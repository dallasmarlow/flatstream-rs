use flatbuffers::FlatBufferBuilder;
use flatstream::{
    DefaultFramer, Durable, Error, ErrorKind, FrameReceipt, NoPostWriteObserver, PostWriteOutcome,
    Result, StreamSerialize, StreamWriter, SyncEveryFrame, SyncMode,
};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct SharedSink {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        let mut bytes = self.bytes.lock().unwrap();
        let mut total = 0;
        for buf in bufs {
            bytes.extend_from_slice(buf);
            total += buf.len();
        }
        Ok(total)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct RefusingSink;

impl Write for RefusingSink {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "sink closed"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct UnsyncableSink {
    bytes: Vec<u8>,
}

impl Write for UnsyncableSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Durable for UnsyncableSink {
    fn sync_data(&mut self) -> io::Result<()> {
        Err(io::Error::other("sync refused"))
    }

    fn sync_all(&mut self) -> io::Result<()> {
        self.sync_data()
    }
}

struct SerializationFailure;

impl StreamSerialize for SerializationFailure {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        _builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        Err(Error::invalid_frame("serialization refused"))
    }
}

#[test]
fn default_post_write_observer_is_zero_sized() {
    assert_eq!(std::mem::size_of::<NoPostWriteObserver>(), 0);
}

#[test]
fn success_event_fires_after_bytes_are_accepted() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::new(Mutex::new(Vec::<FrameReceipt>::new()));
    let observer_bytes = Arc::clone(&bytes);
    let observer_receipts = Arc::clone(&observed);
    let sink = SharedSink {
        bytes: Arc::clone(&bytes),
    };
    let mut writer = StreamWriter::new(sink, DefaultFramer).with_post_write_observer(
        move |event: flatstream::PostWriteEvent<'_>| {
            assert!(event.payload_len.is_some());
            match event.outcome {
                PostWriteOutcome::Succeeded(receipt) => {
                    assert_eq!(
                        observer_bytes.lock().unwrap().len() as u64,
                        receipt.end(),
                        "the callback runs only after the complete frame is accepted"
                    );
                    observer_receipts.lock().unwrap().push(receipt);
                }
                other => panic!("expected success event, got {other:?}"),
            }
        },
    );

    let receipt = writer.write_with_receipt(&"observed").unwrap();
    assert_eq!(&*observed.lock().unwrap(), &[receipt]);
}

#[test]
fn serialization_and_write_failures_are_never_reported_as_success() {
    let kinds = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&kinds);
    let mut serialization_writer = StreamWriter::new(Vec::new(), DefaultFramer)
        .with_post_write_observer(move |event: flatstream::PostWriteEvent<'_>| {
            match event.outcome {
                PostWriteOutcome::SerializationFailed(error) => {
                    assert!(event.payload_len.is_none());
                    assert!(matches!(error.kind(), ErrorKind::InvalidFrame { .. }));
                    observed.lock().unwrap().push("serialization");
                }
                other => panic!("expected serialization failure, got {other:?}"),
            }
        });
    serialization_writer
        .write(&SerializationFailure)
        .expect_err("serialization must fail");

    let observed = Arc::clone(&kinds);
    let mut write_writer = StreamWriter::new(RefusingSink, DefaultFramer).with_post_write_observer(
        move |event: flatstream::PostWriteEvent<'_>| match event.outcome {
            PostWriteOutcome::WriteFailed(error) => {
                assert!(event.payload_len.is_some());
                assert!(matches!(
                    error.kind(),
                    ErrorKind::Io(source) if source.kind() == io::ErrorKind::BrokenPipe
                ));
                observed.lock().unwrap().push("write");
            }
            other => panic!("expected write failure, got {other:?}"),
        },
    );
    write_writer
        .write(&"refused")
        .expect_err("sink must refuse the frame");

    assert_eq!(&*kinds.lock().unwrap(), &["serialization", "write"]);
}

#[test]
fn durability_failure_event_carries_the_accepted_receipt() {
    let accepted = Arc::new(Mutex::new(None));
    let observed = Arc::clone(&accepted);
    let mut writer = StreamWriter::new(UnsyncableSink::default(), DefaultFramer)
        .with_sync_policy(SyncEveryFrame::new(SyncMode::Data))
        .with_post_write_observer(move |event: flatstream::PostWriteEvent<'_>| {
            match event.outcome {
                PostWriteOutcome::DurabilityFailed { receipt, error } => {
                    assert!(matches!(
                        error.kind(),
                        ErrorKind::DurabilityFailed {
                            attempted_watermark,
                            ..
                        } if *attempted_watermark == receipt.end()
                    ));
                    *observed.lock().unwrap() = Some(receipt);
                }
                other => panic!("expected durability failure, got {other:?}"),
            }
        });

    let error = writer
        .write_with_receipt(&"accepted but not durable")
        .expect_err("checkpoint must fail");
    let receipt = accepted
        .lock()
        .unwrap()
        .expect("observer records the accepted frame");
    assert_eq!(receipt.end(), writer.bytes_written());
    assert!(matches!(error.kind(), ErrorKind::DurabilityFailed { .. }));
    assert_eq!(writer.into_inner().bytes.len() as u64, receipt.end());
}
