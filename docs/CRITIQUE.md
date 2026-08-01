# Library critique: technical merit and blind spots

**Date:** 2026-07-24 (updated 2026-07-30)
**Against:** `flatstream` 0.2.8 release candidate after final durability,
position-accounting, and fail-stop hardening
**Lens:** the stated destination — this crate becoming the storage substrate for
a graph database.

> **Benchmark status:** historical findings remain committed, but all exact
> writer timings predate final fail-stop hardening and are not
> release-candidate performance claims.

This is a working engineer's review, not a marketing document. Section 1 is what
I think is genuinely well built and should not be traded away. Section 2 is what
I think is missing or wrong, ordered by how much it will hurt once a graph
database depends on it. Section 3 separates the things I believe are defects
from the things that are merely priority disagreements with the maintainer.

---

## 1. Technical merit

### 1.1 The invariants are tested, not asserted

Most pre-1.0 crates claim "zero-copy" and "zero-allocation" in a README and
nowhere else. This one has the counting allocator in `tests/allocation.rs`
(plus typed-path coverage in `tests/no_alloc_invariants.rs`), a golden hex corpus
(`tests/corpus/*.hex`, 12 files across four framers) that pins the wire format
byte-for-byte, property round-trip tests, I/O fault injection, fuzz targets, and
a Miri script. When I replaced the framing write path wholesale in E1, the
corpus tests were what let me state "byte-identical" as a fact rather than a
hope. That is the difference between a format you can build an index on and one
you cannot.

### 1.2 Dynamic dispatch is enumerated, not merely avoided

The README names every place a boxed or indirect call survives:
`CompositeValidator` (one per composed validator) and `TypedValidator` (a
function pointer). Memory and sync policies are statically dispatched with
zero-sized defaults. A library that
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
been measured. This is not ceremony: the isolated 64 B recheck puts application
harvest/build/index at **63.0%**, flatstream framing/copy + CRC-32 at **17.3%**,
and buffered file output at **19.7%**. With one `sync_data()` per 1000-record
batch, the entire non-sync pipeline is about **1.4%** of elapsed time
(flatstream's share is about 0.25%). At 4 KiB the shares change substantially;
the attribution is useful precisely because it is workload-qualified.

### 1.7 Recovery is a designed feature

`recover_stream` reporting an exact truncation offset for a torn tail is a real
journaling primitive, not a convenience. B1's tests confirm the property that
actually matters to an index builder: every entry at or below the recovery point
still resolves, so a crash costs you the tail and nothing else.

---

## 2. Blind spots

### 2.1 There is no stream-level header — owner-declined

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

**Decision (owner, 2026-07-25):** no core preamble is planned. Flatstream
remains a headerless framing primitive whose composition is agreed out of band.
Applications that persist streams must own a manifest/format-generation marker
and refuse unknown versions before constructing a deframer (Palimpsest already
does this). The risks above are accepted and transferred to that application
contract rather than solved in the wire format.

### 2.2 The read-side position gap is now closed

The reviewed baseline gave only the writer receipts. v0.2.8 now also ships
`bytes_consumed`, receipt-aware forward reads, and stateless `read_frame_at`
with caller-owned scratch. Fresh-reader-per-lookup allocation is eliminated,
and Palimpsest can retire its last checksum/header arithmetic during resume.
Point lookup seeks once, then derives `wire_len` from bytes returned through a
per-call counting reader rather than issuing a post-read position syscall.

The performance result is deliberately narrower than the API win: caller
scratch reaches zero-allocation steady state, while wall-clock throughput
depends on frame size and whether the retained source is buffered. See
`FINDINGS_POSITIONED_READS.md`.

### 2.3 The durability gap was real and is now closed

The reviewed baseline documented `flush()` as explicitly not an `fsync` and
required consumers to reach through `get_mut()` to the inner `File`.

`get_mut()` was also the accessor that silently invalidated `FrameReceipt`
offsets if callers performed out-of-band I/O. The pre-review correction removed
both `get_mut` and `get_ref` (`File` supports shared-reference I/O) rather than
preserving the same hazard through a read-only-looking accessor.

A1 now measures that cadence at **68.6×** the non-sync 64 B pipeline but only
**5.3×** at 4 KiB, proving cadence and payload shape both matter.

**Resolution.** `Durable`, static `SyncPolicy` implementations, manual
`sync_data`/`sync_all`, and durable watermarks provide the synchronization
boundary, and direct source/sink access is no longer exposed. The default
`NoSync` writer remains zero-sized/branch-free; policy writers make group-commit
cadence explicit and report checkpoint failures without pretending the
triggering frame was unwritten.

### 2.4 The concurrency boundary is now documented

The supported model is one writer per stream. A seekable live-file follower uses
`read_frame_at` with a `RetrySafeDeframer`: `UnexpectedEof` inside the current
frame means “wait for more bytes and retry the same absolute offset,” while
`Ok(None)` means “caught up for now.” Recovery interprets the same partial-tail
condition as truncatable only after writing has stopped.

A sequential `StreamReader` that consumed part of a failed frame is now
fail-stop and cannot continue from the middle. The remaining non-goal is
multi-writer coordination, which belongs above this framing library.

### 2.5 The gate is excellent and nothing enforces it

`scripts/gate.sh` is genuinely excellent — fmt, clippy across its feature
matrix, tests, doctests, rustdoc-as-errors, bench and fuzz
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

That specific mismatch is fixed: `fuzz/Cargo.lock` now records `flatstream
0.2.8`. This section records the process gap that allowed the stale lockfile to
land; it is not a claim that the current checkout is still gate-red.

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
  enforced by nothing. Several examples had zero assertions; others printed
  "ok" on paths that would have printed "ok" after processing zero messages.
  `ingest_lobster` guarded its only success condition with a `debug_assert` that
  compiles out of the release builds it actually runs in — on a counter that
  incremented even for skipped files.
- **README snippets were never compiled at all.** rustdoc only tests snippets
  under `src/`. Many of the README's Rust snippets did not build. ASCII
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
foundation rather than another framing helper. A1's qualified 64 B result
(63.0% application / 17.3% flatstream / 19.7% buffered output) is the kind of
attribution that culture can establish without pretending it transfers to the
4 KiB profile.

**Fix before depending on the format:**

| | Item | Why it's first |
|---|---|---|
| 1 | Application-owned format manifest (§2.1) | Core preamble declined; durable consumers must pin framing/checksum/schema generation out of band and reject unknown versions. |
| 2 | Explicit durability API (§2.3) — **resolved in v0.2.8** | Static policies and durable watermarks now encode group commit without `get_mut()`. |
| 3 | `read_frame_at` with caller-supplied scratch (§2.2) — **resolved in v0.2.8** | Forward and random reads now return exact receipts without per-lookup frame-buffer allocation. |
| 4 | Concurrency/live-tail contract (§2.4) — **resolved in v0.2.8** | Single writer; retry-safe positioned followers wait, finalized recovery may truncate. |
| 5 | Some mechanism keeping committed state gate-green (§2.5) | Not necessarily CI. A stale lockfile once made a fresh checkout gate-red; that instance is fixed, but it remained invisible until the next contributor ran the gate. |

Items 2 and 3 plus `FrameReceipt::end()` received maintainer sign-off and are
implemented in v0.2.8.

**Process, not code:** keep the habit of asking what the gate *doesn't* cover —
that question found two real classes of rot today (§2.6). Benchmark isolation
(§2.7) is now codified in `CONTRIBUTING.md` §4 and mechanized in
`scripts/bench_isolated.sh`.
