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
never means unbounded. Unbounded permission doesn't bypass the POV's
authentication frontier or causal delivery.

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
attesting to every ancestor commit. What this means is that all we need to 
retain for each author is the frontier of the commit graph that they have 
signed.

### Creating Local Commits

Signing may require IO, but local edits should show up immediately. The flow is:

1. Prepare the commit, fixing its author, parents, payload, and hash.
2. Add it to the local working state and ask the application's signer to sign it.
3. When the signature arrives and verifies, allow the commit to be published.

Incoming changes don't alter a prepared commit's hash or parents. Signing can
cover a batch through a later frontier signature; we don't need a round trip per
keystroke. The application tracks signing requests and retries. Stockfoil stays
synchronous and doesn't need an API that lists pending signing requests.

Local commits still have to pass the write-permission checks. Signing doesn't
grant access, and permissions may change while we wait.

### Pending Commits

The introduction of signatures, and of write frontiers, makes ingesting ready
commits more complicated. There are two reasons for this: we're still waiting
for the signature for the end of a line of commits, or we're waiting for the
commits from some author to be linked up to the write frontier.

#### Pending Signatures

One consequence of a model where we discard all but the frontier signatures is
that it is possible for signatures to arrive out of order with respect to the
changes. I.e. you could have a bunch of commits which claim to be from some
author but until the signature on the final commit arrives we can't actually
verify this.

For this reason when we receive a signature we need to check if it makes a
whole set of commits verified. Our own prepared commits can already appear in
the working POV while their signatures are pending; that doesn't make
them verified or publishable.

#### Establishing Write Permission

Unbounded authors need no write-frontier proof. Bounded authors need proof that
their commits are inside the frontier, using only the delivered history captured
by the POV. Until we have that proof, those commits don't contribute;
an unresolved bound never means unbounded access. Denied authors can't contribute
under that permission snapshot.

Receiving a commit can supply a missing proof for older commits, so we need to
check whether ingestion changes their participation too.

#### Relationship to sync or merge

Received commits can be forwarded while we wait for signatures or write-frontier
proofs. Those checks gate their contribution, not forwarding.

Our own unpublished commits are different: they must stay out of sync and merge
exports until covered by verified signatures. Exporting a descendant must not
pull unsigned local ancestors along with it. Delivery into the local operation
store is not publication. Signed commits can still be synced even when write
permissions prevent them from contributing.

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

For received commits, it is useful to distinguish these states:

* unauthenticated - we are awaiting a signature which verifies this commit
* detached - we are waiting for the causal dependencies of this commit
* eligible - we have a signature and all causal dependencies, but the write
  permission doesn't yet allow the commit, or we're missing a frontier proof
* materialized - we have a signature, all causal dependencies, and the write
  permission allows the commit (with a frontier proof if bounded)

The first two states are about what data we have received. The latter two can
change when permissions change or new proofs arrive. We keep the delivered
history in the operation store even when its commits don't contribute. Our own
pending-signature commits can also contribute to the local working view, without
being verified yet.

This suggests that as long as we have a way of representing the state of the
commit graph which includes these states then we can define a diff API which
stays stable across revocation changes.

#### Descendants of revoked commits

Descendants of a revoked commit can still contribute if their own authors'
permissions allow them. For example, in Automerge we can treat revoked commits
as deleted while still applying allowed descendant operations. The CRDT decides
what that means for the document.

### POVs

A POV (point of view) captures three things:

* The delivered heads, identifying the captured causally complete commit history.
  Notably this does not include commits which are not causally ready
* An immutable snapshot of the per-author write permissions
* An immutable snapshot of the per-author authentication frontiers: the commits
  this POV treats as authenticated, not proof that signatures are already
  available

Concretely, we can represent a POV as a set of delivered commit hashes,
a mapping from author keys to their write permissions, and a mapping from author
keys to their authentication frontier hashes. The delivered hashes are the heads,
not an enumeration of the whole history. Both mappings are immutable snapshots,
not references to the graph's mutable current state.

A commit contributes if it is in the captured delivered history, is covered by
its author's captured authentication frontier, and its author's write permission
allows it. Unbounded permission only satisfies the last check: it doesn't include
future commits or later authentication evidence.

Proofs of membership in a write frontier are evaluated using only the commit
history captured by the delivered heads, not subsequently received commits or
commits still awaiting causal delivery. Signatures are outside the commit graph,
so their arrival can authenticate already-delivered commits without changing the
delivered heads. Capturing the authenticated frontiers separately preserves the
meaning of earlier POVs when new signatures arrive.

A POV contains enough information to determine the set of contributing
commits regardless of future changes to the write permissions, commit graph, or
available signatures. This means we can define a diff API in terms of POVs.

We use `traverse_delivered_since` to incorporate newly delivered commits into
the CRDT's operation store, independently of whether they contribute under a
POV. Comparing POVs determines participation changes, including
changes to previously delivered commits when new signatures or frontier proofs
arrive. The CRDT uses that comparison to produce document-level changes.

#### Working and Signed POVs

We use the same representation for two views:

* The working POV includes signature-backed coverage plus our own local
  commits whose signatures are pending. Creating a local commit advances its
  author's authentication frontier here immediately.
* The signed POV advances authentication frontiers only when signatures
  verify. It can use the same delivered heads and write permissions as the working
  POV.

Ordinary ingestion doesn't grant this local allowance to incoming commits, even
if they claim our author key. It comes only from the local commit-creation path.
The graph keeps actual signature evidence separate from the working frontier.

Local materialization uses the working POV without waiting for signing.
Comparing it with the signed POV shows the document changes awaiting
signatures. Comparing authentication coverage identifies pending commits even
when their effects aren't visible, such as an overwritten assignment. The signed
view needn't be a prefix: different branches may get signed at different times.

When a signature arrives, the signed view can catch up without changing the
working document. Captured POVs stay unchanged. To preserve historical
signing status, save both POVs; the working one alone doesn't record
which signatures were available then.

#### Resolving POVs

A POV describes which commits contribute to some view of the document.
Its write permissions are already captured in the POV. To resolve this
description against signature evidence we additionally need two things:

* The causal graph referenced by the delivered heads in the POV.
* Authentication evidence that causally covers the captured authenticated
  frontier for each author, including any ancestry proofs needed to establish
  that coverage.

Resolution always requires actual signatures. A POV can name a frontier
whose signatures are pending, but it cannot resolve against signature evidence
until those signatures, or covering signatures, are available. Its meaning stays
the same while we wait. Resolution doesn't manage signing requests or return a
list of pending ones.

Calculating participation is separate from verifying signature evidence. Locally
created working POVs can drive materialization and diffs before signing;
that doesn't make them verified. Received POVs must be resolved before
we use them to drive a view. There is no "trust this frontier" option in resolution.

Coverage is per author and causal, not literal set inclusion: each captured
frontier hash must be equal to or an ancestor of a commit authenticated by a
signature from that same author. For example, a later signature on a descendant
can authenticate an earlier captured frontier even if its original signature has
been discarded. The ancestry evidence used for this validation may extend beyond
the POV's delivered history.

Such additional evidence validates the captured authenticated frontier; it does
not expand that frontier or supply new write-frontier proofs for the POV.
Write-frontier membership is still evaluated only within the captured delivered
history. Missing data or evidence required to resolve the POV means
resolution is incomplete, not that the affected commits are invisible.

Any process that resolves the same POV obtains the same commit
participation, regardless of arrival order or additional local knowledge. This
confers process independence without requiring a shared authorization log or DAG.

#### Testing Pending Local Signatures

The MVR test should hold back a local signature: the edit appears in the working
view, but not the signed view or an export. Their diff shows the pending edit.
After signing, the signed view catches up and export is allowed, without another
change to the working document. Both earlier POVs must still mean the
same thing. Also check batching and that exporting a descendant cannot leak an
unsigned local ancestor.

## API

See the [implementation sketch](authors_signatures_api.md) for proposed API
changes and the MVR test plan.

`set_write_permissions` replaces the whole permission map. Omitted authors are
denied, and existing POVs keep their old snapshots.

```rust
use std::collections::HashMap;

let mut graph: CommitGraph = CommitGraph::new();

// Alice and Bob can write without a frontier. Everyone else is denied.
let mut permissions = HashMap::from([
    (alice.clone(), WritePermission::Unbounded),
    (bob, WritePermission::Unbounded),
]);
graph.set_write_permissions(&permissions);

// now load the commit graph
for commit in commits {
    graph.ingest_commit(commit)?;
}

// At this point the graph is loaded and the permissions have been applied.

// This gives us the "current POV" of the graph. A combination of:
// * the delivered heads of the commit graph
// * an immutable snapshot of the per-author write permissions
// * an immutable snapshot of the per-author authentication frontiers
// This is the working POV; signed_pov() uses verified coverage.
let pov = graph.pov();

// now receive some new changes
graph.ingest_commit(new_commit)?;

// Now incorporate new commits into the operation log
let post_pov = graph.pov();
graph.traverse_delivered_since(pov.delivered_heads(), |commit| {
    // Newly delivered commits did not contribute under the source POV.
    // Store their operations as inactive until we process the participation diff.
    log.add_operations_from(commit, false);
});

// Ingestion can change participation for previously delivered commits too,
// for example when a new commit supplies a missing write-frontier proof.
let ingestion_diff = graph.diff(&pov, &post_pov)?;
for commit_diff in ingestion_diff.changed_commits() {
    let visible = match commit_diff.visibility_delta {
        VisDiff::Before => false,
        VisDiff::After => true,
    };
    // CRDT-specific bookkeeping: update participation, not operation storage.
    log.set_commit_visibility(commit_diff.commit_id, visible);
}
// The operation log now reflects post_pov. Receiving signatures can
// also change participation and should be handled by comparing POVs
// captured before and after signature ingestion, even without new delivery.

let pre_revocation_pov = graph.pov();
// Revoke Alice at these heads. Bob stays unbounded.
permissions.insert(alice, WritePermission::Bounded(revocation_heads));
graph.set_write_permissions(&permissions);

// Now diff again
let diff = graph.diff(&pre_revocation_pov, &graph.pov())?;
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
























