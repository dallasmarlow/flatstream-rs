# Design Document: An Adaptive Memory Reclamation Policy for `StreamWriter`

## 1.0 Executive Summary: A Production-Grade Memory Management Strategy

This document presents the definitive architectural plan for integrating an optional, adaptive memory reclamation policy into the `flatstream-rs` library's simple writer path. The analysis synthesizes the user's prior research on composable policy traits with a formal, hysteresis-based control model to solve the "high-water mark" memory bloat problem in the existing `StreamWriter`.

### 1.1 The Problem: The "High-Water Mark" Dilemma in High-Performance Buffering

The current `StreamWriter` maintains an internal `flatbuffers::FlatBufferBuilder` for the simple write path. This builder’s internal buffer grows to accommodate large messages but does not shrink on `reset()`. This behavior, while a common optimization to avoid frequent reallocations, creates a conflict between allocation performance and long-term memory footprint. A single burst of large messages can cause the builder’s backing buffer to grow and remain large, leading to persistent memory bloat and risk of OOM in long-running services. This must be addressed for production-grade robustness.

### 1.2 The Architectural Choice: A Composable, In-Place Policy

While advanced solutions like global buffer pools exist, they introduce significant architectural complexity. A more targeted, less intrusive solution is to integrate the reclamation logic directly into the `StreamWriter`. This proposal adopts a strategy-coupled design, perfectly paralleling the existing and proven `Checksum` and `Validator` traits.

This approach introduces a new `MemoryPolicy` trait that allows users to inject a pluggable, stateful strategy for deciding when to reclaim buffer memory. This design is:

-   **Localized:** It solves the problem precisely where it occurs without broad architectural changes.
-   **Composable:** It aligns perfectly with the library's core philosophy of composing orthogonal behaviors.
-   **Optional:** It provides a `NoOpPolicy` that guarantees a true zero-cost abstraction, ensuring no performance penalty for users who do not opt-in.

### 1.3 The Recommendation: A Unified Implementation Plan

This report formally adopts the design of a generic `MemoryPolicy` trait, with a primary, sophisticated implementation based on a hysteresis control loop. The implementation will consist of:

1.  Creating a new `src/policy.rs` module to house the `MemoryPolicy` trait and its implementations.
2.  Refactoring `StreamWriter` in `src/writer.rs` to be generic over a `MemoryPolicy` and to integrate the reclamation logic.
3.  Introducing a `StreamWriterBuilder` to provide a fluent, opt-in API for configuring the new feature.

This approach maintains 100% backward compatibility, strengthens the library's core architectural patterns, and solves a critical production-readiness gap, making the "simple" writer path robust for a wider range of demanding, long-running workloads.

## 2.0 Analysis of the High-Water Mark Problem

The root cause is the builder’s backing buffer growth behavior. `FlatBufferBuilder::reset()` prepares the builder for reuse but does not reduce the capacity of its internal buffer. After a burst of large messages, the builder can reach a high-water capacity and keep it. Continuously calling a capacity-shrinking operation after each write would cause thrashing if another large message arrives soon after, so reclamation must be judicious, based on workload signals.

### 2.1 The "Simple Reset" is the Correct Solution

The recommended reclamation action is a "simple reset": drop the existing `FlatBufferBuilder` and replace it with a new builder initialized to a configurable default capacity, for example:

```rust
self.builder = flatbuffers::FlatBufferBuilder::with_capacity(self.default_buffer_capacity);
```

This approach is preferred over a more complex pool for two reasons:

1.  **Safety and Simplicity:** It relies on Rust’s ownership for safe reclamation; no risk of aliasing or use-after-free.
2.  **Sufficient Performance:** With modern global allocators (e.g., `mimalloc`, `jemalloc`), dealloc/alloc cycles are often satisfied from thread-local caches and are very fast. The marginal gains from an application-level pool rarely justify the complexity for this use case.

## 3.0 The Architectural Imperative: A Pluggable Policy Trait

The `flatstream-rs` architecture has already established a clear and successful pattern for handling optional, cross-cutting concerns: the orthogonal, composable trait.

-   **The `Checksum` Trait:** Provides a pluggable strategy for data integrity.
-   **The `Validator` Trait:** Provides a pluggable strategy for data safety.
-   **The "No-Op" Precedent:** The existence of `NoChecksum` and `NoValidator` establishes the library's philosophical guarantee of **zero-cost abstraction**. When a generic component is monomorphized with a no-op implementation, the compiler can completely optimize away the check, resulting in zero runtime overhead.

Any solution to the memory reclamation problem **must** adhere to this established precedent. The correct architectural path is to introduce a new `MemoryPolicy` trait. This approach strengthens the library's core design, turning "memory management" into another independent, composable axis of behavior that users can select and configure, just like integrity and safety.

## 4.0 The Recommended Architecture: The Hysteresis-Based Policy

The proposed solution is to implement the composable `MemoryPolicy` framework and provide a sophisticated, state-of-the-art implementation based on a hysteresis control loop. Hysteresis is the property of a system where its state depends on its history, which is used to prevent the rapid oscillation (thrashing) that a single-threshold system would cause.

### 4.1 The `MemoryPolicy` Trait

This trait defines the core abstraction for all memory management strategies. Importantly, `FlatBufferBuilder` capacity is not observable, so the policy operates on message-size history and optional time.

```rust
/// Reason for a reclamation (reset) action.
pub enum ReclamationReason {
    /// Policy triggered by a message-count-based heuristic.
    MessageCount,
    /// Policy triggered by a time-based cooldown.
    TimeCooldown,
    /// Policy triggered by a hard size limit.
    SizeThreshold,
}

/// A trait that defines a stateful policy for when to reset the internal builder.
pub trait MemoryPolicy: Send + 'static {
    /// Called after each successful write.
    ///
    /// # Arguments
    /// * `last_message_size` - The size of the message just written.
    ///
    /// # Returns
    /// * `Some(ReclamationReason)` if the builder should be reset, otherwise `None`.
    fn should_reset(&mut self, last_message_size: usize) -> Option<ReclamationReason>;
}
```

### 4.2 The `AdaptiveWatermarkPolicy` Implementation (Refined)

This will be the primary, recommended policy. It uses a hysteresis loop based on message-size history and an optional time-based cooldown. Since builder capacity is not observable, the policy relies on a high-watermark of observed message sizes and triggers shrink after a delay once smaller messages are seen.

```rust
use std::time::{Duration, Instant};

pub struct AdaptiveWatermarkPolicy {
    high_watermark_bytes: usize,
    messages_since_high: u32,
    messages_to_wait: u32,
    last_high_seen_at: Option<Instant>,
    cooldown: Option<Duration>,
}

impl MemoryPolicy for AdaptiveWatermarkPolicy {
    fn should_reset(&mut self, last_message_size: usize) -> Option<ReclamationReason> {
        let now = if self.cooldown.is_some() { Some(Instant::now()) } else { None };

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
            self.high_watermark_bytes = last_message_size;
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
```

## 5.0 Definitive Implementation Plan and API Design

This section provides the concrete, code-first blueprint for implementation.

### Step 1: Create `src/policy.rs`

This new module will house the `MemoryPolicy` trait and its core implementations.

-   `trait MemoryPolicy`
-   `struct NoOpPolicy`: A zero-cost policy that always returns `false`. Its `should_reset` method will be marked `#[inline(always)]`. This will be the default.
-   `struct SizeThresholdPolicy`: A simple policy that resets if capacity exceeds a fixed limit.
-   `struct AdaptiveWatermarkPolicy`: The advanced, stateful policy detailed above.

### Step 2: Integrate via Generic `StreamWriter` in `src/writer.rs`

The `StreamWriter` will be refactored to be generic over a `MemoryPolicy` and will use a builder for construction. It retains its internal `FlatBufferBuilder` and executes reclamation by recreating it with a default capacity.

```rust
// In src/policy.rs
pub use policy::{MemoryPolicy, NoOpPolicy, ReclamationReason, AdaptiveWatermarkPolicy};

// In src/writer.rs
use crate::framing::Framer;
use crate::policy::{MemoryPolicy, NoOpPolicy, ReclamationReason};
use flatbuffers::FlatBufferBuilder;
use std::io::Write;

pub struct StreamWriter<'a, W: Write, F: Framer, P = NoOpPolicy, A = flatbuffers::DefaultAllocator>
where
    P: MemoryPolicy,
    A: flatbuffers::Allocator + 'a,
{
    writer: W,
    framer: F,
    builder: FlatBufferBuilder<'a, A>,
    policy: P,
    default_buffer_capacity: usize,
    on_reclaim: Option<Box<dyn Fn(&ReclamationInfo) + Send + Sync + 'static>>,
}

pub struct ReclamationInfo {
    pub reason: ReclamationReason,
    pub last_message_size: usize,
    pub capacity_after: usize,
}

impl<'a, W: Write, F: Framer, P: MemoryPolicy, A: flatbuffers::Allocator + 'a>
    StreamWriter<'a, W, F, P, A>
{
    #[inline]
    pub fn write<T: StreamSerialize>(&mut self, item: &T) -> Result<()> {
        self.builder.reset();
        item.serialize(&mut self.builder)?;
        let payload = self.builder.finished_data();
        let last_message_size = payload.len();
        self.framer.frame_and_write(&mut self.writer, payload)?;

        if let Some(reason) = self.policy.should_reset(last_message_size) {
            self.builder = FlatBufferBuilder::with_capacity(self.default_buffer_capacity);
            if let Some(cb) = &self.on_reclaim {
                (cb)(&ReclamationInfo {
                    reason,
                    last_message_size,
                    capacity_after: self.default_buffer_capacity,
                });
            }
        }
        Ok(())
    }
}
```

### Step 3: Create `StreamWriterBuilder` for Fluent Configuration

To maintain backward compatibility and provide a clean API, construction will be handled by a new builder with both static and dynamic policy commit paths.

```rust
pub struct StreamWriterBuilder<'a, W, F, P = NoOpPolicy, A = flatbuffers::DefaultAllocator>
where
    W: Write,
    F: Framer,
    P: MemoryPolicy,
    A: flatbuffers::Allocator + 'a,
{
    writer: W,
    framer: F,
    policy: P,
    default_buffer_capacity: usize,
    on_reclaim: Option<Box<dyn Fn(&ReclamationInfo) + Send + Sync + 'static>>,
    _phantom: std::marker::PhantomData<(&'a (), A)>,
}

const DEFAULT_BUILDER_CAPACITY: usize = 16 * 1024;

impl<'a, W: Write, F: Framer, A: flatbuffers::Allocator + 'a>
    StreamWriter<'a, W, F, NoOpPolicy, A>
{
    pub fn builder(writer: W, framer: F) -> StreamWriterBuilder<'a, W, F, NoOpPolicy, A> {
        StreamWriterBuilder {
            writer,
            framer,
            policy: NoOpPolicy,
            default_buffer_capacity: DEFAULT_BUILDER_CAPACITY,
            on_reclaim: None,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<'a, W: Write, F: Framer, P: MemoryPolicy, A: flatbuffers::Allocator + 'a>
    StreamWriterBuilder<'a, W, F, P, A>
{
    pub fn with_policy<P2: MemoryPolicy>(self, policy: P2) -> StreamWriterBuilder<'a, W, F, P2, A> {
        StreamWriterBuilder {
            writer: self.writer,
            framer: self.framer,
            policy,
            default_buffer_capacity: self.default_buffer_capacity,
            on_reclaim: self.on_reclaim,
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn with_default_capacity(mut self, capacity: usize) -> Self {
        self.default_buffer_capacity = capacity;
        self
    }

    pub fn with_reclaim_callback<Cb>(mut self, callback: Cb) -> Self
    where
        Cb: Fn(&ReclamationInfo) + Send + Sync + 'static,
    {
        self.on_reclaim = Some(Box::new(callback));
        self
    }

    pub fn build(self) -> StreamWriter<'a, W, F, P, A> {
        StreamWriter {
            writer: self.writer,
            framer: self.framer,
            builder: FlatBufferBuilder::with_capacity(self.default_buffer_capacity),
            policy: self.policy,
            default_buffer_capacity: self.default_buffer_capacity,
            on_reclaim: self.on_reclaim,
        }
    }

    pub fn build_dyn(self) -> StreamWriter<'a, W, F, Box<dyn MemoryPolicy + 'a>, A> {
        StreamWriter {
            writer: self.writer,
            framer: self.framer,
            builder: FlatBufferBuilder::with_capacity(self.default_buffer_capacity),
            policy: Box::new(self.policy),
            default_buffer_capacity: self.default_buffer_capacity,
            on_reclaim: self.on_reclaim,
        }
    }
}
```

### Step 4: Final API Usage (The Payoff)

This implementation results in a powerful, opt-in, and backward-compatible API.

```rust
use flatstream::{StreamWriter, DefaultFramer};

// Simple case: unchanged; no policy overhead.
let mut writer = StreamWriter::new(file, DefaultFramer);

// Advanced: Enable the AdaptiveWatermarkPolicy via builder (static dispatch)
let policy = AdaptiveWatermarkPolicy {
    high_watermark_bytes: 0,
    messages_since_high: 0,
    messages_to_wait: 5,
    last_high_seen_at: None,
    cooldown: Some(std::time::Duration::from_millis(500)),
};

let mut writer_with_policy = StreamWriter::builder(file, DefaultFramer)
    .with_memory_policy(policy)
    .with_default_capacity(16 * 1024)
    .with_reclaim_callback(|info| {
        // emit metrics/logs
        let _ = (info.reason.clone(), info.capacity_after);
    })
    .build();

writer_with_policy.write(&large_message)?; // Sets the watermark
writer_with_policy.write(&small_message)?; // May trigger after delay/cooldown
```

If desired, use `build_dyn()` to avoid generic policy types in the resulting writer at the cost of a vtable call per write.

```rust
let mut writer_dyn = StreamWriter::builder(file, DefaultFramer)
    .with_memory_policy(policy)
    .build_dyn();
```

> **Implementation Note: Accessing Builder Capacity**
>
> Currently, `flatstream-rs` inspects the builder's capacity by accessing the finished buffer slice (`mut_finished_buffer().len()`). While functional, this relies on the implementation detail that the slice length equals the backing buffer's capacity at the moment of finalization.
>
> **Future Plan:** Contribute a public `capacity()` method to the upstream `flatbuffers` crate. Future versions of `flatstream-rs` should be updated to reference this explicit API once available, ensuring long-term stability and correctness.

## 5.1 Documentation & Guidance (Allocator Considerations)

The performance of the “simple reset” (dropping and recreating the `FlatBufferBuilder`) depends on the global allocator. High-performance allocators like `mimalloc` or `jemalloc` typically cache freed blocks in thread-local storage, making the dealloc/alloc cycle fast. The default system allocator may incur higher costs. For performance-sensitive workloads, consider enabling `mimalloc` as the global allocator and benchmark both configurations.

## 5.2 Validation and Benchmarking Checklist

- Oscillation benchmark: alternate 1 KiB ↔ 1 MiB messages to verify reduced oscillation with the refined `AdaptiveWatermarkPolicy`.
- Cooldown overhead: run with and without time-based cooldown to assess `Instant::now()` overhead; document impact.
- Reset strategy: compare `FlatBufferBuilder::new()` vs. `with_capacity(DEFAULT_BUILDER_CAPACITY)` for first-write latency post-reset.
- Allocator comparison: run benchmarks with the system allocator and `mimalloc` to quantify reset cost and document results.
- Regression: verify expert `write_finished()` paths remain allocation-free and unaffected by policy decisions.

## 6.0 Conclusion

The implementation of this feature is a critical step in maturing `flatstream-rs` into a robust, production-grade library. By adopting the proposed `MemoryPolicy` trait and the `AdaptiveWatermarkPolicy`, we solve the high-water mark memory bloat problem in a way that is architecturally consistent, highly performant, and ergonomically sound. This enhancement provides users with fine-grained control over memory management, making the simple writer path safe and reliable for a new class of long-running, high-throughput applications where memory stability is paramount.

## 7.0 Future Work: Custom Allocators (Phase 2)

This design provides the foundation for an even more performant system by separating the policy "trigger" from the reclamation "action". The current design implements the action as a simple and safe buffer replacement.

A future enhancement ("Phase 2") could introduce a custom allocator for the `StreamWriter`'s internal buffer. The `flatbuffers::FlatBufferBuilder` already supports a generic `Allocator`. This allocator could be a true, high-performance buffer pool (implementing the logic from the `HysteresisBufferPool` research).

In this future scenario, the `MemoryPolicy` trait would remain the "trigger," but the "action" would change from recreating the builder to a hypothetical `builder.recycle()` against a pool-backed allocator. This is significantly more complex and not part of this initial implementation. The current design solves the immediate problem and provides the ideal API hook for a future allocator optimization without a breaking change.

