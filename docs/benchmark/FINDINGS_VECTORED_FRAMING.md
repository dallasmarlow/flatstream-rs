# Findings: does collapsing a frame's two writes into one `writev` pay?

**Author:** contributor (E1)
**Date:** 2026-07-24
**Status:** complete — isolated raw `File`, TCP, and `BufWriter` outputs committed

## Hypothesis

Before this change, the two built-in framers put a frame on the wire with two
calls. `DefaultFramer` wrote the length and payload separately:

```rust
writer.write_all(&payload_len.to_le_bytes())?;  // 4 bytes
writer.write_all(payload)?;                     // N bytes
```

`ChecksumFramer` had already merged length and checksum into one stack header in
v0.2.7, then wrote that complete header and the payload as its two calls. E1
changes both built-ins to a two-slice vectored write: complete header plus
payload.

On a `BufWriter` that is two `memcpy`s into the same buffer and costs almost
nothing. On an **unbuffered** sink — a raw `File`, a `TcpStream`, a pipe — each
`write_all` is a separate syscall, so the library imposes **two syscalls per
frame** for what the kernel can accept as one.

Hypothesis: **replacing the two writes with a single `write_vectored` roughly
halves per-frame cost on unbuffered sinks, and is free on buffered ones.**

The second half of that sentence is where the hypothesis can fail, and is the
reason the experiment is worth running. `BufWriter` is the sink this library
recommends in the README, so a `writev` win that costs the recommended path
anything is a bad trade. E1 is therefore as much a regression check on
`BufWriter` as it is a win-measurement on raw sinks.

## Methodology

### Environment

- rustc: `rustc 1.97.1 (8bab26f4f 2026-07-14) (Homebrew)` — the declared MSRV
- OS / arch: macOS 26.5.2 / arm64
- CPU: Apple M4, 10 cores, 32 GiB
- Relevant dependency versions: `flatstream` 0.2.8, `flatbuffers` 25.12.19 (from
  `Cargo.lock`)
- Tool: Criterion 0.5, default sampling, `Throughput::Elements(1000)`
- Feature flags: `crc32` (default and CRC-32 arms)
- Baselines compared against: `TwoCallFramer` and `TwoCallCrc32Framer`, in-bench
  copies of the corresponding pre-E1 bodies — **not** saved Criterion
  baselines. Both include the historical payload-length guard and the checksum
  baseline includes the already-assembled header.

**Instrument choice.** Wall clock (Criterion). The quantity of interest is
syscall cost, which is wall-clock by nature; instruction counts would not capture
it.

### Steps

The full suite must **not** be run in one pass on this hardware — see §T1.
Collect one sink group at a time, with nothing else running:

```bash
scripts/bench_isolated.sh e1_bufwriter vectored_framing bufwriter \
  -- --features crc32 --locked
scripts/bench_isolated.sh e1_file vectored_framing 'file/' \
  -- --features crc32 --locked
scripts/bench_isolated.sh e1_tcp vectored_framing 'tcp/' \
  -- --features crc32 --locked
```

1000 frames per iteration; Criterion's per-element figures are read directly as
**nanoseconds per record**.

### What is being compared

Each default or CRC-32 pair runs through the same `StreamWriter`, the same
`CountingWriter`, the same builder, and the same reused payload. The **only**
difference inside a pair is the call shape at the sink.

Three sinks, chosen to span the interesting cases:

| Sink | Why |
|---|---|
| raw `File` (no `BufWriter`) | one syscall per `write_all`; isolates call count |
| loopback `TcpStream`, `TCP_NODELAY` | the sink where two-call framing can also cost *packets*, not just syscalls |
| `BufWriter<File>` | the recommended path; the one that must not regress |

Nagle is disabled on the TCP arm deliberately: with it on, the kernel coalesces
the header and payload writes and the benchmark would be measuring the kernel's
batching instead of the library's call count.

Two payload sizes, 64 B and 4096 B, to separate fixed per-call overhead from
per-byte cost.

## Findings

All figures below are ns/record, Criterion median with the 95 % CI in brackets.
Raw output is committed in `docs/benchmark/raw/e1_{file,tcp,bufwriter}.txt`;
required surprising-result rechecks are `e1_file_recheck.txt` and
`e1_bufwriter_recheck.txt`.

### F1. Unbuffered sinks — the win

| Sink | Framer | Payload | Two calls | One `writev` | Speedup |
|---|---|---:|---:|---:|---:|
| raw `File` | default | 64 B | 1904.2 [1900.2, 1908.8] | **1039.6** [1038.5, 1040.8] | **1.83×** |
| raw `File` | CRC-32 | 64 B | 1995.4 [1990.9, 2000.9] | **1084.4** [1082.3, 1086.7] | **1.84×** |
| raw `File` | default | 4096 B | 2536.8 [2375.9, 2720.6] | **695.6** [694.5, 696.7] | **3.65×; 2.12× recheck** |
| raw `File` | CRC-32 | 4096 B | 2526.4 [2517.1, 2537.3] | **1545.5** [1540.1, 1551.6] | **1.63×** |
| loopback TCP | default | 64 B | 3290.4 [3200.8, 3386.6] | **1758.1** [1686.5, 1831.7] | **1.87×** |
| loopback TCP | CRC-32 | 64 B | 3169.4 [3095.3, 3251.4] | **1704.2** [1644.7, 1769.7] | **1.86×** |
| loopback TCP | default | 4096 B | 3686.1 [3574.8, 3801.1] | **1914.4** [1857.3, 1975.3] | **1.93×** |
| loopback TCP | CRC-32 | 4096 B | 4186.9 [4071.6, 4311.2] | **2537.5** [2471.8, 2607.6] | **1.65×** |

Every unbuffered pair improves, including the previously unmeasured CRC-32
path. Magnitude depends on checksum work and payload size: the syscall saving is
fixed while CRC/copy work grows. The default-file/4096 B recheck retained the
win but moved it from 3.65× to 2.12×, so only the direction survives there.
Loopback absolute values include scheduling against the drain thread and are
not network-latency claims.

### F2. `BufWriter` — the regression check

| Framer | Payload | First isolated pair | Required recheck | Conclusion |
|---|---:|---:|---:|---|
| default | 64 B | +2.90 ns (+25.2 %) | −0.75 ns (−5.4 %) | inconclusive |
| CRC-32 | 64 B | +1.36 ns (+7.5 %) | +1.29 ns (+7.3 %) | **~7 % regression** |
| default | 4096 B | −108 ns (−9.6 %) | +53 ns (+4.7 %) | inconclusive |
| CRC-32 | 4096 B | −17 ns (−1.2 %) | +33 ns (+2.2 %, overlapping CIs) | inconclusive |

The 64 B CRC-32 arm is the only stable buffered result: one `writev` costs
roughly 1.3 ns/record more. The default/64 B and both 4096 B arms reverse or
disappear on recheck, so no claim survives for them.

It was worse. The first implementation was a straight loop over
`IoSlice::advance_slices`, and it cost ~3.3 ns/record (+24 %). The current code
splits the fast path out:

```rust
match writer.write_vectored(slices) {
    Ok(n) if n == total => Ok(()),            // hot: sink took the whole frame
    Ok(n) => write_remainder(writer, slices, n, total),  // #[cold]
    ...
}
```

With the partial-write bookkeeping moved behind `#[cold] #[inline(never)]`, the
small-frame residual is consistent with summing two slice lengths, constructing
the two-element `IoSlice` array, and one comparison — plus the fact that
`BufWriter::write_vectored` cannot use the specialized single-buffer path a plain
`write_all` takes. At 4096 B the workstation cannot resolve a repeatable result.

### F3. The counting wrapper must preserve native vectoring

`FrameReceipt` offsets are produced by `CountingWriter`. Without its
`write_vectored` override, Rust's provided method would call the wrapper's own
counted `write`, so receipt accounting would remain correct. The regression
would be performance, not corruption: native `File`/`TcpStream` vectoring would
be hidden behind scalar fallback calls.

The override both delegates vectoring and counts what the inner sink accepts:

```rust
fn write_vectored(&mut self, bufs: &[std::io::IoSlice<'_>]) -> std::io::Result<usize> {
    let n = self.inner.write_vectored(bufs)?;
    self.count += n as u64;
    Ok(n)
}
```

`tests/external_index.rs` pins receipt offsets, while
`receipts_are_correct_for_a_vectoring_sink` separately pins that the sink sees
one vectored call rather than scalar fallback.

Note for future work: `Write::is_write_vectored` is still unstable on the MSRV
(`can_vector`, issue #69941), so `CountingWriter` cannot forward it. Nothing in
the current stack queries it — `BufWriter` asks its own inner `File`, not the
`CountingWriter` above it — but a future arrangement that nests differently would
silently lose the vectored path rather than misbehave.

## Conclusion

Single-`writev` framing reduced every measured unbuffered pair. The stable
same-run range is **1.63–2.12×**; the initial 3.65× maximum did not reproduce
and is retained only in the raw/history table above.
For the production-shaped CRC-32/64 B pair it saves about **0.91 µs/frame** on a
raw file and **1.47 µs/frame** on loopback TCP, while costing **1.29–1.36
ns/frame** through `BufWriter`.

**Adopted as the default** in `DefaultFramer` and `ChecksumFramer`, not gated
behind a feature or a sink probe. The E1 acceptance rule permitted gating only
if the path helped some sinks and hurt others; the observed small buffered cost
against the large unbuffered improvement did not meet that bar. A gate would
also add a branch to the path it was meant to protect.

On TCP this is only a call-count optimization. TCP is a byte stream:
`writev` does not guarantee all-or-nothing acceptance, segment boundaries, or
simultaneous arrival of header and payload. The partial-write loop handles
short acceptance; readers must continue to handle arbitrary stream chunking.

### What changes as a result

- `DefaultFramer` / `ChecksumFramer` emit one `write_vectored` per frame.
- `CountingWriter::write_vectored` ships with it (§F3) — mandatory, not optional.
- The wire format is **byte-identical**; the golden hex corpus
  (`tests/wire_format_corpus.rs`, 12 files across four framers) passes unchanged,
  which is what licenses the "no wire change" claim in
  `docs/WIRE_FORMAT_SPEC.md`.
- `docs/DESIGN_v2_8.md` §5 documents the change and the receipt interaction.
- The original E1 task brief's claim that `IoSlice::advance_slices` is unstable
  on the MSRV was wrong (it stabilized in 1.81). Only
  `Write::write_all_vectored` remains unstable.

## Threats to validity

- **T1 — This machine's run-to-run drift is large enough to fabricate results,
  and did.** Between two consecutive full-suite runs with **no code change on the
  measured path**, `bufwriter/two_call/64B` moved −24 % and `file/two_call/64B`
  moved +57 %. Criterion reported both as significant (p < 0.05) because it
  compares against its own saved run and cannot know the machine, not the code,
  changed. The first pass of this experiment reported `BufWriter`/4096 B as a
  **34 % win** for `writev`; it did not survive isolated rechecks.
  Every figure above was re-collected in isolation. **Rule this establishes:
  cross-run absolute comparisons on this hardware are worth nothing at this
  timescale — only A-vs-B pairs from the same isolated run are admissible, and a
  surprising delta must be re-collected before it is written down.** The new
  raw files follow that rule; the contradictory 4096 B buffered recheck is
  reported as inconclusive rather than averaged away.
- **T2 — Single machine, single OS.** Apple M4 / macOS. `writev` cost relative to
  `write` differs on Linux. The *direction* on the measured unbuffered sinks is
  structural; the stable 1.63–2.12× magnitude is not portable.
- **T3 — Loopback TCP is not a network.** Absolute values include local
  scheduling against a draining reader thread and say nothing about real link
  behavior.
- **T4 — `BufWriter` is at the harness limit.** CRC-32/64 B reproduced at
  ~7 %, but default/64 B and both 4096 B conclusions reversed or vanished.
  Instruction counts are the better
  fixed-overhead instrument.
- **T5 — Partial writes are correctness-tested, not perf-tested.** The `#[cold]`
  path is covered by `vectored_tests` (one-byte-at-a-time, partial-vectored, and
  stalled sinks), but it is never benchmarked. A sink that habitually accepts
  partial vectored writes would pay costs not measured here.
- **T6 — The baseline is a reimplementation, not the historical binary.**
  Both two-call framers are source replicas rather than the historical binary.
  They include the historical length guard and combined checksum header and
  were checked against the parent commit, but remain copies.
