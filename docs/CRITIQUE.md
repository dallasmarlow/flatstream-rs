# Library critique: technical merit and blind spots

**Date:** 2026-07-24
**Against:** `flatstream` 0.2.8 (plus the A1/E1/B1/B2/C3 work landed the same day)
**Lens:** the stated destination — this crate becoming the storage substrate for
a graph database.

This is a working engineer's review, not a marketing document. Section 1 is what
I think is genuinely well built and should not be traded away. Section 2 is what
I think is missing or wrong, ordered by how much it will hurt once a graph
database depends on it. Section 3 separates the things I believe are defects
from the things that are merely priority disagreements with the maintainer.

---

## 1. Technical merit

### 1.1 The invariants are tested, not asserted

Most pre-1.0 crates claim "zero-copy" and "zero-allocation" in a README and
nowhere else. This one has `tests/no_alloc_invariants.rs`, a golden hex corpus
(`tests/corpus/*.hex`, 12 files across four framers) that pins the wire format
byte-for-byte, property round-trip tests, I/O fault injection, fuzz targets, and
a Miri script. When I replaced the framing write path wholesale in E1, the
corpus tests were what let me state "byte-identical" as a fact rather than a
hope. That is the difference between a format you can build an index on and one
you cannot.

### 1.2 Dynamic dispatch is enumerated, not merely avoided

The README names every place a boxed or indirect call survives: `MemoryPolicy`
(one boxed call while the policy is above baseline), `CompositeValidator` (one
per composed validator), `TypedValidator` (a function pointer). A library that
knows exactly where its indirection lives is one you can reason about under a
latency budget. Most "zero-cost abstraction" claims are unfalsifiable; this one
is specific enough to be checked, and I checked several.

### 1.3 The zero-copy claim is scoped honestly

The TL;DR pre-empts its own strongest criticism: zero-copy means *payload
access*, a generic `Read` source still copies each frame once into the reader's
buffer, and the copy-free mmap path is future work. Very few libraries
volunteer the limit of their own headline claim. `docs/ZERO_COPY_ANALYSIS.md`
exists specifically because an earlier version of the terminology was sloppy and
someone went back and fixed it.

### 1.4 Bounds precede allocation on the read path

The deframers refuse to size an allocation from an unvalidated length field, the
bound is configurable, custom deframers are told in the docs that they own this
responsibility, and the whole thing is fuzzed. Sizing a `Vec` from an attacker's
length prefix is the single most common vulnerability class in framing code.
It's handled here, and `examples/custom_framer_example.rs` documents the
obligation for third-party framers rather than leaving it implicit.

### 1.5 Error handling is cheap enough not to distort the API

There is a test asserting `Error` is pointer-sized. That is the kind of detail
that decides whether `Result<T>` stays free in a hot loop, and it is enforced
rather than assumed. The `From<flatstream::Error> for io::Error` conversion
added in 2.8 is the right call for application boundaries.

### 1.6 The "no unmeasured claims" rule earns its keep

`CONTRIBUTING.md` §1 forbids publishing performance attributions that have not
been measured. This is not ceremony — it produced a genuinely counterintuitive
result. A1 measured framing at **0.6% of a 64-byte record** and flatstream's
total share (framing + CRC-32) at **14.1%**, against 65.7% for the application's
own harvest and FlatBuffer construction. With `fsync` in the pipeline, all of
flatstream is **1.6%** of the record. An optimization effort guided by intuition
would have gone straight at the framing layer and won nothing.

### 1.7 Recovery is a designed feature

`recover_stream` reporting an exact truncation offset for a torn tail is a real
journaling primitive, not a convenience. B1's tests confirm the property that
actually matters to an index builder: every entry at or below the recovery point
still resolves, so a crash costs you the tail and nothing else.

---

## 2. Blind spots

### 2.1 There is no stream-level header — no magic, no version, no self-description

**This is the one I would fix before anything else, and before any data you care
about is written.**

A stream is a bare sequence of `[len][checksum?][payload]`. There is no preamble.
Consequences, in increasing order of severity:

- **A reader cannot tell how a stream was framed.** `WIRE_FORMAT_SPEC.md` §12
  says the two sides must agree on the checksum algorithm and width
  "out-of-band." The README is blunter: reading a checksummed stream with the
  plain `DefaultDeframer` *does not error* — it silently mis-frames, because the
  checksum bytes parse as payload. For a transient socket that is a bug you find
  in a minute. For a durable journal that a graph database will re-open months
  later, possibly from a different binary, it is a silent-corruption path with no
  detection.
- **There is no format version, so there is no migration path.** The project
  correctly treats an on-wire change as a 3.0-level decision. But without a
  version field, even a *backward-compatible* extension cannot be detected at
  read time. You cannot add an optional field, you cannot introduce a second
  frame kind, and you cannot tell a v3 reader that it is looking at a v2 file.
  The format currently gets exactly one shot at being right forever.
- **Frame-level integrity does not imply stream-level integrity.** Each frame's
  checksum validates that frame. Nothing detects a truncated-then-reappended
  stream, a frame duplicated by a bad copy, or two journals concatenated. For
  crash recovery that is adequate; for a durable artifact underneath a database
  it is not.

**Recommendation.** A 3.0 stream preamble: magic bytes, a format version, and a
framing descriptor (checksum algorithm id and width). Cost: a fixed handful of
bytes once per stream — nothing per frame, so A1's numbers are untouched.
Payoff: journals become self-describing, a mismatched deframer *fails* instead
of mis-parsing, and the format acquires the ability to evolve at all. If you
want the option of a chained/rolling digest later, reserving space for it now is
much cheaper than adding it after there is data on disk.

### 2.2 The write path got receipts; the read path got nothing

v0.2.8 gave the writer `FrameReceipt`, `bytes_written()`, and
`with_start_offset()`. The reader has no `bytes_consumed()`, no per-frame offset,
and no way to read the frame at a known offset.

B1's own example shows the cost. To fetch one indexed frame, the recommended
pattern is:

```rust
cursor.seek(SeekFrom::Start(offset))?;
let mut reader = StreamReader::new(cursor, DefaultDeframer::new());
let payload = reader.read_message()?.expect("a frame begins here");
```

That constructs a `StreamReader` — and allocates its buffer — **per point
lookup**. The example's own comment admits this is the cost a future
`read_frame_at` will remove.

For a graph database, offset-keyed point lookups are not a nice-to-have; they
are the read path. Task D in the backlog covers forward offset reporting and
*explicitly excludes* random access. I think that split is inverted: an index you
can build cheaply but cannot query cheaply is not yet a useful index. I would
promote `read_frame_at(offset, &mut scratch)` — with caller-supplied scratch, so
it inherits the zero-allocation steady state the write path already has — to the
top of the post-3.0 list, ahead of forward offset reporting.

This is a priority disagreement, not a defect. The deferral is deliberate and
documented (`docs/DESIGN_v2_7.md`). I am flagging it because the destination
changes the calculus.

### 2.3 There is no durability API, and the workaround shares a door with a known hazard

`flush()` is documented as explicitly not an `fsync`. There is no `sync()` on
`StreamWriter`. To make a write durable you go through `get_mut()` and call
`sync_data()` on the inner `File`.

`get_mut()` is also the accessor that silently invalidates `FrameReceipt`
offsets if you *write* through it — B1 now has a test pinning that hazard. So
the single escape hatch a journaling consumer is *required* to use for
durability is the same one that quietly corrupts their index if they use it
slightly differently. Reading through it is safe and writing through it is not,
and nothing in the type system says so.

Meanwhile A1 measured `fsync` at **61× the cost of everything else combined** at
64 bytes. The dominant cost in any durable pipeline is entirely outside the
library's model, which means group-commit and batched-fsync policy — the thing
that actually determines a graph database's write throughput — has to be
reinvented by every consumer.

**Recommendation.** An explicit `sync()`/`durable_flush()` on `StreamWriter` for
sinks that can support it, so durability stops being spelled `get_mut()`. Even
if the implementation is three lines, moving it out of the hazardous accessor is
worth it.

### 2.4 Concurrency is entirely unspecified

There is no documented story for:

- more than one writer against a stream (presumably unsupported — but unstated);
- a reader tailing a stream while a writer appends to it.

The second is the interesting one. `recover_stream` handles a torn tail *after a
crash*, but a live reader that catches up to a partially-written frame hits the
same byte pattern with completely different correct behavior: it should wait,
not truncate. A graph database will want live followers. Right now a consumer
building that has no guidance and would likely reach for the recovery path,
which would be wrong.

This needs a documented position more than it needs code. Even "single writer,
readers must not tail a live stream" would be an improvement over silence.

### 2.5 The gate is excellent and nothing enforces it

`scripts/gate.sh` is genuinely excellent — fmt, clippy across four feature
configurations, a test matrix, doctests, rustdoc-as-errors, bench and fuzz
compile checks, and an MSRV assertion. It is better than most production CI.

`CONTRIBUTING.md` §3 states that verification is "local by deliberate choice;
there is no CI." I want to be careful here, because I initially wrote this
section up as an oversight and it is not one. The choice buys real things: no
runner drift, no green-badge theater, an MSRV assertion that means something
because it runs on the declared toolchain rather than whatever the runner
installed, and a contributor culture where the gate is a thing you *run* rather
than a thing that happens to you.

The cost is what I hit on day one. The gate **failed** on a fresh checkout,
because `fuzz/Cargo.lock` still pinned `flatstream 0.2.7` while the workspace
had moved to 0.2.8. That breakage was committed and had survived. A gate that
depends on a human remembering is green exactly as often as someone checks, and
the failure mode is silent — the next contributor cannot distinguish "I broke
this" from "this was already broken," which is a genuinely demoralizing first
hour.

So I would not argue for CI as such. I would argue for closing that specific
gap, and it does not require a runner: a `pre-push` hook, or a gate step that
checks `fuzz/Cargo.lock` against the workspace version, or simply running the
gate as part of the release ritual before a version bump. The principle worth
preserving is that **committed state should never be gate-red**; how that is
achieved is the maintainer's call, and the local-only argument is a good one.

### 2.6 The gate verified everything except the code consumers actually copy

This is the pattern behind two of today's findings, and I think it is worth
naming as a category rather than two separate bugs.

- **Examples were compiled but never run.** `clippy --all-targets` type-checks
  `examples/`, so the maintainer's §1 rule that "examples self-assert" was being
  enforced by nothing. Four examples had zero assertions; several more printed
  "ok" on paths that would have printed "ok" after processing zero messages.
  `ingest_lobster` guarded its only success condition with a `debug_assert` that
  compiles out of the release builds it actually runs in — on a counter that
  incremented even for skipped files.
- **README snippets were never compiled at all.** rustdoc only tests snippets
  under `src/`. **17 of the README's 29 Rust snippets did not build.** Four ASCII
  diagrams used untagged code fences, which rustdoc treats as Rust; one paragraph
  was indented four spaces and so was also parsed as code. Several snippets used
  `#`-prefixed hidden-line syntax, which means someone once intended them to be
  doctests — and those lines render *visibly* on GitHub, so the README was
  simultaneously not-compiling and displaying doctest scaffolding to readers.

Both are fixed and both are now enforced in the gate. The generalizable lesson:
the verification effort was concentrated on `src/`, which is the code that is
already best covered, and absent from `examples/` and `README.md`, which is the
code a new consumer runs first and trusts most.

### 2.7 The benchmark methodology is not robust on this hardware

`CONTRIBUTING.md` §4 requires a committed findings doc for every measurement,
which is the right rule. It does not say anything about isolation, and on this
machine that gap is large enough to produce false results.

Measured, on unchanged code: the `bufwriter/two_call/64B` arm moved **−24%**
between two consecutive full benchmark runs, and `file/two_call/64B` moved
**+57%**. Criterion reported both as statistically significant, because it
compares against its own saved baseline and cannot know that the machine, not
the code, is what changed.

This nearly put a false claim in a findings doc: the first E1 pass showed
vectored framing 34% faster than two-call on `BufWriter` at 4 KB. Re-collected
with one benchmark group running at a time, that "win" was parity.

**Recommendation.** Fold into §4: collect one benchmark group at a time with
nothing else running; only A-vs-B pairs from the same isolated run are
admissible; re-collect any surprising delta before writing it down. E1's
findings doc §2.3 records the incident in full.

### 2.8 The MSRV is declared but not proven

The gate is honest about this: the environment uses Homebrew rust with no
rustup, so there is no older toolchain to test the floor against. It verifies
only that the active compiler equals the declared version. That happens to hold
today (1.97.1 exactly), which is the best case — but the floor is a claim about
compilers that are never exercised. Worth knowing before a consumer pins to it.

---

## 3. Summary

**Do not trade away:** the tested invariants, the golden wire-format corpus, the
enumerated dispatch story, the honest scoping of "zero-copy," and the
no-unmeasured-claims rule. Those are the reasons this crate is a credible
foundation rather than another framing helper. The A1 result — flatstream is
0.6% to 14% of a write, and 1.6% once durability is real — is the kind of fact
that only exists because of that culture, and it should shape where effort goes
next.

**Fix before depending on the format:**

| | Item | Why it's first |
|---|---|---|
| 1 | Stream preamble: magic, version, framing descriptor (§2.1) | Without it the format cannot evolve and a mismatched deframer corrupts silently. Every day of written data raises the cost. |
| 2 | Explicit durability API (§2.3) | `fsync` is 61× everything else and currently lives outside the library, reached through the one accessor that also corrupts receipts. |
| 3 | `read_frame_at` with caller-supplied scratch (§2.2) | Offset-keyed point lookup is a graph database's read path; today it allocates a reader per lookup. |
| 4 | A written position on concurrency (§2.4) | Live tailing and crash recovery see the same bytes and need opposite behavior. |
| 5 | Some mechanism keeping committed state gate-green (§2.5) | Not necessarily CI. The gate was already red on a fresh checkout, and that is invisible until someone pays for it. |

Items 2 and 3 add public API and so need their shape agreed in review before
implementation, per §5. Both are now written up at the API-shape level in
`docs/DESIGN_v2_8.md` §10, together with `FrameReceipt::end()`, each with its
rejected alternatives and the open questions the maintainer should settle.

**Process, not code:** keep the habit of asking what the gate *doesn't* cover —
that question found two real classes of rot today (§2.6). Benchmark isolation
(§2.7) is now codified in `CONTRIBUTING.md` §4 and mechanized in
`scripts/bench_isolated.sh`.
