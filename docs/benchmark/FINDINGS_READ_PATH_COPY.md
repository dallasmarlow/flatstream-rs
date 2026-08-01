# Findings: what fraction of a frame read is the one unavoidable payload copy?

**Author:** Dallas Marlow
**Date:** 2026-07-29
**Status:** complete — isolated runs collected on an Apple M4 Pro, one framing
group at a time, with the CRC-32 256 KiB point recollected (see Findings)

> This document is the A3 deliverable: `benches/read_path_copy.rs` plus the
> measured result below. Numbers were collected one group at a time with
> `scripts/bench_isolated.sh` (§4) and the stamped raw snapshots are committed
> under `docs/benchmark/raw/`.

## Hypothesis

Reading one frame from a generic [`Read`] source copies the payload exactly
once — `read_exact(&mut buffer[..len])` inside `read_payload` (`src/framing.rs`)
— before the payload is handed out as a borrowed `&[u8]`. This is the single
place the current design is not copy-free (README TL;DR; `docs/DESIGN_v2_7.md`
§1), and a borrowed-slice/mmap source that removes it is named there as future
work.

Predictions, stated so the data can refute them:

1. **The copy is O(payload).** Its per-frame cost rises roughly linearly with
   frame size, while header parsing and loop overhead stay flat.
2. **The copy's *fraction* of read time climbs with frame size.** At 64 B the
   fixed header/loop work dominates and the copy is a small share; by 256 KiB the
   copy should dominate default-framed reads.
3. **CRC-32 lowers the copy's fraction, not its absolute cost.** Verification
   adds a second O(payload) pass over the same bytes, so the copy is a smaller
   share of a checksummed read even though the memcpy itself is unchanged.
4. **The copy tracks a raw memcpy.** `read_copy − borrow_slice` should be close
   to `memcpy_only` (the cross-check), especially at large sizes. If it is not,
   the A/B is measuring something other than the copy and the other numbers are
   not trustworthy.

The counter-hypothesis worth being wrong about: if the copy is a negligible
fraction at every size, then a borrowed-slice source buys little on read
throughput and its value would rest on allocation/lifetime ergonomics instead.

## Methodology

`benches/read_path_copy.rs` frames a fixed payload many times into one
in-memory wire and reads it three ways per (framing, size), all over identical
bytes and reading the identical payload:

| Arm | Does | Copies payload? |
|-----|------|-----------------|
| `read_copy`    | the real `Deframer::read_and_deframe` into a reused, pre-sized buffer | **yes** — the memcpy under test |
| `borrow_slice` | identical header parse + (CRC-32) verify over the borrowed bytes, then borrows `&wire[..]` | no |
| `memcpy_only`  | cross-check: raw `copy_from_slice` of the payload bytes only | yes |

- **Copy cost per frame** = `read_copy − borrow_slice`.
- **Copy fraction of read time** = `(read_copy − borrow_slice) / read_copy`.
- `borrow_slice` performs the *same* checksum verification as `read_copy`, so the
  delta isolates the memcpy rather than the CRC pass. `memcpy_only` validates
  that delta against a raw copy.

Framing: **default** (`NoChecksum`, 4-byte header) and **CRC-32**
(`ChecksumFramer`/`ChecksumDeframer`, 8-byte header). Sizes: **64 B, 4 KiB,
64 KiB, 256 KiB**. Per-size frame counts target ~8 MiB of wire (floored at 64
frames) so working sets stay comparable across sizes. The buffer is pre-sized to
the payload, so `read_copy` measures the warmed, allocation-free steady state,
not buffer growth.

### Instrument: wall-clock, and why (not instruction counts)

This is a **wall-clock** (Criterion) experiment, deliberately, even though the
sibling A2 position-accounting work used instruction counts. The quantity under
test is a `memcpy`, which is **memory-bandwidth-bound, not
instruction-bound**: on a wide payload it is a handful of vector-store
instructions moving kilobytes, so an instruction count would report a tiny,
size-insensitive number and badly understate the real cost of moving the bytes.
Time is the honest instrument for a copy, and "fraction of read time" is by
definition a time ratio. The trade-off is Criterion's noise (§4): report medians
and treat sub-noise deltas as no change.

### Environment

Stamped into each raw file by `scripts/bench_isolated.sh`:

- rustc: `rustc 1.97.1 (8bab26f4f 2026-07-14) (Homebrew)` — the MSRV
- OS / arch: `Darwin 25.5.0 arm64` (macOS)
- CPU: Apple M4 Pro (`Mac16,7`) — P-core L1d 128 KiB, 16 MiB L2 shared per
  five-core P-cluster (E-cores: 64 KiB / 4 MiB; `sysctl hw.perflevel*`). The
  working sets (~7–16 MiB of wire) sit at or near the shared L2's capacity, so
  every arm streams from the outer cache/memory boundary, not from L1 — see
  threats.
- Relevant dependency versions: `crc32fast 1.5.0`
- Tool: Criterion 0.5.1
- Feature flags: `--features crc32`
- Baseline: N/A — every comparison is an A-vs-B of arms collected inside one run

### Steps

```bash
# One group at a time, machine otherwise idle. Filter by
# framing so default and CRC-32 are separate isolated collections.
scripts/bench_isolated.sh a3_read_copy_default read_path_copy 'default/' -- --features crc32 --locked
scripts/bench_isolated.sh a3_read_copy_crc32   read_path_copy 'crc32/'   -- --features crc32 --locked

# Required cross-check recollection: read_copy − borrow_slice diverged from
# memcpy_only for CRC-32, so the largest CRC-32 point was recollected before it
# was written down (§4). It reproduced the primary run.
scripts/bench_isolated.sh a3_read_copy_crc32_recheck read_path_copy 'crc32/262144B' -- --features crc32 --locked
```

All reported figures are **per frame**: Criterion's `time:` line is per pass over
the whole wire, and `Throughput::Elements` is the frame count, so per-frame =
per-pass median ÷ frame count. Frame counts this run: 104 857 (64 B), 2 040
(4 KiB), 127 (64 KiB), 64 (256 KiB).

## Findings

Per-frame medians. `read_copy` is the real read; `borrow_slice` is the modeled
borrowed source (same header + CRC work, no copy); `memcpy_only` is the isolated
payload copy. "copy fraction (memcpy/read)" is the robust estimate; "(read−borrow)/read"
is the subtraction estimate, shown for comparison.

| Framing | Size | `read_copy` | `borrow_slice` | `memcpy_only` | copy fraction (memcpy/read) | (read−borrow)/read |
|---------|------|-------------|----------------|---------------|-----------------------------|--------------------|
| default | 64 B    | 1.97 ns    | 0.68 ns    | 1.79 ns    | 91%  | 66%  |
| default | 4 KiB   | 60.97 ns   | 3.07 ns    | 59.89 ns   | 98%  | 95%  |
| default | 64 KiB  | 774.33 ns  | 6.49 ns    | 777.78 ns  | 100% | 99%  |
| default | 256 KiB | 3.25 µs    | 6.23 ns    | 3.26 µs    | 100% | 100% |
| crc32   | 64 B    | 6.85 ns    | 3.68 ns    | 1.78 ns    | 26%  | 46%  |
| crc32   | 4 KiB   | 394.96 ns  | 337.96 ns  | 68.17 ns   | 17%  | 14%  |
| crc32   | 64 KiB  | 6.56 µs    | 5.55 µs    | 766.65 ns  | 12%  | 15%  |
| crc32   | 256 KiB | 25.70 µs   | 23.56 µs   | 3.30 µs    | 13%  | 8%   |

Raw snapshots: `docs/benchmark/raw/a3_read_copy_default.txt`,
`docs/benchmark/raw/a3_read_copy_crc32.txt`, and the required recheck
`docs/benchmark/raw/a3_read_copy_crc32_recheck.txt` (read_copy 1.6430 ms,
borrow_slice 1.4189 ms, memcpy_only 199.22 µs per pass over 64 frames; ÷ 64
gives 25.67 µs / 22.17 µs / 3.11 µs per frame — read_copy reproduces the primary
run exactly, and borrow_slice/memcpy_only land ~6% below it, within run-to-run
drift).

Three things the numbers establish:

1. **The payload copy is `memcpy_only`, and it is framing-independent** — it moves
   the same bytes regardless of header. Per byte it is ~80 GB/s on this M4 Pro
   (256 KiB: 3.26 µs ≈ 80.4 GB/s), and the rate stays flat from 64 KiB to 256 KiB
   (~84 → ~80 GB/s) as the working set grows from ~8 MiB to the 16 MiB shared-L2
   capacity — a streaming-bandwidth number, not a compute-bound one. Per-frame
   cost rises roughly linearly with payload size, confirming prediction 1.
2. **Without a checksum the copy is essentially the entire read.** `borrow_slice`
   never touches the payload, so `read_copy ≈ memcpy_only`: ~98% of read time at
   4 KiB, ~100% at 64 KiB and above. Prediction 2's rising trend holds, but its
   64 B premise does not — even there the robust estimator reads 91%, because the
   fixed header/loop cost is under a nanosecond per frame (though the 64 B row is
   noise-dominated; see threats). The two independent estimates agree to within
   4% at every size ≥ 4 KiB, which is what licenses treating `memcpy_only` as a
   true memcpy.
3. **With CRC-32 the copy is a minority — ~12–17% at ≥ 4 KiB.** Verification is a
   second O(payload) pass, and on this machine/build it runs at ~11–12 GB/s
   (256 KiB: `borrow_slice` 23.56 µs ≈ 11.1 GB/s), roughly **7× the memcpy's
   cost**, so the checksum dominates a checksummed read and the copy shrinks to a
   small share (prediction 3). This holds the mechanism, not just the ranking:
   `borrow_slice` (verify, no copy) ≈ `read_copy` − `memcpy_only` within 5% at
   every size ≥ 4 KiB (the 64 B row is noise-dominated; see threats).

## Conclusion

This is a **baseline experiment and no code change follows.** The wire format is
unchanged; the read path is unchanged; the invariant ("zero-copy" scopes to
payload *access*; a generic `Read` copies once into the reusable buffer) stands.
What the run adds is a measured size for that copy, to bound what the
future borrowed-slice/mmap source (README TL;DR, `DESIGN_v2_7` §1) could reclaim:

- **Un-checksummed, large frames: the source would reclaim nearly all read cost.**
  At 64 KiB+ the copy is ~100% of an in-memory default-framed read, so a borrowed
  slice removes almost the whole per-frame cost. This is the strongest case for
  building it.
- **Checksummed frames: it reclaims only ~12–17%.** The CRC pass remains and
  dominates, so a borrowed source helps checksummed reads far less. Its value
  there is allocation/lifetime ergonomics, not throughput — and no
  read-throughput claim should be attached to it for checksummed streams.
- **Small frames (64 B): no reliable fraction.** Per-frame times are single-digit
  nanoseconds and the two estimators disagree most there (91% vs 66% default;
  26% vs 46% CRC-32), partly because the model's leaner header parse inflates
  the read-vs-borrow gap. A3 bounds the 64 B copy at ~1.8 ns/frame but does not
  establish its share of read time; do not quote one.

So the borrowed-slice source's read-throughput payoff is real but conditional:
it scales with payload size and with the *absence* of a checksum. Cite this doc,
not a fresh estimate, wherever that future work is scoped.

## Threats to validity

- **In-memory source, no real I/O.** The wire is an in-memory `&[u8]`, so
  `read_copy` excludes syscalls, page-cache misses, and device latency. This is
  deliberate — A3 isolates the *copy*, not the source — but it means the copy
  *fraction* reported here is of an in-memory read; a real-file read spends
  additional time in I/O the copy fraction does not shrink to account for. State
  the numbers as "copy vs in-memory read time," not "copy vs end-to-end read."
- **The working sets straddle the outer cache — neither L1-hot nor pure RAM.**
  All four sizes cycle ~7–16 MiB of wire against a 16 MiB L2 shared per P-core
  cluster (P-core L1d 128 KiB), so the copy streams from the L2/RAM boundary and
  the ~80 GB/s is a streaming figure for that regime. A workload whose frames
  stay hot in L1 would see a cheaper copy and therefore a *smaller* copy
  fraction; a fully cache-cold replay from disk could see a somewhat costlier
  one. The flat per-byte rate from 64 KiB (~8 MiB working set) to 256 KiB
  (~16 MiB) shows the conclusions do not hinge on which side of that boundary a
  size lands.
- **The CRC-32 subtraction is catastrophic cancellation, so it is not the copy of
  record.** For CRC framing `read_copy` and `borrow_slice` are both dominated by
  the verify and differ by only the copy, so `read_copy − borrow_slice`
  subtracts two large near-equal numbers: it read 8% in the primary run and
  ~14% on the recheck of the same 256 KiB point — noise, not signal. The
  reported copy fraction uses `memcpy_only` (a standalone copy, stable across
  both runs — 3.30 → 3.11 µs — and matching the default-framing copy at
  3.26 µs), and the subtraction column is shown only
  for comparison. The default-framing cross-check (agreement < 4% at ≥ 4 KiB) is
  what validates `memcpy_only` as a genuine memcpy.
- **`borrow_slice` models an API that does not exist.** It reimplements the
  header parse (`u32::from_le_bytes` over a subslice, versus the deframer's
  `read_exact` probes), so at 64 B — where the delta is smallest — part of the
  gap is the header-parse *mechanism*, not the payload copy. Treat the 64 B row
  as noise-dominated: the two copy-fraction estimates disagree most there (91% vs
  66% default; 26% vs 46% CRC) precisely because the absolute times are single-ns.
- **The CRC verify cost is machine- and build-specific, so the CRC copy fraction
  is the least portable number here.** `crc32fast 1.5.0` on this
  aarch64-apple-darwin build verified at ~11–12 GB/s (~7× the memcpy). CRC-32 is
  hardware-assisted where the target enables it and scalar otherwise; a build or
  CPU where the verify is faster (or the memory slower) would raise the copy's
  share of a checksummed read. The default-framing fractions do not depend on
  this and are the more portable result.
- **Single machine, one isolated run per framing (+ one recheck).** Criterion
  medians drift between runs (§4 documents −24%/+57% on the dev laptop); here
  `memcpy_only` at 4 KiB differed ~14% between the two framing collections, which
  is why only within-run A-vs-B arms are compared and the two collections are
  never differenced against each other.
- **`memcpy` is bandwidth-bound.** Absolute copy costs scale with the machine's
  memory bandwidth and will not transfer to other hardware; only the *fractions*
  and their trend across sizes are the portable conclusions.

[`Read`]: https://doc.rust-lang.org/std/io/trait.Read.html
