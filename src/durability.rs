//! Statically dispatched durability policies for [`StreamWriter`](crate::StreamWriter).
//!
//! Durability is deliberately separate from ordinary [`Write`]:
//! `flush` only moves bytes through userspace buffers, while a durability
//! checkpoint asks the underlying storage to persist them. Installing a policy
//! changes the writer's concrete type, so the default [`NoSync`] path remains a
//! zero-sized, branch-free specialization.

use crate::policy::{Clock, MonotonicClock};
use crate::writer::FrameReceipt;
use crate::{Error, Result};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::num::NonZeroU64;
use std::time::Duration;

/// A write sink that can push accepted bytes toward stable storage.
///
/// Implementations that buffer bytes must flush those bytes before delegating
/// the durability operation. No implementation is provided for `Vec<u8>` or
/// `io::Sink`: reporting a successful durability checkpoint for an in-memory
/// sink would be a lie.
///
/// On macOS, the standard library's `File::sync_all` maps to `fsync`, not
/// `F_FULLFSYNC`; it therefore has the same drive-write-cache caveat as
/// [`File::sync_all`]. Flatstream preserves the portable standard-library
/// semantics rather than introducing a platform-specific unsafe path.
pub trait Durable: Write {
    /// Synchronizes file contents (the `File::sync_data` / `fdatasync` shape).
    fn sync_data(&mut self) -> io::Result<()>;

    /// Synchronizes file contents and metadata (the `File::sync_all` / `fsync` shape).
    fn sync_all(&mut self) -> io::Result<()>;
}

impl Durable for File {
    fn sync_data(&mut self) -> io::Result<()> {
        File::sync_data(self)
    }

    fn sync_all(&mut self) -> io::Result<()> {
        File::sync_all(self)
    }
}

impl<W: Durable> Durable for BufWriter<W> {
    fn sync_data(&mut self) -> io::Result<()> {
        self.flush()?;
        self.get_mut().sync_data()
    }

    fn sync_all(&mut self) -> io::Result<()> {
        self.flush()?;
        self.get_mut().sync_all()
    }
}

impl<W: Durable + ?Sized> Durable for &mut W {
    fn sync_data(&mut self) -> io::Result<()> {
        (**self).sync_data()
    }

    fn sync_all(&mut self) -> io::Result<()> {
        (**self).sync_all()
    }
}

impl<W: Durable + ?Sized> Durable for Box<W> {
    fn sync_data(&mut self) -> io::Result<()> {
        (**self).sync_data()
    }

    fn sync_all(&mut self) -> io::Result<()> {
        (**self).sync_all()
    }
}

/// The strength of a requested durability checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SyncMode {
    /// Synchronize file contents without requiring unrelated metadata updates.
    Data,
    /// Synchronize file contents and metadata.
    All,
}

/// Information supplied to a [`SyncPolicy`] after a complete frame is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncInfo {
    /// First byte of the frame.
    pub frame_start: u64,
    /// Number of on-wire bytes in the frame.
    pub wire_len: u64,
    /// Offset one past the frame's final byte.
    pub frame_end: u64,
}

/// A statically dispatched policy deciding when accepted frames become durable.
///
/// `observe` is called after each successfully written frame. State is reset
/// only through `on_synced`, after the requested durability operation succeeds;
/// a failed checkpoint therefore remains due.
pub trait SyncPolicy: Send {
    /// Observes an accepted frame and optionally requests a checkpoint.
    fn observe(&mut self, info: SyncInfo) -> Option<SyncMode>;

    /// Called after a manual or automatic checkpoint succeeds.
    #[inline(always)]
    fn on_synced(&mut self, _durable_watermark: u64) {}
}

/// The zero-sized default policy. It never requests a checkpoint.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSync;

impl SyncPolicy for NoSync {
    #[inline(always)]
    fn observe(&mut self, _info: SyncInfo) -> Option<SyncMode> {
        None
    }
}

/// Requests a durability checkpoint after every frame.
#[derive(Debug, Clone, Copy)]
pub struct SyncEveryFrame {
    mode: SyncMode,
}

impl SyncEveryFrame {
    /// Creates a policy that requests `mode` after every complete frame.
    pub const fn new(mode: SyncMode) -> Self {
        Self { mode }
    }
}

impl SyncPolicy for SyncEveryFrame {
    #[inline(always)]
    fn observe(&mut self, _info: SyncInfo) -> Option<SyncMode> {
        Some(self.mode)
    }
}

/// Requests a checkpoint after a configured number of frames.
#[derive(Debug, Clone, Copy)]
pub struct SyncEveryNFrames {
    frames: NonZeroU64,
    pending: u64,
    mode: SyncMode,
}

impl SyncEveryNFrames {
    /// Creates a frame-count policy. The counter resets after a successful
    /// manual or automatic checkpoint.
    pub const fn new(frames: NonZeroU64, mode: SyncMode) -> Self {
        Self {
            frames,
            pending: 0,
            mode,
        }
    }
}

impl SyncPolicy for SyncEveryNFrames {
    #[inline]
    fn observe(&mut self, _info: SyncInfo) -> Option<SyncMode> {
        self.pending = self.pending.saturating_add(1);
        (self.pending >= self.frames.get()).then_some(self.mode)
    }

    #[inline]
    fn on_synced(&mut self, _durable_watermark: u64) {
        self.pending = 0;
    }
}

/// Requests a checkpoint after at least a configured number of wire bytes.
#[derive(Debug, Clone, Copy)]
pub struct SyncEveryBytes {
    bytes: NonZeroU64,
    pending: u64,
    mode: SyncMode,
}

impl SyncEveryBytes {
    /// Creates a wire-byte policy. A frame that crosses the threshold is fully
    /// included in the resulting checkpoint.
    pub const fn new(bytes: NonZeroU64, mode: SyncMode) -> Self {
        Self {
            bytes,
            pending: 0,
            mode,
        }
    }
}

impl SyncPolicy for SyncEveryBytes {
    #[inline]
    fn observe(&mut self, info: SyncInfo) -> Option<SyncMode> {
        self.pending = self.pending.saturating_add(info.wire_len);
        (self.pending >= self.bytes.get()).then_some(self.mode)
    }

    #[inline]
    fn on_synced(&mut self, _durable_watermark: u64) {
        self.pending = 0;
    }
}

/// Requests a checkpoint once a monotonic interval has elapsed.
#[derive(Debug, Clone)]
pub struct SyncEveryInterval<C: Clock = MonotonicClock> {
    interval: Duration,
    last_sync: Duration,
    clock: C,
    mode: SyncMode,
}

impl SyncEveryInterval {
    /// Creates an interval policy using the production monotonic clock.
    pub fn new(interval: Duration, mode: SyncMode) -> Self {
        let clock = MonotonicClock::new();
        let last_sync = clock.now();
        Self {
            interval,
            last_sync,
            clock,
            mode,
        }
    }
}

impl<C: Clock> SyncEveryInterval<C> {
    /// Creates an interval policy with an injected deterministic clock.
    pub fn with_clock(interval: Duration, mode: SyncMode, clock: C) -> Self {
        let last_sync = clock.now();
        Self {
            interval,
            last_sync,
            clock,
            mode,
        }
    }
}

impl<C: Clock> SyncPolicy for SyncEveryInterval<C> {
    #[inline]
    fn observe(&mut self, _info: SyncInfo) -> Option<SyncMode> {
        (self.clock.now().saturating_sub(self.last_sync) >= self.interval).then_some(self.mode)
    }

    #[inline]
    fn on_synced(&mut self, _durable_watermark: u64) {
        self.last_sync = self.clock.now();
    }
}

/// Static composition that checkpoints when either inner policy requests it.
///
/// If both policies fire on the same frame, [`SyncMode::All`] wins over
/// [`SyncMode::Data`]. A successful checkpoint resets both policies so their
/// windows stay aligned to the same durable boundary.
#[derive(Debug, Clone, Copy)]
pub struct AnySync<A, B> {
    first: A,
    second: B,
}

impl<A, B> AnySync<A, B> {
    /// Creates a static OR-composition of two policies.
    pub const fn new(first: A, second: B) -> Self {
        Self { first, second }
    }
}

impl<A: SyncPolicy, B: SyncPolicy> SyncPolicy for AnySync<A, B> {
    #[inline]
    fn observe(&mut self, info: SyncInfo) -> Option<SyncMode> {
        match (self.first.observe(info), self.second.observe(info)) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(mode), None) | (None, Some(mode)) => Some(mode),
            (None, None) => None,
        }
    }

    #[inline]
    fn on_synced(&mut self, durable_watermark: u64) {
        self.first.on_synced(durable_watermark);
        self.second.on_synced(durable_watermark);
    }
}

/// Fluent static composition for sync policies.
pub trait SyncPolicyExt: SyncPolicy + Sized {
    /// Requests a checkpoint whenever either policy requests one.
    #[must_use]
    fn or<P: SyncPolicy>(self, other: P) -> AnySync<Self, P> {
        AnySync::new(self, other)
    }
}

impl<P: SyncPolicy> SyncPolicyExt for P {}

/// Installed policy state. Stored only in policy-enabled writers.
#[derive(Debug, Clone)]
pub struct Syncing<P> {
    pub(crate) policy: P,
    pub(crate) durable_watermark: Option<u64>,
}

impl<P> Syncing<P> {
    pub(crate) fn new(policy: P) -> Self {
        Self {
            policy,
            durable_watermark: None,
        }
    }
}

/// Internal bridge between a writer's statically selected sync state and its
/// sink. Public only because it appears in generic method bounds; it is not a
/// user extension point.
#[doc(hidden)]
pub trait SyncPolicyBackend<W: Write>: Send {
    fn after_frame(&mut self, sink: &mut W, receipt: FrameReceipt) -> Result<()>;
}

impl<W: Write> SyncPolicyBackend<W> for NoSync {
    #[inline(always)]
    fn after_frame(&mut self, _sink: &mut W, _receipt: FrameReceipt) -> Result<()> {
        Ok(())
    }
}

impl<W: Durable, P: SyncPolicy> SyncPolicyBackend<W> for Syncing<P> {
    #[inline]
    fn after_frame(&mut self, sink: &mut W, receipt: FrameReceipt) -> Result<()> {
        let info = SyncInfo {
            frame_start: receipt.frame_start,
            wire_len: receipt.wire_len,
            frame_end: receipt.end(),
        };
        if let Some(mode) = self.policy.observe(info) {
            self.checkpoint(sink, mode, receipt.end(), Some(receipt))?;
        }
        Ok(())
    }
}

impl<P: SyncPolicy> Syncing<P> {
    pub(crate) fn checkpoint<W: Durable>(
        &mut self,
        sink: &mut W,
        mode: SyncMode,
        attempted_watermark: u64,
        frame: Option<FrameReceipt>,
    ) -> Result<u64> {
        let result = match mode {
            SyncMode::Data => sink.sync_data(),
            SyncMode::All => sink.sync_all(),
        };
        if let Err(source) = result {
            return Err(Error::durability_failed(
                mode,
                attempted_watermark,
                self.durable_watermark,
                frame.map(|receipt| receipt.frame_start),
                frame.map(|receipt| receipt.wire_len),
                source,
            ));
        }
        self.durable_watermark = Some(attempted_watermark);
        self.policy.on_synced(attempted_watermark);
        Ok(attempted_watermark)
    }
}
