# Findings: does collapsing a frame's two writes into one `writev` pay?

**Author:** contributor (E1, `CONTRIBUTING.md` §6)
**Date:** 2026-07-24
**Status:** in progress — the change is landed and the direction is settled;
absolute numbers are provisional pending re-collection on reference hardware
(see Threats §T1)

## Hypothesis

Before this change, every framer put a frame on the wire with two calls:

```rust
writer.write_all(&payload_len.to_le_bytes())?;  // 4 bytes
writer.write_all(payload)?;                     // N bytes
```

On a `BufWriter` that is two `memcpy`s into the same buffer and costs almost
nothing. On an **unbuffered** sink — a raw `File`, a `TcpStream`, a pipe — each
`write_all` is a separate syscall, so the library imposes **two syscalls per
frame** for what the kernel can accept as one.

Hypothesis: **replacing the two writes with a single `write_vectored` roughly
halves per-frame cost on unbuffered sinks, and is free on buffered ones.**

The second half of that sentence is where the hypothesis can fail, and is the
reason the experiment is worth running. `BufWriter` is the sink this library
recommends (README, `ONBOARDING.md` §4), so a `writev` win that costs the
recommended path anything is a bad trade. E1 is therefore as much a regression
check on `BufWriter` as it is a win-measurement on raw sinks.

## Methodology

### Environment

- rustc: `rustc 1.97.1 (8bab26f4f 2026-07-14) (Homebrew)` — the declared MSRV
- OS / arch: macOS 26.5.2 / arm64
- CPU: Apple M4, 10 cores, 32 GiB
- Relevant dependency versions: `flatstream` 0.2.8, `flatbuffers` 25.12.19 (from
  `Cargo.lock`)
- Tool: Criterion 0.5, default sampling, `Throughput::Elements(1000)`
- Feature flags: none (default)
- Baseline compared against: `TwoCallFramer`, an in-bench copy of the pre-E1
  `DefaultFramer` body — **not** a saved Criterion baseline. See §T1 for why a
  saved cross-run baseline is not admissible on this machine.

**Instrument choice.** Wall clock (Criterion). The quantity of interest is
syscall cost, which is wall-clock by nature; instruction counts would not capture
it.

### Steps

The full 12-arm suite must **not** be run in one pass on this hardware — see §T1.
Collect one sink group at a time, with nothing else running:

```bash
cargo bench --bench vectored_framing -- bufwriter \
  2>&1 | tee docs/benchmark/raw/e1_bufwriter.txt
cargo bench --bench vectored_framing -- 'file/' \
  2>&1 | tee docs/benchmark/raw/e1_file.txt
cargo bench --bench vectored_framing -- 'tcp/' \
  2>&1 | tee docs/benchmark/raw/e1_tcp.txt
```

1000 frames per iteration; Criterion's per-element figures are read directly as
**nanoseconds per record**.

### What is being compared

Both arms run through the same `StreamWriter`, the same `CountingWriter`, the
same builder, and the same reused payload. The **only** difference between them
is the call shape at the sink.

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

All figures ns/record, Criterion median with the 95 % CI in brackets. Raw output
under `docs/benchmark/raw/e1_*.txt` (regenerate with the commands in Steps).

### F1. Unbuffered sinks — the win

| Sink | Payload | Two calls | One `writev` | Speedup |
|---|---|---|---|---|
| raw `File` | 64 B | 1936.6 [1927.1, 1947.4] | **695.5** [691.0, 701.5] | **2.78×** |
| raw `File` | 4096 B | 1875.9 [1825.7, 1914.9] | **752.7** [751.5, 754.2] | **2.49×** |
| loopback TCP | 64 B | 31580 [31465, 31709] | **15989** [15678, 16365] | **1.98×** |
| loopback TCP | 4096 B | 31821 [31631, 32101] | **15935** [15892, 15986] | **2.00×** |

The raw-`File` numbers are internally consistent with a pure syscall-count story,
which is the cross-check that the benchmark is measuring what it claims: two
calls cost 1936 ns, one costs 695 ns, i.e. **~968 ns per `write` versus ~695 ns
for one `writev` carrying the same bytes**. The speedup exceeds 2× because
collapsing the pair also removes one `write_all` loop setup and one return-path
check, not merely one syscall.

That the 64 B and 4096 B rows are nearly equal (695 vs 753 ns) confirms the cost
here is per-*call*, not per-byte: a 64× larger payload adds 8 % to the frame
cost. This is the shape that makes the change worth having — the smaller your
records, the larger the relative win.

TCP lands at exactly 2.00×, the cleanest possible signal that call count is the
entire story on that path. The absolute value (≈16 µs/frame) is dominated by
loopback scheduling against the draining reader thread and should not be read as
a syscall cost; only the ratio is meaningful.

### F2. `BufWriter` — the regression check

| Payload | Two calls | One `writev` | Delta |
|---|---|---|---|
| 64 B | **10.56** [10.42, 10.78] | 11.27 [11.23, 11.35] | **+0.71 ns (+6.8 %)** |
| 4096 B | 1164.7 [1112.5, 1230.2] | 1076.5 [1071.5, 1081.9] | no reliable difference |

Small frames on `BufWriter` cost **0.71 ns/record more** than before. This
reproduced across two isolated runs (+6.8 %, +7.8 %), so it is real, not drift.

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
residual 0.71 ns is what is genuinely irreducible: summing two slice lengths,
constructing the two-element `IoSlice` array, and one comparison — plus the fact
that `BufWriter::write_vectored` cannot use the specialized single-buffer path a
plain `write_all` takes.

At 4096 B the fixed cost is amortized into the memcpy and flush, and the two arms
are indistinguishable. **This is the arm that produced a false result before
isolation** — see §T1.

### F3. A silent-corruption bug this experiment exposed

`FrameReceipt` offsets are produced by `CountingWriter`, which counted bytes by
overriding `write` and `flush`. `Write::write_vectored`'s default implementation
forwards to `write`, so the counter kept working by accident — but only until
something in the stack implemented `write_vectored` natively, which `File`,
`TcpStream`, and `BufWriter` all do.

Had E1 shipped without touching `CountingWriter`, every vectored byte would have
bypassed `self.count`. `bytes_written()` would have returned near-zero, and every
`FrameReceipt` handed to an external index would have pointed at the wrong
offset — with no error, no panic, and no test failure, because no test then
compared receipt offsets against real frame boundaries. The fix is four lines:

```rust
fn write_vectored(&mut self, bufs: &[std::io::IoSlice<'_>]) -> std::io::Result<usize> {
    let n = self.inner.write_vectored(bufs)?;
    self.count += n as u64;
    Ok(n)
}
```

The generalizable lesson: **`CountingWriter` must override every `Write` method
that can move bytes**, and must not be allowed to stay correct by relying on a
default trait method. `tests/external_index.rs` (B1) now asserts receipt offsets
against actual frame boundaries, and `tests/io_fault_injection.rs` drives the
partial-vectored path through the full writer stack, so this class of failure
fails a test instead of shipping.

Note for future work: `Write::is_write_vectored` is still unstable on the MSRV
(`can_vector`, issue #69941), so `CountingWriter` cannot forward it. Nothing in
the current stack queries it — `BufWriter` asks its own inner `File`, not the
`CountingWriter` above it — but a future arrangement that nests differently would
silently lose the vectored path rather than misbehave.

## Conclusion

Single-`writev` framing is a **2–2.8× reduction in per-frame cost on unbuffered
sinks** and costs **0.71 ns/record on small frames through `BufWriter`**.

Set against A1's decomposition, the trade is easy to price. A1 measured a full
64-byte journaling record — harvest, FlatBuffer build, CRC-32 framing, buffered
file write, index update — at **66.1 ns**, rising to **4087.5 ns** once `fsync`
enters the pipeline. So:

- On the recommended `BufWriter` path, E1 costs **0.71 ns of a 66.1 ns record**
  (1.1 %), and **0.017 %** of a durable one. It is below the noise floor of any
  consumer-visible measurement.
- On an unbuffered sink it saves **~1.2 µs per frame** — three orders of
  magnitude more than it costs.

**Adopted as the default** in `DefaultFramer` and `ChecksumFramer`, not gated
behind a feature or a sink probe. `CONTRIBUTING.md` §6 E1 permits gating "only if
it helps some sinks and hurts others"; a 1.1 % cost on one path against a 2.5×
win on another does not meet that bar, and a gate would add a branch to the very
path it was meant to protect.

The change is also a correctness improvement on the TCP path that the timings
understate: with two writes, a frame header could reach the peer in a separate
segment from its payload, so a reader on a slow link sees a length prefix
describing bytes that have not arrived. One `writev` makes the kernel's atomic
acceptance of header-plus-payload the common case rather than an accident of
buffering. (`writev` is **not** atomic across slices in general — no
all-or-nothing claim is made or relied on; the partial-write loop handles the
rest.)

### What changes as a result

- `DefaultFramer` / `ChecksumFramer` emit one `write_vectored` per frame.
- `CountingWriter::write_vectored` ships with it (§F3) — mandatory, not optional.
- The wire format is **byte-identical**; the golden hex corpus
  (`tests/wire_format_corpus.rs`, 12 files across four framers) passes unchanged,
  which is what licenses the "no wire change" claim in
  `docs/WIRE_FORMAT_SPEC.md`.
- `docs/DESIGN_v2_8.md` §5 documents the change and the receipt interaction.
- `CONTRIBUTING.md` §6 E1's claim that `IoSlice::advance_slices` is unstable on
  the MSRV was wrong (it stabilized in 1.81); corrected there. Only
  `Write::write_all_vectored` remains unstable.

## Threats to validity

- **T1 — This machine's run-to-run drift is large enough to fabricate results,
  and did.** Between two consecutive full-suite runs with **no code change on the
  measured path**, `bufwriter/two_call/64B` moved −24 % and `file/two_call/64B`
  moved +57 %. Criterion reported both as significant (p < 0.05) because it
  compares against its own saved run and cannot know the machine, not the code,
  changed. The first pass of this experiment reported `BufWriter`/4096 B as a
  **34 % win** for `writev`; re-collected one group at a time, it was parity.
  Every figure above was re-collected in isolation. **Rule this establishes:
  cross-run absolute comparisons on this hardware are worth nothing at this
  timescale — only A-vs-B pairs from the same isolated run are admissible, and a
  surprising delta must be re-collected before it is written down.** Absolute
  numbers here remain provisional until re-collected on reference hardware; the
  ratios, which are syscall-count-driven, should hold.
- **T2 — Single machine, single OS.** Apple M4 / macOS. `writev` cost relative to
  `write` differs on Linux, and the 2.78× raw-`File` figure must not be quoted as
  cross-platform. The *direction* is structural (one syscall beats two); the
  magnitude is not.
- **T3 — Loopback TCP is not a network.** The 1.98× ratio is trustworthy; the
  16 µs absolute is an artifact of local scheduling against a draining reader
  thread and says nothing about real link behavior.
- **T4 — The `BufWriter` regression is at the edge of what this harness
  resolves.** 0.71 ns/record is ~7 % of a 10.5 ns operation. It reproduced twice
  in isolation, which is why it is reported as real, but it is not resolvable in
  a contended run and would be better characterized by instruction counts (A2's
  instrument).
- **T5 — Partial writes are correctness-tested, not perf-tested.** The `#[cold]`
  path is covered by `vectored_tests` (one-byte-at-a-time, partial-vectored, and
  stalled sinks) and by `tests/io_fault_injection.rs` through the full writer
  stack, but it is never benchmarked. A sink that habitually accepts partial
  vectored writes would pay costs not measured here.
- **T6 — The baseline is a reimplementation, not the historical binary.**
  `TwoCallFramer` is the pre-E1 `DefaultFramer` body copied verbatim into the
  bench. If that copy drifted from what actually shipped in 0.2.7, the comparison
  would be against a strawman. It is four lines and was checked against git
  history, but it is a copy.
