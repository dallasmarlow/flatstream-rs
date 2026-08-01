#!/usr/bin/env bash
# Compile (and run) the README Rust snippets as doctests.
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

TARGET_DIR=$(cargo metadata --locked --format-version 1 --no-deps \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
README_TARGET_DIR="$TARGET_DIR/readme-doctests"

# Use an isolated clean target. Picking an arbitrary `libflatbuffers-*.rlib`
# from the shared target is unsound when another workspace has built the same
# version with a different crate identity: rustdoc then sees two incompatible
# FlatBufferBuilder types despite identical version strings.
cargo clean --quiet --target-dir "$README_TARGET_DIR"
CARGO_TARGET_DIR="$README_TARGET_DIR" \
    cargo build --locked --quiet --features all_checksums

FLATBUFFERS_RLIB=("$README_TARGET_DIR"/debug/deps/libflatbuffers-*.rlib)
if [[ ${#FLATBUFFERS_RLIB[@]} -ne 1 ]]; then
    echo "expected exactly one flatbuffers rlib, found ${#FLATBUFFERS_RLIB[@]}" >&2
    exit 1
fi

echo "== README.md snippets"
rustdoc --edition 2021 --test README.md \
    -L "dependency=$README_TARGET_DIR/debug/deps" \
    --extern flatstream="$README_TARGET_DIR/debug/libflatstream.rlib" \
    --extern flatbuffers="${FLATBUFFERS_RLIB[0]}"
