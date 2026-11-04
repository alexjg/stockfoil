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
# Match the Verus proof escape hatches even if someone qualifies the name.
ESCAPE = re.compile(
    r"\b(?:assume|admit|assume_specification)\s*(?:[!(]|<|\[)"
    r"|\b(?:external_body|external_fn_specification|external_type_specification|"
    r"external_trait_specification|axiom)\b"
    r"|verifier\s*::\s*(?:external|trusted)\b"
    r"|verifier\s*::\s*verify\s*\(\s*false\s*\)"
)


def audit(root):
    manifest = json.loads((root / 'verification/trusted_stdlib.json').read_text())
    expected = {'stockfoil-core/src/verified/stdlib.rs'}
    if set(manifest) != expected:
        raise ValueError('The trusted file allowlist changed; review the audit policy too')
    for relative, entry in manifest.items():
        path = root / relative
        if path.is_symlink() or hashlib.sha256(path.read_bytes()).hexdigest() != entry['sha256']:
            raise ValueError(f'Trusted source changed without an updated manifest: {relative}')
    roots = [root / 'stockfoil-core/src/verified', root / 'verification']
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
    print(f'Trust audit passed: {count} explicit standard-library contracts (not proofs).', flush=True)
