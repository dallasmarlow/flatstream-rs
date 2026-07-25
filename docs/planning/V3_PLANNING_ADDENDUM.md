# Addendum: the two `V3_*` planning documents

**Status:** Commentary — proposes changes to existing documents, implements
nothing
**Date:** 2026-07-24
**Author:** contributor
**Applies to:** `docs/planning/V3_VECTOR_IO.md`, `docs/planning/V3_SCHEMA_AWARE.md`

The 3.0 line is maintainer-directed and out of scope for contributor work
(`CONTRIBUTING.md` §7), so this is deliberately short. It records two things a
contributor reading `docs/planning/` needs to know and cannot currently learn
there.

---

## 1. `V3_VECTOR_IO.md` is finished work sitting in the planning directory

Its content shipped in **0.2.8**. `FINDINGS_VECTORED_FRAMING.md` records the
corrected design — two `IoSlice`s inside the existing `DefaultFramer` and
`ChecksumFramer`, adopted as the default, byte-identical on the wire — and the
golden hex corpus passes unchanged. The document already carries an emphatic
SUPERSEDED banner (2026-07-10) warning that its body "MUST NOT be used as a design
reference" and that its `VectoredFramer` / `VectoredChecksumFramer` types "will
not be built."

Its remaining content is actively wrong against the shipped library: it claims
`writev` provides "**Atomic operations**: All-or-nothing semantics" (it does not,
and the shipped partial-write loop exists because it does not), it prices
syscalls with figures the banner itself calls fabricated, and its v3.0 → v3.1 →
v4.0 migration staging describes deprecating a `DefaultFramer` that now simply
*is* the vectored path.

**The problem is placement, not content.** `docs/planning/` reads as "work that
might happen"; `docs/archive/` reads as "decided, preserved for the reasoning."
This document is unambiguously the second, and its own banner says so. Leaving it
in `planning/` undoes the work the banner is doing — the directory says "consider
this" while the first paragraph says "do not."

**Proposed:** move it to `docs/archive/V3_VECTOR_IO.md`, keeping the banner and
adding one line recording that the corrected design shipped in 0.2.8 with a link
to the findings doc. This matches how `V2_X_FLUENT_BUILDER.md` and
`V2_X_BOXED_TRAITS.md` were handled on 2026-07-24.

One thing in it is worth carrying forward rather than archiving quietly: its
review notes established the principle "Don't add a new `FlatBufferFramer` trait
yet; the current `Framer` is enough. Extra traits add surface area without
immediate gain." That reasoning outlived the document, and §2 below is where it
next applies.

## 2. `V3_SCHEMA_AWARE.md` needs a refresh before it is built against

Its baseline is **v2.5**. It therefore predates the 2.6 hybrid API, 2.7 validation
and adaptive memory policy, and 2.8 receipts and vectored framing — three releases
of decisions it cannot account for. Four consequences:

**Its validation slice has largely shipped already.** §3.2 proposes a
`SchemaRegistry` holding `HashMap<SchemaId, Box<dyn SchemaValidator>>` with
per-message `validate_message`. The crate already has `TypedValidator`
(schema-aware, via a function pointer to a generated `root_as_*_with_opts`
verifier) and `TableRootValidator` (type-agnostic structural verification), both
composable through `FramerExt::with_validator` / `DeframerExt::with_validator`.
The registry would need to argue for a per-message `Box<dyn ..>` dispatch against
a shipped mechanism that is already function-pointer-based — and that argument
runs directly into the `V2_X_BOXED_TRAITS` rule, "static by default, measured
boxed opt-ins."

**Its write path would regress builder reuse.** §3.3's `write_typed` contains
`let mut builder = FlatBufferBuilder::new();` inside the per-message path. A fresh
builder per message discards exactly the optimization
`ARENA_ALLOCATION_RESEARCH.md` identifies as the current best available lever —
"simple builder reuse," worth 4.6% — and which today's `StreamWriter::write`
obtains via `self.builder.reset()`. `SchemaAwareWriter` should own and reuse its
builder the way `StreamWriter` does.

**`filter_by_field` breaks the crate's central property.** §3.4 returns
`Result<Vec<T>>` where `T: flatbuffers::Follow<'static>`, calling
`message.clone()`. Collecting borrowed FlatBuffers accessors into an owned `Vec`
is precisely the zero-copy violation the library exists to prevent, and the
document does not address it. The shape that fits the crate is the one
`process_all` already uses: a callback invoked per message with a borrowed view.

**Two documents propose two unreconciled extensions to the `Framer` trait.**
Schema-aware threads a `MessageFrame` through `frame_and_write_typed`; vectored
I/O proposed a `FlatBufferFramer: Framer` supertrait. Neither references the
other, and there is no picture anywhere of what `Framer` looks like in 3.0. The
vectored review already rejected *its own* extension on general principle (quoted
in §1); that reasoning has never been applied to `frame_and_write_typed`.

**It has no non-goals section.** Its four phases claim every feature in the
document — schema registry, field filtering, streaming analytics, schema
migration, multi-schema streams, a `flatstream-migrate` CLI, `flatc` integration —
in six weeks. Every other design document in this repository scopes what it will
not do; `V3_VECTOR_IO.md`'s non-goals list is the reason its review was able to
trim it down to the version that shipped.

### What is actually wire-dependent

Worth separating, because it determines what could move before 3.0. The stream
header block (magic `"FBST"`, format version, schema id, schema version) and the
per-frame Message Type and Flags bytes are the only parts requiring new normative
bytes. Everything reached through those — `detect_v3_header`, `peek_message_type`,
`process_multi` — inherits the dependency.

The validation slice and the field-access slice do **not** require a header. That
matters for sequencing: the parts of this document with the clearest value are the
parts that need 3.0 least, and one of them has already been delivered in a 2.x
release without a wire change.

**Proposed:** add a banner noting the v2.5 baseline and the four points above;
split the document into the wire-dependent core and the separable slices; and
resolve the `Framer`-trait question in one place before either 3.0 document is
built against.

---

## 3. A note on the planning directory itself

After the moves proposed here and in `ARENA_ALLOCATION_RESEARCH_ADDENDUM.md`,
`docs/planning/` would contain only live work. That is worth maintaining as a
rule: **a document that has shipped, been rejected, or been superseded belongs in
`docs/archive/`**, and the move should happen in the same change that decides its
fate. Three of the directory's current contents are stale in different ways and
none announced it in its filename or location — which is how a contributor ends up
reading a design reference that the maintainer already knows is dead.
