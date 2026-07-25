# Design Document: flatstream-rs v2.8 — Receipts, Vectored Framing, and Durability

**Version:** 1.0
**Status:** Implemented on branch `v0.2.8`; pending review and tag
**Author:** Dallas Marlow
**Date:** 2026-07-24

## 1. Overview

v2.8 is an additive minor release cut in response to
the first real consumer of the library (a terminal scrollback journal built on the
`ONBOARDING.md` §7 profile). It changes no wire bytes or existing method
signatures. The vectored path changes sink call shape, and the durability layer
is opt-in through a new concrete writer type:

1. **Frame receipts** — writer offset reporting, so external "offset → frame"
   indexes stop reimplementing the wire layout.
2. **`From<flatstream::Error> for std::io::Error`** — the reverse of the existing
   conversion, for application boundaries that normalize on `io::Error`.
3. **`OwnedStreamWriter<W, F>`** — a lifetime-free type alias for the common
   `write_finished`-only writer.
4. **Single-`writev` framing** — the built-in framers emit each frame with one
   vectored write rather than two sequential ones. Output is byte-identical;
   isolated results are recorded in `FINDINGS_VECTORED_FRAMING.md`.
5. **Static durability policies** — `Durable` sinks, automatic frame/byte/time
   checkpoint policies, manual sync methods, and durable watermarks without a
   branch or trait object in the default writer.
6. **Static memory policies** — writer/reader reclamation state and custom
   builder factories are generic; `NoMemoryPolicy` is the zero-sized default.
7. **Positioned reads** — receipt-aware forward reads, byte-position reporting,
   and stateless indexed lookup with caller-owned scratch.

The organizing observation: the consumer succeeded without async, compression,
streamset, or segments, and the only friction was two small additive gaps. The
frame-receipt primitive is also exactly what segment indexes and positioned
reads need, so
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

The built-in framers previously put a frame on the wire with two calls:
`DefaultFramer` wrote the length prefix and payload separately, while
`ChecksumFramer` (since v0.2.7) wrote its merged length-plus-checksum header and
then the payload. On a `BufWriter` that is two staging calls; on an unbuffered
sink (raw `File`, `TcpStream`, pipe) it can be two syscalls. Both framers now
build a two-element `IoSlice` array — complete header plus payload — and issue a
single `write_vectored`.

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
  the `vectored_tests` sinks (one-byte-at-a-time, partial-vectored, stalled).

**Measured** (`docs/benchmark/FINDINGS_VECTORED_FRAMING.md`): every raw-File/TCP
pair improved (1.63–3.65× on this machine; a surprising 3.65× arm rechecked at
2.12×). The production-shaped CRC-32/64 B `BufWriter` pair repeatedly cost
1.29–1.36 ns (~7 %) more; default/64 B and both 4 KiB buffered arms were
inconsistent and are reported as inconclusive. The implementation was adopted
unconditionally rather than gated on a sink probe: a gate would add a branch to
the path it was meant to protect.

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

- **Fluent builder objects — remain rejected/archived** (`docs/archive/V2_X_FLUENT_BUILDER.md`).
  The consumer report did not ask for a builder; the `FramerExt`/`DeframerExt`
  extension methods already provide the useful, fluent portion.

## 7. Verification

- **Tests:** the unit, integration, and doctest suites cover the new receipt,
  offset, alias, and error-conversion behavior across the gate's feature matrix.
  Exact counts are omitted because later test additions make them stale. The
  current gate also runs maintained examples and README snippets and
  compile-checks benchmarks and fuzz targets.
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
- **Durability:** `tests/durability.rs` pins cadence, composition, mode strength,
  `BufWriter` flush ordering, watermarks, start offsets, manual resets, partial
  receipt accounting, and failure context. `tests/allocation.rs` proves
  simple-mode and policy-enabled checkpoint loops allocate zero times in steady
  state. Criterion and pinned instruction counts are recorded in
  `FINDINGS_SYNC_POLICY.md`.
- **Memory dispatch:** existing writer/reader reclamation tests pass unchanged,
  including deferred reader shrink and custom factories. The default path loses
  8.21 instructions/frame relative to the optional-box carrier; installed
  policy costs are separated from dispatch in
  `FINDINGS_STATIC_MEMORY_POLICY.md`.
- **Positioned reads:** `tests/positioned_reads.rs` pins byte-exact forward and
  random reads, start offsets, checksums, one-byte source reads, and safe retry
  after a live file grows. Allocation tests prove caller scratch reaches a
  zero-allocation steady state while fresh readers allocate per lookup;
  `FINDINGS_POSITIONED_READS.md` records buffering-dependent wall-clock results.
  Pinned instruction counts in `FINDINGS_POSITION_ACCOUNTING.md` characterize
  discarded/consumed writer receipts and the larger counted-reader boundary;
  no forward wall-clock regression is resolved.
- **Completed verification:** the final local gate and a clean
  `rust:1.97.1-bookworm` gate are green on the exact MSRV; Miri passes all
  in-source unit tests; both fuzz targets complete their 300-second budgets
  without a crash; every Criterion target and the pinned Gungraun suite runs;
  the corpus-backed LOBSTER integration passes.

## 8. Consuming builders are `#[must_use]`

Methods that take `self` and return a modified `Self` (or a wrapping adapter)
carry `#[must_use]`: `with_memory_policy`, `with_memory_policy_and_factory`,
`with_max_frame_len` on both deframers, `with_cooldown`, `with_baseline`, and the
`FramerExt`/`DeframerExt` combinators `bounded`, `observed`, `with_validator`,
plus durability's `with_sync_policy` and `SyncPolicyExt::or`.

Without it, `deframer.with_max_frame_len(1024);` as a statement compiles clean
and does nothing — the default 2 GiB bound stays in force. That is the failure
mode `#[must_use]` exists for: the knob that stands between a corrupt length
header and a huge allocation, silently not set. The same shape applies to
`bounded()` and `with_validator()`, where a dropped result means the limit or
the validator is simply absent. A `compile_fail` doctest on
`DefaultDeframer::with_max_frame_len` pins the diagnostic.

## 9. Breaking Changes

The wire format and every existing constructor/method signature are unchanged.
The durability work adds one source-level break permitted by the pre-1.0 policy:
`ErrorKind::DurabilityFailed` is a new public enum variant, so downstream
exhaustive matches must add an arm. `StreamWriter` gains a defaulted sync-state
type parameter; existing type spellings continue to compile because it defaults
to `NoSync`.

The memory refactor adds defaulted policy-state parameters to `StreamWriter`,
`StreamReader`, `Messages`, and `TypedMessages`. Existing default-state type
spellings continue to compile, but code that explicitly annotated the concrete
return type of `with_memory_policy` or `with_memory_policy_and_factory` must name
the new static policy/factory state.

The `#[must_use]` additions in §8 can produce **new warnings** in downstream code
that discards a builder's result. Every such warning is a latent bug — the call
was already a no-op — so this is intended, and it is a warning, not an error.

## 10. Durability and boundary follow-ups

> **Status:** §10.1–§10.3 are implemented after maintainer sign-off.

### 10.1 Static durability policies — implemented

**Problem.** A1 measured one `sync_data()` per 1000-record batch at 68.6× the
non-sync 64 B pipeline, but only 5.3× at 4 KiB. The separate cadence benchmark
shows why no cadence-independent multiplier exists. The call is important to a
journaling consumer, but previously the only route was through `get_mut()`, the
one accessor documented to bypass the frame counter.

There is a second, quieter trap. `StreamWriter<BufWriter<File>>` is the
recommended shape, and `writer.get_mut().get_ref().sync_all()` on it syncs
whatever happens to have been flushed — frames still sitting in the `BufWriter`
are not on disk, and nothing says so. "Flush before sync" is knowledge the type
system could carry and currently does not.

**Implemented surface.**

```rust
/// A sink that can push accepted bytes to durable storage.
pub trait Durable: std::io::Write {
    /// Contents only (`fdatasync`); cheaper where metadata need not be current.
    fn sync_data(&mut self) -> std::io::Result<()>;
    /// Contents and metadata (`fsync`).
    fn sync_all(&mut self) -> std::io::Result<()>;
}

impl Durable for std::fs::File { /* delegates */ }
impl<W: Durable + Write> Durable for std::io::BufWriter<W> { /* flush, then sync */ }
impl<W: Durable + ?Sized> Durable for &mut W { /* forward */ }
impl<W: Durable + ?Sized> Durable for Box<W> { /* forward */ }

pub trait SyncPolicy: Send {
    fn observe(&mut self, info: SyncInfo) -> Option<SyncMode>;
    fn on_synced(&mut self, durable_watermark: u64) {}
}

let writer = StreamWriter::new(BufWriter::new(file), DefaultFramer)
    .with_sync_policy(SyncEveryNFrames::new(
        std::num::NonZeroU64::new(1000).unwrap(),
        SyncMode::Data,
    ));
```

`NoSync` is the writer's zero-sized default. Installing a policy changes the
writer's sync-state type to `Syncing<P>` and requires `W: Durable`; policy calls
are monomorphized. The default `Vec`/`io::Sink` writer retains its old bounds,
size, and branch-free write path. Built-ins cover every frame, every N frames,
every N wire bytes, and an injected-clock interval. `SyncPolicyExt::or` composes
two policies statically and chooses `SyncMode::All` when simultaneous decisions
have different strength.

**Checkpoint semantics.** Policies observe a frame only after the sink has
accepted it completely. `BufWriter<W>: Durable` flushes before delegating the
sync. A successful checkpoint stores a durable watermark; every receipt whose
`end()` is at or below it is confirmed by that checkpoint. Manual
`sync_data()`/`sync_all()` return the same watermark and reset the policy window.

If a checkpoint fails, the frame is already accepted. Returning an ordinary
`Io` error would invite a duplicate retry, so `ErrorKind::DurabilityFailed`
records the attempted watermark, previous successful watermark, triggering
frame coordinates (for automatic checkpoints), sync mode, and source error.

**Deliberately not implemented for in-memory sinks.** No `impl Durable for
Vec<u8>` or `io::Sink`. A no-op `sync` that returns `Ok` is a lie of exactly the
kind this crate's culture rejects; absence of the impl makes "this sink has no
durability" a compile-time fact. Test code that needs a durable sink can use a
`tempfile`, as the existing tests already do.

**macOS semantics.** `File::sync_all` maps to `fsync`, which on
macOS does **not** flush the drive's write cache; only `F_FULLFSYNC` does. A
crate that forbids unsafe by default cannot add `fcntl(F_FULLFSYNC)` casually.
The trait therefore preserves and documents standard-library semantics without
unsafe code or a runtime dependency.

### 10.2 `FrameReceipt::end()` and `range()` — implemented

**Problem.** `frame_start + wire_len` is written out by hand in the README
recipe, in `examples/external_index.rs`, and in five places in
`tests/external_index.rs`. `FrameReceipt` exists precisely so that consumers do
not do wire arithmetic, and then leaves them doing the one piece that matters —
the piece you must get right to seek to the *next* frame or to bound a read.

**Implemented surface.**

```rust
impl FrameReceipt {
    /// Offset one past the frame's last byte: where the next frame begins.
    pub const fn end(&self) -> u64;
    /// The frame's byte range on the wire, `frame_start..end()`.
    pub const fn range(&self) -> std::ops::Range<u64>;
}
```

Both are `const fn`, total, and derivable from public fields. They add no new
capability, but give checkpoint and index code one named place to compute frame
boundaries.

### 10.3 Positioned reads — implemented

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

The old supported pattern allocated a fresh reader and frame buffer per lookup.
Palimpsest also needed forward receipts to retire its remaining
`8 + payload.len()` resume-scan arithmetic.

**Implemented surface.**

```rust
pub struct ReadFrame<'a> {
    pub payload: &'a [u8],
    pub receipt: FrameReceipt,
}

pub fn read_frame_at<'s, R: Read + Seek, D: Deframer>(
    src: &mut R,
    deframer: &D,
    offset: u64,
    scratch: &'s mut Vec<u8>,
) -> Result<Option<ReadFrame<'s>>>;

impl<R: Read, D: Deframer, M: ReaderMemoryBackend> StreamReader<R, D, M> {
    pub fn with_start_offset(self, offset: u64) -> Self;
    pub fn bytes_consumed(&self) -> u64;
    pub fn read_message_with_receipt(&mut self)
        -> Result<Option<ReadFrame<'_>>>;
}
```

The free function seeks to an absolute offset, decodes exactly one frame through
the supplied deframer, and leaves the source at the next frame boundary. Scratch
grows to a high-water mark and is then allocation-free. Bounds and checksum
semantics remain the deframer's responsibility.

Forward `StreamReader` wraps its source in a counting reader. Receipts and
`bytes_consumed()` use the same optional start-offset base as writer receipts.
Bytes read directly through `get_mut()` bypass accounting and are documented as
the corresponding footgun.

**Measured result.** Fresh-reader-per-lookup allocates every frame; warmed
caller scratch allocates zero times. Wall-clock direction depends on frame size
and buffering: at 4 KiB a retained `BufReader` narrows the caller-scratch cost,
while at 64 KiB bare and buffered caller-scratch reads slightly outperform the
fresh-reader baseline. See `FINDINGS_POSITIONED_READS.md`.

**Live-file implication.** `UnexpectedEof` means the current read attempt
reached EOF mid-frame, not that the file is finalized. A seekable follower can
retry `read_frame_at` with the same absolute offset after more bytes arrive;
each call rewinds before parsing, so a partial attempt cannot strand the reader
mid-frame. Recovery remains the layer that interprets the same condition as a
torn tail after writing has stopped.
