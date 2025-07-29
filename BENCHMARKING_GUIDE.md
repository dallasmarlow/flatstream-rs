# FlatStream-RS Benchmarking Guide

## Overview

This guide covers the comprehensive benchmarking strategy for `flatstream-rs`, including performance regression detection, comparative analysis against other serialization libraries, and detailed performance analysis methodologies.

## Benchmark Categories

### 1. Core Performance Benchmarks
- **Write Performance**: Default framer, XXHash64, CRC32 checksums
- **Read Performance**: Default deframer, XXHash64, CRC32 checksums  
- **Zero-Allocation Reading**: High-performance pattern comparison
- **Write Batching**: Batch vs iterative performance comparison
- **End-to-End Cycles**: Complete write-read cycle performance

### 2. Real-World Scenario Benchmarks
- **High-Frequency Telemetry**: 1000 message scenarios
- **Large Messages**: Real-world message size simulation
- **Memory Efficiency**: Buffer usage and allocation analysis

### 3. Regression Detection Benchmarks
- **Small Message Writing**: Most sensitive to dispatch overhead
- **Monomorphization Stress**: Tests compiler optimization boundaries
- **Instruction Cache Pressure**: Tests binary size and cache efficiency

### 4. Comparative Benchmarks
- **vs Bincode**: Length-prefixed serialization comparison
- **vs Protobuf**: Length-delimited encoding comparison

## Regression Detection Strategy

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
# Run all benchmarks
cargo bench

# Run with all checksum algorithms
cargo bench --features all_checksums

# Run specific benchmark groups
cargo bench --bench benchmarks -- regression_small_messages

# Save baseline for comparison
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