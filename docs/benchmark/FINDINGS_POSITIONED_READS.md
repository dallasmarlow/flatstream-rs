# Findings: caller-scratch positioned reads

**Author:** maintainer-directed positioned-read implementation  
**Date:** 2026-07-25  
**Status:** complete

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
2. `read_frame_at` over a bare `File` with warmed caller scratch;
3. `read_frame_at` over one retained `BufReader<File>` with warmed caller
   scratch.

```bash
scripts/bench_isolated.sh e5_positioned_reads \
  positioned_reads Positioned -- --features crc32 --locked
scripts/bench_isolated.sh e5_forward_position \
  positioned_reads Tracking -- --features crc32 --locked
```

Raw output: `docs/benchmark/raw/e5_positioned_reads.txt` and
`e5_forward_position.txt`.

`tests/allocation.rs` is the categorical instrument: a fresh reader allocates
on every lookup, while warmed `read_frame_at` allocates and reallocates exactly
zero times.

## Findings

| Payload | Fresh reader | `read_frame_at<File>` | `read_frame_at<BufReader<File>>` |
|---|---:|---:|---:|
| 4 KiB | 1.039 µs | 1.584 µs | 1.115 µs |
| 64 KiB | 10.834 µs | 10.291 µs | 10.315 µs |

At 4 KiB, a bare file is 52% slower because the deframer's header/checksum/
payload reads become separate file reads; a retained `BufReader` narrows the
cost to ~7% over the allocating fresh-reader baseline. At 64 KiB, payload work
dominates and both caller-scratch forms are about 5% faster.

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

Ship `read_frame_at` for ownership and allocation control, not as an unconditional
speed claim. Recommend a retained `BufReader<File>` for Palimpsest's smaller
frames only after application measurement; bare `File` and retained buffering
converge for larger payloads. Forward position tracking has no resolved
wall-clock cost in the paired benchmark.

## Threats to validity

- One Apple M4/macOS/filesystem.
- Hot page cache; no storage-latency or cold-cache characterization.
- File-open and application LRU costs excluded.
- Payloads are synthetic FlatBuffer vectors; Palimpsest frames have different
  schema and size distributions.
