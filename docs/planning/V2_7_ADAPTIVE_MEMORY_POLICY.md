# Design Document: An Adaptive Memory Reclamation Policy for `StreamWriter`

## 1.0 Executive Summary: A Production-Grade Memory Management Strategy

This document presents the definitive architectural plan for integrating an optional, adaptive memory reclamation policy into the `flatstream-rs` library's simple writer path. The analysis synthesizes the user's prior research on composable policy traits with a formal, hysteresis-based control model to solve the "high-water mark" memory bloat problem in the existing `StreamWriter`.

### 1.1 The Problem: The "High-Water Mark" Dilemma in High-Performance Buffering

The current `StreamWriter` uses a reusable `Vec<u8>` buffer that grows to accommodate large messages but never subsequently reclaims this memory. This behavior, while a common performance optimization to avoid costly reallocations, creates a critical conflict between allocation performance and long-term memory footprint. A single burst of large messages can cause the writer's memory usage to grow permanently, leading to memory bloat and the risk of OOM (Out Of Memory) termination in long-running applications. This is an unacceptable risk for a production-grade library.

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

The root cause of the memory bloat is the fundamental distinction between a `Vec`'s `length` and its `capacity`. When `StreamWriter` processes a message, it calls `buffer.clear()`, which resets `len` to 0 but leaves `capacity` unchanged. This is an intentional optimization to reuse the existing allocation.

The problem occurs when a burst of large messages arrives. A single large message forces the `Vec` to reallocate, setting a new, high `capacity`. Because this capacity is never reclaimed, the buffer permanently holds onto this peak memory.

The naive solution, calling `Vec::shrink_to_fit()` after every message, introduces a severe performance degradation known as **thrashing**. If the system processes a large message, shrinks the buffer, and immediately receives another large message, it is forced into a costly `free -> alloc` cycle. A "smart" policy is required to prevent this.

### 2.1 The "Simple Reset" is the Correct Solution

The recommended reclamation action is a "simple reset": replacing the internal buffer by creating a new one (`self.buffer = Vec::new()`). This approach is preferred over a more complex, shared buffer pool for two primary reasons:

1.  **Safety and Simplicity:** It is 100% memory safe. The old buffer is `drop`ped, and Rust's ownership model guarantees no other references can exist, eliminating any risk of use-after-free bugs common with manual pool management.
2.  **Sufficient Performance:** While it incurs a `dealloc`/`alloc` cycle, modern global allocators (e.g., `mimalloc`, `jemalloc`) are highly optimized for this pattern. They often cache freed blocks in thread-local storage, making the "reset" operation extremely fast and avoiding expensive calls to the OS. The marginal performance gain from an application-level pool does not justify the significant increase in complexity and safety risk for this use case.

## 3.0 The Architectural Imperative: A Pluggable Policy Trait

The `flatstream-rs` architecture has already established a clear and successful pattern for handling optional, cross-cutting concerns: the orthogonal, composable trait.

-   **The `Checksum` Trait:** Provides a pluggable strategy for data integrity.
-   **The `Validator` Trait:** Provides a pluggable strategy for data safety.
-   **The "No-Op" Precedent:** The existence of `NoChecksum` and `NoValidator` establishes the library's philosophical guarantee of **zero-cost abstraction**. When a generic component is monomorphized with a no-op implementation, the compiler can completely optimize away the check, resulting in zero runtime overhead.

Any solution to the memory reclamation problem **must** adhere to this established precedent. The correct architectural path is to introduce a new `MemoryPolicy` trait. This approach strengthens the library's core design, turning "memory management" into another independent, composable axis of behavior that users can select and configure, just like integrity and safety.

## 4.0 The Recommended Architecture: The Hysteresis-Based Policy

The proposed solution is to implement the composable `MemoryPolicy` framework and provide a sophisticated, state-of-the-art implementation based on a hysteresis control loop. Hysteresis is the property of a system where its state depends on its history, which is used to prevent the rapid oscillation (thrashing) that a single-threshold system would cause.

### 4.1 The `MemoryPolicy` Trait

This trait defines the core abstraction for all memory management strategies.

```rust
/// A trait that defines a policy for when to reset the internal buffer.
pub trait MemoryPolicy: Send + 'static {
    /// Called after each successful write to determine if the buffer should be reset.
    ///
    /// # Arguments
    /// * `capacity` - The total allocated capacity of the writer's buffer.
    /// * `last_message_size` - The size of the message just written.
    ///
    /// # Returns
    /// * `true` if the buffer should be reset to reclaim memory.
    fn should_reset(&mut self, capacity: usize, last_message_size: usize) -> bool;
}
```

### 4.2 The `AdaptiveWatermarkPolicy` Implementation

This will be the primary, recommended policy for production use. It implements a dual-threshold-with-delay strategy to intelligently decide when to shrink the buffer. This is a form of hysteresis loop, preventing thrashing during message bursts.

-   **T1 (Burst Threshold):** A capacity threshold that, when crossed, indicates the start of a "burst" of large messages. The policy enters a cooldown state.
-   **T2 (Max Threshold):** A hard upper limit that acts as a circuit breaker. If a message forces the buffer capacity beyond this, it is considered a wasteful outlier, and the memory is reclaimed immediately.
-   **Delay Period:** The time the policy will wait after a burst (`T1` breach) before shrinking the buffer. This allows the system to retain the larger capacity for a short time in case another large message arrives, thus preventing thrashing.

```rust
pub struct AdaptiveWatermarkPolicy {
    high_watermark_bytes: usize, // Tracks the largest message *seen*.
    messages_since_high: u32,  // Counter for "small" messages.
    messages_to_wait: u32,     // Configurable delay period.
    reset_multiplier: usize,   // e.g., reset if capacity is 2x high_watermark.
}

impl MemoryPolicy for AdaptiveWatermarkPolicy {
    fn should_reset(&mut self, capacity: usize, last_message_size: usize) -> bool {
        // Update the high watermark if this message was larger.
        if last_message_size > self.high_watermark_bytes {
            self.high_watermark_bytes = last_message_size;
            self.messages_since_high = 0;
            return false;
        }

        // If the buffer is much larger than our typical max message size...
        if capacity > self.high_watermark_bytes * self.reset_multiplier {
            self.messages_since_high += 1;

            // ...and we've seen enough smaller messages in a row, it's time to shrink.
            if self.messages_since_high >= self.messages_to_wait {
                self.messages_since_high = 0; // Reset for next time
                self.high_watermark_bytes = 0; // Reset watermark
                return true;
            }
        } else {
            // We are not in a bloated state, so reset the counter.
            self.messages_since_high = 0;
        }

        false
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

The `StreamWriter` will be refactored to be generic over a `MemoryPolicy` and will use a builder for its construction.

```rust
// In src/writer.rs

use crate::policy::{MemoryPolicy, NoOpPolicy};

// The StreamWriter struct is now generic over a policy `P`.
pub struct StreamWriter<W: Write, F: Framer = DefaultFramer, P: MemoryPolicy = NoOpPolicy> {
    writer: W,
    buffer: Vec<u8>,
    framer: F,
    policy: P,
}

// The core write logic is updated to check the policy.
impl<W: Write, F: Framer, P: MemoryPolicy> StreamWriter<W, F, P> {
    fn write_message<'a, M: Into<Message<'a>>>(&mut self, message: M) -> Result<()> {
        let message = message.into();
        self.buffer.clear();

        let total_len = self.framer.frame(message, &mut self.buffer)?;
        self.writer.write_all(&self.buffer[..total_len])?;

        // CHECK THE POLICY
        if self.policy.should_reset(self.buffer.capacity(), total_len) {
            // Reclaim memory via a "simple reset". This drops the old buffer
            // and allocates a new one. This is safe and performant with
            // modern allocators.
            self.buffer = Vec::new();
        }

        Ok(())
    }
    
    // ... other methods
}
```

### Step 3: Create `StreamWriterBuilder` for Fluent Configuration

To maintain backward compatibility and provide a clean API, construction will be handled by a new builder.

```rust
// In src/writer.rs

pub struct StreamWriterBuilder<W: Write, F: Framer, P: MemoryPolicy> {
    // ... fields
}

impl<W: Write, F: Framer, P: MemoryPolicy> StreamWriterBuilder<W, F, P> {
    pub fn new(writer: W) -> StreamWriterBuilder<W, F, NoOpPolicy> { /* ... */ }

    pub fn with_capacity(mut self, capacity: usize) -> Self { /* ... */ }
    
    // The key method to opt-in to the new feature.
    pub fn with_memory_policy<NP: MemoryPolicy>(
        self, 
        policy: NP
    ) -> StreamWriterBuilder<W, F, NP> {
        // ... transition to a builder with the new policy type
    }

    pub fn build(self) -> StreamWriter<W, F, P> { /* ... */ }
}

// StreamWriter::new remains for simple cases, using the NoOpPolicy default.
impl<W: Write> StreamWriter<W> {
    pub fn new(writer: W) -> Self { /* ... */ }
    pub fn builder(writer: W) -> StreamWriterBuilder<W, DefaultFramer, NoOpPolicy> { /* ... */ }
}
```

### Step 4: Final API Usage (The Payoff)

This implementation results in a powerful, opt-in, and backward-compatible API.

**Before (Current API):**

```rust
// Memory usage can grow indefinitely.
let mut writer = StreamWriter::new(file);
writer.write_message(&large_message)?;
writer.write_message(&small_message)?; // Buffer capacity remains high
```

**After (New Fluent, Configurable API):**

```rust
// Simple case: No change, no performance impact.
let mut writer = StreamWriter::new(file);

// Advanced case: Opt-in to the AdaptiveWatermarkPolicy.
let policy = AdaptiveWatermarkPolicy::new(
    /* messages_to_wait */ 5,
    /* reset_multiplier */ 2,
);

let mut writer_with_policy = StreamWriter::builder(file)
    .with_memory_policy(policy)
    .build();

writer_with_policy.write_message(&large_message)?; // Sets the high watermark
writer_with_policy.write_message(&small_message)?; // Buffer will eventually be shrunk
```

## 6.0 Conclusion

The implementation of this feature is a critical step in maturing `flatstream-rs` into a robust, production-grade library. By adopting the proposed `MemoryPolicy` trait and the `AdaptiveWatermarkPolicy`, we solve the high-water mark memory bloat problem in a way that is architecturally consistent, highly performant, and ergonomically sound. This enhancement provides users with fine-grained control over memory management, making the simple writer path safe and reliable for a new class of long-running, high-throughput applications where memory stability is paramount.

## 7.0 Future Work: Custom Allocators (Phase 2)

This design provides the foundation for an even more performant system by separating the policy "trigger" from the reclamation "action". The current design implements the action as a simple and safe buffer replacement.

A future enhancement ("Phase 2") could introduce a custom allocator for the `StreamWriter`'s internal buffer. The `flatbuffers::FlatBufferBuilder` already supports a generic `Allocator`. This allocator could be a true, high-performance buffer pool (implementing the logic from the `HysteresisBufferPool` research).

In this future scenario, the `MemoryPolicy` trait would remain the "trigger," but the "action" would change from `self.buffer = Vec::new()` to a hypothetical `self.buffer.recycle()`, which would return the buffer to the custom pool. This is a significantly more complex task and is not part of this initial implementation. The current design successfully solves the user's problem and provides the ideal API hook for this future optimization without requiring a breaking change.

## 8.0 Analysis and Refinement Directives (v2.7)

This section provides concrete directives to stress-test the current design and improve ergonomics, resilience, and observability prior to implementation. It draws on ergonomic lessons from the fluent adapter style used elsewhere in the crate (for example, `examples/bounded_adapters_example.rs` demonstrates a discoverable `.bounded(...)` fluent API on framers/deframers) and applies similar principles to the writer and memory policy surface.

### 8.1 API Ergonomics and the Builder Pattern

- **Type-state builder complexity**: The proposed `StreamWriterBuilder<W, F, P>` powered by type-state provides zero-cost configuration but may surface verbose types to users during configuration. Investigate an ergonomic hybrid:
  - Store the configured policy inside the builder as `Option<Box<dyn MemoryPolicy>>` for the configuration phase to keep the builder signature simple and discoverable.
  - Offer two commit paths to balance ergonomics with static dispatch:
    - **Ergonomic commit (dynamic dispatch at runtime):** `build_dyn()` returns `StreamWriter<W, F, DynPolicy>` where the writer holds a boxed policy. This is simplest to use and sufficient for many workloads.
    - **Static commit (monomorphized, zero-overhead):** `with_memory_policy<P: MemoryPolicy>(self, policy: P) -> StreamWriterBuilder<W, F, P>` switches the builder to a typed state when users opt into a concrete policy. `build()` then returns `StreamWriter<W, F, P>`.
  - This split mirrors the fluent `.bounded()` discoverability pattern while preserving a zero-cost path for performance-sensitive users.

- **Discoverability**:
  - Keep `StreamWriter::new(writer, framer)` as the simple path and clearly document it as the non-reclaiming default. Add examples showing simple and expert modes side-by-side, and explicitly link to `builder()` for advanced configuration (policies, capacities, callbacks).
  - Add `StreamWriter::builder(writer, framer)` and highlight it in README and API docs as the entry point for advanced tuning, akin to how `.bounded()` advertises limits for framers/deframers.
  - Optional future change: consider `new_default()` naming in a major version to make the distinction explicit, with a staged deprecation cycle.

### 8.2 Refinements to `AdaptiveWatermarkPolicy`

- **State reset behavior**:
  - Current: reset sets `high_watermark_bytes = 0`. In alternating workloads (e.g., 1 KiB then 1 MiB bursts), this can over-trigger growth/shrink cycles.
  - Directive: After a shrink, set `high_watermark_bytes` to `last_message_size` observed immediately prior to the shrink decision. This preserves a recent baseline and reduces oscillation.

- **Message-count and time-based tuning**:
  - Current: `messages_to_wait` with `reset_multiplier` has no time dimension.
  - Directive: Add an optional cooldown using `Instant`/`Duration`. Trigger shrink when either condition is met:
    - `messages_since_high >= messages_to_wait`, or
    - `now - last_high_seen_at >= cooldown`
  - This improves behavior in bursty systems where long idle periods should prefer reclamation.

- **Integer versus float multiplier**:
  - Current: `reset_multiplier: usize`.
  - Directive: Consider `reset_multiplier: f32` for more natural tuning (e.g., 1.5×). Conversion to `usize` for comparisons can clamp via `ceil()` or `max(1, ...)`. The expected overhead is negligible relative to I/O.

Example sketch (new fields and refined logic):

```rust
pub struct AdaptiveWatermarkPolicy {
    high_watermark_bytes: usize,
    messages_since_high: u32,
    messages_to_wait: u32,
    reset_multiplier: f32,      // allow 1.5x, 2.0x, etc.
    last_high_seen_at: Option<std::time::Instant>,
    cooldown: Option<std::time::Duration>,
}

impl MemoryPolicy for AdaptiveWatermarkPolicy {
    fn should_reset(&mut self, capacity: usize, last_message_size: usize) -> bool {
        let now = self.cooldown.map(|_| std::time::Instant::now());

        if last_message_size > self.high_watermark_bytes {
            self.high_watermark_bytes = last_message_size;
            self.messages_since_high = 0;
            self.last_high_seen_at = now;
            return false;
        }

        let threshold = (self.high_watermark_bytes as f32 * self.reset_multiplier).ceil() as usize;
        let over_threshold = self.high_watermark_bytes > 0 && capacity > threshold;

        if over_threshold {
            self.messages_since_high = self.messages_since_high.saturating_add(1);

            let time_ok = match (self.cooldown, self.last_high_seen_at, now) {
                (Some(cd), Some(t0), Some(t1)) => t1.duration_since(t0) >= cd,
                _ => false,
            };

            if self.messages_since_high >= self.messages_to_wait || time_ok {
                // Remember recent baseline instead of zeroing out.
                self.messages_since_high = 0;
                self.high_watermark_bytes = last_message_size;
                self.last_high_seen_at = now;
                return true;
            }
        } else {
            self.messages_since_high = 0;
        }

        false
    }
}
```

### 8.3 Reset Action Details and Default Capacity

- **Reset target capacity**:
  - Current text recommends `self.buffer = Vec::new()`.
  - Directive: Prefer `self.buffer = Vec::with_capacity(DEFAULT_BUFFER_SIZE)` to avoid a post-shrink “first write” allocation on typical messages. Expose `DEFAULT_BUFFER_SIZE` through the builder so users can align it with their workload. Ensure this default is documented near `StreamWriter::with_capacity(...)` for symmetry with reader APIs.

- **Allocator dependency and documentation**:
  - The performance of the simple reset depends on the global allocator. Document that high-performance allocators (e.g., `mimalloc`, `jemalloc`) can make the dealloc/alloc cycle effectively constant-time via thread-local caches, while some system allocators may impose higher costs. Provide guidance on enabling an alternative global allocator and caveats for performance-sensitive environments.

Example reset snippet in the writer (conceptual):

```rust
if self.policy.should_reset(self.buffer.capacity(), total_len) {
    let cap = self.default_buffer_capacity; // from builder configuration
    self.buffer = Vec::with_capacity(cap);
}
```

### 8.4 Observability and Debugging Hooks

Introduce lightweight visibility so users can validate and tune policies in production:

- **Callback hook (builder-configured):**
  - Add `on_reclaim: Option<Box<dyn Fn(&ReclamationInfo) + Send + Sync>>` to the builder/writer. Invoke on each reset.
  - Provide `ReclamationInfo { reason: ReclamationReason, capacity_before: usize, capacity_after: usize, high_watermark_bytes: usize, messages_since_high: u32, reset_multiplier: f32, cooldown_elapsed: Option<std::time::Duration> }`.

- **Return value as `Option<ReclamationReason>`:**
  - Alternative to a boolean: `fn should_reset(...) -> Option<ReclamationReason>` enables the writer to emit structured logs/metrics without additional branching. Consider this for v2.8 to avoid immediate breaking changes; for v2.7, prefer the callback or wrapper below.

- **Instrumented wrapper policy:**
  - Provide `InstrumentedPolicy<P: MemoryPolicy>` that decorates another policy and forwards decisions while emitting logs/metrics. This preserves the current `MemoryPolicy` signature and composes naturally with the library’s style.

Sketch:

```rust
pub enum ReclamationReason {
    ExceededMultiplier,
    TimeCooldownElapsed,
    MaxThresholdTripped,
}

pub struct ReclamationInfo {
    pub reason: ReclamationReason,
    pub capacity_before: usize,
    pub capacity_after: usize,
    pub high_watermark_bytes: usize,
    pub messages_since_high: u32,
}

pub struct InstrumentedPolicy<P, F>
where
    P: MemoryPolicy,
    F: Fn(&ReclamationInfo) + Send + Sync + 'static,
{
    inner: P,
    on_reclaim: F,
}

impl<P, F> MemoryPolicy for InstrumentedPolicy<P, F>
where
    P: MemoryPolicy,
    F: Fn(&ReclamationInfo) + Send + Sync + 'static,
{
    fn should_reset(&mut self, capacity: usize, last_message_size: usize) -> bool {
        let decision = self.inner.should_reset(capacity, last_message_size);
        if decision {
            (self.on_reclaim)(&ReclamationInfo {
                reason: ReclamationReason::ExceededMultiplier, // placeholder; refine if enum-enabled later
                capacity_before: capacity,
                capacity_after: 0,
                high_watermark_bytes: 0,
                messages_since_high: 0,
            });
        }
        decision
    }
}
```

### 8.5 Compatibility and Migration Guidance

- Keep `StreamWriter::new(...)` unchanged for v2.7; add `builder(...)` and emphasize it in docs as the discoverable path for advanced configuration.
- Maintain `MemoryPolicy::should_reset(...) -> bool` in v2.7 for minimal disruption. Add `InstrumentedPolicy` and callback hooks to provide observability now. Consider evolving to `Option<ReclamationReason>` in a future minor release with a straightforward migration path.
- Introduce `DEFAULT_BUFFER_SIZE` as a builder-configurable parameter with a conservative default (e.g., 8–16 KiB). Encourage users to align this with typical serialized message sizes.

### 8.6 Validation and Benchmarking Checklist

- Add micro-benchmarks covering alternating workloads (e.g., 1 KiB ↔ 1 MiB) to verify reduced oscillation with the revised watermark reset.
- Benchmark with and without a time-based cooldown to assess the `Instant::now()` overhead in practice; document impact and provide guidance on enabling/disabling cooldown.
- Compare reset strategies: `Vec::new()` vs. `Vec::with_capacity(DEFAULT_BUFFER_SIZE)` for steady-state and first-write latency post-reset.
- Run allocator comparisons (system allocator vs. `mimalloc`/`jemalloc`) to quantify the reset cost and include results in the README/bench docs.
- Ensure no effect on zero-copy guarantees or expert mode workflows; verify that expert `write_finished()` paths remain allocation-free and unaffected by policy decisions.
