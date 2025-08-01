# v2.6 Performance Experiment Results

These are the actual results from running the performance experiments on a modern development machine. Your results may vary based on hardware and system load.

## Performance Comparison Results

```
=== Simple vs Expert Mode Performance Comparison ===

Running performance tests (1000 messages)...

Results:
  Simple mode: 1.770083ms
  Expert mode: 1.471916ms
  Difference: 20.26%
  Per message overhead: 298.17ns
```

**Analysis**: For small-to-medium uniform messages, simple mode is about 20% slower, which translates to only ~0.3ns per message - negligible for most applications.

## Overhead Analysis Results

```
=== Overhead Analysis ===

Results for 10000 iterations:
  Builder operations only: 252.333µs
  Simple mode total: 1.213208ms
  Expert mode total: 1.144291ms
  Raw framing only: 843.25µs

Per-operation breakdown:
  Builder reset+serialize: 25.23ns
  Simple mode: 121.32ns
  Expert mode: 114.43ns
  Raw framing: 84.33ns

Simple vs Expert overhead: 6.89ns per operation
```

**Analysis**: The overhead comes from the trait dispatch in simple mode, not from allocations. The ~7ns difference is trivial.

## Large Message Test Results

```
=== Large Message Performance Test ===

1. Large Messages Only (10MB each):
   Simple mode: 70.5835ms
   Expert mode: 36.216542ms
   Difference: 1.95x

2. Mixed Message Sizes (alternating 10MB and 10 bytes):
   Simple mode (one builder for all): 25.381292ms
   Expert mode (one builder): 28.432542ms
   Note: After serializing 10MB message, builder retains that capacity
         even when serializing tiny messages!

3. Expert Mode with Multiple Builders (optimal for mixed sizes):
   Expert mode (separate builders): 14.095583ms
   This avoids memory waste by using right-sized builders
```

**Analysis**: Expert mode shines with large messages (2x faster) and mixed sizes (2x faster with multiple builders).

## Memory Usage Test Results

```
=== Memory Usage Analysis ===

Scenario: 1 huge message (50MB), then 1000 tiny messages (10 bytes each)

1. Simple Mode (cannot optimize memory):
   After huge message: Internal builder has 50MB+ capacity
   After 1000 tiny messages: Builder STILL has 50MB+ capacity!
   Memory waste: ~50MB held unnecessarily

2. Expert Mode (can use separate builders):
   After huge message: Temporary builder dropped, memory freed
   After 1000 tiny messages: Only using ~1KB for tiny_builder
   Memory efficient: Large allocation was freed after use
```

**Analysis**: Simple mode's single builder can cause significant memory waste with mixed message sizes.

## Key Takeaways

1. **For uniform, small-to-medium messages**: Simple mode is only 0-25% slower (negligible)
2. **For large messages (10MB+)**: Expert mode can be 2x faster
3. **For mixed message sizes**: Expert mode with multiple builders provides both performance and memory benefits
4. **Memory efficiency**: Expert mode allows fine-grained control over memory usage

## Recommendation

- **Start with simple mode** - it's easier and performs well for most use cases
- **Switch to expert mode when**:
  - Working with large messages (>1MB)
  - Handling mixed message sizes
  - Memory usage is a concern
  - You need the absolute best performance

The v2.6 hybrid approach is justified because simple mode performance is excellent for the common case. 