# Findings: caller-scratch positioned reads

**Author:** maintainer-directed positioned-read implementation
**Date:** 2026-07-25
**Status:** complete; read-counted point-lookup correction added 2026-07-29

## Hypothesis

`read_frame_at` should eliminate the categorical allocation cost of constructing
a fresh `StreamReader` for every indexed lookup. It may also reduce elapsed
time, but that is not guaranteed: a retained `BufReader` can reduce read
syscalls for small frames even though each seek invalidates its buffered
position.

## Methodology

The benchmark writes 1,000 CRC-32 frames, retains one file handle, and cycles
through indexed offsets. It compares:

1. seek + fresh `BufReader` + fresh `StreamReader`;
2. the original caller-scratch implementation over bare `File`, including a
   post-read `stream_position()` call;
3. corrected `read_frame_at` over bare `File`, where a per-call
   `CountingReader` supplies `wire_len` without a second seek syscall;
4. corrected `read_frame_at` over one retained `BufReader<File>` with warmed caller
   scratch.

```bash
POSITIONED_READS_CASE=point scripts/bench_isolated.sh \
  d_point_read_counted positioned_reads '' -- --features crc32 --locked
POSITIONED_READS_CASE=forward scripts/bench_isolated.sh \
  e5_forward_position positioned_reads '' -- --features crc32 --locked
```

Raw output: `docs/benchmark/raw/d_point_read_counted.txt` and the original
`e5_forward_position.txt`.

`tests/allocation.rs` is the categorical instrument: a fresh reader allocates
on every lookup, while warmed `read_frame_at` allocates and reallocates exactly
zero times.

## Findings

| Payload | Fresh reader | Old post-read position | Read-counted `File` | Read-counted `BufReader<File>` |
|---|---:|---:|---:|---:|
| 4 KiB | 1.0736 µs | 1.6036 µs | 1.4473 µs | 0.9763 µs |
| 64 KiB | 11.115 µs | 10.532 µs | 10.395 µs | 10.219 µs |

Removing the post-read position query reduces the bare-file median by **9.7%**
at 4 KiB (1.6036 → 1.4473 µs). At 64 KiB the payload/checksum dominates; the
paired median moves **1.3%** (10.532 → 10.395 µs), a small machine-specific
result rather than a portable speed claim. A deterministic seek-counting test
pins the structural result: one point lookup performs exactly the initial seek,
and receipt length comes from bytes returned through `Read`.

Retained buffering remains workload-dependent. It wins clearly at 4 KiB in this
run and converges with the bare-file path at 64 KiB.

The stable result is allocation behavior, not universal throughput:

- fresh `StreamReader` per lookup allocates a frame buffer every time;
- warmed caller scratch allocates zero times;
- the best source wrapper depends on frame size and OS read behavior.

Palimpsest currently also opens the segment file per cache miss. That open cost
is outside this benchmark; keeping segment handles open is an application-level
decision and must be measured separately.

### Forward tracking overhead

The same-run forward benchmark compares the pre-feature shape (direct deframer
loop over reused scratch) with `StreamReader`'s counted, receipt-capable path:

| Payload | Uncounted loop / 1000 | Counted StreamReader / 1000 |
|---|---:|---:|
| 4 KiB | 417.65 µs | 417.30 µs |
| 64 KiB | 6.783 ms | 6.761 ms |

Confidence intervals overlap in both cases. This harness resolves no forward
read regression from byte counting and receipt arithmetic.

## Conclusion

`read_frame_at` now obtains exact receipts without a redundant post-read seek
query. Ship it primarily for ownership/allocation control; the clear 4 KiB
bare-file improvement is mechanism-aligned, while source buffering and
large-frame elapsed time remain workload-dependent. Recommend a retained
`BufReader<File>` for Palimpsest's smaller frames only after application
measurement. Forward position tracking has no resolved wall-clock cost in the
paired benchmark.

## Threats to validity

- One Apple M4/macOS/filesystem.
- Hot page cache; no storage-latency or cold-cache characterization.
- File-open and application LRU costs excluded.
- Payloads are synthetic FlatBuffer vectors; Palimpsest frames have different
  schema and size distributions.
