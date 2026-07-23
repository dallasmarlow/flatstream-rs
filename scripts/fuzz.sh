#!/usr/bin/env bash
# Time-bounded fuzz of the deframers against arbitrary bytes.
#
# Why: the deframer's parsing of the length header and checksum field is the
# library's attack surface — a corrupt or hostile stream must never panic,
# hang, or size an allocation past the frame bound. The two targets assert
# exactly that (`fuzz/fuzz_targets/`); the checksummed target additionally
# roundtrips every input through ChecksumFramer/Deframer and asserts
# byte-identical recovery.
#
# Manually invoked locally; there is no CI schedule.
# Requires the Rust nightly toolchain + cargo-fuzz (one-time):
#   rustup toolchain install nightly && cargo install cargo-fuzz
#
# Usage: scripts/fuzz.sh [seconds-per-target]      (default 300)
# Corpus accumulates in fuzz/corpus/ across runs — keep it; coverage compounds.
# A crash drops a reproducer under fuzz/artifacts/<target>/.
set -euo pipefail
cd "$(dirname "$0")/.."

SECONDS_PER_TARGET="${1:-300}"

cargo +nightly fuzz run deframe_fuzzer -- -max_total_time="$SECONDS_PER_TARGET"
cargo +nightly fuzz run deframe_checksum_fuzzer -- -max_total_time="$SECONDS_PER_TARGET"
