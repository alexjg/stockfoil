# Current consolidation (2026-11-09)

The verification work on `verus/authorized-api` is consolidated into one commit
on `main` at `f2d470eeae4d11fe7bd7d7f65d9d88bcd8f0a65d`, including the public
partial-resume/plan/acknowledgement replay work. See [HANDOFF.md](HANDOFF.md) for
current scope, validation and continuation instructions. Everything below records
historical rebases and milestones; it is not the current proof boundary.

# Rebase onto rewritten main (2026-11-06)

Base at that rebase: `9a34480be200e64382a5f5ebc1aa3ec8c4906647`.

The seven consolidated verification commits have been replayed onto the new main
root. Main's configurable unlisted-author permissions, checkpoint encoding,
optional sedimentree adapter and revised example integration are preserved.
The permission compiler now proves explicit-entry precedence and uses one shared
bound clock for unlisted authors. The actual public WritePermissions collection
and its lookup/update methods are included in the harness, with no new library
assumptions. See API_TARGET.md for the remaining public ancestry boundary.

Previous base: `2459b9ea80edb40910a33fb3299cf3c59d34c505`.
The notes below describe the earlier consolidation and subsequent layers.
The main history was replaced again. The 20 verification layers through PR #40
are consolidated onto this new root on `verus/authorized-api`; the old PR stack
is no longer maintained. The new main documentation, CI, wire format and examples
are preserved. See [API_TARGET.md](API_TARGET.md) for the next review boundary.

## Preserved production integration

- Shared coordinate/interval helpers, persistent AVL clock operations, clock
  accumulation, and table updates now live under `stockfoil/src/verified`.
- Lazy `difference_intervals` retains its range callback and uses the checked
  interval-bound helper. Full checked difference enumeration supplies fragment
  membership; it does not replace lazy traversal with a collected node list.
- Visibility classification and the per-batch count use the shared checked helpers.
- Head-index normalization uses the same two explicit sorting/dedup contracts.
- Checked history-ID allocation and its exhaustion regression remain in place.

## Subsequent storage layer

`verified/storage.rs` now owns the actual node representation, parameterized over
opaque hash/payload/cache fields. History uses its checked empty construction,
node append and parent-slice accessor. The append proof connects complete ordered
chain tables to the labels in the stored nodes, preserves earlier nodes and parent
slices, and establishes the new node's exact parent span with every parent earlier
than the child. Its graph projection is a DAG; appending preserves old reachability.
No duplicate topology representation or runtime graph scan was introduced.

This closes the stored-label/fresh-index portion of the table boundary. It does
not by itself prove hash resolution, chain selection, cache contents, permission
compilation or public wrappers. The subsequent selection layer is described below.
No new library contracts were added.

## Subsequent chain-selection layer

The direct-parent and fallback searches now execute checked code in
`verified/chain_selection.rs`. Direct selection compares all author bytes and
requires a current tip, choosing the smallest eligible chain ID. Fallback keeps
the supplied author's chain-list order and chooses its first clock-covered tip.
Author registration is checked together with storage append. The graph refinement
uses actual stored tables to prove maximality and freshness, then composes selection
and append with the single-author chain ancestry theorem, now generic over the
real Author type. Nonempty witnesses cover direct/indirect reuse, stale parents,
fresh chains and a merge with reordered/repeated parents.

The fallback requires a correct author-to-chain index and an ancestry clock that
represents the parents' causal past. Later work now checks the hash/author indexes
and concrete cached walk; see [API_TARGET.md](API_TARGET.md) for the current
boundary. The selection layer itself added no library contracts. The subsequent
index layer added the two derived-key contracts documented in
[TRUSTED_STDLIB.md](TRUSTED_STDLIB.md).

## Coverage changed by the rewrite

`history/traversal.rs` now retains chain intervals and emits ancestors first,
with the smallest commit hash among ready commits. Its order is independent of
local delivery order. None of the old numeric-index ordering lemmas proves that
canonical order or the new scheduler's completeness, blocking, progress or space
bounds. The eager `candidates::assemble` function is retained only for Verus as a
reference set model; normal Cargo builds do not compile it.

Raw `History::changes` is now a test/fuzzing boundary, and `next_changes` is
internal except for fuzzing. The authorization regressions moved into History's
unit tests without reopening those APIs. Public materialization is now through
`Materialization::plan`, `complete`, `step`, `update` and their Ledger/Document
integration, including checkpoints, stale completions and materializer effects.
`Ancestry::contains`, `participates`, and lazy `difference` are additional public
boundaries. These new implementations remain unverified end to end.

The existing graph policy, clock, table and set-level proofs remain useful, but
older milestone descriptions elsewhere refer to the previous implementation.
This document takes precedence over those historical coverage claims. The next
API proof must include the new traversal and materialization state machines;
passing the retained proof suite does not establish their correctness.

## Validation

The stack is pushed immediately after conflict resolution and build integration.
The pinned verifier, trust audit, Rust tests and canonical-traversal regressions
are checked against the rebased tip; results are reported in the PR/chat. Earlier
PR descriptions' counts belong to the old base and are not rebase validation.
