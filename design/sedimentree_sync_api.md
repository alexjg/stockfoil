# Sedimentree summaries: current slice

This narrows [Sedimentree Sync](sedimentree_sync.md) to structural summaries and
maintained indexes. The implementation is in `src/sedimentree.rs`; the public
entry point is `CommitGraph::as_sedimentree().summary()`.

No codecs, blob preparation, standalone item verification, reconciliation,
wants/needs/fixups, persistence, or transport are implemented or scaffolded here.
Those belong to later slices, not prerequisites for testing the decomposition.

## What this summary means

The summary describes **publishable delivered history**, using the graph's
existing publication rules. It is independent of write permissions and
`ViewPoint` participation. Revoking an author doesn't change the sedimentree.

Queued commits are absent until all their parents arrive. Unsigned local work
and descendants depending on it are withheld until publishable. Existing graph
policy permits received commits without direct signatures and permits an
author's later signature to cover that author's earlier commits. The structural
summary follows that policy; it does **not** certify fragment-local proofs or
claim that every loose commit has a direct signature.

This is deliberately not yet an inventory of independently verifiable encoded
items. In particular, **blob metadata is deferred**, not populated with dummy
hashes or confused with canonical payload hashes. The eventual Subduction-facing
inventory will need real blob hashes and sizes, obtained during preparation or
from received representations. A topology-only summary cannot supply them.

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

The view borrows only the maintained metadata index, not payloads. It needn't be
parameterized by `P`. `summary()` synchronously returns an owned snapshot and
costs only the work to copy its metadata. It doesn't traverse history, access or
clone payloads, encode anything, or perform IO.

Loose entries are sorted by commit hash; fragments are sorted by full range.
Parents, boundaries, and checkpoints are sets. Parent order in the original
commit envelope remains unchanged; the summary is not a replacement for it.

## Canonical ranges

Depth is the number of leading **zero bytes** in a commit hash. For example,
`00 00 12 ...` has depth 2; `01 ...` and `0f ...` both have depth 0. The live depth
metric is fixed, not configurable.

For each publishable commit at nonzero depth, create a candidate fragment at
that head's depth:

1. Include the head and walk backwards through its parents.
2. Include each encountered ancestor. If its depth is at least the head's depth,
   record it as a boundary and don't traverse its parents on that path.
3. A walk reaching a root terminates there. A root below the required depth is
   included without inventing a boundary identifier.
4. Record nonzero-depth interior commits as checkpoints, excluding the head and
   boundaries. Checkpoints are truncated to 12 bytes only in summary metadata.

Merges can yield multiple boundaries and paths that terminate at different
roots. A root with nonzero depth creates a singleton candidate. Range identity
is `(head, boundary)`, not head alone. These are this slice's explicit range
conventions; compatibility with a pinned adapter still needs checking.

## Maintained indexes and reduced cover

Keep an unfinished prefix at each node and depth `n`:

```text
prefix(n, v) = {v}                              if depth(v) >= n
               {v} ∪ union(prefix(n, parents))  otherwise
```

A depth-`n` head closes `{head} ∪ union(prefix(n, parents))` **before** resetting
its prefix to `{head}`. Roots need no special case. Merges are set unions, so
shared ancestors are counted once. Process parents newest-first: a prefix's largest
local index is its tip, and if that tip is already included, its whole prefix is
included. This skips redundant ancestor parents without comparing their sets.
No ancestry walk is needed to build a range.

These are persistent ordered sets (`im::OrdSet`) of local topological node
indices, not copied hash sets or ordinary vector clocks. Extending a chain shares
the old tree and copies only an insertion path. Unions use the set's sharing-aware
diff and insert only additions, rather than reinserting shared history at merges.
For a much smaller operand, membership checks avoid scanning the larger set just
to add a few nodes. Old prefixes stay available for
late forks, even when their fragments have left the cover. The sets retain holes:
a merge can include an ancestor through one path while a boundary cuts off other
commits in the same causal chain.

Each node stores prefixes only through the deepest checkpoint in its ancestry,
plus one unbounded prefix. That last entry also serves all greater depths. A
history with no checkpoints therefore needs one entry per node, not 32. The index
stores no parent links; it borrows graph metadata when constructing a fragment's
public hashes. Withheld nodes leave holes in the index until publication.

On closing a fragment, visit its members to collect boundaries/checkpoints and
remove covered loose entries. Only existing fragments whose heads are members
can be redundant: shallower ones are contained automatically; equal-depth ones
need an exact subset check. This preserves both adjacent ranges and the case
where a merge bypasses a boundary and includes its old fragment too. Membership
sets are retained for current cover fragments, not in a separate historical
range table. Summary metadata stays sorted by full hashes, independent of local
node numbering.

"Reduced" means removing individually contained ranges and covered loose
commits, not solving a global minimum-set-cover problem.

Publication eligibility still uses the existing graph-wide computation on
mutation. Only newly eligible nodes extend prefixes, in causal order. Permission
changes do nothing; cloning shares persistent set storage; merging uses ordinary
ingestion. `summary()` still just copies the maintained cover.

This trades additional derived state for incremental construction. It is not a
bounded-memory algorithm or a claim of constant-time insertion: merging sets,
emitting fragment metadata, and equal-depth subset checks still cost work.
Persistent integer sets keep this first version simple; compressed bitmap storage
and incremental publication tracking remain possible follow-ups.

## Tests

`tests/sedimentree.rs` tests only summary construction through the graph API:

* Empty graphs, loose chains, parent sets, independent roots, and merge tails.
* Inclusive boundaries, mixed root/boundary walks, and adjacent overlapping ranges.
* Nested fragments, depth-2 covering, and interior checkpoints.
* Queued parents, shuffled arrival order, duplicates, graph merge, and cloning.
* Local publication gating, signature arrival before/after commits, and permission
  independence.
* Owned snapshots and payload-free summary reads.
* Membership holes, late forks from absorbed checkpoints, and publication gaps.

Fixtures mine real commit hashes at the required depths. Unit tests pin the
byte-based metric and compare the prefix recurrence against a boundary-walk
oracle on generated DAGs, including synthetic depth-32 cases. They also exercise
shared chain snapshots, repeated fork/merge diamonds, wide merges, and late forks. An integration
test compares the maintained cover with exhaustive walk-and-subset reduction
after every insertion in generated real-hash DAGs. There are no codec fixtures or deliberately
failing `contract_*` tests.

Run `cargo test --all-targets`.

## Deferred work

A future encoded inventory must not advertise unavailable or incomplete blobs.
Preparation must produce real blob metadata before advertisement, retain exact
servable bytes, and preserve older representations until replacements are ready.
That inventory may differ from the structural cover while preparation is pending.

Independently verifiable items need stricter proof retention than the current
graph-wide signature frontier. Loose blobs need direct author signatures;
fragments need author coverage using only their included ancestry, plus retained
proofs for contained canonical subfragments. Compaction and restoring smaller
fragments must not require new signatures. None of that is claimed or enforced
by this structural summary slice, and global signature pruning is unchanged.

Reconciliation and Subduction adaptation should be designed after those
representation and proof requirements are implemented. Keep commit identity,
canonical payload hash, and transport blob hash distinct. Check the adapter
against a pinned upstream implementation: the current sibling checkout's batch
fingerprints use fragment heads, whereas its design document describes head plus
boundary.
