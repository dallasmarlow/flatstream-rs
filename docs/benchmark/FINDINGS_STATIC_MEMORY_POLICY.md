# Findings: static memory-policy dispatch

**Author:** maintainer-directed memory-policy refactor
**Date:** 2026-07-25
**Status:** historical pre-final-writer measurement — static-dispatch and
allocation conclusions remain, but exact writer instruction/time deltas predate
the final fail-stop gate and should be recollected before publication

## Hypothesis

Replacing `Option<Box<dyn MemoryPolicy>>` and the boxed writer builder factory
with generic policy state should:

1. remove all memory-policy work from the default `NoMemoryPolicy` path;
2. make installed policy calls monomorphized and inlineable;
3. preserve the real algorithmic cost of an installed adaptive policy; and
4. preserve zero-allocation steady state and reclamation behavior.

“Dispatch-free” does not mean an installed policy does no work. Capacity reads,
the baseline gate, and adaptive counters still execute when configured.

## Methodology

### Wall clock

Collected one Criterion group at a time on the Apple M4 / macOS / Rust 1.97.1
environment stamped in the raw files:

```bash
scripts/bench_isolated.sh e4_memory_writer \
  memory_policy_benchmarks policy_overhead -- --locked
scripts/bench_isolated.sh e4_memory_reader_recheck \
  memory_policy_benchmarks reader_policy_overhead -- --locked
```

`GateOpenNoOp` sets a one-byte baseline so the installed-policy backend must
read capacity, pass the baseline gate, and invoke a statically known no-op
decision. The adaptive arm keeps the same open gate but runs its real
bookkeeping without reclaiming.

### Instruction counts

`scripts/instruction_counts.sh` runs the 100-frame writer workloads under the
pinned aarch64 Linux / Rust 1.97.1 / Valgrind / Gungraun environment. Raw output:
`docs/benchmark/raw/e4_instruction_counts.txt`. The preceding pinned run is
preserved in `e3_instruction_counts.txt`, providing the pre-refactor default
writer count in the same container/toolchain configuration.

## Findings

### F1. Default writer

The pinned default writer fell from 27,592 to 26,771 instructions per 100
frames after removing the `Option` check:

- **−821 instructions / 100 frames**
- **−8.21 instructions/frame**
- **−2.98 %**

This is the categorical goal: the default type now carries the zero-sized
`NoMemoryPolicy`, and its backend call inlines away.

### F2. Installed writer policy

| Configuration | Criterion ns/write | Delta from default |
|---|---:|---:|
| `NoMemoryPolicy` default | 8.329 | — |
| static gate-open no-op | 8.353 | +0.024 ns (+0.29%; confidence intervals overlap) |
| static adaptive, inactive | 10.826 | +2.498 ns (+30.0 %) |

Criterion cannot resolve a cost for the static no-op policy. The pinned
instruction counts can:

| Configuration | Instructions / 100 frames | Delta |
|---|---:|---:|
| default | 26,771 | — |
| static memory policy, not due | 28,585 | +1,814 (+6.8%; 18.14/frame) |

The installed path is dispatch-free, not work-free: the 18 instructions/frame
cover the capacity probe, baseline comparison, and inlined no-op decision. The
adaptive policy's larger wall-clock delta is its actual ratio/counter logic.

### F3. Reader policy

The required reader recheck measured 5.380 µs per 1,000 frames without a policy
and 5.289 µs with the static gate-open no-op. The apparent 1.7 % improvement is
not attributed to the policy; it is a code-layout/codegen effect. What survives
the recheck is narrower: installing the static no-op policy produced **no
reader regression detectable by this harness**.

### F4. Correctness and allocation

The existing writer/reader reclamation suite passes unchanged, including custom
builder factories and deferred reader shrink. The default marker, policy state,
and custom factory are all concrete generic types; no policy/factory `Box` or
vtable remains. Allocation tests continue to report exactly zero allocations
and reallocations in warmed steady-state loops.

The explicit oscillation workload (`e4_memory_reclamation.txt`) remains an
intentional cost/footprint trade: ten grow-shrink cycles take 1.167 ms with
adaptive reclamation versus 0.427 ms while retaining the high-water allocation
(2.73× CPU). Static dispatch removes indirection; it does not make allocator
churn free.

## Conclusion

Memory policies are now statically dispatched on both writer and reader.
The default path removes 8.21 instructions/frame from the prior implementation.
An installed gate-open no-op costs 18.14 instructions/frame but no resolvable
wall-clock time in the writer microbenchmark; adaptive policy bookkeeping
remains measurable because it is real policy work rather than dispatch.

## Threats to validity

- Wall-clock deltas at hundredths of a nanosecond are below this workstation's
  stable resolution; instruction counts are the deciding instrument.
- The reader's apparent speedup is not a supported causal claim.
- Instruction deltas are valid only in the recorded pinned environment.
- Reclamation costs intentionally allocate when a policy fires; zero-allocation
  claims apply to warmed steady state, not the opt-in reclaim event.
