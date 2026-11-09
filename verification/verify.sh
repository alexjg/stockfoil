#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
verus_bin=${VERUS_BIN:-verus}
expected=0.2026.11.02.8bf68ee

if ! version=$("$verus_bin" --version); then
    echo "Cannot run Verus. See verification/README.md for installation." >&2
    exit 1
fi
if ! grep -Eq "^[[:space:]]*Version: ${expected//./\\.}$" <<< "$version"; then
    echo "Expected Verus $expected; found:" >&2
    echo "$version" >&2
    exit 1
fi

if [[ $# -gt 1 || ( $# -eq 1 && $1 != --mutation-only ) ]]; then
    echo "Usage: $0 [--mutation-only]" >&2
    exit 2
fi
python3 "$root/verification/check_trusted.py"
if [[ $# -eq 0 ]]; then
    echo "Verifying original suite with --no-cheating"
    "$verus_bin" --crate-type lib --edition 2024 --no-cheating --triggers-mode silent \
        "$root/verification/main.rs"
fi

dependency_args=$(python3 "$root/verification/blake3_args.py")
mapfile -t dependency_args <<< "$dependency_args"
echo "Verifying full suite with the enumerated Rust/stdlib and BLAKE3 contracts"
exec "$verus_bin" --crate-type lib --edition 2024 --cfg stockfoil_stdlib --triggers-mode silent "${dependency_args[@]}" \
    "$root/verification/main.rs"
