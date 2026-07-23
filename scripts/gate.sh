#!/usr/bin/env bash
# The verification gate. Run before every review, merge, or tag.
#
# Each step exists for a reason:
#   fmt         style drift makes diffs unreviewable
#   clippy      lints-as-errors across every target (tests/benches/examples rot silently)
#   test matrix all_checksums (full suite incl. doctests), no-features, and a
#               single-feature build (crc16) that catches #[cfg] gaps; plus
#               the opt-in unsafe_typed integration test so that public feature
#               cannot bit-rot outside the default unsafe-free build
#   rustdoc     broken intra-doc links and doc warnings, as errors
#   bench check benches are compile-checked so they can't bit-rot between runs
#               (actually *running* benches is a separate, deliberate act — see
#               the README "Benchmarking" section for the Criterion baseline flow)
#   fuzz check  fuzz targets compile against every built-in checksum; executing
#               them remains a separate nightly/cargo-fuzz step
#   MSRV        confirms the active toolchain satisfies the rust-version floor
#               declared in Cargo.toml. This environment uses Homebrew rust (no
#               rustup), so there is no separate MSRV toolchain to install: when
#               the active rustc equals the floor, every step above already ran
#               on exactly the MSRV.
set -euo pipefail
cd "$(dirname "$0")/.."

# Single source of truth for the MSRV: the rust-version field in Cargo.toml.
MSRV=$(grep -m1 '^rust-version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')

echo "== fmt"
cargo fmt --check

echo "== clippy (all targets, all_checksums, -D warnings)"
cargo clippy --locked --all-targets --features all_checksums -- -D warnings

echo "== clippy: individual checksum feature configurations"
for feature in xxhash crc32 crc16; do
    cargo clippy --locked --all-targets --no-default-features --features "$feature" -- -D warnings
done

echo "== clippy: unsafe_typed opt-in (all targets)"
cargo clippy --locked --all-targets --features all_checksums,unsafe_typed -- -D warnings

echo "== test: all_checksums"
cargo test --locked --features all_checksums

echo "== test: no default features"
cargo test --locked

echo "== test: crc16 only"
cargo test --locked --no-default-features --features crc16

echo "== test: unsafe_typed opt-in"
cargo test --locked --features all_checksums,unsafe_typed --test stream_deserialize_integration_tests

echo "== rustdoc (-D warnings)"
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --features all_checksums

echo "== bench compile check"
cargo check --locked --benches --features all_checksums
cargo check --locked --benches --features comparative_bench
cargo check --locked --benches --features instruction_bench,all_checksums

echo "== fuzz target compile check"
cargo check --locked --manifest-path fuzz/Cargo.toml --bins

RUSTC_VER=$(rustc --version | awk '{print $2}')
echo "== MSRV: declared $MSRV, active rustc $RUSTC_VER"
if [ "$RUSTC_VER" = "$MSRV" ]; then
    echo "   verified — every step above ran on exactly the declared MSRV"
elif [ "$(printf '%s\n%s\n' "$MSRV" "$RUSTC_VER" | sort -V | head -n1)" = "$MSRV" ]; then
    echo "   note: active rustc is newer than the MSRV; the crate builds here, but"
    echo "         the floor itself is not proven (no older toolchain to test against)"
else
    echo "!! active rustc ($RUSTC_VER) is OLDER than the declared MSRV ($MSRV)"
    exit 1
fi

echo "gate green"
