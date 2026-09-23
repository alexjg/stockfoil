# Sedimentree summaries: current slice

This narrows [Sedimentree Sync](sedimentree_sync.md) to structural summaries and
maintained indexes. The public types are in `src/sedimentree.rs`; the skip index
is in `src/commit_graph.rs`. The entry point is
`CommitGraph::as_sedimentree().summary()`.

No minimization, codecs, blob preparation, standalone item verification,
reconciliation, wants/needs/fixups, persistence, or transport are implemented.
Those belong to later slices, not prerequisites for testing the decomposition.

## What this summary means

The summary is **non-minimal**. It contains one canonical fragment for every
publishable delivered commit of nonzero depth, even when that fragment is wholly
contained in another. Loose commits are exactly the publishable delivered commits
covered by no fragment. Minimization will be a separate operation; it must not
remove the historical fragments needed for future construction and late forks.

Publication uses the graph's existing rules, independently of write permissions
and `ViewPoint` participation. Revoking an author doesn't change the sedimentree.
Queued commits are absent until all their parents arrive. Unsigned local work
and descendants depending on it are withheld until publishable. Existing graph
policy permits received commits without direct signatures and permits an
author's later signature to cover that author's earlier commits. The structural
summary follows that policy; it does **not** certify fragment-local proofs or
claim that every loose commit has a direct signature.

This is not an inventory of independently verifiable encoded items. Blob hashes,
sizes, encoding, and retained representations belong to a later slice. A
topology-only summary cannot supply them.

## Public API

```rust
struct FragmentRange {
    head: CommitHash,
    boundary: BTreeSet<CommitHash>,
}

struct Checkpoint([u8; 12]);

struct LooseCommit {
    commit: CommitHash,
    parents: BTreeSet<CommitHash>,
}

struct Fragment {
    range: FragmentRange,
    checkpoints: BTreeSet<Checkpoint>,
}

struct SedimentreeSummary {
    loose_commits: Vec<LooseCommit>,
    fragments: Vec<Fragment>,
}

impl<P: Clone> CommitGraph<P> {
    fn as_sedimentree(&self) -> SedimentreeView<'_>;
}

impl SedimentreeView<'_> {
    fn summary(&self) -> SedimentreeSummary;
}
```

The view borrows topology metadata, not payloads, and is not parameterized by
`P`. Creating it computes publication eligibility using the existing graph
policy. `summary()` returns an owned snapshot by scanning the node and fragment
indexes, selecting uncovered loose commits, and sorting metadata. Neither step
rebuilds fragment ranges, accesses or clones payloads, encodes anything, or
performs IO. Summary construction is not yet proportional only to output size;
publication computation may traverse graph ancestry.

Loose entries are sorted by commit hash; fragments are sorted by full range.
Parents, boundaries, and checkpoints are sets. Parent order in the original
commit envelope remains unchanged; the summary is not a replacement for it.

## Canonical ranges

Depth is the number of leading **zero bytes** in a commit hash. For example,
`00 00 12 ...` has depth 2; `01 ...` and `0f ...` both have depth 0. The live depth
metric is fixed, not configurable.

For each publishable commit at nonzero depth, the canonical fragment is defined
by this reference walk:

1. Include the head and walk backwards through its parents.
2. Include each encountered ancestor. If its depth is at least the head's depth,
   record it as a boundary and don't traverse its parents on that path.
3. A walk reaching a root terminates there. A root below the required depth is
   included without inventing a boundary identifier.
4. Record nonzero-depth interior commits as checkpoints, excluding the head and
   boundaries. Checkpoints are truncated to 12 bytes only in summary metadata.

Merges can yield multiple boundaries and paths that terminate at different
roots. Paths may overlap or bypass a boundary through another branch. A root
with nonzero depth creates a singleton fragment with no boundaries. Range
identity is `(head, boundary)`, not head alone, although a fully delivered head
has exactly one canonical range. These are this slice's explicit conventions;
compatibility with a pinned adapter still needs checking.

## Maintained skip-DAG index

The index covers all delivered history. Publication filtering is separate.
Each nonzero-depth node points to the one fragment it heads, not to all fragments
containing it. Fragments use local node indices for heads and boundaries; all
canonical fragments remain available, including redundant ones.

To construct a fragment headed at depth `d`, start from the head's parents and
use a visited set:

* At a commit of depth at least `d`, record a boundary and stop that path.
* At a shallower nonzero-depth commit, follow its fragment's boundaries instead
  of its commit parents. Include that head and its fragment's cached interior
  checkpoints in the new checkpoint set.
* At a depth-zero commit, record it as a directly visited loose member and
  continue through its parents.

A shallower fragment cannot hide a depth-`d` boundary in its interior: such a
commit would already be one of its boundaries. This is why the shortcut yields
the same boundaries as the reference walk. Merge paths are deduplicated, and
shallow roots end naturally. No per-node ancestry sets or per-depth prefix sets
are maintained.

For summary construction, each publishable fragment contributes its direct
loose-member list to coverage. Loose members hidden by a shortcut are already
recorded by the skipped fragment, which is retained and also publishable whenever
the containing fragment is publishable. Thus unioning these lists across all
publishable fragments identifies all covered depth-zero commits without
expanding fragment interiors. Every publishable nonzero-depth commit has its
own fragment and is therefore never loose. Withheld fragments must not suppress
otherwise publishable loose commits.

These lists are not a disjoint partition or a complete membership index for
each fragment. Exact per-fragment membership and redundancy queries may need
additional traversal or constituent links later.

## Tests

`tests/sedimentree.rs` replaces the old reduced-cover examples with seeded,
randomized public-API property tests. A deliberately independent oracle models
causal delivery and publication, then constructs every fragment with plain
parent walks. It does not use the production skip index or minimize its output.

Generated DAGs combine a nested/fork/merge backbone with random extensions and
parent ordering. The backbone guarantees shallower skips, strictly deeper and
equal-depth stops, shallow and nonzero roots, overlapping paths, bypassed
boundaries, and late forks rather than relying on rare hashes to exercise them.
Fixtures mine actual commit hashes at depths 0, 1, and 2 and are shared across
arrival schedules to keep the suite small and reproducible.

The tests compare complete summaries after operations, covering:

* Causal, reverse, and shuffled arrivals, including empty and queued-only graphs.
* Duplicate envelopes before and after causal delivery.
* Replica partitions, clone independence, and merge convergence.
* Local publication gating, received descendants of withheld commits, signatures
  arriving before their targets, and signatures from the wrong author.
* Every canonical fragment, including contained fragments; checkpoints, loose
  coverage, boundary sets, and deterministic output ordering.

Run `cargo test --test sedimentree`, or `cargo test --all-targets` for the entire
project. Assertion messages include the seed and operation for reproduction.
The default is eight DAGs; increase `SEDIMENTREE_TEST_CASES` for longer runs.
Reproduce an individual case with, for example:

```sh
SEDIMENTREE_TEST_SEED=0x17 SEDIMENTREE_TEST_CASES=1 cargo test --test sedimentree
```

## Deferred work

Minimization and redundancy information should be derived from the full
sedimentree, not used to discard its historical structure.

A future encoded inventory must not advertise unavailable or incomplete blobs.
Preparation must produce real blob metadata before advertisement, retain exact
servable bytes, and preserve older representations until replacements are ready.
That inventory may differ from the structural summary while preparation is pending.

Independently verifiable items need stricter proof retention than the current
graph-wide signature frontier. Loose blobs need direct author signatures;
fragments need author coverage using only their included ancestry, plus retained
proofs for contained canonical subfragments. Compaction and restoring smaller
fragments must not require new signatures. None of that is claimed or enforced
by this structural summary slice, and global signature pruning is unchanged.

Reconciliation and Subduction adaptation should follow those representation and
proof requirements. Keep commit identity, canonical payload hash, and transport
blob hash distinct, and check the adapter against a pinned upstream implementation.
