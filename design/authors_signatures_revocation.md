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
never means unbounded. Unbounded permission still requires authentication and
causal delivery.

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
whole set of commits verified.

#### Establishing Write Permission

Unbounded authors need no write-frontier proof. Bounded authors need proof that
their commits are inside the frontier, using only the delivered history captured
by the perspective. Until we have that proof, those commits don't contribute;
an unresolved bound never means unbounded access. Denied authors can't contribute
under that permission snapshot.

Receiving a commit can supply a missing proof for older commits, so we need to
check whether ingestion changes their participation too.

#### Relationship to sync or merge

One key point here is that when we are waiting for signatures or waiting for 
commits to be linked up to the write frontier, we are _only_ waiting for 
materialization reasons. From a sync perspective we can send this stuff 
immediately.

## Revocation History

The `stockfoil::CommitGraph::traverse_since` API is one example of a very
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

A fruitful way to think about this is to imagine a commit as being in a number
of states:

* unauthenticated - we are awaiting a signature which verifies this commit
* detached - we are waiting for the causal dependencies of this commit
* eligible - we have a signature and all causal dependencies, but the write
  permission doesn't yet allow the commit, or we're missing a frontier proof
* materialized - we have a signature, all causal dependencies, and the write
  permission allows the commit (with a frontier proof if bounded)

The first two states are about what data we have received. The latter two can
change when permissions change or new proofs arrive. We keep the delivered
history in the operation store even when its commits don't contribute.

This suggests that as long as we have a way of representing the state of the
commit graph which includes these states then we can define a diff API which
stays stable across revocation changes.

#### Descendants of revoked commits

Descendants of a revoked commit can still contribute if their own authors'
permissions allow them. For example, in Automerge we can treat revoked commits
as deleted while still applying allowed descendant operations. The CRDT decides
what that means for the document.

### Perspectives

A "perspective" captures three things:

* The delivered heads, identifying the captured causally complete commit history.
  Notably this does not include commits which are not causally ready
* An immutable snapshot of the per-author write permissions
* An immutable snapshot of the per-author authenticated frontiers, this
  captures the idea that receiving new signatures can change the view even 
  though it does not change the commit graph

Concretely, we can represent a perspective as a set of delivered commit hashes,
a mapping from author keys to their write permissions, and a mapping from author
keys to their authenticated frontier hashes. The delivered hashes are the heads,
not an enumeration of the whole history. Both mappings are immutable snapshots,
not references to the graph's mutable current state.

A commit contributes if it is in the captured delivered history, is covered by
its author's captured authenticated frontier, and its author's write permission
allows it. Unbounded permission only satisfies the last check: it doesn't include
future commits or later authentication evidence.

Proofs of membership in a write frontier are evaluated using only the commit
history captured by the delivered heads, not subsequently received commits or
commits still awaiting causal delivery. Signatures are outside the commit graph,
so their arrival can authenticate already-delivered commits without changing the
delivered heads. Capturing the authenticated frontiers separately preserves the
meaning of earlier perspectives when new signatures arrive.

A perspective contains enough information to determine the set of contributing
commits regardless of future changes to the write permissions, commit graph, or
available signatures. This means we can define a diff API in terms of
perspectives.

We use `traverse_delivered_since` to incorporate newly delivered commits into
the CRDT's operation store, independently of whether they contribute under a
perspective. Comparing perspectives determines participation changes, including
changes to previously delivered commits when new signatures or frontier proofs
arrive. The CRDT uses that comparison to produce document-level changes.

#### Resolving Perspectives

A perspective describes which commits contribute to some view of the document.
Its write permissions are already captured in the perspective. To resolve this
description we additionally need two things:

* The causal graph referenced by the delivered heads in the perspective.
* Authentication evidence that causally covers the captured authenticated
  frontier for each author, including any ancestry proofs needed to establish
  that coverage.

Coverage is per author and causal, not literal set inclusion: each captured
frontier hash must be equal to or an ancestor of a commit authenticated by a
signature from that same author. For example, a later signature on a descendant
can authenticate an earlier captured frontier even if its original signature has
been discarded. The ancestry evidence used for this validation may extend beyond
the perspective's delivered history.

Such additional evidence validates the captured authenticated frontier; it does
not expand that frontier or supply new write-frontier proofs for the perspective.
Write-frontier membership is still evaluated only within the captured delivered
history. Missing data or evidence required to resolve the perspective means
resolution is incomplete, not that the affected commits are invisible.

Any process that resolves the same perspective obtains the same commit
participation, regardless of arrival order or additional local knowledge. This
confers process independence without requiring a shared authorization log or DAG.

## API

See the [implementation sketch](authors_signatures_api.md) for proposed API
changes and the MVR test plan.

`set_write_permissions` replaces the whole permission map. Omitted authors are
denied, and existing perspectives keep their old snapshots.

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

// This gives us the "current perspective" of the graph. A combination of:
// * the delivered heads of the commit graph
// * an immutable snapshot of the per-author write permissions
// * an immutable snapshot of the per-author authenticated frontiers
let perspective = graph.perspective();

// now receive some new changes
graph.ingest_commit(new_commit)?;

// Now incorporate new commits into the operation log
let post_perspective = graph.perspective();
graph.traverse_delivered_since(perspective.delivered_heads(), |commit| {
    // Newly delivered commits did not contribute under the source perspective.
    // Store their operations as inactive until we process the participation diff.
    log.add_operations_from(commit, false);
});

// Ingestion can change participation for previously delivered commits too,
// for example when a new commit supplies a missing write-frontier proof.
let ingestion_diff = graph.diff(perspective, post_perspective);
for commit_diff in ingestion_diff.changed_commits() {
    let visible = match commit_diff.visibility_delta {
        VisDiff::Before => false,
        VisDiff::After => true,
    };
    // CRDT-specific bookkeeping: update participation, not operation storage.
    log.set_commit_visibility(commit_diff.commit_id, visible);
}
// The operation log now reflects post_perspective. Receiving signatures can
// also change participation and should be handled by comparing perspectives
// captured before and after signature ingestion, even without new delivery.

let pre_revocation_perspective = graph.perspective();
// Revoke Alice at these heads. Bob stays unbounded.
permissions.insert(alice, WritePermission::Bounded(revocation_heads));
graph.set_write_permissions(&permissions);

// Now diff again
let diff = graph.diff(pre_revocation_perspective, graph.perspective());
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
























