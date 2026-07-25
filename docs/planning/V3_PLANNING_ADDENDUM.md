# Addendum: the two `V3_*` planning documents

**Status:** Commentary — proposes changes to existing documents, implements
nothing
**Date:** 2026-07-24
**Author:** contributor
**Applies to:** `docs/planning/V3_VECTOR_IO.md`, `docs/planning/V3_SCHEMA_AWARE.md`

> **Owner decision (2026-07-25):** the core stream-header/preamble direction is
> declined. Headerless v0.2 framing remains normative; durable applications own
> external format manifests. Header-dependent analysis below is retained only
> to explain which older proposals no longer apply.

The 3.0 line is maintainer-directed and out of scope for contributor work
(`CONTRIBUTING.md` §7), so this is deliberately short. It records two things a
contributor reading `docs/planning/` needs to know and cannot currently learn
there.

---

## 1. `V3_VECTOR_IO.md` is retired in place

Its useful core idea shipped in **0.2.8**.
`FINDINGS_VECTORED_FRAMING.md` records the corrected design — two `IoSlice`s
inside the existing `DefaultFramer` and `ChecksumFramer`, adopted as the default,
byte-identical on the wire — and the golden hex corpus passes unchanged. The
isolated raw benchmark results and required rechecks are now committed.

The old proposal was actively wrong against the shipped library: it claimed
`writev` provided all-or-nothing atomicity, priced syscalls with unsupported
figures, and staged deprecation of a `DefaultFramer` that now *is* the vectored
path. Its body has therefore been replaced with a short retired tombstone that
states the current behavior and preserves existing links.

The normal placement would be `docs/archive/`, but this correction deliberately
does not perform a git rename. The tombstone's first heading and status make its
retired state explicit despite the retained path.

One conclusion is preserved in the tombstone: the current `Framer` trait is
enough; no `FlatBufferFramer` extension or parallel vectored-framer types were
needed. That reasoning outlived the proposal, and §2 below is where it next
applies.

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

Apart from explicit tombstones such as `V3_VECTOR_IO.md`, `docs/planning/` should
contain only live work. A document that has shipped, been rejected, or been
superseded normally belongs in `docs/archive/`, and that move should happen in
the same change that decides its fate. When a rename is intentionally deferred,
the retained file must be reduced to an unmistakable retired marker rather than
leave obsolete design content in a planning location.
