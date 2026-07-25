# Design Document: flatstream-rs v2.8 — Writer Frame Receipts and Boundary Ergonomics

**Version:** 1.0
**Status:** Implemented on branch `v0.2.8`; pending review and tag
**Author:** Dallas Marlow
**Date:** 2026-07-24

## 1. Overview

v2.8 is a small, **fully additive, non-breaking** minor release cut in response to
the first real consumer of the library (a terminal scrollback journal built on the
`ONBOARDING.md` §7 profile). It adds four things and changes nothing else — no
wire-format change, no trait-signature change, no observable behavior change on
existing code paths:

1. **Frame receipts** — writer offset reporting, so external "offset → frame"
   indexes stop reimplementing the wire layout.
2. **`From<flatstream::Error> for std::io::Error`** — the reverse of the existing
   conversion, for application boundaries that normalize on `io::Error`.
3. **`OwnedStreamWriter<W, F>`** — a lifetime-free type alias for the common
   `write_finished`-only writer.
4. **Single-`writev` framing** — the built-in framers emit each frame with one
   vectored write rather than two sequential ones. Byte-identical output; 2–2.8×
   cheaper per frame on unbuffered sinks.

The organizing observation: the consumer succeeded without async, compression,
streamset, or segments, and the only friction was two small additive gaps. The
frame-receipt primitive is also exactly what the v3/segment direction (F1-IMPL's
stream-position accounting, E2's range parser, future segment indexes) needs, so
this release is being pulled by real usage rather than speculative design. `v0.2.7`
stays immutable; `0.3.x+` stays reserved for the roadmap.

## 2. Frame Receipts (writer offset reporting)

The friction, verbatim from the feedback: the consumer tracked
`bytes += 8 + builder.finished_data().len()` alongside every write, coupling its
index to the documented wire layout. v2.8 has the writer report the offset instead.

- **`FrameReceipt { frame_start: u64, wire_len: u64 }`** — the byte position and
  on-wire size of one frame. The next frame begins at `frame_start + wire_len`.
- **`write_with_receipt` / `write_finished_with_receipt`** return a `FrameReceipt`;
  the existing `write` / `write_finished` are unchanged and now delegate to them
  (the discarded receipt's arithmetic dead-code-eliminates, so their hot-path
  codegen is unaffected — see §6).
- **`bytes_written(&self) -> u64`** is the running stream offset (the `frame_start`
  the next write will receive). **`with_start_offset(u64)`** sets the base for a
  writer positioned over a nonzero region of a file, so receipts can carry absolute
  file offsets; it defaults to 0 (stream-relative).

**Implementation.** The writer wraps its underlying `W` in an internal
`CountingWriter<W>` that tallies bytes *actually accepted* by `W`. `wire_len` is the
delta across the framer's write, so it is correct for **any** framer — default,
checksummed, or custom — with **no change to the `Framer` trait** (a trait-signature
change would have been the breaking alternative and was rejected). A mid-frame I/O
error tears the frame; the stream is then recovered/truncated per the E1 recovery
contract, so an unobservable partial count does not affect a well-formed stream.
`get_ref`/`get_mut`/`into_inner` continue to expose `&W`/`&mut W`/`W`.

## 3. `From<flatstream::Error> for std::io::Error`

The crate already had `From<std::io::Error> for Error`; the reverse was missing, so
application code surfacing `io::Error` at its boundaries wrote
`map_err(io::Error::other)` everywhere. The new impl (via `io::Error::other`)
produces an `io::Error` of kind `Other` with the flatstream error preserved as the
inner payload (recoverable through `get_ref`/`into_inner`, and forwarded by
`Display`). Callers that must branch on an original I/O kind match on `Error::kind`
before converting. Cheap and `#[cold]`, consistent with the other conversions.

The alternative — unwrapping the `Io` variant so a round-tripped I/O error keeps
its original kind — was considered and **declined** (2026-07-24): it makes the
conversion's result kind depend on the source variant, and the non-I/O variants
(`ChecksumMismatch`, `ValidationFailed`, `InvalidFrame`) would still be wrapped as
`Other` regardless, so the asymmetry buys little while `other()` is uniform and
discards no context. Revisit only if round-tripping I/O errors through `Error`
proves common in practice.

## 4. `OwnedStreamWriter<W, F>` alias

`StreamWriter`'s `'a` lifetime originates in its internal `FlatBufferBuilder<'a>`
and only bites when the builder borrows external data. `write_finished`-only
consumers never exercise it yet still had to name or infer it (a surprising
`E0106`). `OwnedStreamWriter<W, F> = StreamWriter<'static, W, F, DefaultAllocator>`
pins it away. No second writer implementation was introduced — the alias suffices;
a distinct builder-less type is reconsidered only if the alias proves insufficient.

## 5. Single-`writev` framing

`DefaultFramer` and `ChecksumFramer` previously put a frame on the wire with two
calls — `write_all(&len_prefix)` then `write_all(payload)`. On a `BufWriter` that
is two `memcpy`s; on an unbuffered sink (raw `File`, `TcpStream`, pipe) it is **two
syscalls per frame** for bytes the kernel will accept in one. Both framers now
build a two-element `IoSlice` array and issue a single `write_vectored`.

- **The wire format does not change.** The same bytes are emitted in the same
  order; only the call shape differs. The golden hex corpus
  (`tests/wire_format_corpus.rs`, 12 files across four framers) passes unchanged,
  which is what licenses that claim rather than inspection.
- **`Framer` is unchanged.** This is an implementation change inside the two
  built-in framers. Third-party framers are unaffected and keep working; a custom
  framer may adopt the same shape via the shared `write_all_vectored` pattern
  documented in `src/framing.rs`.
- **Partial writes are handled, not assumed away.** `write_vectored` is not atomic
  across slices. The full-acceptance case is peeled out as the hot path; the
  remainder loop lives behind `#[cold] #[inline(never)]`, retries `Interrupted`,
  and converts a stalled sink into `WriteZero` rather than spinning. Covered by
  the `vectored_tests` sinks (one-byte-at-a-time, partial-vectored, stalled) and
  end-to-end in `tests/io_fault_injection.rs`.

**Measured** (`docs/benchmark/FINDINGS_VECTORED_FRAMING.md`): 2.78× on raw `File`
and 1.98× on loopback TCP at 64 B; on the recommended `BufWriter` path, small
frames cost **0.71 ns/record more** (+6.8 %), which against A1's 66.1 ns
non-durable record is 1.1 %, and 0.017 % of a durable one. Adopted unconditionally
rather than gated on a sink probe: a gate would add a branch to the very path it
was meant to protect.

**Receipt interaction — the reason this belongs in the same release as §2.**
`CountingWriter` originally tallied bytes by overriding `write` and `flush` only.
`Write::write_vectored`'s *default* implementation forwards to `write`, so the
counter was correct by accident — an accident that ends the moment anything in the
stack implements `write_vectored` natively, which `File`, `TcpStream`, `BufWriter`,
and `Vec` all do. Shipping §5 without touching `CountingWriter` would have made
`bytes_written()` return near-zero and every `FrameReceipt` point at a wrong offset,
silently: no error, no panic, and no failing test, because nothing then compared
receipt offsets against real frame boundaries. `CountingWriter::write_vectored` now
ships alongside, and `tests/external_index.rs` asserts receipts against actual
frame boundaries so the class of failure cannot recur unnoticed.

One follow-up is knowingly left open: `Write::is_write_vectored` is unstable on the
MSRV (`can_vector`, rust#69941), so `CountingWriter` cannot forward it. Nothing in
the current stack queries it — `BufWriter` probes its own inner `File`, not the
`CountingWriter` above it — but an arrangement that nested them differently would
quietly lose the vectored path rather than misbehave.

## 6. Deferred and Rejected

- **Offset-addressed read (`read_frame_at` / `StreamReader::seek_to`) — deferred to
  E2.** A general seek has too many undefined interactions (stateful deframers,
  pending memory-policy shrink, buffered readers, v2-vs-v3 header base) to rush; it
  is the job of E2's range parser. `examples/external_index.rs` demonstrates the
  supported seek-and-fresh-reader pattern in the meantime, and documents the
  per-fetch allocation that E2 will remove with caller-supplied scratch.
- **Fluent builder objects — remain rejected/archived** (`docs/archive/V2_X_FLUENT_BUILDER.md`).
  The consumer report did not ask for a builder; the `FramerExt`/`DeframerExt`
  extension methods already provide the useful, fluent portion.

## 7. Verification

- **Tests:** 42 lib unit tests (incl. new receipt, offset, alias, and
  error-conversion tests) plus the integration and doctest suites, green across
  `all_checksums`, no-features, and `crc16`-only. `cargo fmt --check`, `clippy
  --all-targets -D warnings` (all-features and no-features), and `rustdoc -D
  warnings` all clean.
- **Write-path performance (the consumer's headline concern):** measured against
  the saved post-`v0.2.7` Criterion baseline, the `CountingWriter` + receipt
  routing shows **no regression** — "Sustained Performance: Writing 1000 small
  messages" reports *No change* (simple mode) and *within noise* (expert mode);
  "Checksum Writers" reports *No change* (XXHash64), *within noise* (CRC32), and
  *improved* (CRC16). The offset counter is two integer adds per frame, in the
  noise as expected.
- **Example as executable claim:** `examples/external_index.rs` asserts the index
  tiles the stream contiguously and covers every byte, and that seek-based random
  access returns byte-exact payloads (run by `scripts/examples.sh`).
- **Non-vectoring sinks do not regress:** `is_write_vectored()` is unstable on the
  MSRV (rust-lang/rust#69941), so a sink that does not implement `writev` cannot
  be detected — and does not need to be. The `vectored_tests` case
  `a_non_vectoring_sink_costs_the_same_two_calls_it_did_before` pins that such a
  sink still sees exactly two calls, with byte-identical output.

## 8. Consuming builders are `#[must_use]`

Thirteen methods take `self` and return a modified `Self` (or a wrapping
adapter): `with_memory_policy`, `with_memory_policy_and_factory`,
`with_max_frame_len` on both deframers, `with_cooldown`, `with_baseline`, and the
`FramerExt`/`DeframerExt` combinators `bounded`, `observed`, `with_validator`.
Only `with_start_offset` and `ValidatorChain::add` carried `#[must_use]`.

Without it, `deframer.with_max_frame_len(1024);` as a statement compiles clean
and does nothing — the default 2 GiB bound stays in force. That is the failure
mode `#[must_use]` exists for: the knob that stands between a corrupt length
header and a huge allocation, silently not set. The same shape applies to
`bounded()` and `with_validator()`, where a dropped result means the limit or
the validator is simply absent. A `compile_fail` doctest on
`DefaultDeframer::with_max_frame_len` pins the diagnostic.

## 9. Breaking Changes

None to the API or the wire format. v2.8 is purely additive; every existing
constructor, method signature, and wire byte is unchanged. Version bumped
`0.2.7` → `0.2.8`.

The `#[must_use]` additions in §8 can produce **new warnings** in downstream code
that discards a builder's result. Every such warning is a latent bug — the call
was already a no-op — so this is intended, and it is a warning, not an error.

## 10. Proposed for 2.8 — pending review sign-off

> **Status: not implemented.** These three add public surface, which §5 of
> `CONTRIBUTING.md` requires be agreed in review *before* it is built. They are
> written up here at the shape-of-the-API level so that agreement is possible.
> Each states the problem from measured evidence, the proposed surface, the
> alternatives rejected, and the open questions the maintainer should settle.

### 10.1 `Durable`: an explicit durability API

**Problem.** A1 measured `fsync` at roughly 61× the cost of everything else in a
record. It is, by a wide margin, the most important call a journaling consumer
makes — and the library has no opinion about it. Today you reach it through
`get_mut()`, which is the one accessor documented to **bypass the frame
counter** and corrupt subsequent receipts. The most performance-critical and
most correctness-critical operation in the write path is reached through the
crate's designated footgun.

There is a second, quieter trap. `StreamWriter<BufWriter<File>>` is the
recommended shape, and `writer.get_mut().get_ref().sync_all()` on it syncs
whatever happens to have been flushed — frames still sitting in the `BufWriter`
are not on disk, and nothing says so. "Flush before sync" is knowledge the type
system could carry and currently does not.

**Proposed surface.**

```rust
/// A sink that can push accepted bytes to durable storage.
pub trait Durable {
    /// Contents only (`fdatasync`); cheaper where metadata need not be current.
    fn sync_data(&mut self) -> std::io::Result<()>;
    /// Contents and metadata (`fsync`).
    fn sync_all(&mut self) -> std::io::Result<()>;
}

impl Durable for std::fs::File { /* delegates */ }
impl<W: Durable + Write> Durable for std::io::BufWriter<W> { /* flush, then sync */ }
impl<W: Durable + ?Sized> Durable for &mut W { /* forward */ }
impl<W: Durable + ?Sized> Durable for Box<W> { /* forward */ }

impl<'a, W: Write + Durable, F: Framer> StreamWriter<'a, W, F> {
    /// Flushes this writer, then syncs the sink. Returns the **durable
    /// watermark**: every frame whose `end()` is at or below the returned
    /// offset is on stable storage.
    pub fn sync_data(&mut self) -> Result<u64>;
    pub fn sync_all(&mut self) -> Result<u64>;
}
```

**Why the watermark return.** It is what a commit protocol actually needs. A
consumer holding receipts can answer "is transaction T durable?" by comparing
`receipt.end()` against the last watermark, with no bookkeeping of its own and
no second call to `bytes_written()` that could race with an intervening write.
It costs nothing to return — it is `bytes_written()` at sync time.

**Deliberately not implemented for in-memory sinks.** No `impl Durable for
Vec<u8>` or `io::Sink`. A no-op `sync` that returns `Ok` is a lie of exactly the
kind this crate's culture rejects; absence of the impl makes "this sink has no
durability" a compile-time fact. Test code that needs a durable sink can use a
`tempfile`, as the existing tests already do.

**Honesty requirement — macOS.** `File::sync_all` maps to `fsync`, which on
macOS does **not** flush the drive's write cache; only `F_FULLFSYNC` does. A
crate that documents its zero-copy scope this carefully should not ship a method
named `sync_all` without saying so. Proposed: document it plainly on the trait,
and leave `F_FULLFSYNC` out of scope rather than adding a platform-conditional
behavior difference the tests cannot portably verify. This is worth an explicit
maintainer decision, because the alternative view — that `sync_all` should mean
"actually durable" everywhere and take the `F_FULLFSYNC` cost on macOS — is
defensible.

**Open questions.** (a) Trait name: `Durable` vs `SyncTarget`. (b) Whether
`sync_data`/`sync_all` should return the watermark or `()`. (c) Whether the
macOS caveat is documented or handled.

### 10.2 `FrameReceipt::end()`

**Problem.** `frame_start + wire_len` is written out by hand in the README
recipe, in `examples/external_index.rs`, and in five places in
`tests/external_index.rs`. `FrameReceipt` exists precisely so that consumers do
not do wire arithmetic, and then leaves them doing the one piece that matters —
the piece you must get right to seek to the *next* frame or to bound a read.

**Proposed surface.**

```rust
impl FrameReceipt {
    /// Offset one past the frame's last byte: where the next frame begins.
    pub const fn end(&self) -> u64;
    /// The frame's byte range on the wire, `frame_start..end()`.
    pub const fn range(&self) -> std::ops::Range<u64>;
}
```

Both are `const fn`, total, and derivable from public fields, so this adds no
capability — only a name for the thing everyone is already computing, and one
place to be correct about checksum bytes being inside `wire_len`.

**Open question.** Whether `range()` earns its place alongside `end()`, or
whether `end()` alone is the smaller surface worth having.

### 10.3 `read_frame_at`: offset-keyed point lookup

**Problem.** §6 deferred this to E2 on the grounds that a general seek has too
many undefined interactions — stateful deframers, a pending memory-policy
shrink, buffered readers, header-base ambiguity. That reasoning holds for
`StreamReader::seek_to`, a *method* that would have to reconcile a seek against
a live reader's buffer and policy state. It does not hold for a **free
function** that owns nothing:

- `Deframer::read_and_deframe(&self, reader, buffer) -> Result<Option<usize>>`
  already takes `&self` and a caller-supplied `Vec<u8>`. It is stateless by
  signature, and checksum verification composes for free.
- No `StreamReader` is constructed, so there is no memory policy, no buffer
  ownership question, and no pending shrink.
- Header-base ambiguity is already settled: `FrameReceipt::frame_start` is an
  absolute file offset whenever `with_start_offset` was set, and its rustdoc now
  says so directly rather than "relative to the writer's start offset," which
  read as though the start offset still had to be added.

Today the supported pattern allocates a fresh reader *and* a fresh buffer per
lookup. For a graph database, offset-keyed point lookup **is** the read path.

**Proposed surface.**

```rust
/// Reads the single frame beginning at `offset`, into caller-owned `scratch`.
/// Returns the payload, or `None` at clean EOF.
pub fn read_frame_at<'s, R: Read + Seek, D: Deframer>(
    src: &mut R,
    deframer: &D,
    offset: u64,
    scratch: &'s mut Vec<u8>,
) -> Result<Option<&'s [u8]>>;
```

The whole body is a `seek` plus the existing `read_and_deframe`. The value is
not the code, it is (a) the allocation moving to the caller, who can reuse one
`scratch` across a whole traversal, and (b) making the supported random-access
pattern a tested function rather than an example to copy.

**Documented caveats.** Passing a `BufReader` is legal but usually wrong —
`seek` discards its buffer, so it pays for buffering it cannot use; a bare
`File` is the right argument for scatter reads. And `offset` must be a frame
boundary from a receipt: pointing it mid-frame yields a garbage length header,
which the frame-length bound rejects but cannot distinguish from corruption.

**Open questions.** (a) Free function versus an associated function on
`StreamReader`. (b) Module placement — `reader`, or a new `random_access`. (c)
Whether this supersedes E2's scope or is a stepping stone to it.
