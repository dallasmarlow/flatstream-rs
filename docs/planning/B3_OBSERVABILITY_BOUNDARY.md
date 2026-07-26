# B3 — Observability boundary: design note (first deliverable)

> **Status: design note + self-asserting example only.** No public API is added
> by this deliverable. It resolves the "resolve first" question the backlog
> (`docs/CONTRIBUTING.md` §6, task B3) puts ahead of any public type, and pins
> the resolution with a runnable, dependency-free example
> (`examples/observability_boundary.rs`). Implementing public observability
> types requires separate maintainer sign-off; see §7.

## 1. The question B3 puts first

The backlog states the blocker directly:

> `ObserverFramer` runs before delegated I/O and therefore cannot report
> success, receipt bounds, or latency. Decide whether the correct deliverable is
> only an application recipe/wrapper or a generic post-operation hook with
> explicit success/failure events.

This is a real structural fact about the code, not a stylistic preference.
`ObserverFramer::frame_and_write` invokes its callback and *then* delegates to
the inner framer's I/O (`src/framing.rs`):

```rust
impl<F: Framer, C: Fn(&[u8])> Framer for ObserverFramer<F, C> {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        (self.callback)(payload);              // fires BEFORE any byte is written
        self.inner.frame_and_write(writer, payload)
    }
}
```

The consequences, each of which disqualifies `ObserverFramer` as an
*observability* (as opposed to *payload-inspection*) primitive:

1. **It cannot report success or failure.** The callback has already run by the
   time `frame_and_write` returns `Err`. An observer that increments a
   "frames written" counter in the callback counts a frame that a subsequent
   `WriteZero`/`Io` error means never fully reached the wire. The constraint in
   the backlog — "errors must not be reported as successful frames" — is
   violated by construction.
2. **It cannot see receipt bounds.** `frame_start`/`wire_len` are computed by
   `StreamWriter::write_with_receipt` *around* the framer call
   (`src/writer.rs`); the framer itself never sees them. The `ObserverFramer`
   callback receives only the payload slice, so it cannot record where the frame
   landed.
3. **It cannot time the write meaningfully.** The callback fires before I/O, so
   wrapping it in a timer measures the callback, not the write.
4. **It cannot observe durability.** A durability checkpoint runs *after* a
   complete frame is accepted (`SyncPolicy::after_frame`, `src/writer.rs`), far
   below the payload-inspection point, and can fail after bytes were already
   accepted (`ErrorKind::DurabilityFailed`). The framer-level callback is
   nowhere near this event.

`ObserverDeframer` has the symmetric-but-milder shape: its callback fires
*after* a successful inner read, so it does observe success — but it still sees
only the payload, never the `FrameReceipt` (`receipt.range()`, `wire_len`), and
never an error, because the callback is skipped entirely on `Err`.

So `ObserverFramer`/`ObserverDeframer` are correctly named: they are **payload
observers**, useful for content-derived metrics (bytes seen, message shapes,
sampling). They are *not* operation observers, and stretching them into that
role would produce silently wrong telemetry.

## 2. Resolution

**The first deliverable is an application recipe at the operation boundary — not
a new hot-path hook, and not any new public API.**

The decisive reason is that flatstream *already* exposes, at the operation
boundary, exactly the three things a framer-level hook cannot:

| Observable | Where it already lives | Type |
| --- | --- | --- |
| Success/failure of a write | return value of `write` / `write_with_receipt` | `Result<()>` / `Result<FrameReceipt>` |
| Frame bounds (offset, length, end) | `FrameReceipt { frame_start, wire_len }`, `.end()`, `.range()` | returned by the `_with_receipt` writers |
| Success/failure + bounds of a read | `read_message_with_receipt` / `process_all_with_receipt` | `Result<Option<ReadFrame>>`, `ReadFrame::receipt` |
| Durability outcome + watermarks | `sync_data`/`sync_all` return `Result<u64>`; `ErrorKind::DurabilityFailed` carries `attempted_watermark`, `previous_watermark`, triggering `frame_start`/`wire_len` | error variant + return value |

Every event the backlog's constraints care about — "errors must not be reported
as successful frames", "durability failure occurs after bytes were accepted",
"receipt bounds", "latency" — is observable *by the caller* at the `write` /
`read` / `sync` call site, where success and bounds are both in hand and a timer
brackets the real I/O. The application wraps its own call site. flatstream adds
nothing to its hot path, takes on no OTEL/metrics dependency, and keeps the
default writer branch-free.

This also satisfies the backlog constraint "callback cost exists only in the
installed concrete type": with a caller-side recipe the cost exists only in the
application's own wrapper, which is the strongest possible version of that
property.

### Why not a generic post-operation hook (yet)

A generic post-op hook — e.g. a `StreamWriter` that takes an
`Fn(Result<&FrameReceipt>)` and invokes it after each write — is *implementable*
and would sit at the correct point. It is deferred, not rejected, because:

- It is **new public API on the core writer/reader**, which §6 says needs design
  sign-off before implementation. This note is the pre-sign-off artifact; it
  should not also ship the API.
- Its marginal value over "wrap your own call site" is unproven. The call site
  already returns the receipt and the `Result`. A hook mainly helps when the
  write call site is buried inside a generic pipeline the application cannot
  wrap — a real but unquantified case.
- Any hook must answer sub-questions this note deliberately leaves open for the
  sign-off discussion: does it fire on the *durability* checkpoint as well as
  the frame write (two distinct failure points)? Does it borrow the receipt or
  copy it? Is it one hook or a small event enum (`FrameWritten`, `SyncOk`,
  `SyncFailed`)? Answering these in code before agreeing the shape would be the
  premature-API mistake §6 guards against.

The recipe below is designed so that *if* a hook is later approved, the recipe
remains the documentation of what the hook must expose — nothing in it becomes
wrong.

## 3. The recipe

Two halves, each a thin wrapper the **application** owns:

**Write side.** Call `write_with_receipt`; time the call; branch on the
`Result`. On `Ok(receipt)` record `receipt.wire_len` and `receipt.range()`
against a success counter and a latency accumulator. On `Err(e)` record a
failure against a *separate* counter — never the success one — and inspect
`e.kind()` to distinguish a framing/`Io` failure from `DurabilityFailed` (whose
bytes are already on the wire).

**Read side.** Call `read_message_with_receipt` (or
`process_all_with_receipt`); on `Ok(Some(frame))` record `frame.receipt`; on
`Ok(None)` the stream ended cleanly at a boundary; on `Err` record a failure and
inspect the kind (an `UnexpectedEof` on a live file is a torn tail / retry
signal, not a corruption event — see ONBOARDING §6).

**Durability side.** Bracket `sync_data`/`sync_all` with a timer; on `Err`, the
error is `DurabilityFailed` and its `attempted_watermark` tells the caller how
far the frames-accepted watermark had advanced when stable storage was not
confirmed — the caller must not re-emit those frames. With an automatic
`SyncPolicy` installed, the same error surfaces from the triggering `write`
call itself — which is why the write-side wrapper must classify
`DurabilityFailed` separately rather than lump it in with I/O refusals.

The invariant the recipe pins, and that the example asserts: **a write that
returns `Err` increments the failure counter and leaves the success counter and
the durable-bytes total untouched.** That is precisely the property
`ObserverFramer` cannot provide, which is the whole reason B3 exists.

## 4. Constraints check (from §6)

- **No OTEL dependency, no metrics crate.** The example translates events into a
  plain in-process `struct` of counters. Mapping those to OTEL/Prometheus is the
  application's job and is named as such.
- **No span per frame by default.** The recipe adds nothing to flatstream's hot
  path; the per-frame cost is whatever the application's own wrapper does.
- **Errors must not be reported as successful frames.** Enforced by branching on
  the `Result` at the boundary and asserted in the example.
- **Durability failure occurs after bytes were accepted.** Surfaced via
  `DurabilityFailed::attempted_watermark` and asserted in the example: a failed
  checkpoint's watermark equals the bytes the sink actually accepted, and the
  recipe classifies it separately so the caller neither counts the frame as ok
  nor re-emits it.

## 5. Self-asserting example

`examples/observability_boundary.rs` (added by this deliverable, run by
`scripts/examples.sh`) is dependency-free and asserts:

1. A three-frame write records exactly three successes, zero failures, and a
   recorded byte total equal to the writer's `bytes_written()`.
2. Each recorded frame range tiles the stream contiguously and byte-exactly
   (`prev.end() == next.frame_start`), the same contiguity property
   `external_index` pins — here observed through the telemetry wrapper.
3. A forced write failure (a sink that refuses after N bytes) increments the
   **failure** counter and leaves the **success** counter and recorded-byte
   total unchanged — the property `ObserverFramer` cannot deliver.
4. The read side, driven with `process_all_with_receipt`, recovers the same
   frame count and the same contiguous ranges the write side recorded.
5. With an automatic sync policy installed over a sink that accepts bytes but
   cannot confirm a checkpoint, the write call returns
   `Err(DurabilityFailed)`: the telemetry classifies it as a durability
   failure (not an I/O refusal, not a success), and the error's
   `attempted_watermark` equals both the writer's advanced position and the
   bytes the sink actually holds — pinning "durability failure occurs after
   bytes were accepted".

## 6. What this deliverable is not

- Not a public `ObserverFramer` change. The payload observers stay as they are;
  their rustdoc already scopes them to payload inspection.
- Not a benchmark. No performance claim is made; the recipe's cost is the
  application's, and A-lane findings docs are the place for any number.
- Not a wire change. Receipts and errors already exist; nothing new touches the
  bytes.

## 7. Next step (blocked on sign-off)

If the maintainer wants a first-party post-operation hook, the follow-up is a
separate design proposal covering: the event surface (single callback vs. event
enum), whether it fires on durability checkpoints, borrow-vs-copy of the
receipt, the concrete zero-sized default (a `NoObserver` analogous to `NoSync`),
and an overhead benchmark + findings doc proving the installed-cost-only
property. Only then does public API land.
