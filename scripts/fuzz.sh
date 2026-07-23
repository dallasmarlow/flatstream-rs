#!/usr/bin/env bash
# Time-bounded fuzz of the deframers against arbitrary bytes.
#
# Why: the deframer's parsing of the length header and checksum field is the
# library's attack surface — a corrupt or hostile stream must never panic,
# hang, or size an allocation past the frame bound. The two targets assert
# exactly that (`fuzz/fuzz_targets/`); the checksummed target exercises every
# built-in checksum width and additionally roundtrips every input through the
# matching ChecksumFramer/Deframer with byte-identical recovery.
#
# Manually invoked locally; there is no CI schedule.
# cargo-fuzz requires a nightly toolchain. When rustup with a nightly is
# present, it is used directly (with cargo-fuzz: `cargo install cargo-fuzz`).
# Otherwise — e.g. this Homebrew-rust, no-rustup workstation — the run falls
# back to the official nightly Linux container (same pattern as
# instruction_counts.sh; nightly floats by design here, which is what fuzzing
# wants). Build artifacts go to a named volume; the corpus lives in the
# bind-mounted repo, so coverage still compounds across runs.
#
# Usage: scripts/fuzz.sh [seconds-per-target]      (default 300)
# Corpus accumulates in fuzz/corpus/ across runs — keep it; coverage compounds.
# A crash drops a reproducer under fuzz/artifacts/<target>/.
set -euo pipefail
cd "$(dirname "$0")/.."

SECONDS_PER_TARGET="${1:-300}"

if command -v rustup >/dev/null 2>&1 && rustup toolchain list 2>/dev/null | grep -q nightly; then
    cargo +nightly fuzz run deframe_fuzzer -- \
        -max_total_time="$SECONDS_PER_TARGET" -timeout=10
    cargo +nightly fuzz run deframe_checksum_fuzzer -- \
        -max_total_time="$SECONDS_PER_TARGET" -timeout=10
else
    echo "no rustup nightly on this machine — running inside the nightly Linux container"
    docker run --rm \
        -v "$PWD":/work -v flatstream-fuzz-target:/work/fuzz/target \
        -w /work rustlang/rust:nightly bash -c "
        set -euo pipefail
        cargo install -q --locked cargo-fuzz
        cargo fuzz run deframe_fuzzer -- \
            -max_total_time=$SECONDS_PER_TARGET -timeout=10
        cargo fuzz run deframe_checksum_fuzzer -- \
            -max_total_time=$SECONDS_PER_TARGET -timeout=10
    "
fi
