#!/usr/bin/env python3
"""Fail closed on changes to the enumerated project library assumptions.

This source audit is a review guardrail, not a Rust parser or a soundness proof.
The original suite additionally runs under Verus's own --no-cheating check.
"""
import hashlib
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parent.parent
# These hashes guard production-to-proof wiring, not application assumptions.
PUBLIC_API_WIRING = {
    'stockfoil/src/lib.rs',
    'stockfoil/src/reader.rs',
    'stockfoil/src/history/mod.rs',
    'stockfoil/src/history/local.rs',
    'stockfoil/src/meta.rs',
    'stockfoil/src/history/ancestry.rs',
    'stockfoil/src/history/traversal.rs',
    'stockfoil/src/history/view.rs',
    'stockfoil/src/history/clock.rs',
    'stockfoil/src/ledger.rs',
    'stockfoil/src/document.rs',
    'stockfoil/src/drive_error.rs',
    'stockfoil/src/error.rs',
    'stockfoil/src/materialize.rs',
    'stockfoil/src/history/checkpoint.rs',
    'stockfoil/src/verified/mod.rs',
}
BLAKE3_DEPENDENCIES = {
    'Cargo.lock', 'stockfoil/Cargo.toml',
    'verification/deps/Cargo.toml', 'verification/deps/Cargo.lock',
    'verification/deps/src/lib.rs', 'verification/blake3_args.py',
}

# Match the Verus proof escape hatches even if someone qualifies the name.
ESCAPE = re.compile(
    r"\b(?:assume|admit|assume_specification)\s*(?:[!(]|<|\[)"
    r"|\b(?:external_body|external_fn_specification|external_type_specification|"
    r"external_trait_specification|axiom)\b"
    r"|verifier\s*::\s*(?:external|trusted)\b"
    r"|verifier\s*::\s*verify\s*\(\s*false\s*\)"
)


def audit(root):
    wiring = json.loads((root / 'verification/public_api_wiring.json').read_text())
    if set(wiring) != PUBLIC_API_WIRING:
        raise ValueError('The public API wiring allowlist changed; review the audit policy too')
    for relative, digest in wiring.items():
        path = root / relative
        if path.is_symlink() or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError(f'Public API wiring changed without review: {relative}')
    manifest = json.loads((root / 'verification/trusted_stdlib.json').read_text())
    expected = {'stockfoil/src/verified/stdlib.rs'}
    if set(manifest) != expected:
        raise ValueError('The trusted file allowlist changed; review the audit policy too')
    for relative, entry in manifest.items():
        path = root / relative
        if path.is_symlink() or hashlib.sha256(path.read_bytes()).hexdigest() != entry['sha256']:
            raise ValueError(f'Trusted source changed without an updated manifest: {relative}')
        dependencies = entry.get('dependencies', {})
        if set(dependencies) != {
            'stockfoil/src/verified/author_key.rs',
            'stockfoil/src/verified/commit_hash.rs',
            'stockfoil/src/verified/node_key.rs',
        }:
            raise ValueError('The key-model dependency allowlist changed; review the audit policy too')
        for dependency, digest in dependencies.items():
            path = root / dependency
            if path.is_symlink() or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
                raise ValueError(f'Trusted key dependency changed without an updated manifest: {dependency}')
    blake = json.loads((root / 'verification/trusted_blake3.json').read_text())
    if set(blake) != {'stockfoil/src/verified/blake3_boundary.rs'}:
        raise ValueError('The BLAKE3 boundary allowlist changed')
    for relative, entry in blake.items():
        path = root / relative
        if path.is_symlink() or hashlib.sha256(path.read_bytes()).hexdigest() != entry['sha256']:
            raise ValueError(f'Trusted BLAKE3 boundary changed: {relative}')
        if set(entry['dependencies']) != BLAKE3_DEPENDENCIES:
            raise ValueError('The BLAKE3 dependency allowlist changed')
        for dependency, digest in entry['dependencies'].items():
            path = root / dependency
            if path.is_symlink() or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
                raise ValueError(f'Pinned BLAKE3 dependency changed: {dependency}')
    manifest.update(blake)
    roots = [root / 'stockfoil/src/verified', root / 'verification']
    for directory in roots:
        for path in directory.rglob('*.rs'):
            relative = str(path.relative_to(root))
            if path.is_symlink():
                raise ValueError(f'Symlink in proof sources: {relative}')
            source = path.read_text()
            if relative not in manifest and ESCAPE.search(source):
                raise ValueError(f'Unlisted trusted declaration in {relative}')
            # The harness must not silently import proofs from unaudited files.
            if re.search(r'\binclude\s*!', source):
                raise ValueError(f'Unaudited include in {relative}')
            for target in re.findall(r'#\[path\s*=\s*"([^"]+)"\]', source):
                resolved = (path.parent / target).resolve()
                if not any(resolved.is_relative_to(d.resolve()) for d in roots):
                    raise ValueError(f'Proof import outside audited roots: {relative}: {target}')
    return sum(len(entry['contracts']) for entry in manifest.values())


if __name__ == '__main__':
    try:
        count = audit(ROOT)
    except (ValueError, KeyError, OSError) as error:
        raise SystemExit(str(error)) from error
    print(f'Trust audit passed: {count} explicit library contracts (38 Rust/stdlib + 5 BLAKE3; not proofs).', flush=True)
