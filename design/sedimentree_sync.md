# Sedimentree Sync

Stockfoil is designed to sync CRDTs via
[subduction](https://github.com/inkandswitch/subduction). Subduction expects
the commit graph of the CRDT to be expressed as "fragments" and
"loose commits". The idea here is that we bundle a document into an
increasingly large fragments. Each fragment is defined by a head commit and
a set of ancestor commits of the head commit which we call the "boundary". The 
members of a fragment are the head's inclusive history minus the union of
all boundary histories:

```
members = H(head) - union(H(boundary))
```

Boundaries themselves are excluded. An edge bypassing a boundary does not
reintroduce that boundary's ancestors into the payload.

For sedimentree we define the head and boundary commits based on hash hardness.
A depth 2 fragment is one where the head and boundary commits have at least two
leading zero bytes in their hash. This means that nodes with
overlapping commit graphs will agree on the fragments in the overlapping
regions. With some redundancy.

From stockfoils perspective this means that we need to be able to produce
the sedimentree decomposition of the commit graph to be handed over to 
subduction and we need to be able to accept that same encoding from subduction.

## Dependencies and verification

`CommitGraph<C: PayloadCodec>` directly owns byte decoding, authentication,
history, and the received queue. The verification and queue rules below are
implemented by its private structured helpers, not a separate graph API. See the
[wire specification](codec-wire-v1.md) for framing and payload reconstruction.

Boundary discovery remains a parent walk stopping at ancestors at least as deep
as the head. Redundant ancestor boundaries are retained. Member enumeration is
separate: ancestry clocks subtract all boundary history, using sorted per-chain
node positions to enumerate the remaining intervals.

The private `UnverifiedItem::Fragment` carries `range`, member metadata, and signature
evidence. Verification derives the exact external parents after collecting all
member hashes, independently of input order:

```
external_parents = parents(members) - members
```

The **range boundary** describes which history is excluded. **External parents**
describe where member metadata connects to that history: directly referenced
boundaries as well as parents excluded through boundary history, including bypass
edges. A boundary reached only through excluded history is not an external parent.
`VerifiedItem` retains only external parents alongside the range; its
`external_parents()` accessor exposes this sorted, distinct set. The
`dependencies()` accessor returns all admission prerequisites as the sorted,
distinct union of the range boundary and external parents. For a standalone
commit, both accessors return its parents. `UnverifiedItem::dependencies()` derives
the same prerequisites from untrusted metadata and the claimed boundary.
Signatures cover every supplied member; signature targets must themselves be
members, not prerequisites.

Verification is stateless: it checks signatures, item-local reachability,
acyclicity, and depth constraints. Every external parent at least as deep as the
head must be in the declared boundary. Low-depth external parents are allowed
provisionally; verification cannot prove that they belong to boundary history.
Admission therefore:

1. Requires every admission prerequisite to be admitted, not merely a local draft.
   Otherwise the verified item and its immediately prepared payloads are queued
   inside `CommitGraph`, outside delivered history.
2. Checks that external parents lie in boundary history and supplied members do not.
3. Walks backwards from all external parents, stopping at commits at least as deep
   as the head, and requires those stops to exactly match the range boundary.
   Directly referenced boundaries stop immediately; other external parents lead
   through excluded history to further boundary stops.

Admission does not rebuild a member map or rewalk supplied member edges:
stateless reachability and exact external-parent derivation, together with these
placement checks, already establish exact normalized membership.

These checks are mandatory even when a correctly constructed source tree should
guarantee the prerequisites. A directly received, ready fragment with incorrect
placement returns an admission error carrying `IngestError::InvalidFragment`
before graph/queue mutation (application decoding has already run). A queued fragment is checked when its dependencies become
admitted; invalid placement removes that item and reports a deferred rejection
without delivering any members. Successful dependencies and independent ready
items remain admitted, rather than being rolled back. Dependents of a rejected
item remain pending unless another certificate provides the needed commits.

## Graph-owned received queue

`CommitGraph::ingest(&[u8])` returns `Result<IngestReport, Error<C::Error>>`.
Bounded decoding reconstructs owned payloads and metadata; authentication precedes
private structured admission. Missing dependencies queue the item rather than
returning an error. The received queue currently has no aggregate resource
limits; per-item `DecodeLimits` are not queue quotas.

The graph owns decoded payloads while the item waits. Exact queued duplicates
reuse their retained payloads, though each encoded ingestion still decodes and
verifies its input. Queue identity includes member hashes and signature evidence,
not just `ItemId`, so a malformed candidate cannot suppress a distinct valid one
for the same range. Decoding completes before graph/queue mutation; codec external
side effects are not rolled back. Existing delivered payloads always win,
including members delivered by local insertion or another item after queueing.

Queued items are not delivered, admitted, materialized, or exportable. They do
not affect heads, viewpoints, traversal, or `visit_changes`. Delivered local
drafts remain separate working history; they do not satisfy received dependencies
until admitted. Dependency-indexed waiters wake on every newly admitted hash,
including all members of a fragment, not only its head. Admissions from local
signing also wake waiters, and newly ready received items and certifiable local
work drain automatically without callers resubmitting items.

Every operation that can admit history reports both its own admissions and the
queued work it unblocks. `ingest_signature` returns
`Result<IngestReport, SignatureError>`; local `commit` returns a fallible draft
including `Draft::report`. Encoded native merges are fallible and preserve any partial report in
`MergeError::report`. The report contains:

* `admitted: Vec<ItemId>`: admitted certificates, including duplicates and local
  certificates. This is not a delivery stream; materialize with `visit_changes`.
* `rejected: Vec<IngestRejection>`: deferred per-item failures with `item: ItemId`
  and `error: IngestError`. Inspect these even when the initiating operation succeeds.
* `pending: usize`: received items still queued after the call, not local drafts.

`pending_items()` exposes that count, counting exact queued duplicates once.
`missing_dependencies()` iterates distinct unadmitted prerequisite hashes in
unspecified order; some may already be delivered as local drafts. These queries
allow dependency fetching without an external verified-item retry queue.
Cloning the graph clones queued items and their owned payloads. Export and merge
do not transfer the sender's queued items; merge can unblock the receiver's queue.

## Structural summaries and certified covers

Structural summaries, their minimization, certified cover selection, and their
carrier types are internal, not public graph APIs. Structural summaries describe
indexed canonical ranges. Their checkpoints and
loose-commit coverage use normalized members, never excluded boundary history.
Minimization preserves equal-depth overlaps. A deeper range can replace a
shallower one only when it contains the entire payload and each child boundary
is either a member or an explicit boundary of the covering range.

A checkpoint inside a received payload can head a canonical fragment containing
ancestors excluded from that received payload. Indexing that structural range
does not certify it for export. `CommitGraph::minimal_cover` therefore selects
only retained fragment certificates plus signed standalone commits, and can
differ from the minimized structural summary. Export, merge, and certificate
pruning use this certified cover. Boundary history remains independently
obtainable. Relays may ingest items in any arrival order; graph-owned queueing
ensures admission and delivery occur only once dependencies are admitted.

`CommitGraph::export_changes` returns owned `EncodedItem`s with transport
metadata, for synchronization and persistence alike. An empty cursor captures the
full current certified inventory; there is no public single-item export. Fragment metadata includes `checkpoints: BTreeSet<Checkpoint>`,
filled from the certified cover's sorted, deduplicated 12-byte prefixes. These are
reconciliation hints, not signature evidence, and are not transmitted in the
Stockfoil envelope. Admission and re-export derive them locally. Checkpoints do
not change item identity or export-cursor behavior. Structured export helpers
remain private implementation details.

## Binary Format

`CommitGraph<C>` exports complete encoded items and their transport metadata.
Its required `PayloadCodec` defines both canonical commit bytes and compressed
fragment payloads. Stockfoil owns the [v1 envelope](codec-wire-v1.md), including
Hexane-compressed structure, hash reconstruction, and signature evidence. Adapters
consume metadata without decoding outgoing bytes; checkpoint prefixes are derived
locally rather than transmitted as evidence. There is no core Subduction dependency.

Inbound resource limits can reject a large certified fragment. They do not block
export of trusted retained history after older certificates have been pruned.
Receivers can explicitly raise limits and retry, but the format supplies no
authenticated chunking or smaller fallback certificate. Aggregate queue budgets
and fixed-memory streaming remain separate work.

## Sync without subduction

Subduction implements a bunch of the logic for detecting the missing pieces 
each ond of the wire needs. However, we won't always have subduction available
and it's still useful to be able to do synchronization ourselves. For this
purpose we should have some way of ingesting a sedimentree summary (defined in
our own data types) from another stockfoil document and determining a list of
wants, needs, and fixups.

A "fixup" in this context is a statement of which loose commits or fragments
in the combined sedimentrees are redundant.
