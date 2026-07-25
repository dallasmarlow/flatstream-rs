#!/usr/bin/env bash
# Run every non-mutating example, fail on the first one that errors.
#
# Why this exists: the examples are executable claims about the library's real
# behavior and wire format — they assert their own expected values (exact alert
# counts, measured per-frame overhead, corruption detection, index contiguity),
# so running them all is a cheap end-to-end proof that the docs' mental model
# matches the bytes. Run after API changes and before tagging; it has caught
# wrong assumptions before. `scripts/gate.sh` calls this.
#
# The list is derived from `examples/` rather than hand-kept, because a
# hand-kept list silently stops covering new examples — `external_index` was
# missing from it for exactly that reason, despite being the example
# `docs/DESIGN_v2_8.md` §6 cites as an executable claim.
#
# `ingest_lobster` regenerates local corpus files and is therefore opt-in:
#   RUN_LOBSTER_INGEST=1 scripts/examples.sh
# It needs verified ZIPs under tests/corpus/lobster/zips (see the README's
# LOBSTER section); without them it prints a notice and exits cleanly.
set -euo pipefail
cd "$(dirname "$0")/.."

FEATURES=all_checksums

for example_path in examples/*.rs; do
    example=$(basename "$example_path" .rs)
    case "$example" in
        # Cargo gates these two behind required-features; they are run
        # separately below because they need different flags.
        typed_reading_flatc_example | ingest_lobster) continue ;;
    esac
    echo "== $example"
    cargo run -q --locked --example "$example" --features "$FEATURES"
    echo
done

echo "== typed_reading_flatc_example (generated schema)"
cargo run -q --locked --example typed_reading_flatc_example --features flatc_example
echo

if [[ "${RUN_LOBSTER_INGEST:-0}" == "1" ]]; then
    echo "== ingest_lobster (corpus regeneration explicitly enabled)"
    cargo run -q --locked --example ingest_lobster --features lobster
    echo
else
    echo "== ingest_lobster skipped (set RUN_LOBSTER_INGEST=1 to regenerate corpus)"
    echo
fi

echo "all examples ran clean"
