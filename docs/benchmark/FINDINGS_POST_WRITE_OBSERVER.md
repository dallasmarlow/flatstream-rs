# Findings: what does an installed post-write observer cost?

**Author:** maintainer-directed B3 follow-up
**Date:** 2026-07-29
**Status:** historical pre-final-writer measurement — raw output at
`docs/benchmark/raw/b3_post_write_observer.txt`. The final fail-stop writer adds
the same poison-state gate to both arms, but this exact delta has not been
recollected on the release candidate.

## Hypothesis

The zero-sized `NoPostWriteObserver` default should leave observation absent
from the monomorphized write path. Installing a concrete observer that records
receipt, payload length, and elapsed time should add a measurable per-frame
cost dominated by the monotonic-clock pair, while retaining zero-allocation
steady state.

## Methodology

### Environment

- rustc: `rustc 1.97.1 (8bab26f4f 2026-07-14) (Homebrew)`
- OS / arch: Darwin 25.5.0 / arm64
- CPU: Apple M4, 10 cores, 32 GiB
- Tools: Criterion 0.5.1, default sampling; Gungraun/Callgrind 0.19.4
  in the pinned Rust 1.97.1 aarch64 Linux container
- Feature flags: none
- Baseline: paired `StreamWriter` with the default
  `NoPostWriteObserver`, collected inside the same isolated run

### Steps

```bash
scripts/bench_isolated.sh b3_post_write_observer \
  post_write_observer '' -- --locked
scripts/instruction_counts.sh 2>&1 | \
  tee docs/benchmark/raw/b3_instruction_counts.txt
```

Both arms write the same finished 64-byte FlatBuffer 1,000 times through the
same pre-sized in-memory sink and consume the final byte count. The installed
arm additionally consumes every event's payload length, elapsed duration, and
receipt through `black_box`; this prevents LLVM from erasing the clock reads or
callback. Criterion reports batch time; per-frame figures below divide the
batch median by 1,000.

Allocation is checked categorically rather than inferred from timing:
`tests/allocation.rs` arms the counting allocator around 256 observed writes and
requires exactly zero allocations and reallocations.

## Findings

| Writer state | 1,000-frame median [95% CI] | Median per frame |
|---|---:|---:|
| default `NoPostWriteObserver` | 1.9884 µs [1.9855, 1.9917] | 1.988 ns |
| installed receipt + latency observer | 33.620 µs [33.543, 33.749] | 33.620 ns |

The installed observer adds **31.632 ns/frame** in this in-memory workload. The
counting allocator reports zero allocations and reallocations for every
observed frame. The event callback runs exactly once for success,
serialization failure, write failure, and durability failure; correctness
tests, not this benchmark, pin those classifications.

The default state is zero-sized and its `ENABLED = false` specialization removes
event timing/callback work at compile time. This benchmark uses that default as
the paired baseline; it does not compare against a historical pre-B3 binary and
therefore makes no cross-revision nanosecond claim.

### Pinned instruction-count sanity check

The current default `write_default` workload executes 26,812 instructions per
100 frames. The preceding locally retained pinned snapshot
(`e4_instruction_counts.txt`) recorded 26,771: **+41 instructions per 100
frames (+0.15%, 0.41/frame)** across the full correction-set revision. Treat
that as an upper bound, not an exact observer attribution: the run includes the
other source corrections in this review pass, and Gungraun's machine-local
"change" columns point at a different saved baseline. The raw current counts and
environment fingerprint are in `b3_instruction_counts.txt`.

## Conclusion

The first-party post-write hook is suitable as an explicit opt-in. Its measured
installed cost is real and dominated by timing, but it exists only in the
installed concrete type and allocates nothing per frame. The default writer
performs no clock read/callback; the full correction set is within 0.41
instruction/frame of the preceding pinned default snapshot.

No OTEL or metrics dependency follows. Applications translate
`PostWriteEvent` into their own counters/traces, and can omit the observer
entirely on paths where roughly 32 ns/frame matters.

## Threats to validity

- One Apple-M4/macOS run; `Instant::now()` cost is platform-dependent.
- The sink is a hot pre-sized `Vec`, so the observer is a large fraction of an
  unrealistically cheap write. Real file/network I/O changes the percentage;
  only the absolute paired delta belongs to this workload.
- The callback performs minimal `black_box` consumption. A metrics backend,
  locking, formatting, or sampling policy adds application-specific work.
- No historical binary baseline was collected. The default specialization's
  zero-sized/type-level shape and allocation behavior are categorical; elapsed
  cross-revision equivalence is not claimed.
