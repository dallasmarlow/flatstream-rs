use flatbuffers::FlatBufferBuilder;
use flatstream::{
    AnySync, DefaultFramer, Durable, ErrorKind, NoOpPolicy, NoSync, StreamWriter, SyncEveryBytes,
    SyncEveryFrame, SyncEveryNFrames, SyncMode, SyncPolicyExt,
};
use std::io::{self, BufWriter, Write};
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Default)]
struct RecordingDurable {
    bytes: Vec<u8>,
    flushes: usize,
    data_syncs: usize,
    all_syncs: usize,
    bytes_at_sync: Vec<usize>,
    fail_sync: Option<Arc<AtomicBool>>,
}

impl Write for RecordingDurable {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        let mut total = 0;
        for buf in bufs {
            self.bytes.extend_from_slice(buf);
            total += buf.len();
        }
        Ok(total)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

impl Durable for RecordingDurable {
    fn sync_data(&mut self) -> io::Result<()> {
        if self
            .fail_sync
            .as_ref()
            .is_some_and(|fail| fail.load(Ordering::Relaxed))
        {
            return Err(io::Error::other("injected sync_data failure"));
        }
        self.data_syncs += 1;
        self.bytes_at_sync.push(self.bytes.len());
        Ok(())
    }

    fn sync_all(&mut self) -> io::Result<()> {
        if self
            .fail_sync
            .as_ref()
            .is_some_and(|fail| fail.load(Ordering::Relaxed))
        {
            return Err(io::Error::other("injected sync_all failure"));
        }
        self.all_syncs += 1;
        self.bytes_at_sync.push(self.bytes.len());
        Ok(())
    }
}

fn finished(builder: &mut FlatBufferBuilder, value: &str) {
    builder.reset();
    let value = builder.create_string(value);
    builder.finish(value, None);
}

#[test]
fn default_no_sync_is_zero_sized_and_manual_sync_returns_watermark() {
    assert_eq!(std::mem::size_of::<NoSync>(), 0);

    let mut writer = StreamWriter::new(RecordingDurable::default(), DefaultFramer);
    let receipt = writer.write_with_receipt(&"manual").unwrap();
    let watermark = writer.sync_data().unwrap();
    assert_eq!(watermark, receipt.end());

    let sink = writer.into_inner();
    assert_eq!(sink.data_syncs, 1);
    assert_eq!(sink.all_syncs, 0);
    assert_eq!(sink.bytes.len() as u64, watermark);
}

#[test]
fn every_frame_updates_the_durable_watermark() {
    let mut writer = StreamWriter::new(RecordingDurable::default(), DefaultFramer)
        .with_sync_policy(SyncEveryFrame::new(SyncMode::Data));

    let first = writer.write_with_receipt(&"first").unwrap();
    assert_eq!(writer.durable_watermark(), Some(first.end()));
    let second = writer.write_with_receipt(&"second").unwrap();
    assert_eq!(writer.durable_watermark(), Some(second.end()));

    let sink = writer.into_inner();
    assert_eq!(sink.data_syncs, 2);
    assert_eq!(
        sink.bytes_at_sync,
        vec![first.end() as usize, second.end() as usize]
    );
}

#[test]
fn composed_policies_choose_the_stronger_mode_once() {
    let policy: AnySync<_, _> = SyncEveryNFrames::new(NonZeroU64::new(2).unwrap(), SyncMode::Data)
        .or(SyncEveryBytes::new(
            NonZeroU64::new(1).unwrap(),
            SyncMode::All,
        ));
    let mut writer =
        StreamWriter::new(RecordingDurable::default(), DefaultFramer).with_sync_policy(policy);

    let receipt = writer.write_with_receipt(&"strongest").unwrap();
    assert_eq!(writer.durable_watermark(), Some(receipt.end()));
    let sink = writer.into_inner();
    assert_eq!(sink.data_syncs, 0);
    assert_eq!(sink.all_syncs, 1);
}

#[test]
fn a_manual_checkpoint_resets_the_policy_window() {
    let policy = SyncEveryNFrames::new(NonZeroU64::new(2).unwrap(), SyncMode::Data);
    let mut writer =
        StreamWriter::new(RecordingDurable::default(), DefaultFramer).with_sync_policy(policy);

    writer.write(&"before manual").unwrap();
    assert_eq!(writer.durable_watermark(), None);
    let manual = writer.sync_all().unwrap();
    assert_eq!(writer.durable_watermark(), Some(manual));

    writer.write(&"one").unwrap();
    assert_eq!(writer.durable_watermark(), Some(manual));
    let second = writer.write_with_receipt(&"two").unwrap();
    assert_eq!(writer.durable_watermark(), Some(second.end()));

    let sink = writer.into_inner();
    assert_eq!(sink.all_syncs, 1);
    assert_eq!(sink.data_syncs, 1);
}

#[test]
fn bufwriter_flushes_before_delegating_sync() {
    let sink = BufWriter::with_capacity(64 * 1024, RecordingDurable::default());
    let mut writer = StreamWriter::new(sink, DefaultFramer)
        .with_sync_policy(SyncEveryFrame::new(SyncMode::Data));
    let receipt = writer.write_with_receipt(&"buffered").unwrap();

    let buffered = writer.into_inner();
    let sink = buffered.into_inner().unwrap();
    assert_eq!(sink.data_syncs, 1);
    assert_eq!(sink.bytes_at_sync, vec![receipt.wire_len as usize]);
}

#[test]
fn sync_failure_reports_the_accepted_frame_and_previous_watermark() {
    let fail = Arc::new(AtomicBool::new(true));
    let sink = RecordingDurable {
        fail_sync: Some(Arc::clone(&fail)),
        ..RecordingDurable::default()
    };
    let mut writer = StreamWriter::new(sink, DefaultFramer)
        .with_sync_policy(SyncEveryFrame::new(SyncMode::Data));

    let err = writer
        .write_with_receipt(&"accepted but not durable")
        .unwrap_err();
    match err.into_kind() {
        ErrorKind::DurabilityFailed {
            mode,
            attempted_watermark,
            previous_watermark,
            frame_start,
            wire_len,
            ..
        } => {
            assert_eq!(mode, SyncMode::Data);
            assert_eq!(previous_watermark, None);
            assert_eq!(frame_start, Some(0));
            assert_eq!(attempted_watermark, wire_len.unwrap());
            assert_eq!(writer.bytes_written(), attempted_watermark);
        }
        other => panic!("expected DurabilityFailed, got {other:?}"),
    }
    assert_eq!(writer.durable_watermark(), None);
    assert!(!writer.into_inner().bytes.is_empty());
}

#[test]
fn start_offset_is_reflected_in_automatic_watermarks() {
    let mut builder = FlatBufferBuilder::new();
    finished(&mut builder, "offset");

    let mut writer = StreamWriter::new(RecordingDurable::default(), DefaultFramer)
        .with_start_offset(10_000)
        .with_sync_policy(SyncEveryFrame::new(SyncMode::All));
    let receipt = writer.write_finished_with_receipt(&mut builder).unwrap();

    assert_eq!(receipt.frame_start, 10_000);
    assert_eq!(writer.durable_watermark(), Some(receipt.end()));
    assert_eq!(writer.into_inner().all_syncs, 1);
}

struct PartialVectoredSink {
    bytes: Vec<u8>,
    limit: usize,
    vectored_calls: usize,
}

impl Write for PartialVectoredSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = buf.len().min(self.limit);
        self.bytes.extend_from_slice(&buf[..n]);
        Ok(n)
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.vectored_calls += 1;
        let mut remaining = self.limit;
        let mut total = 0;
        for buf in bufs {
            let n = buf.len().min(remaining);
            self.bytes.extend_from_slice(&buf[..n]);
            total += n;
            remaining -= n;
            if remaining == 0 {
                break;
            }
        }
        Ok(total)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn receipts_count_every_partial_vectored_write() {
    let sink = PartialVectoredSink {
        bytes: Vec::new(),
        limit: 3,
        vectored_calls: 0,
    };
    let mut writer = StreamWriter::new(sink, DefaultFramer);
    let mut builder = FlatBufferBuilder::new();
    finished(&mut builder, "partial receipt accounting");

    let receipt = writer.write_finished_with_receipt(&mut builder).unwrap();
    assert_eq!(receipt.frame_start, 0);
    assert_eq!(writer.bytes_written(), receipt.end());
    let sink = writer.into_inner();
    assert_eq!(receipt.wire_len, sink.bytes.len() as u64);
    assert!(
        sink.vectored_calls > 1,
        "the sink must genuinely force the CountingWriter partial path"
    );
}

#[test]
fn memory_and_sync_policies_compose_in_either_installation_order() {
    let sync = || SyncEveryFrame::new(SyncMode::Data);
    let mut memory_then_sync = StreamWriter::new(RecordingDurable::default(), DefaultFramer)
        .with_memory_policy(NoOpPolicy)
        .with_sync_policy(sync());
    memory_then_sync.write(&"first").unwrap();
    assert_eq!(memory_then_sync.into_inner().data_syncs, 1);

    let mut sync_then_memory = StreamWriter::new(RecordingDurable::default(), DefaultFramer)
        .with_sync_policy(sync())
        .with_memory_policy(NoOpPolicy);
    sync_then_memory.write(&"second").unwrap();
    assert_eq!(sync_then_memory.into_inner().data_syncs, 1);
}
