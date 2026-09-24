# Sedimentree summaries and reads

This documents the current structural summaries, maintained skip-DAG index, and
borrowed reads. Public types are in `src/sedimentree.rs`; graph indexes and reads
are in `src/commit_graph.rs`.

[Verified ingestion and minimal-cover synchronization](verified_ingestion_api.md)
defines the transport contract: stateless verification, conditional admission,
and export of stored certificates from the authenticated fragment index.
`CommitGraph::minimal_cover() -> MinimalCover` is the transport inventory.
`CommitGraph::as_sedimentree()` and borrowed reads inspect local topology;
they do not promise arbitrary item exports.

## Structural summaries and the published cover

`summary()` is **non-minimal**. It contains every indexed canonical fragment,
even when wholly contained in another. A standalone authenticated commit of
nonzero depth does not imply an indexed fragment: its fragment certificate may
not have arrived. Loose commits, at any depth, are eligible delivered commits
not supplied by an indexed fragment beyond its prerequisite boundaries.
`minimized_summary()` omits structurally contained fragments without removing
historical records needed for construction and late forks. Both summaries have
identical loose commits. Neither exposes auxiliary lower-level fragments at a
head.

Eligibility is independent of write permissions and `ViewPoint` participation.
Revoking an author does not change the sedimentree. Queued commits are absent
until their parents arrive. Unsigned local work and descendants depending on it
are withheld until admitted. A signature on a pending local fragment head may
cover that author's earlier members within the fragment. Separate signatures
are rejected for unknown or already-admitted commits. Received history enters through
`ingest(VerifiedItem, ...)`, not raw insertion.

Only authenticated fragments enter the fragment index. Raw signatures live on
their target nodes, keyed by signer; fragments retain per-author signature-boundary
references, not copies of arbitrary-target evidence. `minimal_cover()` uses the same structural
reduction as `minimized_summary()`; there is no separate candidate or cached
published cover. Pending local items do not block unrelated admissions. Every
cover entry already has its certificate, and the complete cover is causally
admissible into an empty receiver.

Neither a structural summary nor `MinimalCover` is an inventory of encoded blobs.
Payload bytes, transport blob hashes, encoding, persistence, reconciliation, and
network IO are outside these APIs.

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

struct MinimalCover {
    loose_commits: Vec<LooseCommit>,
    fragments: Vec<Fragment>,
}

impl<P: Clone> CommitGraph<P> {
    fn as_sedimentree(&self) -> SedimentreeView<'_>;
    fn minimal_cover(&self) -> MinimalCover;
    fn export(&self, id: &ItemId) -> Result<UnverifiedItem, ExportError>;
    fn traverse_fragment<'graph>(
        &'graph self,
        range: &FragmentRange,
        visit: impl FnMut(&'graph CommitMeta, &'graph P) -> ControlFlow<()>,
    ) -> Result<(), FragmentReadError>;
    fn publishable_commit(&self, hash: &CommitHash) -> Option<(&CommitMeta, &P)>;
}

impl SedimentreeView<'_> {
    fn summary(&self) -> SedimentreeSummary;
    fn minimized_summary(&self) -> SedimentreeSummary;
}
```

The view borrows topology metadata, not payloads, and is not parameterized by
`P`. Creating it computes eligibility from durable authenticated state. Both
summary methods return owned snapshots. They derive constituent fragments by
walking the level below, collect interior checkpoints, and traverse level-one
commit parents to identify covered commits. Boundaries come from the maintained
index, not a new full graph walk. No payloads are accessed or cloned, and no
encoding or IO occurs. Structural summary construction traverses local topology;
it is not merely proportional to output size.

`minimal_cover()` uses that structural reduction. `export()` reads cover entries
or a cover fragment's standalone signed head announcement. Fragment export follows
stored signature-boundary references and uses the indexed traversal below to
gather ordered metadata. Neither performs signature-coverage discovery.

Loose entries are sorted by commit hash; fragments are sorted by full range.
Parents, boundaries, and checkpoints are sets. Parent order and multiplicity in
the original commit envelope remain unchanged; summaries do not replace it.

## Canonical ranges

Depth is the number of leading **zero bytes** in a commit hash. For example,
`00 00 12 ...` has depth 2; `01 ...` and `0f ...` both have depth 0. The depth
metric is fixed, not configurable.

For each eligible commit at nonzero depth, the canonical fragment is defined by
this reference walk:

1. Include the head and walk backwards through its parents.
2. Include each encountered ancestor. If its depth is at least the head's depth,
   record it as a boundary and do not traverse its parents on that path.
3. A walk reaching a root terminates there. A root below the required depth is
   included without inventing a boundary identifier.
4. Record nonzero-depth interior commits as checkpoints, excluding the head and
   boundaries. Checkpoints are truncated to 12 bytes only in summary metadata.

Merges can yield multiple boundaries and paths terminating at different roots.
Paths may overlap or bypass a boundary through another branch. A root with
nonzero depth defines a singleton fragment with no boundaries; a standalone
announcement of that root does not itself install the fragment. Range identity
is `(head, boundary)`, not head alone, although a fully delivered head has exactly
one canonical range.

## Maintained skip-DAG index

The skip index contains only authenticated, parent-closed history. Pending local
nodes and standalone announcements have no indexed fragments. When a depth-`d`
node's canonical range is admitted, either directly or as an introduced constituent
of a larger certified fragment, it heads explicit fragments at levels `1..=d`.
Its `headed_fragments[n - 1]` points to the fragment at level `n`, not to fragments
containing that commit. Each record stores its head, level, boundaries, and
per-author signature-boundary references. Contained and auxiliary fragments can
discard those references. Signature bytes remain on their targets while referenced
by any cover entry. There are no cached contents, checkpoint sets, loose-member
lists, or separate transport-cover index.

Internal identity is `(head, level)`: different levels can have identical
head/boundary ranges but remain distinct records. The public canonical fragment
is the maximum-level record at that head. All records remain available, including
redundant ones.

Build headed fragments from lower levels to higher levels:

* Level 1 walks backwards from the head through depth-zero commits, stopping at
  any other nonzero-depth commit, even an unindexed standalone announcement.
  Roots end naturally. Include head and stopping
  commits in membership; only stopping commits become boundaries.
* For level `n > 1`, start at the same head's newly created level-`n-1` fragment.
  Follow level-`n-1` fragment boundaries. A boundary of depth at least `n` stops
  the walk. Otherwise it has depth exactly `n-1`; follow the fragment it heads
  at that level. Deduplicate converging paths.

This walks the immediately lower level, never arbitrary commit interiors above
level 1. No depth-`n` boundary can be hidden inside a lower-level fragment.
The same traversal derives constituent fragments when summarizing. Individual
commits are the level-zero constituents, derived from commit parent links.
Constituents overlap; they are not a disjoint partition.

## Structural minimization

Define `A <= B` when A is B or is recursively a lower-level constituent of B.
`minimized_summary()` emits the maximal containment frontier: a unique minimal
cover under this structural relation, not minimum set cover or exhaustive
commit-set containment. Same-level fragments can overlap or even have accidental
whole-range containment through bypass paths; both remain unless a deeper
fragment structurally covers them.

Each constituent record is inserted before its containing record, including
lower-level records at the same head. Reverse fragment insertion order is thus
a valid topological order for reduction. For every eligible canonical fragment
not already covered, emit it and descend through its constituents, marking
fragment records covered. At level 1, mark commit members except the emitted
fragment's outer boundaries: these remain prerequisites, not supplied
authentication. Only eligible commits still uncovered are loose. Including a
boundary must **not** mark that commit's headed fragments covered or suppress
its prerequisite item merely by inclusion. Node and fragment coverage are
tracked separately and exist only for the duration of summary construction.

`summary()` uses the same walks but emits all canonical fragments, even already
covered ones. Interior constituent heads provide its checkpoints; the outer
head is excluded, and outer boundaries are never constituent heads. Metadata
truncation is never used to make coverage decisions. Only eligible heads
contribute to coverage. Admission attaches a complete certificate before the
item enters this index; reduction also identifies certificates to discard.

## Borrowed content reads

`CommitGraph::traverse_fragment` resolves an existing canonical maximum-level
range, validates its boundaries and publication eligibility, then borrows each
included commit's metadata and payload. It includes the head and boundaries,
visits every member once, and does not clone or encode payloads. The callback
may retain references for the duration of the graph borrow. `Break(())` stops
the entire traversal; both normal completion and an early break return `Ok(())`.

`FragmentReadError` distinguishes an unknown or queued head (`MissingCommit`),
a head without an indexed canonical fragment (`NotFragment`, including standalone
checkpoints), withheld history (`NotPublishable`), and a
range differing from the canonical boundaries (`BoundaryMismatch`). Validation
happens before invoking any callback. These are canonical reads, not arbitrary
head/boundary walks; internal lower-level ranges are not separately readable
through this API.

Ordering is the lexicographically smallest topological order by full commit
hash. Membership is derived through lower-level fragments down to level-one
commit walks. Kahn's algorithm then operates on all parent edges between members,
with a hash-ordered ready set and distinct-parent counts. Parents outside the
fragment do not constrain ordering. Crucially, a boundary's parent reached by
another merge path *does* constrain ordering: stopping membership discovery at
that boundary does not remove edges from the induced graph used for sorting.
The order is independent of arrival order, local node IDs, or hash-map iteration.
The sorting stage uses temporary per-fragment state and costs approximately
`O(E + V log V)`, in addition to eligibility and membership discovery. Export
uses this traversal to produce ordered metadata; there is no cached member order.

`publishable_commit` reads an individual authenticated, parent-closed commit,
returning `None` for unknown, queued, or withheld commits. It does not require
that the commit remain loose. Previously advertised loose entries and fragments
remain readable after later fragments cover them or minimization omits them.

Readability is not exportability. Current cover entries and their signed head
announcements export; a request for any other contained or replaced item returns
`ExportError::NotInMinimalCover`.
Inventory snapshots do not pin historical exports. Borrowed reads supply neither
signature certificates nor a way to bypass the maintained inventory. See
[verified ingestion](verified_ingestion_api.md) for evidence lifetime, stale
inventory handling, and dependency-ordered exchange.
