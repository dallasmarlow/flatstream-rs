#!/usr/bin/env bash
# Run every non-mutating example, fail on the first one that errors.
#
# Why this exists: the examples are executable claims about the library's real
# behavior and wire format — several assert their own expected values (exact
# alert counts, measured per-frame overhead, corruption detection), so running
# them all is a cheap end-to-end proof that the docs' mental model matches the
# bytes. Run after API changes and before tagging; it has caught wrong
# assumptions before.
#
# `ingest_lobster` regenerates local corpus files and is therefore opt-in:
#   RUN_LOBSTER_INGEST=1 scripts/examples.sh
# It needs verified ZIPs under tests/corpus/lobster/zips (see the README's
# LOBSTER section); without them it prints a notice and exits cleanly.
set -euo pipefail
cd "$(dirname "$0")/.."

FEATURES=all_checksums

EXAMPLES=(
    telemetry_agent
    sized_checksums_example
    typed_reading_example
    adaptive_policy
    multiple_builders_example
    bounded_adapters_example
    observer_adapters_example
    validation_example
    custom_framer_example
    custom_allocator_example
    ergonomics_example
)

for ex in "${EXAMPLES[@]}"; do
    echo "== $ex"
    cargo run -q --locked --example "$ex" --features "$FEATURES"
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
