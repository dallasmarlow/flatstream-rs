# FlatStream-RS Benchmarking Guide

## Overview

This guide covers the comprehensive benchmarking strategy for `flatstream-rs`, including performance regression detection, comparative analysis against other serialization libraries, and detailed performance analysis methodologies.

## **v2.5 Benchmarking Strategy Update**

### **Establishing v2 as the Definitive Performance Baseline**

The comprehensive performance comparison between v1 and v2 has been completed and documented in `BENCHMARK_COMPARISON.md`. The v2 trait-based architecture has been established as the **definitive performance baseline** with significant improvements:

- **Write Performance**: ~89-90% faster than v1
- **Read Performance**: ~5-80% faster than v1 (depending on scenario)
- **High-Frequency Operations**: ~80-90% faster than v1

### **v2.5 Benchmarking Goals**

The primary goal of v2.5 benchmarking is to **defend the v2 performance baseline** against regressions while validating that the new "Processor API" design maintains or improves upon v2 performance characteristics.

#### **Key Performance Validation Points**

1. **Zero-Allocation Writes**: Verify that external builder management maintains v2 write performance
2. **Zero-Copy Reads**: Confirm that `process_all()` and `messages()` maintain v2 read performance
3. **Hot Loop Performance**: Validate that the new API patterns don't introduce overhead in high-frequency scenarios
4. **Memory Efficiency**: Ensure that removing internal builder management doesn't impact memory usage

#### **Regression Detection Strategy**

The v2.5 implementation will be measured against the v2 baseline using the same rigorous methodology:

```bash
# Establish v2 baseline (final v2 performance)
git checkout main
cargo bench -- --save-baseline v2_final

# Implement v2.5 changes
git checkout v2.5-processor-api
cargo bench -- --save-baseline v2_5_implementation

# Compare results
critcmp v2_final v2_5_implementation
```

**Acceptance Criteria**: v2.5 performance should be within ±2% of v2 baseline across all benchmarks.

## Benchmark Categories

### 1. Core Performance Benchmarks
- **Write Performance**: Default framer, XXHash64, CRC32, CRC16 checksums
- **Read Performance**: Default deframer, XXHash64, CRC32, CRC16 checksums  
- **Zero-Allocation Reading**: High-performance pattern comparison
- **End-to-End Cycles**: Complete write-read cycle performance
- **Parameterized Checksum Comparison**: Direct performance comparison across checksum algorithms

### 2. Real-World Scenario Benchmarks
- **High-Frequency Telemetry**: 1000 message scenarios
- **Large Messages**: Real-world message size simulation
- **Memory Efficiency**: Buffer usage and allocation analysis

### 3. Regression Detection Benchmarks
- **Small Message Writing**: Most sensitive to dispatch overhead
- **Monomorphization Stress**: Tests compiler optimization boundaries
- **Instruction Cache Pressure**: Tests binary size and cache efficiency

### 4. Comparative Benchmarks
- **vs Bincode / Serde JSON**: Length-prefixed serialization comparison (implemented)
- **vs Protobuf**: Length-delimited encoding comparison (future work)

### 5. **v2.5-Specific Benchmarks**
- **External Builder Performance**: Validate zero-allocation write patterns
- **Processor API Performance**: Measure `process_all()` vs `messages()` performance
- **Closure Overhead**: Verify that closure-based processing doesn't introduce overhead
- **Hot Loop Optimization**: Test the new API in high-frequency telemetry scenarios

## **Historical Regression Detection Strategy (Archived)**

*Note: This section documents the original v1 vs v2 comparison methodology for historical reference.*

### What If the 8% Performance Degradation Was Real?

For a high-performance library, an 8% regression is a serious issue that warrants investigation. While Rust's zero-cost abstractions are powerful, there are subtle scenarios where a refactor like this could theoretically introduce overhead.

#### Potential Causes of a Real Performance Regression

**1. Failure to Monomorphize (Dynamic Dispatch)**
- **What it is**: If the compiler cannot specialize the generic code and falls back to using trait objects (`dyn Framer`), it would introduce dynamic dispatch.
- **Impact**: Instead of a direct function call, the program has to look up the correct function pointer in a virtual table (vtable) at runtime. This vtable lookup is a small but measurable overhead on every single write call. More importantly, it acts as an optimization barrier, preventing the compiler from inlining the framing logic. An 8% slowdown is plausible in this scenario, especially for very small messages where the dispatch overhead is a larger percentage of the total work.

**2. Compiler Optimization Boundaries**
- **What it is**: The new function boundaries between the `StreamWriter` and the `Framer` trait could prevent certain optimizations.
- **Impact**: Compilers perform complex analyses to optimize loops, vectorize instructions (SIMD), and reorder operations. Sometimes, breaking a large, monolithic function into smaller ones can prevent the compiler from "seeing" the full scope of an operation, thus inhibiting these optimizations. This is less likely with modern LLVM, but it's possible that the v1 design's monolithic loop was uniquely suited to a specific optimization that is no longer possible in the v2 design.

**3. Increased Binary Size and Instruction Cache Misses**
- **What it is**: Monomorphization creates a copy of the generic function for each concrete type. If you used many different `Framer` types, this could lead to a larger binary.
- **Impact**: The CPU stores recently used machine code instructions in a very fast, small memory called the L1 instruction cache (i-cache). If the program's code is too large or jumps around too much, the CPU has to fetch instructions from slower memory, causing a "cache miss." This would likely only be a factor in extremely complex applications using dozens of different framing strategies, but it is a real-world performance consideration in large systems.

### How Would We Know? (Detecting the Regression)

Yes, we would absolutely know, and we would detect it with a rigorous benchmarking process. The key is to compare the performance of two different versions of the code under the exact same conditions.

#### Step-by-Step Guide to Detecting a Regression

**1. Establish a Baseline**
```bash
# Check out the last known "good" commit (e.g., the final commit of the v1 design)
git checkout <v1_commit_hash>
cargo bench -- --save-baseline v1
```

**2. Run Benchmarks on the New Code**
```bash
# Check out the new commit (the v2 refactor)
git checkout main
cargo bench -- --save-baseline v2
```

**3. Compare the Results**
```bash
# Install the comparison tool if you haven't already
cargo install critcmp

# Compare the two saved baselines
critcmp v1 v2
```

The output would look something like this, immediately highlighting any regressions:
```
group                            v1                                   v2
-----                            -----                                ---
write_100_messages_checksum      1.2134ms                             1.3105ms (+8.00%)
read_100_messages_checksum       0.8451ms                             0.9127ms (+8.00%)
```

This process provides concrete, undeniable evidence of a performance change, allowing us to pinpoint exactly which operations were affected.

## Parameterized Checksum Benchmarking

### Direct Performance Comparison Across Checksum Algorithms

The benchmark suite includes parameterized benchmarks that directly compare the performance characteristics of different checksum algorithms. This provides empirical data to help users choose the optimal checksum for their specific use case.

#### Implementation Details

The parameterized benchmarks use Criterion's `BenchmarkId` and `Throughput` features to create fair comparisons:

```rust
/// Generic benchmark function for writing with a given checksum algorithm
fn bench_write_with_checksum<C: Checksum + Default + Copy>(c: &mut Criterion, checksum_name: &str) {
    let mut group = c.benchmark_group("write_checksum_variants");
    let messages = create_test_messages(SMALL_MESSAGE_COUNT);
    
    // Calculate total throughput in bytes for fair comparison
    let total_bytes: usize = messages.iter().map(|msg| msg.len()).sum();
    group.throughput(Throughput::Bytes(total_bytes as u64));

    group.bench_with_input(
        BenchmarkId::new("write_100_messages", checksum_name), 
        &messages, 
        |b, msgs| {
            b.iter(|| {
                let mut buffer = Vec::new();
                let checksum = C::default();
                let framer = ChecksumFramer::new(checksum);
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
                
                for message in msgs {
                    writer.write(message).unwrap();
                }
                
                black_box(buffer);
            });
        }
    );

    group.finish();
}
```

#### Benchmark Categories

The parameterized benchmarks cover three key scenarios:

1. **Write Performance**: Measures the overhead of different checksum algorithms during serialization
2. **Read Performance**: Measures the overhead of checksum verification during deserialization  
3. **Write-Read Cycle**: Measures end-to-end performance including both serialization and verification

#### Sample Results

When running with all checksums enabled (`cargo bench --features all_checksums`), the results show:

```
write_checksum_variants/write_100_messages/XXHash64
                        time:   [2.1538 µs 2.1947 µs 2.2626 µs]
                        thrpt:  [1.1072 GiB/s 1.1415 GiB/s 1.1632 GiB/s]

write_checksum_variants/write_100_messages/CRC32
                        time:   [2.3239 µs 2.3422 µs 2.3614 µs]
                        thrpt:  [1.0609 GiB/s 1.0696 GiB/s 1.0780 GiB/s]

write_checksum_variants/write_100_messages/CRC16
                        time:   [5.0367 µs 5.0529 µs 5.0714 µs]
                        thrpt:  [505.86 MiB/s 507.70 MiB/s 509.33 MiB/s]
```

#### Performance Insights

The benchmarks reveal important trade-offs:

- **XXHash64**: Fastest algorithm, 8-byte checksum, excellent for high-performance scenarios
- **CRC32**: Good balance of speed and error detection, 4-byte checksum
- **CRC16**: Slowest algorithm, 2-byte checksum, suitable for space-constrained scenarios

#### Usage

To run parameterized checksum benchmarks:

```bash
# Run with specific checksum features
cargo bench --features xxhash
cargo bench --features crc32
cargo bench --features crc16

# Run with all checksums for comparison
cargo bench --features all_checksums
```

## Comparative Benchmarking

### Benchmarking Against Bincode and Protobuf

Yes, this is not only possible but is a crucial step in understanding how `flatstream-rs` positions itself in the broader ecosystem. This involves creating a new benchmark suite that compares the end-to-end process of serializing, framing, and writing data for each format.

#### Step-by-Step Guide to Comparative Benchmarking

**Step 1: Add Dependencies to Cargo.toml**
```toml
[dev-dependencies]
# ... existing dev-dependencies ...
serde = { version = "1.0", features = ["derive"] }
bincode = "1.3"
prost = "0.12"

[build-dependencies]
prost-build = "0.12"
```

**Step 2: Define a Common Data Structure and Protobuf Schema**

Create `benches/common.rs`:
```rust
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct TelemetryData {
    pub timestamp: u64,
    pub device_id: String,
    pub value: f64,
    pub is_critical: bool,
}
```

Create `benches/telemetry.proto`:
```protobuf
syntax = "proto3";

package telemetry;

message TelemetryData {
  uint64 timestamp = 1;
  string device_id = 2;
  double value = 3;
  bool is_critical = 4;
}
```

**Step 3: Create a build.rs to Compile the Proto File**
```rust
// build.rs (in the root of the project)
fn main() {
    prost_build::compile_protos(&["benches/telemetry.proto"], &["benches/"]).unwrap();
}
```

**Step 4: Implement the Benchmark**

The comparative benchmarks are structured in `benches/benchmarks.rs` and provide a fair, apples-to-apples comparison of the end-to-end performance of serializing and framing a stream of data, giving you invaluable insights into where `flatstream-rs` excels and where other libraries might have an advantage.

## Running Benchmarks

### Basic Benchmark Commands
```bash
# 1) Core suite (flatstream-only benches)
cargo bench | tee bench_results.txt

# 2) Comparative suite (flatstream vs bincode/serde_json)
cargo bench --features comparative_bench --bench comparative_benchmarks | tee bench_results.comparative.txt

# 3) Simple streams suite (primitive types, plus read-only deframer isolation)
cargo bench --features comparative_bench --bench simple_benchmarks | tee bench_results.simple.txt

# Save a baseline for comparison
cargo bench -- --save-baseline my_baseline

# Compare baselines
critcmp baseline1 baseline2
```

### Regression Detection Workflow
```bash
# 1. Establish baseline on known good version
git checkout v1.0.0
cargo bench -- --save-baseline v1_baseline

# 2. Test current version
git checkout main
cargo bench -- --save-baseline current_baseline

# 3. Compare results
critcmp v1_baseline current_baseline

# 4. Analyze specific regressions
critcmp v1_baseline current_baseline --threshold 5.0
```

### Comparative Benchmarking
```bash
# Run comparative benchmarks (when implemented)
cargo bench --bench comparative_benchmarks

# Run with different data sizes
cargo bench --bench comparative_benchmarks -- --parameter message_count=1000
```

## Performance Analysis

### Key Metrics to Monitor

**1. Throughput Metrics**
- Messages per second (write/read)
- Bytes per second (write/read)
- End-to-end latency

**2. Memory Metrics**
- Buffer usage patterns
- Allocation frequency
- Memory efficiency

**3. CPU Metrics**
- Instruction cache misses
- Branch prediction accuracy
- CPU utilization

### Performance Thresholds

**Acceptable Performance Ranges:**
- **Write Performance**: < 2µs per 100 messages
- **Read Performance**: < 3µs per 100 messages
- **Zero-Allocation Reading**: > 80% improvement over iterator
- **Memory Efficiency**: < 1KB overhead per 1000 messages

**Regression Thresholds:**
- **Critical**: > 10% performance degradation
- **Warning**: > 5% performance degradation
- **Monitor**: > 2% performance degradation

## Continuous Benchmarking

### CI/CD Integration

**GitHub Actions Workflow:**
```yaml
name: Performance Benchmarks
on: [push, pull_request]

jobs:
  benchmark:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      - run: cargo bench --features all_checksums
      - run: cargo install critcmp
      - run: critcmp baseline current --threshold 5.0
```

### Automated Regression Detection

**Pre-commit Hooks:**
```bash
#!/bin/bash
# .git/hooks/pre-commit

# Run regression-sensitive benchmarks
cargo bench --bench benchmarks -- regression_small_messages
cargo bench --bench benchmarks -- regression_monomorphization

# Compare against baseline
critcmp baseline current --threshold 2.0
```

## Best Practices

### 1. Consistent Environment
- Use the same hardware for baseline comparisons
- Control for system load and background processes
- Run benchmarks multiple times to account for variance

### 2. Representative Workloads
- Test with realistic data sizes and patterns
- Include edge cases and stress scenarios
- Test both small and large message counts

### 3. Comprehensive Coverage
- Test all feature combinations
- Include regression detection scenarios
- Compare against industry standards

### 4. Documentation
- Document benchmark methodology
- Track performance changes over time
- Maintain baseline comparisons

## Troubleshooting

### Common Issues

**1. High Variance in Results**
- Check for background processes
- Ensure consistent CPU frequency
- Run more iterations

**2. Unexpected Performance Changes**
- Verify compiler optimizations are enabled
- Check for code generation differences
- Analyze assembly output

**3. Comparative Benchmark Failures**
- Ensure fair comparison methodology
- Verify dependency versions
- Check for implementation differences

### Performance Investigation Tools

**1. Criterion Analysis**
```bash
# Generate detailed HTML reports
cargo bench -- --verbose
```

**2. Assembly Analysis**
```bash
# Generate assembly for specific functions
cargo rustc --release -- --emit asm
```

**3. Profiling**
```bash
# Profile with perf
perf record --call-graph=dwarf cargo bench
perf report
```

## Conclusion

This comprehensive benchmarking strategy ensures that `flatstream-rs` maintains high performance while providing confidence in the library's capabilities. The combination of regression detection, comparative analysis, and real-world scenario testing provides a complete picture of the library's performance characteristics.

For questions or contributions to the benchmarking suite, please refer to the project's contribution guidelines. 