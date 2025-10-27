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

/// A trait that defines a stateful policy for when to reset the internal builder.
///
/// The policy operates on message-size history. Builder capacity is not observable,
/// so policies should use observed sizes and optional time-based signals.
pub trait MemoryPolicy {
    /// Called after each successful write.
    ///
    /// Arguments
    /// - `last_message_size`: The size in bytes of the message just written.
    ///
    /// Returns
    /// - `Some(ReclamationReason)` if the builder should be reset, otherwise `None`.
    fn should_reset(&mut self, last_message_size: usize) -> Option<ReclamationReason>;
}

// Allow boxed trait objects to be used where a `MemoryPolicy` is expected.
impl<T: MemoryPolicy + ?Sized> MemoryPolicy for Box<T> {
    #[inline]
    fn should_reset(&mut self, last_message_size: usize) -> Option<ReclamationReason> {
        (**self).should_reset(last_message_size)
    }
}

/// A zero-cost policy that never triggers a reset.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpPolicy;

impl MemoryPolicy for NoOpPolicy {
    #[inline(always)]
    fn should_reset(&mut self, _last_message_size: usize) -> Option<ReclamationReason> {
        None
    }
}

use core::cmp;
use std::time::{Duration, Instant};

/// An adaptive, hysteresis-based policy that reduces oscillation.
///
/// It tracks the largest recently observed message (high-water mark) and waits
/// for a configurable number of smaller messages and/or a cooldown period before
/// triggering a reset. Upon reset, the baseline is updated to the recent smaller size
/// to further dampen oscillations.
#[derive(Debug, Clone)]
pub struct AdaptiveWatermarkPolicy {
    pub high_watermark_bytes: usize,
    pub messages_since_high: u32,
    /// How many messages smaller than the watermark to observe before resetting.
    pub messages_to_wait: u32,
    pub last_high_seen_at: Option<Instant>,
    /// Optional cooldown; if elapsed since the last high-water event, triggers reset.
    pub cooldown: Option<Duration>,
}

impl Default for AdaptiveWatermarkPolicy {
    fn default() -> Self {
        Self {
            high_watermark_bytes: 0,
            messages_since_high: 0,
            messages_to_wait: 5,
            last_high_seen_at: None,
            cooldown: None,
        }
    }
}

impl MemoryPolicy for AdaptiveWatermarkPolicy {
    fn should_reset(&mut self, last_message_size: usize) -> Option<ReclamationReason> {
        let now = if self.cooldown.is_some() {
            Some(Instant::now())
        } else {
            None
        };

        if last_message_size > self.high_watermark_bytes {
            // New high-water mark. Reset counters.
            self.high_watermark_bytes = last_message_size;
            self.messages_since_high = 0;
            self.last_high_seen_at = now;
            return None;
        }

        // Smaller than watermark: increment counter
        self.messages_since_high = self.messages_since_high.saturating_add(1);

        let count_ok = self.messages_since_high >= self.messages_to_wait;
        let time_ok = match (self.cooldown, self.last_high_seen_at, now) {
            (Some(cd), Some(t0), Some(t1)) => t1.duration_since(t0) >= cd,
            _ => false,
        };

        if count_ok || time_ok {
            // Reset baseline to the recent smaller size to reduce oscillation
            self.high_watermark_bytes = cmp::max(self.high_watermark_bytes / 2, last_message_size);
            self.messages_since_high = 0;
            self.last_high_seen_at = now;
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
    fn should_reset(&mut self, last_message_size: usize) -> Option<ReclamationReason> {
        if last_message_size > self.grow_above_bytes {
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


