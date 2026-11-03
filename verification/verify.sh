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

exec "$verus_bin" --crate-type lib --edition 2024 --no-cheating --triggers-mode silent \
    "$root/verification/main.rs"
