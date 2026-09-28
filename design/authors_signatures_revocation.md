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
commits waiting for missing dependencies. `CommitGraph` has no causal queue;
all missing-dependency buffering belongs outside the graph.
Admitted commits are exactly delivered commits not marked `pending_local`;
they are verified and parent-closed by invariant.

#### Pending Signatures

Signatures and changes can arrive out of order. Received items must be verified
before admission, and missing dependencies and unverified items are buffered
outside `CommitGraph`. They are not delivered commits awaiting a participation
check.

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
items first check that external dependencies are admitted, then insert commits
parents-first. Range boundaries and external parents derived from member metadata
must all be admitted; normalized range placement is checked against their history
before payload callbacks or mutation. Every insertion is therefore causally
ready by invariant; missing dependencies are
buffered outside the graph, not queued inside it. Write-frontier proofs gate
participation, not forwarding of admitted commits.

Our own unpublished commits must stay out of sync and merge exports until
covered by verified signatures. Exporting a descendant must not pull unsigned
local ancestors along with it. Delivery into the local operation store is not
publication. Admitted commits can still be synced even when write permissions
prevent them from contributing.

## Revocation History

The `stockfoil::CommitGraph::traverse_delivered_since` API is one example of a very
useful kind of API which allows applications to perform incremental updates.
Another example would be a `diff` API which allows applications to compute the
difference between two parts of the commit graph. At the moment these kinds of
API are passed the heads of the document, but this is not sufficient for
applications which want to do things like diff the before and after of a
revocation. 

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

For received commits, distinguish data buffered outside the graph from admitted
history:

* unverified or detached - awaiting verification or causal dependencies outside
  `CommitGraph`
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
  Causally unready work remains outside the graph.
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
selected by the history heads, not subsequently received commits or items
buffered outside the graph awaiting dependencies. A pending draft outside the
signed history cannot serve as a permission bound there. Signing may change the
heads selected by a new signed viewpoint, but cannot change the history or
participation of an already captured viewpoint.

A viewpoint contains enough information to determine the set of contributing
commits regardless of future changes to the write permissions, commit graph, or
available signatures. This means we can define a diff API in terms of viewpoints.

We use `traverse_delivered_since` to incorporate newly delivered commits into
the CRDT's operation store, independently of whether they contribute under a
viewpoint. Comparing viewpoints determines participation changes, including
changes to previously delivered commits when newly selected history supplies
frontier proofs. Newly captured signed viewpoints can also change as drafts are
admitted. The CRDT uses that comparison to produce document-level changes.

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

When a signature arrives, the signed view can catch up without changing the
working document. Captured viewpoints stay unchanged. To preserve historical
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
change to the working document. Both earlier viewpoints must still mean the
same thing. Also check that admitted ancestors under drafts remain in the signed
history, and that a pending draft cannot supply a permission bound there. Check
batching and that exporting a descendant cannot leak an unsigned local ancestor.

## API

See the [implementation sketch](authors_signatures_api.md) for proposed API
changes and the MVR test plan.

`set_write_permissions` replaces the whole permission map. Omitted authors are
denied, and existing viewpoints keep their old snapshots.

```rust
use std::collections::HashMap;

let mut graph: CommitGraph = CommitGraph::new();

// Alice and Bob can write without a frontier. Everyone else is denied.
let mut permissions = HashMap::from([
    (alice.clone(), WritePermission::Unbounded),
    (bob, WritePermission::Unbounded),
]);
graph.set_write_permissions(&permissions);

// Load received items after verification, in dependency order.
// Buffer unverified items and missing dependencies outside the graph.
for commit in commits {
    graph.ingest_commit(commit)?;
}

// At this point the graph is loaded and the permissions have been applied.

// This gives us the "current viewpoint" of the graph. A combination of:
// * the heads of the commit graph
// * an immutable snapshot of the per-author write permissions
// This is the working viewpoint, including local drafts.
// signed_view_point() instead selects the maximal admitted commits as heads.
let view_point = graph.view_point();

// Now admit a verified received change whose dependencies are admitted.
graph.ingest_commit(new_commit)?;

// Now incorporate new commits into the operation log
let post_view_point = graph.view_point();
graph.traverse_since(view_point.heads(), |commit| {
    // Newly delivered commits did not contribute under the source viewpoint.
    // Store their operations as inactive until we process the participation diff.
    log.add_operations_from(commit, false);
});

// Ingestion can change participation for previously delivered commits too,
// for example when a new commit supplies a missing write-frontier proof.
let ingestion_diff = graph.diff(&view_point, &post_view_point)?;
for commit_diff in ingestion_diff.changed_commits() {
    let visible = match commit_diff.visibility_delta {
        VisDiff::Before => false,
        VisDiff::After => true,
    };
    // CRDT-specific bookkeeping: update participation, not operation storage.
    log.set_commit_visibility(commit_diff.commit_id, visible);
}
// The operation log now reflects post_view_point. Signing local drafts changes
// newly captured signed heads, not this working document. To update a signed
// materialization, compare signed viewpoints captured before and after signing.

let pre_revocation_view_point = graph.view_point();
// Revoke Alice at these heads. Bob stays unbounded.
permissions.insert(alice, WritePermission::Bounded(revocation_heads));
graph.set_write_permissions(&permissions);

// Now diff again
let diff = graph.diff(&pre_revocation_view_point, &graph.view_point())?;
for commit_diff in diff.changed_commits() {
    match commit_diff.visibility_delta {
        VisDiff::Before => {
            // commit contributed before but does not contribute after
        }
        VisDiff::After => {
            // commit did not contribute before but contributes after
        }
    }
}


// Another way is to walk our CRDT operation log checking visibility against
// the diff
for op in crdt_specific_op_log {
    match diff.visibility(op.commit_id) {
        Vis::Before => {
            // commit was visible before but not after
        }
        Vis::After => {
            // commit was not visible before but is after
        }
        Vis::Both => {
            // commit was visible and is still visible
        }
        Vis::Neither => {
            // commit was invisible and is still invisible
        }
    }
}
```
























