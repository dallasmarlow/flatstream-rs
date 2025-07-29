# v2.6 Performance Experiments

These scripts were created during the documentation review to understand the actual performance characteristics of the v2.6 "Hybrid API" implementation. They helped clarify several misconceptions about the performance differences between simple and expert modes.

## Scripts

### Core Performance Analysis

#### 1. `performance_comparison.rs`
Compares the raw performance of simple mode vs expert mode with uniform message sizes. 

**Key Finding**: Simple mode is only 0-25% slower than expert mode for common cases.

#### 2. `overhead_analysis.rs`
Breaks down where the performance overhead comes from in simple mode.

**Key Finding**: The overhead is minimal (~0.3ns per operation) and comes from trait dispatch, not from allocations or copying.

#### 3. `large_message_test.rs`
Tests performance with large messages (10MB+) and mixed message sizes.

**Key Findings**:
- Expert mode can be ~2x faster for very large messages
- Mixed message sizes benefit from multiple builders in expert mode
- Simple mode suffers from memory bloat with mixed sizes

#### 4. `memory_usage_test.rs`
Demonstrates the memory implications of simple vs expert mode.

**Key Finding**: Simple mode's single internal builder can waste significant memory when dealing with mixed message sizes.

### Verification Scripts

#### 5. `zero_copy_verification.rs`
Verifies that both simple and expert modes maintain zero-copy behavior by tracking memory addresses and data patterns.

**Key Finding**: Both modes write data directly from builder to output without intermediate copies. Reading provides zero-copy slices from the internal buffer.

#### 6. `checksum_performance_comparison.rs`
Compares performance and overhead of different checksum algorithms (CRC16, CRC32, XXHash64).

**Key Finding**: CRC16 has 75% less overhead than XXHash64 (2 bytes vs 8 bytes), making it ideal for high-frequency small messages.

#### 7. `wire_format_verification.rs`
Verifies the stream format matches documentation exactly: `[4-byte length | variable checksum | payload]`.

**Key Finding**: All binary fields use little-endian as documented, checksum sizes vary by algorithm (0-8 bytes).

#### 8. `builder_reuse_verification.rs`
Demonstrates that builders are properly reused and shows memory bloat scenarios.

**Key Finding**: Builder `reset()` preserves allocated capacity, leading to memory bloat in simple mode after large messages.

#### 9. `error_handling_verification.rs`
Tests all error types: I/O, ChecksumMismatch, InvalidFrame, UnexpectedEof.

**Key Finding**: All error types work as documented, with proper propagation and clean EOF handling.

#### 10. `throughput_measurement.rs`
Measures actual throughput to verify performance claims.

**Key Finding**: Library achieves >50k messages/sec as claimed, with expert mode showing consistent performance advantages.

#### 11. `trait_composability_demo.rs`
Demonstrates trait-based composability and static dispatch through monomorphization.

**Key Finding**: Zero-cost abstractions achieved through compile-time monomorphization, no vtable lookups.

## Running the Experiments

These are standalone Rust programs that can be run with:

```bash
# From the project root
rustc experiments/v2_6/performance_comparison.rs -L target/release/deps \
  --extern flatstream_rs=target/release/libflatstream_rs.rlib \
  --extern flatbuffers=target/release/deps/libflatbuffers-*.rlib \
  -o target/performance_comparison

./target/performance_comparison
```

Or more simply, copy them to the examples directory and run with cargo:

```bash
cp experiments/v2_6/performance_comparison.rs examples/
cargo run --example performance_comparison --release
```

## Conclusions

These experiments led to several important clarifications in the documentation:

1. **Both modes are zero-copy** - The performance differences come from memory management flexibility, not from data copying.

2. **Simple mode performance is excellent** - For uniform, small-to-medium messages, the 0-25% overhead is negligible.

3. **Expert mode benefits are situational** - It's most beneficial for:
   - Large messages (10MB+) where trait dispatch overhead becomes noticeable
   - Mixed message sizes where multiple builders avoid memory bloat
   - Memory-constrained systems where fine-grained control matters

4. **The v2.6 compromise was justified** - If simple mode were 2-10x slower, forcing expert mode would make sense. But with only 0-25% difference for common cases, the hybrid approach provides the best balance of usability and performance.

These findings were incorporated into the updated documentation to provide a more accurate picture of the v2.6 implementation's characteristics. 