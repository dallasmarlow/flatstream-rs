//! Memory reclamation policies for `StreamWriter` and `StreamReader`.
//!
//! # The High-Water Mark Problem
//!
//! Standard `FlatBufferBuilder`s (and the reader's internal buffer) grow to
//! accommodate the largest message seen so far. This capacity is retained
//! indefinitely to avoid re-allocation overhead. In long-running services handling
//! bursty workloads (e.g., rare large config dumps amidst small telemetry events),
//! this can lead to "memory bloat" where a service holds onto peak memory usage
//! long after the burst has passed.
//!
//! # The Solution: Adaptive Policies
//!
//! This module provides policies to detect when a buffer is "over-provisioned"
//! relative to the current workload. When a policy triggers, the writer resets its
//! builder (the reader its buffer), freeing the large allocation and replacing it
//! with a smaller baseline capacity. This trades a small amount of CPU (allocator
//! churn) for significant memory savings.
//!
//! Install a policy with `StreamWriter::with_memory_policy` /
//! `StreamReader::with_memory_policy`. The policy is consulted once per message —
//! a single predictable branch when none is installed — and only while the current
//! capacity exceeds the policy's baseline capacity (at or below the baseline there
//! is nothing to reclaim, so policy state does not churn at steady state). Policies
//! apply only to buffers the library owns: the writer's simple mode (`write()`) and
//! the reader's internal buffer, never to caller-owned builders (`write_finished()`).

use std::time::{Duration, Instant};

/// Default baseline capacity restored when a memory policy triggers a reclaim:
/// 16 KiB — large enough that typical telemetry-sized messages never regrow the
/// buffer after a reclaim, small enough to matter against megabyte high-water marks.
pub const DEFAULT_BASELINE_CAPACITY: usize = 16 * 1024;

/// Reason for a reclamation (reset) action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReclamationReason {
    /// Policy triggered by a message-count-based heuristic.
    MessageCount,
    /// Policy triggered by a time-based cooldown.
    TimeCooldown,
    /// Policy triggered by a hard size limit.
    SizeThreshold,
}

/// Information about a reclamation event.
///
/// `capacity_after` is the configured baseline the buffer is reclaimed *to*.
/// On the writer the rebuild happens immediately; on the reader the shrink is
/// scheduled and applied at the start of the next read (so the payload just
/// returned is never invalidated) — i.e., on the reader this is the *scheduled*
/// post-reclaim capacity.
#[derive(Debug, Clone, Copy)]
pub struct ReclamationInfo {
    pub reason: ReclamationReason,
    pub last_message_size: usize,
    pub capacity_before: usize,
    pub capacity_after: usize,
}

/// A trait that defines a stateful policy for when to reset an internal buffer.
///
/// Requires `Send` so that writers and readers carrying a policy can move
/// across threads (the common pattern: construct on the main thread, hand off
/// to a dedicated journaling thread). Policies are exclusively owned (`&mut
/// self`), so `Sync` is not required.
pub trait MemoryPolicy: Send {
    /// Called after each successful message: a `write()` on the writer, a
    /// successful read on the reader.
    ///
    /// Arguments
    /// - `last_message_size`: The size in bytes of the message just written/read.
    /// - `current_capacity`: The current capacity of the owned buffer (the
    ///   writer's internal builder, or the reader's internal buffer).
    ///
    /// Returns
    /// - `Some(ReclamationReason)` if the buffer should be reset, otherwise `None`.
    fn should_reset(
        &mut self,
        last_message_size: usize,
        current_capacity: usize,
    ) -> Option<ReclamationReason>;

    /// Optional hook called after a reset occurs.
    /// Useful for logging or metrics without overhead when unused.
    #[inline(always)]
    fn on_reclaim(&mut self, _info: &ReclamationInfo) {}

    /// The capacity the buffer is reclaimed *to* when this policy fires.
    ///
    /// The baseline is policy configuration: a policy decides both *when* to
    /// reclaim and *what* to shrink back to. Defaults to
    /// [`DEFAULT_BASELINE_CAPACITY`] (16 KiB). The writer and reader read this
    /// once at installation and cache it — the steady-state gate ("consult the
    /// policy only while capacity exceeds the baseline") therefore costs a
    /// plain integer compare, never a virtual call.
    ///
    /// Wrapper/observer policies that delegate to an inner policy must forward
    /// this method, or the inner policy's configured baseline is lost.
    fn baseline_capacity(&self) -> usize {
        DEFAULT_BASELINE_CAPACITY
    }
}

/// A policy that never triggers a reset.
///
/// Useful as a benchmark baseline and as the inner policy for observer/wrapper
/// compositions. Note that *not installing a policy at all* is cheaper still
/// (no boxed call); this type exists for cases where a policy slot must be
/// filled but should do nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpPolicy;

impl MemoryPolicy for NoOpPolicy {
    #[inline(always)]
    fn should_reset(
        &mut self,
        _last_message_size: usize,
        _current_capacity: usize,
    ) -> Option<ReclamationReason> {
        None
    }
}

/// A monotonic time source for time-based policy triggers.
///
/// This is a determinism seam: production code uses the default
/// [`MonotonicClock`]; tests and simulators inject a clock they control, so
/// time-dependent behavior is reproducible without sleeping. Implementations
/// report time elapsed since their own origin — only the difference between
/// two `now()` readings is meaningful — and must never run backwards.
/// (`Send` for the same reason as [`MemoryPolicy`]: policies move across
/// threads with their writer/reader, and they carry their clock.)
pub trait Clock: Send {
    /// Time elapsed since the clock's origin.
    fn now(&self) -> Duration;
}

/// The production [`Clock`]: monotonic time from a stored [`Instant`] origin.
#[derive(Debug, Clone, Copy)]
pub struct MonotonicClock {
    origin: Instant,
}

impl MonotonicClock {
    /// Creates a clock whose origin is "now".
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MonotonicClock {
    #[inline]
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// An adaptive, capacity-aware policy with hysteresis to avoid thrashing.
///
/// # How it works
///
/// This policy implements a **hysteresis loop** to prevent rapid allocation/deallocation
/// cycles ("thrashing") when message sizes fluctuate.
///
/// 1. **Detection**: It monitors the ratio between the builder's current capacity and
///    the size of the messages being written.
/// 2. **Signal**: If `capacity > message_size * size_ratio_threshold`, the builder is
///    considered "over-provisioned."
/// 3. **Stability**: It requires this signal to persist for `messages_to_wait` consecutive
///    writes (or a time duration) before triggering a reset. This ensures we don't
///    shrink immediately after a large message, only to grow again for the next one.
#[derive(Debug, Clone)]
pub struct AdaptiveWatermarkPolicy<C: Clock = MonotonicClock> {
    /// Trigger when `current_capacity >= last_message_size * size_ratio_threshold`.
    pub size_ratio_threshold: usize,
    /// How many qualifying messages to observe before resetting.
    pub messages_to_wait: u32,
    /// Optional cooldown; if elapsed since the last overprovision event, triggers reset.
    pub cooldown: Option<Duration>,
    /// The capacity the buffer is reclaimed to when this policy fires.
    pub baseline_capacity: usize,
    // Internal state
    messages_since_over: u32,
    last_over_seen_at: Option<Duration>,
    clock: C,
}

impl AdaptiveWatermarkPolicy {
    /// Default over-provisioning ratio: the buffer counts as over-provisioned
    /// while `capacity >= message_size * 4`. A factor of 4 tolerates the slack
    /// a doubling-growth builder normally carries, so only genuinely stale
    /// burst capacity registers as reclaimable.
    pub const DEFAULT_SIZE_RATIO_THRESHOLD: usize = 4;

    /// Default hysteresis length: 5 consecutive over-provisioned messages must
    /// be observed before firing, so a lone small message after a burst cannot
    /// trigger a shrink that the next large message would immediately undo.
    pub const DEFAULT_MESSAGES_TO_WAIT: u32 = 5;

    /// Creates a policy that triggers once `messages_to_wait` consecutive
    /// messages have each observed `capacity >= message_size * size_ratio_threshold`.
    ///
    /// (The state fields are private, so this constructor — or `Default`, or
    /// [`with_clock`](Self::with_clock) — is the way to build one.)
    pub fn new(size_ratio_threshold: usize, messages_to_wait: u32) -> Self {
        Self::with_clock(
            size_ratio_threshold,
            messages_to_wait,
            MonotonicClock::new(),
        )
    }
}

impl<C: Clock> AdaptiveWatermarkPolicy<C> {
    /// Like [`new`](AdaptiveWatermarkPolicy::new), but with an injected
    /// [`Clock`] — the determinism seam for tests and simulators, which drive
    /// time explicitly instead of sleeping. The clock is a generic default
    /// parameter, so the production path pays no dispatch for the seam.
    pub fn with_clock(size_ratio_threshold: usize, messages_to_wait: u32, clock: C) -> Self {
        Self {
            size_ratio_threshold,
            messages_to_wait,
            cooldown: None,
            baseline_capacity: DEFAULT_BASELINE_CAPACITY,
            messages_since_over: 0,
            last_over_seen_at: None,
            clock,
        }
    }

    /// Adds a time-based trigger: reset once the over-provisioned signal has
    /// persisted for `cooldown`, even if the message count has not been reached.
    pub fn with_cooldown(mut self, cooldown: Duration) -> Self {
        self.cooldown = Some(cooldown);
        self
    }

    /// Sets the capacity the buffer is reclaimed to when this policy fires
    /// (default 16 KiB).
    pub fn with_baseline(mut self, bytes: usize) -> Self {
        self.baseline_capacity = bytes;
        self
    }
}

impl Default for AdaptiveWatermarkPolicy {
    fn default() -> Self {
        Self::new(
            Self::DEFAULT_SIZE_RATIO_THRESHOLD,
            Self::DEFAULT_MESSAGES_TO_WAIT,
        )
    }
}

impl<C: Clock> MemoryPolicy for AdaptiveWatermarkPolicy<C> {
    fn should_reset(
        &mut self,
        last_message_size: usize,
        current_capacity: usize,
    ) -> Option<ReclamationReason> {
        if last_message_size == 0 {
            // Avoid division-by-zero style logic; treat as no signal
            self.messages_since_over = 0;
            self.last_over_seen_at = None;
            return None;
        }

        let overprovisioned =
            current_capacity >= last_message_size.saturating_mul(self.size_ratio_threshold);
        // Clock read at most once per consult, and only when a cooldown is
        // configured: the count-only configuration performs no clock reads.
        let now = self.cooldown.as_ref().map(|_| self.clock.now());

        if overprovisioned {
            self.messages_since_over = self.messages_since_over.saturating_add(1);
            if self.last_over_seen_at.is_none() {
                self.last_over_seen_at = now;
            }
        } else {
            // Reset counters when signal disappears to avoid thrashing
            self.messages_since_over = 0;
            self.last_over_seen_at = None;
        }

        let count_ok = overprovisioned && self.messages_since_over >= self.messages_to_wait;
        let time_ok = match (self.cooldown, self.last_over_seen_at, now) {
            (Some(cd), Some(t0), Some(t1)) => t1.saturating_sub(t0) >= cd,
            _ => false,
        };

        if count_ok || time_ok {
            self.messages_since_over = 0;
            // Clear the timer entirely: after a fire the buffer is reclaimed to
            // the baseline, and the caller's gate stops consulting this policy
            // until capacity next exceeds it. A timestamp left here would
            // survive that gap and trigger an immediate, premature
            // TimeCooldown on re-entry; the window must restart at the next
            // over-provisioned observation instead.
            self.last_over_seen_at = None;
            return Some(if time_ok {
                ReclamationReason::TimeCooldown
            } else {
                ReclamationReason::MessageCount
            });
        }

        None
    }

    fn baseline_capacity(&self) -> usize {
        self.baseline_capacity
    }
}

/// A simple threshold policy that resets after a sustained period of
/// smaller messages following a large one. This is a simplified variant
/// of the adaptive policy with explicit thresholds.
#[derive(Debug, Clone, Copy)]
pub struct SizeThresholdPolicy {
    /// Consider any message strictly greater than this value as a "large" event.
    pub grow_above_bytes: usize,
    /// Consider any message strictly less than this value as a "small" event.
    pub shrink_below_bytes: usize,
    /// How many consecutive small messages to observe before resetting.
    pub messages_to_wait: u32,
    /// The capacity the buffer is reclaimed to when this policy fires.
    pub baseline_capacity: usize,
    // Internal state
    large_event_seen: bool,
    small_since_large: u32,
}

impl SizeThresholdPolicy {
    /// Default "large event" threshold: a message above 1 MiB arms the policy.
    pub const DEFAULT_GROW_ABOVE_BYTES: usize = 1 << 20;

    /// Default "small message" threshold: messages below 1 KiB count toward
    /// the consecutive-small run once the policy is armed.
    pub const DEFAULT_SHRINK_BELOW_BYTES: usize = 1 << 10;

    /// Default consecutive-small count before firing.
    pub const DEFAULT_MESSAGES_TO_WAIT: u32 = 8;

    pub fn new(grow_above_bytes: usize, shrink_below_bytes: usize, messages_to_wait: u32) -> Self {
        Self {
            grow_above_bytes,
            shrink_below_bytes,
            messages_to_wait,
            baseline_capacity: DEFAULT_BASELINE_CAPACITY,
            large_event_seen: false,
            small_since_large: 0,
        }
    }

    /// Sets the capacity the buffer is reclaimed to when this policy fires
    /// (default 16 KiB).
    pub fn with_baseline(mut self, bytes: usize) -> Self {
        self.baseline_capacity = bytes;
        self
    }
}

impl Default for SizeThresholdPolicy {
    fn default() -> Self {
        Self::new(
            Self::DEFAULT_GROW_ABOVE_BYTES,
            Self::DEFAULT_SHRINK_BELOW_BYTES,
            Self::DEFAULT_MESSAGES_TO_WAIT,
        )
    }
}

impl MemoryPolicy for SizeThresholdPolicy {
    fn should_reset(
        &mut self,
        last_message_size: usize,
        _current_capacity: usize,
    ) -> Option<ReclamationReason> {
        // A large *message* marks the event (capacity cannot be the marker: it
        // only drops via the reset this policy triggers, so gating on capacity
        // would make the policy unable to ever fire).
        if last_message_size > self.grow_above_bytes {
            self.large_event_seen = true;
            self.small_since_large = 0;
            return None;
        }

        if !self.large_event_seen {
            return None;
        }

        if last_message_size < self.shrink_below_bytes {
            self.small_since_large = self.small_since_large.saturating_add(1);
            if self.small_since_large >= self.messages_to_wait {
                self.large_event_seen = false;
                self.small_since_large = 0;
                return Some(ReclamationReason::SizeThreshold);
            }
        } else {
            // A mid-sized message interrupts the consecutive-small run.
            self.small_since_large = 0;
        }

        None
    }

    fn baseline_capacity(&self) -> usize {
        self.baseline_capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// Simulator-controlled clock: tests advance time explicitly, so the
    /// time-based assertions below are deterministic and sleep-free.
    #[derive(Clone, Default)]
    struct TestClock(Arc<AtomicU64>); // elapsed milliseconds

    impl TestClock {
        fn advance_ms(&self, ms: u64) {
            self.0.fetch_add(ms, Ordering::Relaxed);
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::Relaxed))
        }
    }

    #[test]
    fn monotonic_clock_is_monotonic() {
        let clock = MonotonicClock::new();
        let first = clock.now();
        assert!(clock.now() >= first);
    }

    #[test]
    fn test_noop_policy() {
        let mut policy = NoOpPolicy;
        assert_eq!(policy.should_reset(100, 1000), None);
        assert_eq!(policy.should_reset(1000, 1000), None);
    }

    #[test]
    fn test_adaptive_hysteresis() {
        let mut policy = AdaptiveWatermarkPolicy::new(10, 3);

        let capacity = 1000;

        // 1. Message too large relative to capacity (150 > 1000/10), no shrink signal
        assert_eq!(policy.should_reset(150, capacity), None);
        assert_eq!(policy.messages_since_over, 0);

        // 2. Message small enough (90 <= 1000/10), signal starts
        assert_eq!(policy.should_reset(90, capacity), None);
        assert_eq!(policy.messages_since_over, 1);

        // 3. Another small message
        assert_eq!(policy.should_reset(80, capacity), None);
        assert_eq!(policy.messages_since_over, 2);

        // 4. Large message interrupts sequence
        assert_eq!(policy.should_reset(200, capacity), None);
        assert_eq!(policy.messages_since_over, 0);

        // 5. Sequence completes
        assert_eq!(policy.should_reset(50, capacity), None); // 1
        assert_eq!(policy.should_reset(50, capacity), None); // 2
        assert_eq!(
            policy.should_reset(50, capacity),
            Some(ReclamationReason::MessageCount)
        ); // 3 -> Reset

        // After reset returns Some, counters usually reset by the caller re-init or manually,
        // but the policy internal state also resets.
        assert_eq!(policy.messages_since_over, 0);
    }

    #[test]
    fn test_adaptive_cooldown() {
        // High count so the trigger relies on time alone
        let clock = TestClock::default();
        let mut policy = AdaptiveWatermarkPolicy::with_clock(10, 100, clock.clone())
            .with_cooldown(Duration::from_millis(50));

        let capacity = 1000;
        let small_msg = 50;

        // First trigger starts the clock
        assert_eq!(policy.should_reset(small_msg, capacity), None);
        assert!(policy.last_over_seen_at.is_some());

        // Immediate follow-up: no reset
        assert_eq!(policy.should_reset(small_msg, capacity), None);

        // Advance past the cooldown
        clock.advance_ms(60);

        // Next write triggers reset via time
        assert_eq!(
            policy.should_reset(small_msg, capacity),
            Some(ReclamationReason::TimeCooldown)
        );
        assert_eq!(policy.messages_since_over, 0);
    }

    #[test]
    fn test_adaptive_cooldown_timer_resets_after_fire() {
        // Regression: after the policy fires, its cooldown timer must be
        // cleared. A stale timestamp would otherwise survive periods where the
        // caller does not consult the policy at all (capacity at the reclaim
        // baseline) and cause an immediate, premature TimeCooldown on the next
        // over-provisioned observation.
        let clock = TestClock::default();
        let mut policy = AdaptiveWatermarkPolicy::with_clock(10, 100, clock.clone())
            .with_cooldown(Duration::from_millis(50));
        let capacity = 1000;
        let small_msg = 50;

        // Arm and fire once via time.
        assert_eq!(policy.should_reset(small_msg, capacity), None);
        clock.advance_ms(60);
        assert_eq!(
            policy.should_reset(small_msg, capacity),
            Some(ReclamationReason::TimeCooldown)
        );

        // Simulate an idle / gated-out period longer than the cooldown.
        clock.advance_ms(60);

        // The next over-provisioned observation must START a fresh window,
        // not fire against the stale timestamp.
        assert_eq!(policy.should_reset(small_msg, capacity), None);

        // ...and fires again only after the cooldown elapses from that restart.
        clock.advance_ms(60);
        assert_eq!(
            policy.should_reset(small_msg, capacity),
            Some(ReclamationReason::TimeCooldown)
        );
    }

    #[test]
    fn test_size_threshold_policy() {
        // Large above 1000 bytes, small below 200 bytes, wait for 2 small messages.
        let mut policy = SizeThresholdPolicy::new(1000, 200, 2);
        let capacity = 8192; // capacity is irrelevant to this policy

        // Small messages before any large event: no signal, ever.
        assert_eq!(policy.should_reset(100, capacity), None);
        assert_eq!(policy.should_reset(100, capacity), None);
        assert_eq!(policy.should_reset(100, capacity), None);

        // A large message marks the event.
        assert_eq!(policy.should_reset(5000, capacity), None);

        // A mid-sized message interrupts the consecutive-small run
        // (resets the counter) but does not clear the large event.
        assert_eq!(policy.should_reset(100, capacity), None); // small: 1
        assert_eq!(policy.should_reset(500, capacity), None); // mid: run resets
        assert_eq!(policy.should_reset(100, capacity), None); // small: 1 again

        // The second consecutive small message triggers the reset.
        assert_eq!(
            policy.should_reset(100, capacity),
            Some(ReclamationReason::SizeThreshold)
        ); // 2 -> reset

        // State is cleared: small messages alone do not re-trigger.
        assert_eq!(policy.should_reset(100, capacity), None);
        assert_eq!(policy.should_reset(100, capacity), None);
        assert_eq!(policy.should_reset(100, capacity), None);

        // A new large event re-arms the policy.
        assert_eq!(policy.should_reset(5000, capacity), None);
        assert_eq!(policy.should_reset(100, capacity), None); // 1
        assert_eq!(
            policy.should_reset(100, capacity),
            Some(ReclamationReason::SizeThreshold)
        ); // 2 -> reset
    }
}
