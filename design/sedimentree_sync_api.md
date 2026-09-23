# Sedimentree summaries: current slice

This narrows [Sedimentree Sync](sedimentree_sync.md) to structural summaries and
maintained indexes. The public types are in `src/sedimentree.rs`; the skip index
is in `src/commit_graph.rs`. The entry point is
`CommitGraph::as_sedimentree().summary()`.

No codecs, blob preparation, standalone item verification, reconciliation,
wants/needs/fixups, persistence, or transport are implemented.
Those belong to later slices, not prerequisites for testing the decomposition.

## What this summary means

The summary is **non-minimal**. It contains one canonical fragment for every
publishable delivered commit of nonzero depth, even when that fragment is wholly
contained in another. Loose commits are exactly the publishable delivered commits
covered by no fragment. `minimized_summary()` is a separate operation that
omits structurally contained fragments, without removing the historical records
needed for future construction and late forks. Both summaries have identical
loose commits. Neither exposes the auxiliary lower-level fragments at a head.

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
    fn minimized_summary(&self) -> SedimentreeSummary;
}
```

The view borrows topology metadata, not payloads, and is not parameterized by
`P`. Creating it computes publication eligibility using the existing graph
policy. Both summary methods return owned snapshots. They derive constituent
fragments by walking the level below, collect interior checkpoints, and traverse
level-one commit parents to identify covered commits. Final metadata is sorted.
Fragment boundaries come from the maintained index, not a new full graph walk.
No payloads are accessed or cloned, and no encoding or IO occurs. Summary
construction is not proportional only to output size; publication computation
may also traverse graph ancestry.

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
A depth-`d` node heads explicit fragments at every level `1..=d`. Its
`headed_fragments[n - 1]` points to the fragment at level `n`, not to fragments
containing that commit. Each record stores only its head, level, and boundaries.
There are no cached contents, checkpoint sets, or loose-member lists.

Internal identity is `(head, level)`: different levels can have identical
head/boundary ranges but must remain distinct records. The public canonical
fragment is the maximum-level record at that head. All records remain available,
including redundant ones.

Build headed fragments from lower levels to higher levels:

* Level 1 walks backwards from the head through depth-zero commits, stopping at
  any other nonzero-depth commit. Roots end naturally. Include head and stopping
  commits in membership; only stopping commits become boundaries.
* For level `n > 1`, start at the same head's newly created level-`n-1` fragment.
  Follow level-`n-1` fragment boundaries. A boundary of depth at least `n` stops
  the walk. Otherwise it has depth exactly `n-1`; follow the fragment it heads
  at that level. Deduplicate converging paths.

This walks the immediately lower level, never arbitrary commit interiors above
level 1. No depth-`n` boundary can be hidden inside a lower-level fragment.
The same traversal derives constituent fragments on demand when summarizing.
Individual commits are the level-zero constituents, derived from commit parent
links. Constituents overlap; they are not a disjoint partition.

## Structural minimization

Define `A <= B` when A is B or is recursively a lower-level constituent of B.
`minimized_summary()` emits the maximal containment frontier: a unique minimal
cover under this structural relation, not minimum set cover or exhaustive
commit-set containment. Same-level fragments can overlap or even have accidental
whole-range containment through bypass paths; both are retained unless a deeper
fragment structurally covers them.

Each constituent record is inserted before its containing record, including
lower-level records at the same head. Reverse fragment insertion order is thus
a valid topological order for reduction. For every publishable canonical
fragment not already covered, emit it and descend through its constituents,
marking fragment records covered. At level 1, mark the inclusive commit members.
Only commits still uncovered are loose. Covering a boundary commit must **not**
mark that commit's headed fragments covered. Node and fragment coverage are
tracked separately and exist only for the duration of summary construction.

`summary()` uses the same walks but emits all canonical fragments, even already
covered ones. Interior constituent heads provide its checkpoints; the outer
head is excluded, and outer boundaries are never constituent heads. Metadata
truncation is never used to make coverage decisions. Only publishable fragments
contribute to coverage, so withheld descendants cannot suppress published loose
commits.

## Tests

`tests/sedimentree.rs` replaces the old reduced-cover examples with seeded,
randomized public-API property tests. A deliberately independent oracle models
causal delivery and publication, then constructs every fragment with plain
parent walks. It does not use the production skip index. An independent
reduction oracle uses full-hash membership: a canonical fragment is structurally
covered if its head is strictly shallower than another head and is an interior
member of that head's fragment. Equal-depth containment is deliberately retained.

Generated DAGs combine a nested/fork/merge backbone with random extensions and
parent ordering. The backbone guarantees shallower skips, strictly deeper and
equal-depth stops, shallow and nonzero roots, overlapping paths, bypassed
boundaries, and late forks rather than relying on rare hashes to exercise them.
Explicit witnesses retain same-level whole-range containment and close a
checkpoint-free loose chain with a depth-2 head.
Fixtures mine actual commit hashes at depths 0, 1, and 2 and are shared across
arrival schedules to keep the suite small and reproducible.

The tests compare both summary forms after operations, covering:

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

Further redundancy information should be derived from the full sedimentree,
not used to discard its historical structure. More aggressive set-based
minimization and cached summary acceleration remain separate possibilities.

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
