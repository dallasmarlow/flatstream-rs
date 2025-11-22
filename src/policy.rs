//! Memory reclamation policies for `StreamWriter`.
//!
//! This module defines a composable `MemoryPolicy` trait and several
//! implementations to control when the simple writer path should reset
//! its internal `FlatBufferBuilder` to reclaim memory after bursts of
//! large messages.

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
#[derive(Debug, Clone, Copy)]
pub struct ReclamationInfo {
    pub reason: ReclamationReason,
    pub last_message_size: usize,
    pub capacity_before: usize,
    pub capacity_after: usize,
}

/// A trait that defines a stateful policy for when to reset the internal builder.
pub trait MemoryPolicy {
    /// Called after each successful write.
    ///
    /// Arguments
    /// - `last_message_size`: The size in bytes of the message just written.
    /// - `current_capacity`: The current capacity of the internal builder.
    ///
    /// Returns
    /// - `Some(ReclamationReason)` if the builder should be reset, otherwise `None`.
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

/// A zero-cost policy that never triggers a reset.
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

use std::time::{Duration, Instant};

/// An adaptive, capacity-aware policy with hysteresis to avoid thrashing.
#[derive(Debug, Clone)]
pub struct AdaptiveWatermarkPolicy {
    /// Trigger when `current_capacity >= last_message_size * shrink_multiple`.
    pub shrink_multiple: usize,
    /// How many qualifying messages to observe before resetting.
    pub messages_to_wait: u32,
    /// Optional cooldown; if elapsed since the last overprovision event, triggers reset.
    pub cooldown: Option<Duration>,
    // Internal state
    messages_since_over: u32,
    last_over_seen_at: Option<Instant>,
}

impl Default for AdaptiveWatermarkPolicy {
    fn default() -> Self {
        Self {
            shrink_multiple: 4,
            messages_to_wait: 5,
            cooldown: None,
            messages_since_over: 0,
            last_over_seen_at: None,
        }
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
            current_capacity >= last_message_size.saturating_mul(self.shrink_multiple);
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
            self.last_over_seen_at = now;
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
        current_capacity: usize,
    ) -> Option<ReclamationReason> {
        if current_capacity > self.grow_above_bytes {
            self.large_event_seen = true;
            self.small_since_large = 0;
            return None;
        }

        if self.large_event_seen && last_message_size < self.shrink_below_bytes {
            self.small_since_large = self.small_since_large.saturating_add(1);
            if self.small_since_large >= self.messages_to_wait {
                self.large_event_seen = false;
                self.small_since_large = 0;
                return Some(ReclamationReason::SizeThreshold);
            }
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
        let mut policy = AdaptiveWatermarkPolicy {
            shrink_multiple: 10,
            messages_to_wait: 3,
            cooldown: None,
            messages_since_over: 0,
            last_over_seen_at: None,
        };

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
        let mut policy = AdaptiveWatermarkPolicy {
            shrink_multiple: 10,
            messages_to_wait: 100, // High count, rely on time
            cooldown: Some(Duration::from_millis(50)),
            messages_since_over: 0,
            last_over_seen_at: None,
        };

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
}
