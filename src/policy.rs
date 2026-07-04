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
//! capacity exceeds the configured reclaim baseline (at or below the baseline there
//! is nothing to reclaim, so policy state does not churn at steady state). Policies
//! apply only to buffers the library owns: the writer's simple mode (`write()`) and
//! the reader's internal buffer, never to caller-owned builders (`write_finished()`).

use std::time::{Duration, Instant};

/// Default baseline capacity restored when a memory policy triggers a reclaim.
pub(crate) const DEFAULT_RECLAIM_CAPACITY: usize = 16 * 1024;

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
pub struct AdaptiveWatermarkPolicy {
    /// Trigger when `current_capacity >= last_message_size * size_ratio_threshold`.
    pub size_ratio_threshold: usize,
    /// How many qualifying messages to observe before resetting.
    pub messages_to_wait: u32,
    /// Optional cooldown; if elapsed since the last overprovision event, triggers reset.
    pub cooldown: Option<Duration>,
    // Internal state
    messages_since_over: u32,
    last_over_seen_at: Option<Instant>,
}

impl AdaptiveWatermarkPolicy {
    /// Creates a policy that triggers once `messages_to_wait` consecutive
    /// messages have each observed `capacity >= message_size * size_ratio_threshold`.
    ///
    /// (The state fields are private, so this constructor — or `Default` — is
    /// the way to build one.)
    pub fn new(size_ratio_threshold: usize, messages_to_wait: u32) -> Self {
        Self {
            size_ratio_threshold,
            messages_to_wait,
            cooldown: None,
            messages_since_over: 0,
            last_over_seen_at: None,
        }
    }

    /// Adds a time-based trigger: reset once the over-provisioned signal has
    /// persisted for `cooldown`, even if the message count has not been reached.
    pub fn with_cooldown(mut self, cooldown: Duration) -> Self {
        self.cooldown = Some(cooldown);
        self
    }
}

impl Default for AdaptiveWatermarkPolicy {
    fn default() -> Self {
        Self::new(4, 5)
    }
}

impl MemoryPolicy for AdaptiveWatermarkPolicy {
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
        // NOTE: direct clock read; to be routed through an injected `Clock`
        // trait in the v0.3 hardening pass (determinism seam — see
        // docs/planning/NEXT_STEPS.md item B8).
        let now = self.cooldown.as_ref().map(|_| Instant::now());

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
            (Some(cd), Some(t0), Some(t1)) => t1.duration_since(t0) >= cd,
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
    // Internal state
    large_event_seen: bool,
    small_since_large: u32,
}

impl SizeThresholdPolicy {
    pub fn new(grow_above_bytes: usize, shrink_below_bytes: usize, messages_to_wait: u32) -> Self {
        Self {
            grow_above_bytes,
            shrink_below_bytes,
            messages_to_wait,
            large_event_seen: false,
            small_since_large: 0,
        }
    }
}

impl Default for SizeThresholdPolicy {
    fn default() -> Self {
        Self::new(1 << 20, 1 << 10, 8) // 1 MiB grow threshold, 1 KiB shrink threshold, 8 messages
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut policy =
            AdaptiveWatermarkPolicy::new(10, 100).with_cooldown(Duration::from_millis(50));

        let capacity = 1000;
        let small_msg = 50;

        // First trigger starts the clock
        assert_eq!(policy.should_reset(small_msg, capacity), None);
        assert!(policy.last_over_seen_at.is_some());

        // Immediate follow-up: no reset
        assert_eq!(policy.should_reset(small_msg, capacity), None);

        // Wait for cooldown
        std::thread::sleep(Duration::from_millis(60));

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
        let mut policy =
            AdaptiveWatermarkPolicy::new(10, 100).with_cooldown(Duration::from_millis(50));
        let capacity = 1000;
        let small_msg = 50;

        // Arm and fire once via time.
        assert_eq!(policy.should_reset(small_msg, capacity), None);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(
            policy.should_reset(small_msg, capacity),
            Some(ReclamationReason::TimeCooldown)
        );

        // Simulate an idle / gated-out period longer than the cooldown.
        std::thread::sleep(Duration::from_millis(60));

        // The next over-provisioned observation must START a fresh window,
        // not fire against the stale timestamp.
        assert_eq!(policy.should_reset(small_msg, capacity), None);

        // ...and fires again only after the cooldown elapses from that restart.
        std::thread::sleep(Duration::from_millis(60));
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
