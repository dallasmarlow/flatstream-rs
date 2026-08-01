# B3 — Post-write observability boundary

> **Status: implemented after maintainer sign-off (2026-07-29).**
> `PostWriteObserver` is a statically dispatched `StreamWriter` hook;
> `examples/observability_boundary.rs` and `tests/post_write_observer.rs` pin
> its semantics. `docs/benchmark/FINDINGS_POST_WRITE_OBSERVER.md` records the
> pre-final-writer timing; recollect before publishing a release-candidate
> overhead.

## 1. The structural problem

`ObserverFramer` is a payload-inspection adapter. Its callback executes before
delegated framing/I/O:

```rust
fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
    (self.callback)(payload);
    self.inner.frame_and_write(writer, payload)
}
```

That location cannot truthfully report operation outcomes:

- a later sink error would already have been counted as success;
- `FrameReceipt` is computed by `StreamWriter`, outside the framer;
- callback timing excludes the actual write;
- automatic durability runs only after a complete frame is accepted and may
  return `DurabilityFailed`.

`ObserverDeframer` is similarly scoped to payload inspection: it runs after a
successful payload read but has no receipt or failure event. These adapters
remain useful for content-derived inspection and keep their existing semantics.
They are not operation telemetry.

## 2. Decision

Operation observation belongs on `StreamWriter`, around the complete public
`write*` operation. The approved surface is:

```rust
pub struct NoPostWriteObserver;

pub trait PostWriteObserver: Send {
    const ENABLED: bool = true;
    fn on_write(&mut self, event: PostWriteEvent<'_>);
}

pub struct PostWriteEvent<'a> {
    pub payload_len: Option<usize>,
    pub elapsed: Duration,
    pub outcome: PostWriteOutcome<'a>,
}

pub enum PostWriteOutcome<'a> {
    Succeeded(FrameReceipt),
    SerializationFailed(&'a Error),
    WriteFailed {
        frame_start: u64,
        bytes_accepted: u64,
        error: &'a Error,
    },
    DurabilityFailed {
        receipt: FrameReceipt,
        error: &'a Error,
    },
}

impl StreamWriter<...> {
    pub fn with_post_write_observer<O: PostWriteObserver>(
        self,
        observer: O,
    ) -> StreamWriter<..., O>;
}
```

Closures implementing `for<'event> FnMut(PostWriteEvent<'event>) + Send` work
directly as observers. There is no trait object, metrics dependency, or payload
copy.

## 3. Ordering and outcome contract

For simple-mode `write`/`write_with_receipt`, observation starts before
serialization and completes after framing, memory-policy bookkeeping, and any
automatic durability checkpoint. Expert-mode `write_finished*` starts from the
already-finished payload. The callback executes immediately before the method
returns its final `Result`; callback time is excluded from `event.elapsed`.

Exactly one event is emitted:

1. **`SerializationFailed`** — no payload was completed and no framing I/O was
   attempted; `payload_len` is `None`.
2. **`WriteFailed`** — framing/sink I/O failed. The event reports the attempted
   frame start and exact accepted-byte count; no complete receipt is fabricated.
   A nonzero accepted count poisons the writer, which rejects later writes and
   checkpoints until the sink is consumed, recovered, and reconstructed.
3. **`DurabilityFailed`** — the complete frame was accepted and its receipt is
   supplied, but the automatic checkpoint failed. The caller must not re-emit
   the frame.
4. **`Succeeded`** — the complete frame was accepted and any checkpoint due for
   this operation succeeded. The receipt is final and exact.

The success callback therefore cannot fire before `write_vectored` resolves or
before an automatic checkpoint reports its outcome. Errors are borrowed only
for the callback; the original owned error is returned unchanged.

## 4. Static default and cost

`StreamWriter` gains a final defaulted generic observer state:

```rust
pub struct StreamWriter<..., O = NoPostWriteObserver> {
    observer: O,
}
```

`NoPostWriteObserver` is zero-sized and sets `ENABLED = false`. Its
monomorphization does not call `Instant::now` and invokes no callback. Installing
an observer enables one start/end clock pair plus the concrete callback on each
write.

The isolated benchmark measured:

- default: 1.988 ns/frame in the in-memory harness;
- installed receipt + latency observer: 33.620 ns/frame;
- paired installed delta: 31.632 ns/frame;
- zero allocations/reallocations per observed frame.

These are Apple-M4 in-memory figures, not portable percentages. Full method and
threats are in `FINDINGS_POST_WRITE_OBSERVER.md`.

## 5. Scope boundaries

- **Payload adapters remain payload adapters.** `FramerExt::observed` and
  `DeframerExt::observed` are not silently redefined.
- **Automatic durability is part of the write event.** Emitting a separate sync
  event for the same call would double-report one operation.
- **Manual checkpoints remain explicit calls.** Applications can bracket
  `sync_data`/`sync_all`; those methods do not masquerade as frame writes.
- **Reads already expose success, receipt bounds, clean EOF, and errors through
  `read_message_with_receipt`/`process_all_with_receipt`.** A symmetric reader
  hook is not added speculatively in this change.
- **There is no batch event.** Flatstream has no `write_batch` API; applications
  define their own transaction/batch boundaries.
- **Observer panics are not caught.** A callback is application code and follows
  ordinary Rust panic behavior.

## 6. Executable evidence

`tests/post_write_observer.rs` asserts:

- the default observer is zero-sized;
- a success callback runs after all frame bytes are accepted;
- serialization and sink failures never emit success;
- durability failure carries the accepted frame receipt and preserves the
  `DurabilityFailed` context.

`tests/allocation.rs` requires an installed observer to allocate and reallocate
exactly zero times over the armed steady-state loop.

`examples/observability_boundary.rs` translates events into dependency-free
mock telemetry and asserts contiguous successful ranges, failure separation,
read-side receipt agreement, and accepted-but-not-durable classification.

## 7. Compatibility

The wire format and existing constructor/method calls are unchanged.
`StreamWriter` and `OwnedStreamWriter` gain a defaulted observer type parameter;
existing type spellings continue to compile. Code that explicitly names the
return type of `with_post_write_observer` must include its concrete observer
state, as with installed sync and memory policies.
