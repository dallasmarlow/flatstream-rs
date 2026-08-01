# Findings: static durability-policy cost and checkpoint cadence

**Author:** maintainer-directed E3 implementation
**Date:** 2026-07-24
**Status:** historical pre-final-writer measurements — Criterion and pinned
instruction-count raw outputs committed. Final writer hardening and the restored
interval policy are not characterized by these dispatch numbers; retain the
cadence/storage findings, but recollect before publishing per-frame policy cost.

## Hypothesis

The zero-sized `NoSync` specialization should preserve the pre-policy write
path. Installing a static policy that does not fire adds a small monomorphized
counter/compare cost but no allocation or dynamic dispatch. Once a policy
fires, storage checkpoint latency should dominate and scale with the chosen
cadence; there is no cadence-independent “fsync cost per record.”

## Methodology

### Environment

Record the exact `rustc`, dependency lock, host, and filesystem in each raw
snapshot. Wall-clock results are machine-specific. Instruction counts are
comparable only inside the pinned `scripts/instruction_counts.sh` environment.

### Commands

Run one group at a time on an otherwise idle machine:

```bash
scripts/bench_isolated.sh e3_sync_dispatch sync_policy Dispatch -- --locked
scripts/bench_isolated.sh e3_sync_file sync_policy Cadence -- --locked
scripts/bench_isolated.sh a1_durability write_pipeline_decomposition \
  Durability -- --features crc32 --locked
scripts/instruction_counts.sh
```

`Sync Policy Dispatch` compares the default writer, an installed policy that is
not due, and one mock checkpoint after 1,000 frames. The mock sink isolates
policy machinery; it is not evidence about storage durability latency.

`Sync Policy File Cadence` uses `BufWriter<File>` and real `sync_data` calls at
different frame cadences. `A1 Durability Cadence` repeats the comparison inside
the terminal-journaling workload with framing, CRC-32, index updates, and an
identical record stream.

Allocation behavior is categorical rather than timed:
`tests/allocation.rs` requires both simple-mode writes and a policy-enabled loop
containing a checkpoint to allocate and reallocate exactly zero times in steady
state.

## Findings

### F1. Static policy decision cost

`docs/benchmark/raw/e3_sync_dispatch.txt` measures 1,000 frames per Criterion
iteration:

| Configuration | ns/frame | Delta from default |
|---|---:|---:|
| default `NoSync` | 2.141 | — |
| installed frame-count policy, not due | 2.423 | **+0.281 ns (+13.1%)** |
| installed policy, one mock checkpoint | 2.439 | **+0.298 ns (+13.9%)** |

The selected design achieves zero overhead only for the default type
specialization. Installing a policy is cheap in absolute terms but is not
“free”: its counter and comparison are visible on this extremely small mock
sink workload. One mock checkpoint adds only ~0.017 ns/frame beyond observing
the policy; that number says nothing about real storage.

Both simple mode and the policy-enabled checkpoint loop allocate and reallocate
exactly zero times in the armed steady-state tests.

### F2. Real file checkpoint cadence

`docs/benchmark/raw/e3_sync_file.txt` writes 100 frames through
`BufWriter<File>`:

| Policy | Batch time | Amortized ns/frame | Approx. ms/checkpoint |
|---|---:|---:|---:|
| sync every frame | 372.49 ms | 3 724 900 | 3.72 |
| sync every 10 frames | 39.953 ms | 399 530 | 4.00 |
| sync every 100 frames | 4.389 ms | 43 890 | 4.39 |

Checkpoint count, not policy dispatch, determines elapsed time. The roughly
3.7–4.4 ms/checkpoint range is this filesystem/device only.

### F3. Cadence inside the journaling workload

`docs/benchmark/raw/a1_durability.txt` repeats the terminal-journaling shape
over 16 records:

| Policy | 16-record time | Amortized ms/record |
|---|---:|---:|
| sync every frame | 55.617 ms | 3.476 |
| sync every 4 frames | 14.400 ms | 0.900 |
| sync every 16 frames | 4.342 ms | 0.271 |

The near-linear cadence response is the result A1's original single
once-per-1,000 rung could not establish. “Durability dominates” is meaningful
only after naming the checkpoint policy.

### F4. Instruction counts

The latest pinned aarch64 Linux Gungraun run
(`docs/benchmark/raw/e4_instruction_counts.txt`) counted 100 end-to-end default
writes after the static-memory refactor:

| Configuration | Instructions / 100 frames | Delta |
|---|---:|---:|
| default `NoSync` | 26,771 | — |
| static policy, not due | 29,785 | **+3,014 (+11.3%; 30.14/frame)** |
| static policy, one mock checkpoint | 30,095 | **+3,324 (+12.4%; 33.24/frame)** |

The instruction result confirms the wall-clock mechanism: an installed policy
has a real but small counter/decision cost, while the one mock checkpoint adds
310 instructions to the 100-frame workload. The Gungraun output
also contained machine-local prior baselines for older workloads; E3 uses only
the same-run current counts above.

## Conclusion

The default writer remains a zero-sized, statically dispatched `NoSync`
specialization. An installed non-triggering policy costs about 0.281 ns and
30.1 instructions per frame in the mock-sink workloads and zero allocations;
real durability latency is orders of magnitude larger and scales with
checkpoint count. Every latency claim must name cadence, filesystem, device,
OS, and sync mode.

`SyncEveryInterval` was restored by owner direction after this run. Its
per-accepted-frame monotonic clock check is deliberately opt-in and is not
measured here. Applications may alternatively schedule manual syncs externally.

## Threats to validity

- A mock `Durable` sink measures policy instructions, not persistence.
- Checkpoints delegate to the standard library, which on Apple platforms
  issues `fcntl(F_FULLFSYNC)` for both sync modes (verified in the Rust 1.97.1
  sources; this bullet previously claimed plain `fsync`). Measured sync
  latencies therefore include a full drive-write-cache flush.
- SSD/filesystem cache behavior can move absolute sync latency substantially.
- A cadence measured at 16 or 100 records must not be extrapolated to every
  application batching policy.
