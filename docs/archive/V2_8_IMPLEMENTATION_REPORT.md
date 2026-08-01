# v0.2.8 Implementation Report

**Status:** implemented; pre-release record  
**Release line:** `v0.2.8`  
**Recorded:** 2026-08-01  
**Wire format:** unchanged from v0.2.7

## 1. Purpose of this report

v0.2.8 grew from production use rather than a format-roadmap exercise. A real
journaling consumer needed exact frame positions and less application-owned
wire arithmetic. The work then exposed adjacent boundary problems: partial I/O
could leave a stream misaligned, mutable source/sink access could invalidate
position accounting, durability failure was easy to mistake for write failure,
and point reads needed an ownership model that did not allocate a new reader
for every lookup.

The implementation therefore became a boundary-hardening release rather than a
single API addition. This report records:

- what shipped;
- why the chosen shape was selected;
- alternatives that were rejected or deferred;
- how those decisions constrained the implementation;
- what evidence closes the release; and
- which conditions should cause a deferred decision to be reopened.

It is an archival rationale, not a replacement for the normative wire
specification or public rustdoc.

## 2. Release thesis

The release keeps the existing headerless frame:

```text
[u32 little-endian payload length][optional fixed-width checksum][payload]
```

No preamble, schema identifier, checksum identifier, compression marker,
segment header, or container metadata was added. Persistent applications remain
responsible for recording and validating their framing, checksum, and schema
generation out of band.

Within that constraint, v0.2.8 makes byte ownership, frame position, failure,
durability, and observation explicit without adding payload copies, hot-path
trait objects, or steady-state allocations.

## 3. Delivered implementation

### 3.1 Exact frame receipts

`FrameReceipt { frame_start, wire_len }` is returned by receipt-aware writer
methods and reused by read APIs. `end()`, `checked_end()`, and `range()` remove
the remaining caller-side offset arithmetic. Receipts derive the common value
traits needed for indexes, including `Copy`, `Hash`, and `Ord`.

The writer counts bytes actually accepted by the sink. It does not infer frame
length from a known built-in header width, so receipts remain correct for
custom framers and partial writes.

This decision avoided changing the `Framer` trait. Making every framer return a
length would have spread a breaking signature change across an extension point
when the outer writer already had enough information to count accepted bytes.

### 3.2 Position-aware and positioned reads

Sequential readers expose `bytes_consumed()` and receipt-aware methods.
`read_frame_at` performs a stateless point read from an absolute offset using
caller-owned scratch. Once scratch reaches its high-water mark, repeated point
reads allocate nothing.

The point-read API is a free function rather than `StreamReader::seek_to`.
A method on a live reader would have to reconcile a seek with its reusable
buffer, pending memory reclamation, source buffering, and sequential position.
The free function owns none of that state.

`RetrySafeDeframer` makes same-offset retry semantics explicit. A deframer may
opt in only when rewinding the source is sufficient to restore its decode state
after an incomplete attempt.

### 3.3 Fail-stop streams

A failed write that accepted frame bytes poisons the writer. A failed
sequential read that consumed frame bytes poisons the reader. Later operations
return the typed `ErrorKind::Poisoned`; `is_poisoned()` permits inspection
without provoking another failure.

Zero-byte I/O failures do not poison because the stream remains at a known frame
boundary and the operation may be retried.

The recovery shape is deliberate:

1. stop using the poisoned stream;
2. consume it with `into_inner`;
3. recover or truncate only a verified torn tail;
4. reconstruct at the recovered absolute offset.

This prevents a partial frame from being followed by bytes that are incorrectly
treated as a new frame.

### 3.4 No direct source/sink escape hatch

`get_ref` and `get_mut` were removed from `StreamWriter` and `StreamReader`.
Keeping only shared access would not solve the accounting problem because some
I/O types can perform I/O through a shared reference.

Out-of-band I/O now requires consuming the stream and reconstructing it with
the correct offset. This makes position accounting an invariant rather than a
documentation warning.

### 3.5 Error conversion

`Error` converts into `std::io::Error` while retaining the complete flatstream
error as the inner payload:

- underlying I/O failures preserve their standard I/O kind;
- `UnexpectedEof` remains `io::ErrorKind::UnexpectedEof`;
- protocol, validation, poison, and durability failures map to `InvalidData`.

The initial branch implementation mapped every case to `Other`. That was
reversed because callers returning `io::Result` still need standard control
flow, not only a diagnostic source chain.

### 3.6 Static durability policies

`Durable` distinguishes userspace flushing from storage synchronization.
`BufWriter<W>` flushes before delegating a checkpoint. Frame-, byte-, and
interval-based policies compose statically, and manual checkpoints return a
durable byte watermark in the same coordinate system as receipts.

Durability failure is not represented as an ordinary write error. The frame was
already accepted, so `DurabilityFailed` carries the attempted watermark,
previous watermark, mode, source error, and triggering receipt coordinates.
Blindly retrying the write would duplicate a frame.

No `Durable` implementation exists for in-memory sinks. A successful no-op
checkpoint would make a false persistence claim.

The standard library is the platform boundary. As of Rust 1.97.1, both
`File::sync_data` and `File::sync_all` use `fcntl(F_FULLFSYNC)` on Apple
platforms; Linux retains the `fdatasync`/`fsync` distinction. No unsafe or
platform-specific durability shim was added.

### 3.7 Static memory policy state

Memory policy and builder factory state moved into concrete generic types.
`NoMemoryPolicy` remains the zero-sized default, so the default hot path does
not pay for an optional branch or boxed callback.

Reader reclamation is scheduled after a successful read and performed only at
the beginning of the next read. This ordering ensures `shrink_to` cannot
invalidate the payload slice just returned to the caller. Reclamation itself
may allocate or move memory and is explicitly outside the steady-state claim.

### 3.8 Post-write observation

Payload observers remain payload adapters. They cannot truthfully report final
I/O success, receipt bounds, durability outcome, or operation latency.

`PostWriteObserver` therefore wraps the complete writer operation and reports
one final outcome:

- serialization failed;
- framing or sink I/O failed, with exact accepted-byte count;
- the frame was accepted but durability failed; or
- the frame and any required checkpoint succeeded.

The default observer is zero-sized and performs no clock read or callback.
Installed observation remains statically dispatched and dependency-free.

### 3.9 Common API polish

The release also:

- exports `ObserverFramer` and `ObserverDeframer` at crate root;
- adds common `Debug`, `Clone`, `Copy`, equality, hash, and ordering traits
  where the contained state permits them;
- adds `#[must_use]` to consuming configuration methods;
- provides `OwnedStreamWriter` for the common non-borrowing builder lifetime;
- preserves the original error as the source when crossing an `io::Error`
  boundary; and
- removes stale internal planning comments and duplicate implementation blocks.

## 4. Vectored framing decision

The built-in framers now present the complete stack header and borrowed payload
as two `IoSlice`s. A native vectored sink can accept both with one
`write_vectored` call. A cold continuation handles partial acceptance,
`Interrupted`, and `WriteZero`.

This changes call shape, not bytes:

- payload bytes are still borrowed rather than copied by flatstream;
- `writev` is not treated as atomic;
- non-vectoring sinks fall back to the same header-then-payload progression; and
- the golden wire corpus remains byte-identical.

The evidence was mixed by sink:

- unbuffered file and TCP pairs improved materially in the isolated runs;
- a tiny CRC-32 `BufWriter` case repeatedly regressed by roughly 1.3 ns per
  frame;
- other buffered cases were inconsistent.

An earlier direction described vectored framing as opt-in. The landed
implementation instead made it the built-in default, favoring the unbuffered
syscall reduction over the small measured buffered regression. This is a
performance trade, not a semantic requirement of receipts or fail-stop
handling.

There is no stable `Write::is_write_vectored` probe on the MSRV, so the
implementation cannot select the path dynamically without nightly Rust.
`CountingWriter` forwards `write_vectored` directly so its accounting wrapper
does not erase native vectoring.

Reopen the default-versus-opt-in decision if the dominant deployment profile is
shown to be tiny frames through `BufWriter`, if a stable capability mechanism
becomes available, or if same-run evidence shows a meaningful regression on a
supported sink.

## 5. Deferred and rejected decisions

### 5.1 Core stream preamble or v3 header — declined for this line

The core remains headerless. A preamble would change every on-wire byte and
create format/version ownership inside a framing library. Applications must
carry a manifest for framing, checksum, and schema generation.

**Impact on delivered work:** receipts and recovery operate on existing v0.2
frames; no negotiation or auto-detection path exists.

**Reopen only when:** the maintainer starts an explicit new-format line with a
migration and compatibility plan.

### 5.2 `IncompleteFrame` error kind — declined

`UnexpectedEof` describes the current read attempt, not permanent source
finality. A live seekable source may grow; a finalized journal may have a torn
tail.

**Impact on delivered work:** live followers retry `read_frame_at` from the
same offset, while recovery authorizes truncation only after writing has
stopped. A sequential reader cannot resume after consuming part of a frame.

**Reopen only when:** a new source abstraction can represent lifecycle finality
without conflating it with read mechanics.

### 5.3 Checksum inner composition — declined

All current adapters are payload-level pass-throughs and already compose around
a terminal checksum framer. Moving them inside would checksum the same bytes.
A terminal framer inside another terminal framer would duplicate the length
prefix. The only genuinely new capability would be checksumming transformed
bytes, which is a wire-format decision.

**Impact on delivered work:** `ChecksumFramer` and `ChecksumDeframer` remain
terminal strategies with fixed payload-checksum semantics.

**Reopen only when:** an approved transform such as compression or encryption
creates a concrete need to define checksum coverage over pre- or post-transform
bytes.

### 5.4 Transparent core compression — rejected

The feasibility controls showed that even extreme byte savings did not improve
the current buffered, flush-only path; incompressible data expanded. Compression
would also require a raw fallback, decoded-size bounds, bomb protection,
per-frame codec metadata, and explicit checksum semantics.

**Impact on delivered work:** no production compression adapter, codec in the
runtime dependency graph, or wire-format change was added. Codec dependencies
remain benchmark-only.

**Reopen only when:** representative application traces and a named
storage/bandwidth constraint justify an application-owned format experiment.

### 5.5 OTEL or metrics runtime dependency — rejected

The library needs an observation boundary, not ownership of an observability
ecosystem.

**Impact on delivered work:** `PostWriteObserver` is a concrete generic callback
and applications translate events into their chosen metrics or tracing stack.

**Reopen only when:** a dependency-independent event boundary is proven
insufficient by multiple consumers.

### 5.6 Dynamic policy dispatch — rejected as the default

Boxed memory, durability, or observation policy calls would put optional
indirection on every frame.

**Impact on delivered work:** policy state is represented by generic type
parameters; zero-sized defaults compile away. Type signatures become more
verbose when a policy is installed.

**Reopen only when:** runtime policy replacement is a demonstrated requirement
and its cost is isolated.

### 5.7 Direct mutable access — removed, not deferred

An accessor that permits out-of-band I/O makes subsequent receipt coordinates
untrustworthy.

**Impact on delivered work:** `into_inner` and reconstruction are the only
supported escape path.

**Reopen only when:** a future API can preserve or explicitly rebase accounting
without allowing hidden source/sink movement.

### 5.8 General seek on `StreamReader` — deferred

A stateful seek method has unresolved interactions with buffering, pending
reclamation, custom deframer state, and sequential poison semantics.

**Impact on delivered work:** indexed access is the separate `read_frame_at`
primitive with caller-owned scratch and a retry-safe deframer.

**Reopen only when:** a concrete workload requires a stateful seek and specifies
how each of those states is reset.

### 5.9 Batch API — not added

Batch semantics belong either in the FlatBuffer schema (one root containing a
vector) or in application transaction boundaries. A library `write_batch`
would not make multiple frames atomic.

**Impact on delivered work:** applications loop over frame writes and choose
their own flush/checkpoint boundary.

**Reopen only when:** a precise batch receipt, failure, and durability contract
is proposed.

### 5.10 Symmetric reader observer — deferred

Read APIs already expose payload, receipt, clean EOF, and errors directly. A
second callback surface was not added speculatively.

**Impact on delivered work:** the post-operation hook is writer-only; payload
observer adapters retain their existing read/write roles.

**Reopen only when:** an application demonstrates missing read-side information
that cannot be collected around receipt-aware methods.

### 5.11 Fluent builder object — remains rejected

Existing extension traits provide the useful fluent composition without a
second configuration object or type-erased kernel.

**Impact on delivered work:** public construction remains ordinary Rust generic
composition.

**Reopen only when:** repeated, concrete ergonomics failures cannot be solved
with extension methods or aliases.

## 6. Compatibility and source-level changes

The on-wire format is unchanged. Source-level changes are intentional and
permitted on the pre-1.0 line:

- `ErrorKind::DurabilityFailed` and `ErrorKind::Poisoned` add variants that
  affect exhaustive matches;
- direct reader/writer source/sink accessors are removed;
- reader/writer and iterator-like types gain defaulted generic policy state;
- installed policy or observer return types include their concrete state;
- receipt/position APIs introduced on the branch use fallible rebasing and
  retry-safe point-read bounds; and
- consuming configuration methods may produce new `unused_must_use` warnings
  where their returned wrapper was previously discarded.

## 7. Invariants preserved

The final shape preserves the project contracts:

- payload bytes are not copied by the flatstream write path;
- a generic `Read` copies payload bytes once into reusable storage;
- steady-state frame loops allocate zero times after high-water marks are
  reached;
- reader reclamation cannot invalidate an outstanding payload borrow;
- static dispatch remains the default for framing, checksums, policy, and
  observation;
- default builds forbid unsafe code;
- recovery truncates only on `UnexpectedEof` after source finality is known; and
- wire bytes remain normative and byte-exact.

## 8. Verification record

The release candidate was verified with:

- the canonical feature-matrix gate on Rust 1.97.1;
- the same gate in `rust:1.97.1-bookworm`;
- Clippy across all targets and all features;
- README snippet execution and rustdoc with warnings denied;
- the targeted Miri library and positioned-read suites;
- both deframer fuzz targets for 300 seconds each;
- allocation-counting tests covering default, receipt, checksum, policy,
  observer, point-read, and native-vectored paths;
- byte-exact wire corpus and round-trip tests; and
- fault-injection tests for partial writes, partial reads, device errors,
  durability failures, retries, and poison rejection.

Benchmark raw `.txt` output is machine-local and gitignored. Reviewed findings
documents retain the methodology, summarized measurements, limitations, and
decisions that followed.

## 9. Durable decision index

The most important rationale remains available in:

- `docs/DESIGN_v2_8.md` — implemented API and release design;
- `docs/WIRE_FORMAT_SPEC.md` — normative bytes and checksum semantics;
- `docs/planning/E2_CHECKSUM_COMPOSITION.md` — checksum composition decline;
- `docs/planning/B3_OBSERVABILITY_BOUNDARY.md` — writer observation boundary;
- `docs/benchmark/FINDINGS_VECTORED_FRAMING.md` — vectored I/O trade-off;
- `docs/benchmark/FINDINGS_SYNC_POLICY.md` — durability cadence and caveats;
- `docs/benchmark/FINDINGS_STATIC_MEMORY_POLICY.md` — static memory state;
- `docs/benchmark/FINDINGS_POSITIONED_READS.md` — point-read ownership;
- `docs/benchmark/FINDINGS_READ_PATH_COPY.md` — generic-read copy boundary;
- `docs/benchmark/FINDINGS_COMPRESSION_FEASIBILITY.md` — compression rejection;
- `tests/allocation.rs` — enforced steady-state allocation contract; and
- `tests/position_accounting_faults.rs` and `tests/post_write_observer.rs` —
  fail-stop and outcome semantics.

## 10. Final assessment

v0.2.8 does not attempt to become a storage format or observability platform.
Its technical contribution is narrower: it makes the current framing layer
honest about where bytes are, what succeeded, what became durable, what may be
retried, and which state must be discarded after a partial operation.

The deferred decisions are part of that shape. Avoiding a preamble preserves
wire compatibility; rejecting transformed-byte checksum composition prevents an
incidental format change; static policies protect the hot path; the free point
read avoids stateful seek ambiguity; and the dependency-free observer exposes
the right boundary without pulling application infrastructure into the crate.

Future work should reopen those decisions only when a concrete workload supplies
the missing contract, not because the abstraction can be made more general.
