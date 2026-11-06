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
        'author identity ignores the last byte',
        'stockfoil/src/verified/author.rs',
        'while i < 32',
        'while i < 31',
        'last',
    ),
    (
        'direct selection ignores author identity',
        'stockfoil/src/verified/chain_selection.rs',
        'owners[chain as usize].same_as(author)',
        'true',
        'last',
    ),
    (
        'direct selection reuses a stale chain member',
        'stockfoil/src/verified/chain_selection.rs',
        'tip.index() == parent.index()',
        'true',
        'last',
    ),
    (
        'direct selection prefers the largest chain',
        'stockfoil/src/verified/chain_selection.rs',
        'if chain < c { chain } else { c }',
        'if chain > c { chain } else { c }',
        'last',
    ),
    (
        'ancestral selection ignores tip coverage',
        'stockfoil/src/verified/chain_selection.rs',
        'if reached.contains(chain, tips[chain as usize])',
        'if true',
        'last',
    ),
    (
        'ancestral selection skips later candidates',
        'stockfoil/src/verified/chain_selection.rs',
        '        i += 1;',
        '        i = chains.len();',
        'last',
    ),
    (
        'chain registration omits the new author',
        'stockfoil/src/verified/chain_selection.rs',
        'owners.push(author);',
        'let _ = author;',
        'last',
    ),
    (
        'chain registration stores the wrong author',
        'stockfoil/src/verified/chain_selection.rs',
        'owners.push(author);',
        'owners.push(Author([0u8; 32]));',
        'last',
    ),
    (
        'storage overwrites the parent span start',
        'stockfoil/src/verified/storage.rs',
        'node.parents_start = ids.len();',
        'node.parents_start = 0;',
        'last',
    ),
    (
        'storage drops the parent span length',
        'stockfoil/src/verified/storage.rs',
        'node.parents_len = incoming.len() as u32;',
        'node.parents_len = 0;',
        'last',
    ),
    (
        'storage inserts a self-parent',
        'stockfoil/src/verified/storage.rs',
        'ids.push(incoming[i]);',
        'ids.push(position);',
        'last',
    ),
    (
        'storage overwrites the selected chain',
        'stockfoil/src/verified/storage.rs',
        '    nodes.push(node);',
        '    node.chain = 0; nodes.push(node);',
        'last',
    ),
    (
        'storage clears previously delivered nodes',
        'stockfoil/src/verified/storage.rs',
        '    nodes.push(node);',
        '    nodes.clear(); nodes.push(node);',
        'last',
    ),
    (
        'parent slice reads a different node span',
        'stockfoil/src/verified/storage.rs',
        '&ids[node.parents_start..node.parents_start + node.parents_len as usize]',
        '&ids[0..node.parents_len as usize]',
        'last',
    ),
    (
        'chain append records the wrong current tip',
        'stockfoil/src/verified/chain_tables.rs',
        'tips[chain as usize] = position;',
        'tips[chain as usize] = NodeIdx::new(0);',
        'last',
    ),
    (
        'chain append clears existing members',
        'stockfoil/src/verified/chain_tables.rs',
        'tables[chain as usize].push(position);',
        'tables[chain as usize].clear();',
        'last',
    ),
    (
        'new chain omits its initial node',
        'stockfoil/src/verified/chain_tables.rs',
        'let row = vec![position];',
        'let row: Vec<NodeIdx> = Vec::new();',
        'last',
    ),

    (
        'candidate assembly drops new invisible deliveries',
        'stockfoil/src/verified/candidates.rs',
        'difference::difference(tables, delivered, processed, &mut pending);',
        'difference::difference(tables, processed, processed, &mut pending);',
        'last',
    ),
    (
        'candidate assembly drops visibility changes',
        'stockfoil/src/verified/candidates.rs',
        'difference::symmetric_difference(tables, before, after, &mut pending);',
        'difference::symmetric_difference(tables, before, before, &mut pending);',
        'last',
    ),
    (
        'candidate projection shifts node indices',
        'verification/candidate_refinement.rs',
        'node@ as usize)',
        '(node@ as usize + 1) as usize)',
        'first',
    ),
    (
        'nonempty stdlib consumer cannot prove false',
        'verification/candidate_examples.rs',
        '    let unchanged =',
        '    assert(false); let unchanged =',
        'last',
    ),

    (
        'normalization clears the candidate set',
        'stockfoil/src/verified/candidates.rs',
        '    super::stdlib::dedup_nodes(nodes);',
        '    super::stdlib::dedup_nodes(nodes); nodes.clear();',
        'last',
    ),

    (
        'builder ignores a coordinate',
        'stockfoil/src/verified/tree.rs',
        'self.clock.include(chain, position);',
        'let _ = (chain, position);',
        'last',
    ),
    (
        'builder discards accumulated inputs on merge',
        'stockfoil/src/verified/tree.rs',
        'self.clock = self.clock.union(clock);',
        'self.clock = clock.clone();',
        'last',
    ),
    (
        'builder finish drops its result',
        'stockfoil/src/verified/tree.rs',
        '    {\n        self.clock\n    }\n}',
        '    {\n        Clock::default()\n    }\n}',
        'last',
    ),
    (
        'difference drops the first selected node',
        'stockfoil/src/verified/difference.rs',
        'let mut i = start;',
        'let mut i = end;',
        'last',
    ),
    (
        'difference emits the wrong node',
        'stockfoil/src/verified/difference.rs',
        'out.push(nodes[i]);',
        'out.push(nodes[start]);',
        'last',
    ),
    (
        'symmetric difference omits the reverse direction',
        'stockfoil/src/verified/difference.rs',
        'difference(chain_nodes, b, a, out);',
        'difference(chain_nodes, a, b, out);',
        'last',
    ),
    (
        'coordinate refinement shifts chain ids',
        'verification/tree_refinement.rs',
        '(e.0 as usize, e.1)',
        '((e.0 as usize + 1) as usize, e.1)',
        'last',
    ),
    (
        'coordinate bounds drop every permission',
        'verification/tree_refinement.rs',
        'policy(e.0, e.1))',
        'None)',
        'last',
    ),
    (
        'mask fails to cap expanding bounds',
        'stockfoil/src/verified/tree.rs',
        'if let Some(value) = super::clock::cap(tip, limit) {',
        'if let Some(value) = limit {',
        'last',
    ),
    (
        'mask drops position zero',
        'stockfoil/src/verified/tree.rs',
        'entries.push((chain, value));',
        'if value.index() > 0 { entries.push((chain, value)); }',
        'last',
    ),
    (
        'mask ignores denied coordinates',
        'stockfoil/src/verified/tree.rs',
        'let limit = bound(chain, tip);',
        'let _ = bound(chain, tip); let limit = Some(tip);',
        'last',
    ),
    (
        'sorted reconstruction replaces a tip with zero',
        'stockfoil/src/verified/tree.rs',
        'let (chain, tip) = entries[mid];',
        'let (chain, _) = entries[mid]; let tip = NodeIdx::new(0);',
        'last',
    ),
    (
        'sorted reconstruction swaps children',
        'stockfoil/src/verified/tree.rs',
        'let out = Self::node(chain, tip, left, right);',
        'let out = Self::node(chain, tip, right, left);',
        'last',
    ),
    (
        'tip table shifts chain ids',
        'stockfoil/src/verified/tree.rs',
        'entries.push((i as ChainId, tips[i]));',
        'entries.push(((i + 1) as ChainId, tips[i]));',
        'last',
    ),
    (
        'union ignores the smaller tree',
        'stockfoil/src/verified/tree.rs',
        'let next = iter.next();',
        'let next = None;',
        'last',
    ),
    (
        'union replaces tips with zero',
        'stockfoil/src/verified/tree.rs',
        'big.include(chain, tip);',
        'big.include(chain, NodeIdx::new(0));',
        'last',
    ),
    (
        'union discards the larger initial tree',
        'stockfoil/src/verified/tree.rs',
        '(self.clone(), other)',
        '(Self::default(), other)',
        'last',
    ),
    (
        'iterator descends right before visiting the root',
        'stockfoil/src/verified/tree.rs',
        'self.node = &e.left;',
        'self.node = &e.right;',
        'last',
    ),
    (
        'iterator revisits the left subtree after popping',
        'stockfoil/src/verified/tree.rs',
        'self.node = &e.right;',
        'self.node = &e.left;',
        'last',
    ),
    (
        'iterator drops the yielded entry',
        'stockfoil/src/verified/tree.rs',
        'Some((e.chain, e.tip))',
        'None',
        'last',
    ),
    (
        'set ignores replacement tips',
        'stockfoil/src/verified/tree.rs',
        'Self::node(chain, tip, e.left.clone(), e.right.clone())',
        'Self::node(chain, e.tip, e.left.clone(), e.right.clone())',
        'last',
    ),
    (
        'set drops a new coordinate',
        'stockfoil/src/verified/tree.rs',
        'return Self::node(chain, tip, Self::default(), Self::default());',
        'return Self::default();',
        'last',
    ),
    (
        'include skips every update',
        'stockfoil/src/verified/tree.rs',
        'if super::clock::contains(self.position(chain), position) {',
        'if true {',
        'last',
    ),
    (
        'left rotation associates the wrong tip',
        'stockfoil/src/verified/tree.rs',
        'l.tip,\n            l.left.clone(),',
        'tip,\n            l.left.clone(),',
        'last',
    ),
    (
        'right rotation associates the wrong tip',
        'stockfoil/src/verified/tree.rs',
        'r.tip,\n            Self::node(chain, tip, left, r.left.clone()),',
        'tip,\n            Self::node(chain, tip, left, r.left.clone()),',
        'last',
    ),
    (
        'left-right rotation skips the inner rotation',
        'stockfoil/src/verified/tree.rs',
        'if l.right.height() > l.left.height() {',
        'if false {',
        'last',
    ),
    (
        'right-left rotation skips the inner rotation',
        'stockfoil/src/verified/tree.rs',
        'if r.left.height() > r.right.height() {',
        'if false {',
        'last',
    ),
    (
        'balancing skips the left-heavy case',
        'stockfoil/src/verified/tree.rs',
        'if left.height() > right.height() + 1 {',
        'if false {',
        'last',
    ),
    (
        'node height uses the shorter child',
        'stockfoil/src/verified/tree.rs',
        'let height = 1 + if lh >= rh { lh } else { rh };',
        'let height = 1 + if lh <= rh { lh } else { rh };',
        'last',
    ),
    (
        'node size omits the root',
        'stockfoil/src/verified/tree.rs',
        'let len = 1 + left.len() as u32 + right.len() as u32;',
        'let len = left.len() as u32 + right.len() as u32;',
        'last',
    ),
    (
        'node construction swaps the children',
        'stockfoil/src/verified/tree.rs',
        'Entry { chain, tip, height, len, left, right }',
        'Entry { chain, tip, height, len, left: right, right: left }',
        'last',
    ),
    (
        'AVL lookup follows the wrong left subtree',
        'stockfoil/src/verified/tree.rs',
        'node = &e.left;',
        'node = &e.right;',
        'last',
    ),
    (
        'AVL lookup follows the wrong right subtree',
        'stockfoil/src/verified/tree.rs',
        'node = &e.right;',
        'node = &e.left;',
        'last',
    ),
    (
        'AVL lookup omits a matching root',
        'stockfoil/src/verified/tree.rs',
        'return Some(e.tip);',
        'return None;',
        'last',
    ),
    (
        'clock clone drops the shared tree',
        'stockfoil/src/verified/tree.rs',
        'Some(e) => Self(Some(Arc::clone(e))),',
        'Some(e) => Self(None),',
        'last',
    ),
    (
        'cap expands permission',
        'stockfoil/src/verified/clock.rs',
        'Some(limit) => Some(if tip.index() <= limit.index() { tip } else { limit }),',
        'Some(limit) => Some(if tip.index() >= limit.index() { tip } else { limit }),',
        'last',
    ),
    (
        'membership excludes the tip',
        'stockfoil/src/verified/clock.rs',
        'Some(tip) => p.index() <= tip.index()',
        'Some(tip) => p.index() < tip.index()',
        'last',
    ),
    (
        'interval search excludes the tip',
        'stockfoil/src/verified/clock.rs',
        'if nodes[mid].index() <= tip.index()',
        'if nodes[mid].index() < tip.index()',
        'last',
    ),
    (
        'absent clock drops the first node',
        'stockfoil/src/verified/clock.rs',
        'let start = match before { None => 0,',
        'let start = match before { None => 1,',
        'last',
    ),
    (
        'positive batch makes no progress',
        'stockfoil/src/verified/events.rs',
        'next + if max <= remaining { max } else { remaining }',
        'next + if max <= remaining { 0 } else { remaining }',
        'last',
    ),
    (
        'visibility swaps before and after',
        'stockfoil/src/verified/events.rs',
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


def verify(root, mutation=False):
    return subprocess.run(
        ["bash", str(root / "verification/verify.sh")] + (["--mutation-only"] if mutation else []),
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
            shutil.copytree(ROOT / "stockfoil/src/verified", root / "stockfoil/src/verified")
            shutil.copytree(ROOT / "verification", root / "verification")
            path = root / relative
            source = path.read_text()
            split = source.rpartition if occurrence == "last" else source.partition
            prefix, matched, suffix = split(before)
            if not matched:
                raise SystemExit(f"Mutation no longer matches source: {name}")
            path.write_text(prefix + after + suffix)
            result = verify(root, mutation=True)
            output = result.stdout + result.stderr
            if result.returncode == 0 or not any(
                message in output
                for message in ("assertion failed", "postcondition not satisfied", "invariant not satisfied")
            ):
                raise SystemExit(f"Expected a proof failure for {name}:\n{output}")
            print(f"Rejected: {name}", flush=True)


if __name__ == "__main__":
    main()
