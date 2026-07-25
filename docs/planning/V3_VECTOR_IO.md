# Retired plan: vectored I/O

> **RETIRED (2026-07-24).** This path is retained as a tombstone so existing
> links do not break; it is not live planning. The corrected write-side design
> shipped in 0.2.8 and is documented in
> `docs/benchmark/FINDINGS_VECTORED_FRAMING.md` and `docs/DESIGN_v2_8.md` §5.
> The isolated benchmark findings are complete; see the cited findings document
> for the stable conclusions and explicitly inconclusive buffered arms.

The original proposal has been removed because it mixed the useful call-count
idea with invalid or obsolete claims:

- `Write::write_vectored` provides no portable all-or-nothing guarantee across
  slices. A call may accept only a prefix, and TCP exposes a byte stream rather
  than message or segment boundaries. The shipped helper loops over partial
  writes and relies on no stronger platform-specific guarantee.
- FlatBuffer payloads produced by `FlatBufferBuilder::finished_data()` are
  contiguous; vectored framing joins the complete stack header and borrowed
  payload. It does not scatter-gather internal FlatBuffer objects.
- No `VectoredFramer`, `VectoredChecksumFramer`, `FlatBufferFramer`, vectored
  reader, or batch-writer API was added. `DefaultFramer` and `ChecksumFramer`
  use the existing `Framer` trait.
- The original syscall prices and projected speedups were unsupported. Only the
  in-progress E1 findings may be used, and their exact figures are not final.

Keeping this retired marker in `docs/planning/` is an explicit no-rename
compromise. It prevents the old proposal from appearing to be future work while
preserving the path referenced by older notes.
