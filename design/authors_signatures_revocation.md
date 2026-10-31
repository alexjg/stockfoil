# Authors, Signatures, and Write Frontiers

## Motivation

Stockfoil is a support library for building CRDTs that work with the keyhive
authentication library and the subduction sync protocol. In the keyhive world
authentication and authorization are achieved by maintaining a set of Ed25519
signing keys mapped to the "frontier" of the graph that they are allowed to
write to. The advantage of this approach is that revocation is never actually
a deletion, which means that if a revocation happens whilst under partition
we just re-materialize the graph with the new authorization.

The interface keyhive provides us with is just the mapping from signing keys to
"revocation heads" - the frontier of the commit graph which the signing key is
allowed to write to. We need a scheme for actually binding signing keys to
commits.

## Approach

The basic idea here is to introduce a piece of metadata on each commit which we
call the "author". The author is an Ed25519 signing key. We also introduce
signatures on commits, but we don't include the signature inside the commit
envelope because  we want to be able to discard most signatures.

### Write Permissions

Each author has one of three write permissions:

* **Unbounded** - no write frontier to check.
* **Bounded(heads)** - allows that author's commits at or causally before at
  least one of these heads.
* **Denied** - none of that author's commits can contribute.

Missing authors are denied. An empty bounded frontier also permits nothing; it
never means unbounded. Unbounded permission doesn't include commits outside the
viewpoint's selected history or bypass causal delivery.

The application supplies the permissions and checks their authority. Stockfoil
only needs the snapshot, not the history that produced it. Revoking an author
usually means replacing their unbounded permission with a bounded frontier.

### Signatures and Compression

Shipping signatures around is complicated by the requirements of compression.
We want to be able to compress runs of commits together, omitting all but the
latest commit hashes. For many data types this can yield significant
compression. In automerge we get the metadata overhead of each commit down to
about one bit per commit. This compression gets completely blown out of the 
water if we have to include the signature for each commit.

Therefore, we want to keep the signatures _outside_ of the commit envelop and
only retain the signature for the latest commit in a run. If a commit is by
some author, and we have their signature for that commit, then they are
attesting to every ancestor commit. Covering signatures and the ancestry needed
to verify them can therefore replace older signatures once that evidence is
publishable. This is transport evidence for verification and export, retained
independently of viewpoints. `CommitGraph` does not maintain per-author
`signature_frontiers` or a separate authenticated set.

#### Borrowed authentication

Discarding signatures means a fragment can't always authenticate itself. Say Bob
writes `R → C → S → H` and his fragment carries only his signature on `H`. If
somebody branches from `C`, a later fragment on that branch contains `C` but not
`S` or `H`. Bob's latest commit in it is `C`, and the only surviving evidence
for `C` is the signature on `H`, outside the fragment. Peers which received
Bob's fragment never had a signature on `C` at all. This happens with ordinary
concurrency (a draft based on `C` signed after Bob's fragment arrives), and
routinely once commits can be created from arbitrary points in the graph.

So a fragment certificate may declare a *borrowed region*: tips among its
members whose ancestry within the fragment need not be covered by its own
signatures. Verification checks author coverage everywhere outside the region,
and reports the members the item does not cover itself as `unauthenticated()`.
Those members aren't dependencies. Each must be authenticated by admitted
history, or by an item admitted in the same batch as the borrower. A proof from
an item we can't admit never counts: otherwise our cover could contain a
borrower whose proof we can't relay, and peers syncing from us would be stuck.

Requiring proofs from items that are *already* admitted would deadlock, since a
lender can depend on the borrower. For example, the lender's boundary may be one
of the borrower's members. Items which prove each other's history are instead
admitted together. A batch is the largest set of queued items which supply each
other's unadmitted dependencies and proofs, found by discarding unsupported items
until nothing changes. Batches can only form around borrowers: without borrowing,
dependencies follow the acyclic commit graph, and single-item admission already
admits everything admissible. Placement is validated for the whole batch, against
the graph plus the batch's own metadata, before anything is inserted. A rejected
item is removed and the batch replanned without it, so its borrowers wait for a
valid proof. Directly received items use the same placement check with no extra
metadata.

Locally, a certificate borrows only members which are already admitted; an
unsigned draft must still wait for its own signature. Certificates which borrow
are re-derived from every signature we hold before pruning, so they become
self-contained whenever we have evidence inside the fragment. Because a replaced
item is contained in its replacement, its signatures are also members of the
replacement, so re-derivation carries its authentication forward.

The guarantee is now about the cover, not each item: every cover authenticates
everything it advertises, even mid-sync. A peer syncing one branch may need items
from another branch to authenticate the shared history. If a lender is stuck,
for example on a dependency that never arrives, its borrower stays queued on the
node that received both, rather than being advertised to peers who can't use it.

### Creating Local Commits

Signing may require IO, but local edits should show up immediately. The flow is:

1. Prepare the commit, fixing its author, parents, payload, and hash.
2. Add it to the local working state and ask the application's signer to sign it.
3. When the signature arrives and verifies, allow the commit to be published.

`commit_by` chooses the graph heads as parents, so normal prepared work
meets the local ingestion precondition. `ingest_local_commit` asserts that every
parent is already delivered before any mutation and panics on misuse; it never
buffers causally unready work. Delivered parents may be unsigned local drafts,
not yet admitted. Successful local ingestion delivers the commit immediately.

Incoming changes don't alter a prepared commit's hash or parents. Signing can
cover a batch through a later frontier signature; we don't need a round trip per
keystroke. The application tracks signing requests and retries. Stockfoil stays
synchronous and doesn't need an API that lists pending signing requests.

Local commits still have to pass the write-permission checks. Signing doesn't
grant access, and permissions may change while we wait.

### Pending Commits

Authentication gates admission of received commits, while write permissions
gate participation in a selected history. Our own prepared commits are the
local exception: they are delivered as `pending_local` drafts before signing.
`pending_local` denotes only delivered but unsigned/unadmitted local work, never
commits waiting for missing dependencies. `CommitGraph` separately owns a queue
of verified received items awaiting admitted dependencies, with no resource limits
currently. Queued items own their payloads but are not delivered, admitted,
materialized, or exportable; they affect neither heads nor viewpoints.
Admitted commits are exactly delivered commits not marked `pending_local`;
they are verified and parent-closed by invariant. Delivered local drafts do not
satisfy received admission dependencies.

#### Pending Signatures

Signatures and changes can arrive out of order. Unverified input stays outside
`CommitGraph`. Received items must be verified before graph ingestion, which
queues them if dependencies are not yet admitted. These queued items are not
delivered commits awaiting a participation check.

The local `ingest_signature` API accepts signatures for known, already delivered
pending local commits only; it cannot ingest a signature before its local
commit. A verified covering signature can release a batch of local commits
for admission and publication while preserving parent closure. Our own prepared
commits can already appear in the working viewpoint while their signatures are
pending; that doesn't make them admitted or publishable.

#### Establishing Write Permission

Unbounded authors need no write-frontier proof. Bounded authors need proof that
their commits are inside the frontier, using only the delivered history captured
by the viewpoint. Until we have that proof, those commits don't contribute;
an unresolved bound never means unbounded access. Denied authors can't contribute
under that permission snapshot.

Receiving a commit can supply a missing proof for older commits in a newly
captured viewpoint, so we need to check whether ingestion changes their
participation too. It cannot supply a bound outside an earlier viewpoint's
selected history. In particular, a pending draft cannot serve as a bound in a
signed history that excludes it.

#### Relationship to sync or merge

Only admitted history is exported through sync or merge. Received verified
items first check that range boundaries and external parents derived from member
metadata are admitted. Missing dependencies cause graph-owned queueing, not
`IngestError::MissingDependency` (which has been removed). Once dependencies are
admitted, normalized range placement is checked against their history before
inserting commits parents-first. Every insertion remains causally ready by
invariant. Write-frontier proofs gate participation, not forwarding of admitted
commits.

Payload callbacks run immediately even for queued items, for members not already
delivered. The queue owns their results, not the callback; exact queued duplicates
reuse those payloads. All callbacks run before graph or queue mutation, so a panic
leaves both unchanged. If a member is delivered while the item waits, including
through local insertion, that delivered payload is preserved on later admission.
A ready invalid fragment returns `Err(IngestError::InvalidFragment)` before
callbacks or mutation. A queued fragment found invalid later is removed without
delivering members and reported per item, without rolling back its successful
dependencies or independent admissions.

Received ingestion returns `Result<IngestReport, IngestError>`, local signing
returns `Result<IngestReport, SignatureError>`, and local insertion and merge
return `IngestReport`. Its `admitted: Vec<ItemId>` records certificates (including
duplicates and local certificates), not newly delivered commits;
`rejected: Vec<IngestRejection>` records deferred failures as `{ item, error }`;
and `pending` counts queued received items, not local drafts. Inspect rejections
even after successful signing or dependency ingestion. Rejected items' dependents
remain pending unless another certificate provides their missing commits.

Dependency-indexed waiters wake on every admission, including local signing and
all members of a fragment, and the graph automatically drains newly ready work.
`pending_items()` reports the received queue count; `missing_dependencies()`
iterates distinct unadmitted prerequisites in unspecified order, including any
still-unadmitted local drafts. Cloning a graph clones its queued items and owned
payloads. Merge transfers neither the sender's queue nor drafts nor permissions,
though imported admitted history can unblock the destination's queue.

Our own unpublished commits must stay out of sync and merge exports until
covered by verified signatures. Exporting a descendant must not pull unsigned
local ancestors along with it. Delivery into the local operation store is not
publication. Admitted commits can still be synced even when write permissions
prevent them from contributing.

## Revocation History

Tracking only delivered heads, as with the lower-level `traverse_since` API,
is not enough for incremental materialization: revocation can change a document
without delivering new commits. The standard `visit_changes` API instead tracks
both delivered history and the previous viewpoint in a `MaterializationCursor`.
It reports new data and participation changes through one callback. A separate
`diff` API is available for specialized participation queries without maintaining
a materialized view.

Ideally we want an API which makes revocation view changes explicit. This is
complicated though. Revocation view changes are driven by two things: the
arrival of commits which prove that some commits are inside or outside the
write frontier, and explicit APIs for updating the revocations for some author.

One way of looking at this stuff is that it is process local. This is a little
unsatisfying though. Many systems (e.g. a centralized log of revocations) would
be able to produce a global history of revocation changes. And users would like
to be able to answer questions about what changed from one revocation view to
another.

### Eligibility

For received commits, distinguish unverified input, queued items, and admitted
history:

* unverified - awaiting verification outside `CommitGraph`
* verified but queued - owned by `CommitGraph` with payloads, awaiting admitted
  causal dependencies; not delivered history and unavailable to materialization
* admitted but nonparticipating - verified with admitted causal dependencies,
  but outside the selected history or disallowed by its write permissions
* participating - in the selected history and allowed by its write permissions
  (with a frontier proof inside that history if bounded)

Authentication is admission, not participation. We keep admitted history in the
operation store even when its commits don't contribute. Our own pending local
commits can also contribute to the local working view without being admitted
yet. Once a history is selected, participation depends only on that history and
the captured write permissions, allowing stable diffs across revocation changes.

#### Descendants of revoked commits

Descendants of a revoked commit can still contribute if their own authors'
permissions allow them. For example, in Automerge we can treat revoked commits
as deleted while still applying allowed descendant operations. The CRDT decides
what that means for the document.

### Viewpoints

A viewpoint captures only two things:

* The history heads, identifying the selected causally complete commit history.
  Queued received work remains outside delivered history.
* An immutable snapshot of the per-author write permissions

The captured hashes are heads, not an enumeration of the whole history.
`ViewPoint::from_parts(heads, write_permissions)` reconstructs this
owned description without validating it. Neither field references the graph's
mutable current state; there is no authentication-frontier field.

A commit contributes if it is in the selected history and its author's write
permission allows it. Unbounded permission only satisfies the last check: it
doesn't include future commits or commits outside that history. Authentication
is enforced at admission, not as another participation predicate.

Proofs of membership in a write frontier are evaluated using only the history
selected by the history heads, not subsequently delivered commits or received
items queued inside the graph awaiting admitted dependencies. A pending draft
outside the signed history cannot serve as a permission bound there. Signing may change the
heads selected by a new signed viewpoint, but cannot change the history or
participation of an already captured viewpoint.

A viewpoint contains enough information to determine the set of contributing
commits regardless of future changes to the write permissions, commit graph, or
available signatures. This means we can define a diff API in terms of viewpoints.

`visit_changes` incorporates newly delivered commits and participation changes
through one callback. A `MaterializationCursor` tracks the processed delivery
heads separately from the materialized viewpoint, so historical reads do not
rewind delivery. Comparing viewpoints determines participation changes, including
changes to previously delivered commits when newly selected history supplies
frontier proofs. Newly captured signed viewpoints can also change as drafts are
admitted. The CRDT uses these updates to produce document-level changes. The
lower-level `traverse_since` and `diff` APIs remain available independently.
See [incremental materialization](authors_signatures_api.md#incremental-materialization)
for the combined API and cursor contract.

#### Working and Signed Viewpoints

We use the same representation for two views:

* `view_point()` selects all delivered history, including our own local drafts
  whose signatures are pending, and snapshots the current write permissions.
* `signed_view_point()` selects the maximal admitted commits as heads and
  snapshots the same current write permissions. Its heads and history can differ
  from the working viewpoint. Admitted ancestors underneath pending drafts are
  retained; simply filtering the working heads would incorrectly lose them.

Ordinary ingestion doesn't grant this local allowance to incoming commits, even
if they claim our author key. It comes only from the local commit-creation path.
Actual transport signature evidence is retained independently for export and
verification, not as a viewpoint frontier or a participation predicate.

Local materialization uses the working viewpoint without waiting for signing.
Comparing it with the signed viewpoint shows participation changes awaiting
signatures. Comparing their selected histories identifies pending commits even
when their effects aren't visible, such as an overwritten assignment. The signed
history is parent-closed but needn't be a single linear prefix: different
branches may get signed at different times.

When a signature arrives, admitting already delivered local work alone lets the
signed view catch up without changing working participation. Signing can also
unblock queued received items, adding new delivered history that may change
participation in newly captured views. Captured viewpoints stay unchanged. To preserve historical
signing status, save both viewpoints; the working one alone doesn't record
which commits were admitted then.

#### Comparing Viewpoints

A viewpoint describes which commits contribute to some view of the document.
`diff` requires every history head in both viewpoints to be delivered. A head
absent from the graph returns `DiffError::MissingCommit(hash)`, regardless of
write permissions. Delivered pending local heads are accepted, allowing local
materialization and diffs before signing without making those drafts admitted.
Signing changes neither a captured viewpoint's meaning nor its comparability
on a graph that already has its history.

Calculating participation is separate from validating admission. `diff` checks
graph history and write permissions only, not admission or signatures, for both
local and received viewpoints. Callers requiring admitted history can check
that `has_commit` holds for each head. Since admission is verified and
parent-closed, that also establishes admission of the selected ancestors.

For a single-view membership query, prepare `diff(&view, &view)`. On the
resulting diff, `visibility(&commit)` returns `Vis::Both` for a contributing
commit and `Vis::Neither` for a known non-contributing commit. An unknown commit
is a `QueryError::UnknownCommit`, not proof of invisibility.

Actual signature and ancestry evidence remains available independently for
transport verification and export. Evidence outside the selected history may
help admit a commit, but cannot expand a captured viewpoint or supply a
write-frontier bound outside its history. Unknown permission bounds grant
nothing; missing history heads prevent the comparison rather than making
selected commits invisible.

Any process with the selected history delivered obtains the same commit
participation, regardless of arrival order or additional local knowledge. This
confers process independence without requiring a shared authorization log or DAG.

#### Testing Pending Local Signatures

The MVR test should hold back a local signature: the edit appears in the working
view, but not the signed view or an export. Their diff shows the pending edit.
After signing, the signed view catches up and export is allowed, without another
change to the working document when no received items are unblocked. Both earlier viewpoints must still mean the
same thing. Also check that admitted ancestors under drafts remain in the signed
history, and that a pending draft cannot supply a permission bound there. Check
batching and that exporting a descendant cannot leak an unsigned local ancestor.

## API

See the [implementation sketch](authors_signatures_api.md) for proposed API
changes and the MVR test plan.

`set_write_permissions` replaces the whole permission map. Omitted authors are
denied, and existing viewpoints keep their old snapshots. Use `visit_changes`
for both initial materialization and subsequent updates; delivery and revocation
bookkeeping belong in the same callback.

In this sketch, `Operation`, `log`, `load_payload`, and `handle_rejections` are
application-specific.
Payload decoding and validation remain the application's responsibility.

```rust
use std::collections::HashMap;
use stockfoil::{CommitGraph, MaterializationChange, MaterializationCursor, Vis, WritePermission};

let mut graph = CommitGraph::<Operation>::new();
let mut permissions = HashMap::from([
    (alice, WritePermission::Unbounded),
    (bob, WritePermission::Unbounded),
]);
graph.set_write_permissions(&permissions);
let mut cursor = MaterializationCursor::new();

// Define the materialization logic once, for cold starts and every update.
let mut apply_change = |change: MaterializationChange<'_, Operation>| {
    if change.newly_delivered {
        // Retain new operations, initially inactive, even if currently denied.
        log.add_operations_from(change.meta, change.payload, false);
    }
    match change.visibility {
        Vis::After => log.set_commit_visibility(*change.meta.hash(), true),
        Vis::Before => log.set_commit_visibility(*change.meta.hash(), false),
        Vis::Both | Vis::Neither => {}
    }
};

// Verify before graph ingestion; arrival order need not be causal.
// The graph owns queued payloads, but materialization sees only delivery.
for item in received_items {
    let report = graph.ingest(item.verify()?, load_payload)?;
    handle_rejections(report.rejected);
}
cursor = graph.visit_changes(&cursor, &graph.view_point(), &mut apply_change)?;

// New history may also enable previously stored writes via frontier proofs.
let report = graph.ingest(new_item.verify()?, load_payload)?;
handle_rejections(report.rejected);
cursor = graph.visit_changes(&cursor, &graph.view_point(), &mut apply_change)?;

// Revoke Alice at these heads. Bob stays unbounded. No new delivery is needed.
permissions.insert(alice, WritePermission::Bounded(revocation_heads));
graph.set_write_permissions(&permissions);
cursor = graph.visit_changes(&cursor, &graph.view_point(), &mut apply_change)?;
```

For an admitted-only view, pass `signed_view_point()` instead of `view_point()`;
the same callback handles later signing without storing operations twice. For a
historical read that must not change live state, copy the materializer together
with its cursor, then update the copy to the requested viewpoint. The MVR example
uses exactly this approach.

Raw traversal and participation-only `diff` queries remain available for graph
inspection or specialized consumers, but are not needed for this update loop.
























