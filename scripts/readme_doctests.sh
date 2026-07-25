#!/usr/bin/env bash
# Compile (and run) the README's Rust snippets as doctests.
#
# Why this exists: rustdoc only tests snippets inside `src/`. The README is the
# first thing a consumer copies from, and until this script it was the one body
# of example code in the repo that nothing verified — 17 of its 29 snippets did
# not compile.
#
# Conventions this enforces:
#   ```rust          must compile and run
#   ```rust,ignore   deliberately not compiled — reserved for snippets that need
#                    generated schema code (`my_schema::...`) that this repo
#                    cannot supply
#   ```text          diagrams and wire layouts. An *untagged* fence is treated
#                    as Rust by rustdoc, so diagrams must say `text` explicitly
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET_DIR=$(cargo metadata --format-version 1 --no-deps \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')

cargo build --locked --quiet --features all_checksums

FLATBUFFERS_RLIB=$(ls -t "$TARGET_DIR"/debug/deps/libflatbuffers-*.rlib | head -1)

exec rustdoc --edition 2021 --test README.md \
    -L "dependency=$TARGET_DIR/debug/deps" \
    --extern flatstream="$TARGET_DIR/debug/libflatstream.rlib" \
    --extern flatbuffers="$FLATBUFFERS_RLIB"
