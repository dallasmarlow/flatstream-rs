use flatbuffers::FlatBufferBuilder;
use flatstream::{
    DefaultFramer, Durable, Error, ErrorKind, FrameReceipt, Framer, NoPostWriteObserver,
    PostWriteOutcome, Result, StreamSerialize, StreamWriter, SyncEveryFrame, SyncMode,
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

struct PartialThenFail {
    bytes: Vec<u8>,
    accept: usize,
}

impl Write for PartialThenFail {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.accept == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "injected partial failure",
            ));
        }
        let n = self.accept.min(buf.len());
        self.bytes.extend_from_slice(&buf[..n]);
        self.accept -= n;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct WriteAllFramer;

impl Framer for WriteAllFramer {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        writer.write_all(b"HEAD")?;
        writer.write_all(payload)?;
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
            PostWriteOutcome::WriteFailed {
                frame_start,
                bytes_accepted,
                error,
            } => {
                assert!(event.payload_len.is_some());
                assert_eq!(frame_start, 0);
                assert_eq!(bytes_accepted, 0);
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

#[test]
fn partial_custom_frame_failure_is_counted_and_poisons_the_writer() {
    let failures = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&failures);
    let sink = PartialThenFail {
        bytes: Vec::new(),
        accept: 5,
    };
    let mut writer = StreamWriter::new(sink, WriteAllFramer).with_post_write_observer(
        move |event: flatstream::PostWriteEvent<'_>| {
            if let PostWriteOutcome::WriteFailed {
                frame_start,
                bytes_accepted,
                ..
            } = event.outcome
            {
                observed.lock().unwrap().push((frame_start, bytes_accepted));
            }
        },
    );

    assert!(!writer.is_poisoned());
    writer
        .write(&"partially accepted")
        .expect_err("the custom frame must fail after five bytes");
    assert_eq!(writer.bytes_written(), 5);
    assert!(
        writer.is_poisoned(),
        "accepted partial-frame bytes must fail-stop the writer"
    );

    let error = writer
        .write(&"must not append behind the torn frame")
        .expect_err("a partially failed writer must remain poisoned");
    assert!(matches!(error.kind(), ErrorKind::Poisoned));
    assert_eq!(writer.bytes_written(), 5);

    assert_eq!(&*failures.lock().unwrap(), &[(0, 5), (5, 0)]);
    assert_eq!(writer.into_inner().bytes.len(), 5);
}
