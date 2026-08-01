# E2 — Checksum framer/deframer inner composition: decision memo

> **Status: decision memo only — DECLINED, with rationale.** For task E2, the
> semantic question is resolved *before* any code. The outcome the backlog
> explicitly admits as valid — "a documented
> *declined, with rationale*" — is the outcome here. No public API changes; no
> wire change. Revisit only under the trigger in §5.

## 1. The proposal

The archived fluent-builder exploration (`docs/archive/V2_X_FLUENT_BUILDER.md`
§4.2) flagged one remaining composability gap: `BoundedFramer<F>`,
`ValidatingFramer<F, V>`, and `ObserverFramer<F, C>` all wrap an inner
`Framer`, but `ChecksumFramer<C>` / `ChecksumDeframer<C>` stand alone as
*terminal* framers. E2 asks whether `ChecksumFramer` should also take an
`inner: F`, so a checksum could sit mid-chain rather than only at the end.

The backlog is clear that the constructor break is *not* the blocker
(§1 permits breaking changes). The blocker is semantic: **what does the checksum
cover once composed?**

## 2. What the checksum covers today (normative)

`docs/WIRE_FORMAT_SPEC.md` §3, §5, §8 fix this and it is not negotiable outside
3.0:

- Frame layout is `[4-byte LE length][N-byte checksum][payload]`.
- **The checksum covers the payload bytes only** — the length prefix is not
  included in the calculation (§5, §8).
- The length prefix is emitted by the framer itself and precedes the checksum
  field at a fixed offset.

So `ChecksumFramer::frame_and_write(w, payload)` emits
`[len][checksum(payload)][payload]`, computing the checksum over the raw
`payload` slice (`src/framing.rs`). `ChecksumDeframer` reads the merged
`[len | checksum]` header, then the payload, then verifies `checksum(payload)`.

## 3. Why inner composition has no coherent, in-scope meaning

### 3.1 Every existing adapter is a payload-level pass-through

`BoundedFramer`, `ValidatingFramer`, and `ObserverFramer` all receive the
`payload` slice, act on it *without transforming it* (bound-check its length,
validate it, observe it), and hand the **same bytes** to their inner framer
(`src/framing.rs`). None of them alter the bytes that eventually get framed.

Consequence: for all three, wrapping *around* a terminal `ChecksumFramer` —
which is already supported today via their existing `inner` parameter — is
**semantically identical** to any hypothetical wrapping *inside* it, because the
bytes the checksum sees are the same either way:

```rust
// All supported today; all checksum the identical payload bytes:
DefaultFramer                            // no checksum
ChecksumFramer::new(Crc32::new())        // terminal checksum
BoundedFramer::new(ChecksumFramer::new(Crc32::new()), max)
ValidatingFramer::new(ChecksumFramer::new(Crc32::new()), v)
ObserverFramer::new(ChecksumFramer::new(Crc32::new()), cb)
```

Ordering is inert for pass-through adapters. There is no chain expressible with
an inner-checksum that produces a *different, useful, in-scope* result than a
chain already expressible today.

### 3.2 A terminal inner would double the length prefix

`ChecksumFramer` owns the length prefix. The only framers that could serve as
its `inner` are (a) another terminal framer (`DefaultFramer`, another
`ChecksumFramer`) — which emits its *own* length prefix, so the wire becomes
`[len][checksum][len][payload]`, an incoherent double-header and a wire change;
or (b) another pass-through adapter — which itself still bottoms out at a
terminal framer, recreating case (a). There is no inner that both (i) avoids a
second length prefix and (ii) adds capability over §3.1's existing composition.

### 3.3 The one genuinely new capability is a wire-format decision (3.0)

The *only* composition inner-checksum would newly enable is a checksum computed
over **transformed** bytes rather than the raw payload — e.g. checksum the
*compressed* or *encrypted* output, or checksum the pre-transform input while
framing the post-transform bytes. That is precisely a choice about **what the
checksum covers**, which §5/§8 of the wire spec make normative and which §7 of
CONTRIBUTING assigns to the 3.0 format work. It cannot be introduced as an
incidental composability convenience on the 0.2 line.

No payload-transforming adapter exists in the crate today (compression is the
benchmark-only A4 experiment, explicitly *not* a production framer). So the
capability inner-checksum would unlock has no consumer, and the moment it does
have one, the design question is a wire/format question, not a mid-chain
question.

## 4. Cost of doing it anyway

Even setting merit aside, adding `inner: F` to `ChecksumFramer<C>` /
`ChecksumDeframer<C>`:

- Adds a second generic parameter to the crate's most-used checksummed framer,
  so every downstream signature that names `ChecksumFramer<C>` /
  `ChecksumDeframer<C>` (examples, tests, the `ChecksumFramer<C>` re-exports,
  existing `CRC-32` consumers) gains a type parameter for no behavioral gain.
- Invites exactly the incoherent chains of §3.2 into the type system, which then
  need documentation and tests to explain why they are wrong.
- Spends a breaking change (cheap per §1, but not free in churn) to reach a state
  observably equivalent to today for every in-scope chain.

## 5. Decision

**Decline.** Keep `ChecksumFramer<C>` / `ChecksumDeframer<C>` terminal. The
composability gap is real in the abstract but empty in practice on the 0.2 line:
every payload-level adapter already composes around a terminal checksum with
identical checksum semantics (§3.1), and the only new capability
inner-composition would grant is a change to what the checksum covers — a
normative wire-format decision reserved for 3.0 (§3.3).

**Revisit only when** the 3.0 format work (or an approved payload-transforming
adapter such as a production compression/encryption framer) introduces a real,
demonstrated need to checksum transformed bytes. At that point it is a
wire-format design item under §7, opened as its own proposal, not a resurrection
of this mid-chain-composition convenience.

## 6. Recorded in

This decline is the E2 counterpart to the fluent-builder rejection already
recorded in `docs/DESIGN_v2_8.md` §6. The entry there keeps the "deferred and
rejected" ledger complete.
