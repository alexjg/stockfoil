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

Boundary discovery remains a parent walk stopping at ancestors at least as deep
as the head. Redundant ancestor boundaries are retained. Member enumeration is
separate: ancestry clocks subtract all boundary history, using sorted per-chain
node positions to enumerate the remaining intervals.

`UnverifiedItem::Fragment` carries `range`, member metadata, and signature
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
placement returns `Err(IngestError::InvalidFragment)` before payload callbacks or
graph/queue mutation. A queued fragment is checked when its dependencies become
admitted; invalid placement removes that item and reports a deferred rejection
without delivering any members. Successful dependencies and independent ready
items remain admitted, rather than being rolled back. Dependents of a rejected
item remain pending unless another certificate provides the needed commits.

## Graph-owned received queue

`CommitGraph::ingest` accepts a `VerifiedItem` and a payload callback and returns
`Result<IngestReport, IngestError>`. Missing dependencies queue the item rather
than returning an error; `IngestError::MissingDependency` has been removed.
Unverified input stays outside the graph. The received queue currently has no
resource limits.

The callback runs immediately for each member not already delivered, even when
the item must wait. The graph owns the returned payloads, never the callback.
Exact queued duplicates reuse their payloads without invoking the callback again;
queue identity includes member hashes and signature evidence, not just `ItemId`,
so a malformed candidate cannot suppress a distinct valid one for the same range.
Callbacks complete before any graph or queue mutation; a panic leaves both
unchanged, though callback side effects are not rolled back. Existing delivered
payloads always win, including members delivered by local insertion or another
item after queueing.

Queued items are not delivered, admitted, materialized, or exportable. They do
not affect heads, viewpoints, traversal, or `visit_changes`. Delivered local
drafts remain separate working history; they do not satisfy received dependencies
until admitted. Dependency-indexed waiters wake on every newly admitted hash,
including all members of a fragment, not only its head. Admissions from local
signing also wake waiters, and newly ready received items and certifiable local
work drain automatically without callers resubmitting items.

Every operation that can admit history reports both its own admissions and the
queued work it unblocks. `ingest_signature` returns
`Result<IngestReport, SignatureError>`; `ingest_local_commit` and `merge` return
`IngestReport` directly. The report contains:

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

Structural summaries describe indexed canonical ranges. Their checkpoints and
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

`export` and `export_changes` return owned `UnverifiedItem` records. Their fragment
variants include `checkpoints: BTreeSet<Checkpoint>`, filled from the certified
cover's sorted, deduplicated 12-byte prefixes. Verification ignores these hints;
`VerifiedItem` still contains the full authenticated member metadata and IDs.
Admission and re-export derive canonical checkpoints locally. Decoders whose
formats omit hints can supply an empty set. Checkpoints do not change item
identity or export-cursor behavior.

## Binary Format

Subduction expects a binary format for commits and for fragments. The idea here
is that the CRDT knows a lot about how it could be encoded efficiently and 
how runs of commits can be encoded efficiently. For stockfoil this means that
we need to provide an extension point for the CRDT implementation to define
how to encode and decode commits and commit bundles. The received queue and
operation reports do not change any binary API or wire format; they operate on
verified metadata and application-owned decoded payloads.

## Sync without subduction

Subduction implements a bunch of the logic for detecting the missing pieces 
each ond of the wire needs. However, we won't always have subduction available
and it's still useful to be able to do synchronization ourselves. For this
purpose we should have some way of ingesting a sedimentree summary (defined in
our own data types) from another stockfoil document and determining a list of
wants, needs, and fixups.

A "fixup" in this context is a statement of which loose commits or fragments
in the combined sedimentrees are redundant.
