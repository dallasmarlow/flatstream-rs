# Planning: source clarity and structure

**Status:** Needs re-audit — source line references predate the final v0.2.8
hardening pass; do not assign tasks from this snapshot without rechecking them
**Date:** 2026-07-24
**Author:** contributor
**Targets:** pre-3.0, incremental
**Wire format:** unchanged. **Public API:** unchanged except §S1 (docs only) and
§S6 (re-exports, additive).

---

## 0. The thesis

The maintainer's stated bar is that every section of `src/` has a clear and
defined purpose and high-quality comments throughout. Measured against that bar,
the crate is not uniformly short — it is **bimodal**. `recover.rs`, `writer.rs`,
and `policy.rs` contain some of the best explanatory comments I have read in a
Rust crate this size. `checksum.rs` and `validation.rs` contain almost none where
it matters, and what they do contain is often narration of the obvious.

That distribution matters more than the average, because it means **the crate
already contains its own style guide.** The work proposed here is not to invent a
commenting standard and impose it. It is to extract the standard that the strong
files already follow, write it down once, and bring the rest of `src/` up to it.

A raw comment-density count hides this. Splitting production code from tests, and
rustdoc (`///`, `//!`) from explanatory inline comments (`//`):

| file | prod lines | rustdoc | **inline `//` in prod** |
|---|---|---|---|
| framing.rs | 676 | 155 | 38 |
| writer.rs | 542 | 248 | 28 |
| policy.rs | 423 | 138 | 16 |
| reader.rs | 465 | 223 | 10 |
| validation.rs | 325 | 66 | **9** |
| error.rs | 221 | 60 | 0 |
| checksum.rs | **173** | 48 | **4** |
| recover.rs | 192 | 115 | 3 |
| traits.rs | 84 | 45 | 3 |
| lib.rs | 155 | 107 | 5 |

`checksum.rs` is half tests; its production body is 173 lines carrying four
inline comments, and §2 shows that all four are narration or vestigial. Its count
of substantive "why" comments is zero. `error.rs`'s zero is fine — it is
declarative code whose one design decision is explained in rustdoc on the type.

---

## 1. The house style, as the code already practices it

Four patterns recur in the strong files. They are the standard; the tasks below
just apply them.

**1. State the constraint that forced the code, not what the code does.**

`src/framing.rs` explains why the single-call case is peeled out of the
partial-write loop — because routing a whole small frame through
`IoSlice::advance_slices` costs more than the vectored write saves — and cites
the findings doc that measured it. A reader who wants to "simplify" the peel now
knows what they would be giving up.

**2. Record the decision *not* taken.** `src/validation.rs` L309–312 is the
model:

```rust
// Intentionally no Default implementation for `TypedValidator` to prevent
// accidental construction of a no-op typed validator. Use the explicit
// schema-verifier constructors: `from_verify_named`, ...
```

Absence is invisible in code. Only a comment can carry it.

**3. Justify every default with a number and a reason.** `policy.rs` gives
`DEFAULT_SIZE_RATIO_THRESHOLD`, `DEFAULT_MESSAGES_TO_WAIT`, and
`DEFAULT_BASELINE_CAPACITY` a paragraph each. `validation.rs` calls `max_depth:
64, max_tables: 1_000_000` "conservative defaults" and stops.

**4. Explain the boundary, not just the purpose.** `recover.rs`'s module doc
enumerates which errors do *not* authorize truncation and why for each, then
closes with an explicit `Scope:` paragraph. It is the best module doc in the
crate and the template for §S2.

---

## 2. What is below the bar

Every item cites file and line so a reviewer can verify without taking this
document's word for it.

### 2.1 Two comments state the wrong reason

These are worse than missing comments, because a reader who trusts them is
actively misled.

**`validation.rs` L87** labels a bounds precondition as a performance
optimization:

```rust
// Fast path trivial size sanity check; avoids constructing options for empty buffers.
if payload.len() < 4 {
```

The `< 4` check is the *only* thing keeping the four unchecked indexes at L105–106
in bounds:

```rust
let root_rel =
    u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
```

The comment invites a future contributor to delete a "pointless
micro-optimization" and get a panic on a 3-byte payload.

**`policy.rs` L273** describes arithmetic that is not in the function:

```rust
if last_message_size == 0 {
    // Avoid division-by-zero style logic; treat as no signal
```

The arithmetic at L280 is `saturating_mul`. The guard's real purpose is that with
`last_message_size == 0` the product is zero, so `current_capacity >= 0` holds
unconditionally and *every* zero-length message would read as over-provisioned,
firing a reclaim every `messages_to_wait` messages. The reader is sent looking
for a `/` that does not exist.

### 2.2 The two weak files, specifically

**`checksum.rs`.** All four of its inline comments are narration (`// crc32fast
returns a u32, so we cast it to u64`, L117; the same for crc16 at L144; `// Always
succeeds - no verification needed` above a body that is `Ok(())`, L169) or
vestigial (L149, speculative "we can provide" voice, sitting above the real
rustdoc). Meanwhile the genuinely non-obvious things are silent:

- **L41–44** `write_bytes` copies all 8 bytes and returns a `SIZE`-length prefix,
  rather than copying `SIZE` bytes. This is deliberate — a fixed-8
  `copy_from_slice` has a statically known length so the bounds assert folds away
  and it compiles to one store. `framing.rs` makes exactly this argument for
  header assembly. Here it reads as sloppiness someone will "fix."
- **L62–67** `width_mask`'s `size >= 8` arm exists because `1u64 << 64` is a shift
  overflow, not because `u64::MAX` is tidier.
- **L162** `NoChecksum::SIZE = 0` means `ChecksumFramer<NoChecksum>` emits a frame
  byte-identical to `DefaultFramer`'s, readable by `DefaultDeframer`. That is a
  wire-format equivalence anyone reasoning about stream compatibility needs, and
  it is written down nowhere.
- **L13** `Checksum` lacks the `Send + Sync` bound that `Validator` has and that
  `MemoryPolicy` argues for, with no stated rationale.

**`validation.rs`.** Beyond L87 above:

- **L101–112** omits the three facts that matter. `root_rel` is
  **attacker-controlled** — it comes straight from payload bytes — and is passed
  as a position with no bounds check of our own, safe only because
  `Verifier::visit_table` bounds-checks it. That is the single most important
  safety fact in the file. Also unstated: why the root is hand-computed rather
  than using `flatbuffers::root_with_opts` (that needs a concrete type; this
  validator is deliberately type-agnostic), and that `.map(|tv| tv.finish())` is
  what pops the verifier's depth accounting rather than a discarded value.
- **L61–67** `max_depth: 64, max_tables: 1_000_000` — magic numbers called
  "conservative" with no statement of what they bound or what a caller should
  raise them to.
- **L130** `SizeValidator::new(100, 10)` is accepted and rejects every payload.
  No invariant stated, no `debug_assert`.
- **L85 / L136 / L315** use `#[inline]`, `#[inline(always)]`, and `#[inline]` with
  no rationale, in a crate where `framing.rs` and `writer.rs` justify every such
  choice.

### 2.3 Two dead cross-references

```rust
// This addresses Lesson 4 and 16 for memory efficiency.   // reader.rs L86
// This addresses Lesson 5 and 9 by starting with simple... // traits.rs L33
```

These name a document that does not exist in the repository. They are
unactionable for anyone who is not the original author with the original notes
open, and they should be replaced with the substance or deleted.

### 2.4 Module boundaries

`error.rs` has **no module doc at all** — the only such file — despite encoding a
real decision (the boxed-kind pattern that keeps `Error` pointer-sized, explained
on the type but never framed as the module's reason to exist).

`traits.rs`'s one-liner claims "Core traits for the flatstream library," which is
**wrong**: `Framer`, `Deframer`, `Validator`, `Checksum`, `MemoryPolicy`, and
`Clock` are all core traits living in other files. A reader following that
sentence looks in the wrong place.

`framing.rs`, `writer.rs`, `reader.rs`, and `checksum.rs` have one-line docs that
state a purpose but no boundary — nothing says what belongs there versus
elsewhere. In `framing.rs`'s case the one-liner does not even cover the contents
(§2.5).

### 2.5 Cohesion

**`framing.rs` (926 lines) has two clean seams.**

L35–112 is `write_all_vectored` / `write_remainder`: a general-purpose `std::io`
polyfill for the unstable `Write::write_all_vectored`. It knows nothing about
length prefixes, checksums, or frames and would work unchanged for any protocol.
L465–631 is five adapter types (`BoundedFramer`, `Validating{Framer,Deframer}`,
`Observer{Framer,Deframer}`) that share a shape with each other and no code with
the concrete framers above them.

The module doc is the evidence the problem is real: "Defines the framing and
deframing strategies for the byte stream" describes neither an I/O polyfill nor a
combinator layer. When a one-line boundary statement cannot cover a module's
contents, the contents have outgrown the boundary.

**`writer.rs`'s `CountingWriter` (L23–91) has a twin in another module.**
`recover.rs` L67–78 defines `CountingReader` with the same concept, the same
field names, and the same purpose — byte accounting to produce offsets. Neither
references the other, though they are the write-side and read-side halves of one
invariant.

**`evaluate_memory_policy` is duplicated across modules.** `writer.rs` L387–420
and `reader.rs` L185–206 are structurally identical — same `let Some(slot) = ...
else { return }`, same `capacity > slot.baseline_capacity` gate, same
`should_reset` → act → `on_reclaim` sequence — differing only in how capacity is
obtained and whether reclamation is immediate or deferred. Even the comments are
near-copies. A fix to the gate logic in one will silently miss the other.

**`policy.rs`'s `Clock`/`MonotonicClock` (L125–165)** is a determinism seam, not a
memory-reclamation concept, used by only one of the two policies, and exported at
the crate root under the very generic bare name `Clock`.

### 2.6 Undocumented invariants

Eleven places where correctness depends on a fact established elsewhere and not
restated. The most consequential:

- **`framing.rs` L202–207.** The literal `12` in `let mut header = [0u8; 12]` is
  `4 + 8`; the `try_into().unwrap()` is infallible only because `12 - 4 == 8`; and
  `header[..4 + C::SIZE]` is in bounds only because of the const assert in
  `ChecksumFramer::new` **40 lines away**. No site references another.
- **`reader.rs` L152–155.** The pending shrink must run *before*
  `read_and_deframe`, never after, because it drops the buffer the previously
  returned payload borrows from. The scheduling side explains this well at
  L195–197; the enforcement site — the code a refactorer would actually move —
  only narrates the condition.
- **`framing.rs` L263.** `read_payload` gates on `buffer.len()`, not `capacity()`,
  so the high-water mark is the vector's *length*. Two consequences nobody wrote
  down: `with_capacity` yields len 0, so the first read still resizes and zeroes;
  and `apply_pending_shrink` installs a fresh len-0 `Vec`, resetting the mark so
  the next read re-resizes and re-zeroes. This sits in tension with the `Deframer`
  trait doc's "never shrink it, so steady-state reads touch memory exactly once."
- **`reader.rs` L164.** `&self.buffer[..n]` is in bounds only by the `Deframer`
  contract. Since `Deframer` is a **public** trait users implement, a third-party
  deframer returning `Some(n)` without growing the buffer panics here.
- **`writer.rs` L366–368.** `wire_len` cannot underflow only because
  `CountingWriter::count` is monotonically non-decreasing — never stated. (Note
  `start_offset` cancels algebraically; `self.writer.count - count_before` is
  equivalent and obviously non-negative.)
- **`policy.rs` L296–300.** `time_ok` does not guard on `overprovisioned` the way
  `count_ok` does. It is correct only because `last_over_seen_at` is cleared on
  every non-over-provisioned observation at L293, so `Some(t0)` implies currently
  over-provisioned. That two-step inference spans branches and is what stops a
  stale timer firing on a healthy buffer.

### 2.7 Public API doc coverage

Neither `lib.rs` nor `Cargo.toml` sets `missing_docs`. The crate's only
crate-level lint is `#![cfg_attr(not(feature = "unsafe_typed"), forbid(unsafe_code))]`.

Consequently **18 public functions have no rustdoc**, clustered almost entirely in
`new()` constructors — exactly what an unenforced lint lets slip. Nine are in
`framing.rs`, including **`Framer::frame_and_write` (L121), the sole method of a
public trait** that third parties implement. Four are the checksum constructors,
two in `validation.rs` (including `SizeValidator::new`, the one with the unstated
`min <= max` invariant), two are `Messages::next` / `TypedMessages::next`, and one
is `SizeThresholdPolicy::new` — three unlabeled numeric parameters directly below
three thoroughly documented `DEFAULT_*` constants that are its intended arguments.

The internal inconsistency is stark: `DefaultDeframer::with_max_frame_len` has a
full explanation *and* a `compile_fail` doctest; `DefaultDeframer::new` six lines
above has nothing.

Public fields are also bare: all four `ReclamationInfo` fields, both
`ChecksumMismatch` fields, and `ValidationFailed::validator` (whose sibling
`reason` *is* documented).

`writer.rs`, `error.rs`, `recover.rs`, and `traits.rs` are already at 100%.

### 2.8 Export-surface inconsistency

`ObserverFramer` and `ObserverDeframer` are `pub` but **not** re-exported at the
crate root, while their siblings `BoundedFramer`, `ValidatingFramer`, and
`ValidatingDeframer` are. Yet `FramerExt::observed` and `DeframerExt::observed`
*return* them — so a user calling a re-exported method receives a type they cannot
name without reaching into `flatstream::framing`. The `Checksum` trait has the
same shape: `NoChecksum` is re-exported, but the trait you must implement to write
a custom checksum is not.

For a library whose goal is elegant composability, being unable to name the type a
combinator hands you is a real wart.

---

## 3. Proposed tasks

Self-contained, orderable, each with a definition of done. None changes behavior;
S1 and S6 are the only ones that touch the public surface, both additively.

**S1 — Turn on `missing_docs` and clear it.** Add `#![warn(missing_docs)]` to
`lib.rs`, document the 18 functions and the public fields in §2.7. Consider
`-D warnings` in the gate once clean so it cannot regress.
*Done when:* the gate is green with the lint on, and no item is documented with
filler ("Creates a new X") where the constructor has a constraint worth stating —
`ChecksumDeframer::new`'s `SIZE <= 8` const assert and `SizeValidator::new`'s
ordering requirement in particular.

**S2 — Module boundary statements.** Give `error.rs` a module doc. Correct
`traits.rs`'s false claim. Extend the one-liners on `framing.rs`, `writer.rs`,
`reader.rs`, and `checksum.rs` to state a boundary, using `recover.rs`'s `Scope:`
paragraph as the template.
*Done when:* each module doc answers "what belongs here, and what belongs
elsewhere," and no module doc omits a type the module contains.

**S3 — Fix the two wrong comments (§2.1).** Small, and first among the comment
work because these are the only two that can actively cause a defect.
*Done when:* `validation.rs` L87 states the bounds precondition and names the
indexes it protects; `policy.rs` L273 states the always-over-provisioned
consequence rather than a nonexistent division.

**S4 — Bring `checksum.rs` and `validation.rs` to the house style.** Delete the
narration in §2.2, add the substantive comments it displaced.
*Done when:* every item in §2.2 is addressed. **Explicitly not measured by comment
count** — see §4.

**S5 — Write down the invariants (§2.6).** Where correctness depends on a fact
established elsewhere, name the other site in the comment.
*Done when:* each of the eleven is stated at the site that depends on it, not only
at the site that establishes it. `framing.rs`'s 12-byte header and the reader's
shrink ordering are the two that most need it.

**S6 — Export-surface consistency (§2.8).** Re-export `ObserverFramer`,
`ObserverDeframer`, and the `Checksum` trait at the crate root, or document why
they are deliberately module-scoped.
*Done when:* every type returned by a root-level API is nameable from the root.

**S7 — Structural seams (§2.5).** The largest change and the one most worth
confirming in review before starting, since it moves code without changing it.
Suggested order, most to least clear-cut:

1. Extract `write_all_vectored` / `write_remainder` to `src/vectored.rs`. Purely
   mechanical, and it gives the excellent 30-line explanation above them a home
   instead of burying it above `framing.rs`'s first real type.
2. Unify or cross-reference `CountingWriter` / `CountingReader`.
3. De-duplicate `evaluate_memory_policy`, or at minimum have each cite the other.
4. Move `Clock` / `MonotonicClock` out of `policy.rs`.
5. Extract the five adapters to `framing/adapters.rs`.

*Done when:* every `pub` path is unchanged (re-export from the old location if
needed) and the gate is green. **A move that changes a public path is a breaking
change and needs sign-off first (§5 of `CONTRIBUTING.md`).**

---

## 4. What this proposal explicitly does not want

**Do not treat comment count as the metric.** The failure mode is obvious and
worse than the disease: `// increment the counter` above `count += 1` raises the
number and lowers the value. The table in §0 exists to *locate* the problem, not
to define success. `recover.rs` would score badly on inline comments (3) and is
the best-documented file in the crate, because its explanation lives in rustdoc
where it belongs.

**Do not refactor beyond §S7's list.** The modules that are cohesive —
`recover.rs`, `error.rs`, `traits.rs`, `validation.rs` — should be left alone
structurally. The seams named in §2.5 were identified by a specific test: code
that shares no concepts with its neighbors and would work unchanged elsewhere.
Nothing else in `src/` meets it.

**Do not add doc comments that restate the signature.** `missing_docs` is
satisfied by `/// Creates a new X.`, and satisfying it that way converts a
detectable gap into an undetectable one. Where a constructor genuinely has nothing
to say, that is a signal the *type* doc should carry the weight.

---

## 5. Sequencing

S3 first — two lines, removes the only actively misleading comments. Then S1, the
mechanical floor, which is also the one that prevents regression. Then S2 and S5,
which together are what makes the codebase navigable by someone who did not write
it. S4 is the bulk of the writing. S6 is small and independent. S7 last, and only
with sign-off.

S1 through S6 could reasonably be one focused branch each; S7 wants one branch per
extraction so each move is reviewable in isolation.

---

## 6. Why this is worth doing before 3.0

The owner has declined a core stream preamble; persistent consumers retain an
application-owned manifest. Future schema/container work may still touch
`framing.rs`, but this document's preamble assumption and exact source-line
inventory are no longer planning inputs until the audit is rerun.

Doing this after 3.0 means doing it to a larger, less familiar `framing.rs`.
