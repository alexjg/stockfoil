#!/usr/bin/env python3
"""Check that representative semantic bugs are rejected by the full verifier."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parent.parent
# Last occurrence changes executable code while leaving its contract intact.
# The policy mutation deliberately changes the spec; a concrete witness must
# detect its disagreement with the intended out-of-history-frontier behavior.
MUTATIONS = [
    (
        'AVL lookup follows the wrong left subtree',
        'stockfoil-core/src/verified/tree.rs',
        'node = &e.left;',
        'node = &e.right;',
        'last',
    ),
    (
        'AVL lookup follows the wrong right subtree',
        'stockfoil-core/src/verified/tree.rs',
        'node = &e.right;',
        'node = &e.left;',
        'last',
    ),
    (
        'AVL lookup omits a matching root',
        'stockfoil-core/src/verified/tree.rs',
        'return Some(e.tip);',
        'return None;',
        'last',
    ),
    (
        'clock clone drops the shared tree',
        'stockfoil-core/src/verified/tree.rs',
        'Some(e) => Self(Some(Arc::clone(e))),',
        'Some(e) => Self(None),',
        'last',
    ),
    (
        'cap expands permission',
        'stockfoil-core/src/verified/clock.rs',
        'Some(limit) => Some(if tip.index() <= limit.index() { tip } else { limit }),',
        'Some(limit) => Some(if tip.index() >= limit.index() { tip } else { limit }),',
        'last',
    ),
    (
        'membership excludes the tip',
        'stockfoil-core/src/verified/clock.rs',
        'Some(tip) => p.index() <= tip.index()',
        'Some(tip) => p.index() < tip.index()',
        'last',
    ),
    (
        'interval search excludes the tip',
        'stockfoil-core/src/verified/clock.rs',
        'if nodes[mid].index() <= tip.index()',
        'if nodes[mid].index() < tip.index()',
        'last',
    ),
    (
        'absent clock drops the first node',
        'stockfoil-core/src/verified/clock.rs',
        'let start = match before { None => 0,',
        'let start = match before { None => 1,',
        'last',
    ),
    (
        'positive batch makes no progress',
        'stockfoil-core/src/verified/events.rs',
        'next + if max <= remaining { max } else { remaining }',
        'next + if max <= remaining { 0 } else { remaining }',
        'last',
    ),
    (
        'visibility swaps before and after',
        'stockfoil-core/src/verified/events.rs',
        '(true, false) => Vis::Before,',
        '(true, false) => Vis::After,',
        'last',
    ),
    (
        'new invisible commits omitted from candidates',
        'verification/materialization.rs',
        'delivered.difference(processed)',
        'ISet::<int>::empty()',
        'first',
    ),
    (
        'out-of-history frontier grants permission',
        'verification/authorization.rs',
        'frontier.contains(f) && h.contains(f) && reaches(g, a, f)',
        'frontier.contains(f) && reaches(g, a, f)',
        'first',
    ),
    (
        'missing permission defaults to unbounded',
        'verification/authorization.rs',
        'else { Permission::Denied }',
        'else { Permission::Unbounded }',
        'first',
    ),
    (
        'unbounded permission escapes selected history',
        'verification/authorization.rs',
        '0 <= a < authors.len() && history(g, heads).contains(a)',
        '0 <= a < authors.len()',
        'first',
    ),
    (
        'frontier excludes itself',
        'verification/authorization.rs',
        'frontier.contains(f) && h.contains(f) && reaches(g, a, f)',
        'frontier.contains(f) && h.contains(f) && a != f && reaches(g, a, f)',
        'first',
    ),
    (
        'permission selected using the wrong author',
        'verification/authorization.rs',
        'permission(perms, authors[a]), a)',
        'permission(perms, 0usize), a)',
        'first',
    ),
]


def verify(root):
    return subprocess.run(
        ["bash", str(root / "verification/verify.sh")],
        cwd=root,
        env=os.environ.copy(),
        capture_output=True,
        text=True,
        timeout=120,
    )


def main():
    baseline = verify(ROOT)
    if baseline.returncode:
        raise SystemExit("Baseline failed:\n" + baseline.stdout + baseline.stderr)
    print(baseline.stdout.strip(), flush=True)
    for name, relative, before, after, occurrence in MUTATIONS:
        with tempfile.TemporaryDirectory(prefix="stockfoil-verus-") as directory:
            root = Path(directory)
            shutil.copytree(ROOT / "stockfoil-core/src/verified", root / "stockfoil-core/src/verified")
            shutil.copytree(ROOT / "verification", root / "verification")
            path = root / relative
            source = path.read_text()
            split = source.rpartition if occurrence == "last" else source.partition
            prefix, matched, suffix = split(before)
            if not matched:
                raise SystemExit(f"Mutation no longer matches source: {name}")
            path.write_text(prefix + after + suffix)
            result = verify(root)
            output = result.stdout + result.stderr
            if result.returncode == 0 or not any(
                message in output
                for message in ("assertion failed", "postcondition not satisfied", "invariant not satisfied")
            ):
                raise SystemExit(f"Expected a proof failure for {name}:\n{output}")
            print(f"Rejected: {name}", flush=True)


if __name__ == "__main__":
    main()
