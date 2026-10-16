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
call the "author". The author is an Ed25519 public key and is part of the commit
hash. We also introduce signatures on commits, but we don't include the
signature inside the commit envelope because we want to be able to discard most
signatures.

### Write Permissions

Each author has one of three write permissions (`WritePermission`):

* **Unbounded** - no write frontier to check.
* **Bounded(heads)** - allows that author's commits at or causally before at
  least one of these heads.
* **Denied** - none of that author's commits can contribute.

Missing authors are denied. An empty bounded frontier also permits nothing; it
never means unbounded. Unbounded permission doesn't include commits outside the
viewpoint's selected history or bypass authentication and causal delivery.

The application supplies the permissions and checks their authority. Stockfoil
only needs the snapshot, not the history that produced it, and keeps no
permission state of its own: callers pass a `WritePermissions`
(`HashMap<Author, WritePermission>`) whenever they capture a viewpoint.
Revoking an author usually means replacing their unbounded permission with a
bounded frontier in the map the application passes next time.

### Signatures and Compression

Shipping signatures around is complicated by the requirements of compression.
We want to be able to compress runs of commits together, omitting all but the
latest commit hashes. For many data types this can yield significant
compression. In automerge we get the metadata overhead of each commit down to
about one bit per commit. This compression gets completely blown out of the
water if we have to include the signature for each commit.

Therefore, we keep the signatures _outside_ of the commit envelope and only
retain the signature for the latest commit in a run. If a commit is by some
author, and we have their signature for that commit, then they are attesting to
every ancestor commit by that author. Within a bundle, every member must be an
ancestor-or-self of a member its author signed (see the wire format,
[Authentication](wire-v2.md#6-authentication)).

Signature evidence lives in *certificates*: one per admitted standalone commit
or fragment. A certificate keeps only the signatures that cover its members:
per author, those that no later signature by the same author in the item
already reaches. Signatures by keys that author no member cover nothing and
are dropped, whether they arrived on a received item or were made locally.
Admitting a fragment retires the standalone certificates and smaller fragments
it fully covers from the export inventory. Evidence is used for verification
and export only; it is not a viewpoint frontier or a participation predicate.

### Creating Local Commits

Signing may require IO, but local edits should show up immediately. The flow is:

1. Prepare the commit, fixing its author, parents, payload, and hash.
2. Deliver it to local history as an unadmitted *draft* and ask the
   application's signer to sign it.
3. When the signature arrives and verifies, admit the commit so it can be
   exported.

`History` never holds payload bytes. `begin_commit(author, parents)` checks that
every parent is delivered (drafts included) before any payload is read and
returns a `CommitBuilder`. The caller streams canonical payload bytes through
`update`, stores them wherever it likes, and calls `finish` to get a `Prepared`
holding the hash, author, ordered parents, and payload hash. `publish_commit
(prepared, payload_id)` then delivers the draft with the application's logical
payload ID. `Prepared::signing_bytes()` is the raw commit hash. The native
`Graph::commit(author, parents, payload)` does all of this, storing the payload
in its `Storage`. The caller chooses the parents, typically the delivered heads.

Incoming changes don't alter a prepared commit's hash or parents. The
application tracks signing requests and retries; `History::drafts()` lists the
unadmitted local commits, parents first. Stockfoil stays synchronous.

Local commits still have to pass the write-permission checks. Signing doesn't
grant access, and permissions may change while we wait.

### Drafts and Admission

Authentication gates admission, while write permissions gate participation in a
selected history. History contains two kinds of delivered commits:

* **Admitted** commits are authenticated: verified and parent-closed by
  invariant (`is_admitted`, `Heads::Admitted`).
* **Drafts** are delivered local commits that are not yet admitted. Only
  `publish_commit` creates them; received items are delivered only on admission,
  whatever author key they claim.

There is no queue inside `History`. Receiving is an `Admission` operation owned
by the host (`History::receive`). It reads, decodes, hashes, and verifies the
stored item without touching history, then calls `History::advance` to check
that the item's dependencies are admitted and that a fragment sits at its
canonical place. If a dependency is missing, `advance` returns
`Next::Waiting(hash)` and changes nothing. The host keeps the operation and
retries after `History::epoch()` changes; verification is not repeated.
Delivered drafts do not satisfy received dependencies. Once ready, the operation
asks the host to bind payload IDs for undelivered members and then publishes the
bundle atomically, parents first. Duplicates keep the payload ID of their first
delivery.

The native `Graph<C, S>` runs these requests synchronously. `Graph::receive`
stores the item and returns `Error::MissingDependency(hash)` when the admission
would wait; the item stays in storage for a later `Graph::admit`.
`Graph::begin_admit` and `Graph::drive(&key, &mut op)` keep the verified
operation, alongside the item's storage key, instead, returning
`Driven::Waiting(hash)`. Errors are terminal for that operation and
leave history untouched; storage may keep unused blobs or payload IDs.

#### Signing Drafts

`History::sign(author, hash, signature)` verifies the signature and stores it on
a delivered draft. An unknown commit is `Error::UnknownCommit` and an admitted
one is `Error::NotDraft`; a signature cannot arrive before its local commit.
`sign` then admits every signed draft whose certificate is complete, repeating
until nothing more can be admitted, and returns the admitted certificates'
`ItemId`s (possibly none).

A signed draft's certificate is a standalone commit at depth 0, otherwise its
canonical fragment. It is complete when its signatures cover every member and
every external parent and boundary commit is admitted. So signing a child before
its depth-0 parent admits nothing, and signing the parent later admits both. A
signature on a fragment head can admit the whole fragment, covering its
unsigned drafts by the same author. Admitting a received item that contains a
draft also admits it, and any drafts that completes are reported in
`Receipt::drafts_admitted`.

Our own drafts appear in the working viewpoint while their signatures are
pending; that doesn't make them admitted or exportable.

#### Establishing Write Permission

Unbounded authors need no write-frontier proof. Bounded authors need proof that
their commits are inside the frontier, using only the delivered history captured
by the viewpoint. Until we have that proof, those commits don't contribute;
an unresolved bound never means unbounded access. Denied authors can't contribute
under that permission snapshot.

Receiving a commit can supply a missing proof for older commits in a newly
captured viewpoint, so we need to check whether delivery changes their
participation too. It cannot supply a bound outside an earlier viewpoint's
selected history. In particular, a draft cannot serve as a bound in a signed
history that excludes it.

#### Relationship to Sync

Only admitted history is exported. The export inventory holds one certificate
per admitted standalone commit or fragment; drafts enter it only when signing
admits them. `History::export(&cursor)` plans the certificates an
`ExportCursor` has not acknowledged, `export_item` produces each item's metadata
and member payload IDs for the application encoder, and `ExportPlan::cursor()`
advances only over acknowledged items. Because admission is parent-closed,
exporting a descendant cannot pull unsigned local ancestors along with it.
Delivery into local history is not publication.

Export plans are not in dependency order, so receivers must tolerate waiting.
Write-frontier proofs gate participation, not forwarding: admitted commits are
exported even when write permissions prevent them from contributing. Export
transfers neither drafts nor permissions.

## Revocation History

Tracking only delivered heads, as with the lower-level `traverse_since` API,
is not enough for incremental materialization: revocation can change a document
without delivering new commits. `History::changes` instead tracks both the
delivered history and the previous viewpoint in a `MaterializationCursor`, and
reports new data and participation changes in one sequence of records. A
separate `diff` API is available for participation queries without maintaining
a materialized view.

Ideally we want an API which makes revocation view changes explicit. This is
complicated though. Revocation view changes are driven by two things: the
arrival of commits which prove that some commits are inside or outside the
write frontier, and explicit updates to the revocations for some author.

One way of looking at this stuff is that it is process local. This is a little
unsatisfying though. Many systems (e.g. a centralized log of revocations) would
be able to produce a global history of revocation changes. And users would like
to be able to answer questions about what changed from one revocation view to
another.

### Eligibility

For received commits, distinguish unverified or waiting input from admitted
history:

* unverified or waiting - held by the host as stored bytes and an `Admission`
  operation, possibly verified but waiting for admitted dependencies; not
  delivered history and unavailable to materialization
* admitted but nonparticipating - verified with admitted causal dependencies,
  but outside the selected history or disallowed by its write permissions
* participating - in the selected history and allowed by its write permissions
  (with a frontier proof inside that history if bounded)

Authentication is admission, not participation. We keep admitted history even
when its commits don't contribute. Our own drafts can also contribute to the
local working view without being admitted yet. Once a history is selected,
participation depends only on that history and the captured write permissions,
allowing stable diffs across revocation changes.

#### Descendants of revoked commits

Descendants of a revoked commit can still contribute if their own authors'
permissions allow them. For example, in Automerge we can treat revoked commits
as deleted while still applying allowed descendant operations. The CRDT decides
what that means for the document.

### Viewpoints

A `ViewPoint` captures only two things:

* The history heads, identifying the selected causally complete commit history.
* An owned snapshot of the per-author write permissions.

The captured hashes are heads, not an enumeration of the whole history.
`ViewPoint::from_parts(heads, write_permissions)` reconstructs this owned
description without validating it. There is no authentication-frontier field.

A commit contributes if it is in the selected history and its author's write
permission allows it. Unbounded permission only satisfies the last check: it
doesn't include future commits or commits outside that history. Authentication
is enforced at admission, not as another participation predicate.

Proofs of membership in a write frontier are evaluated using only the history
selected by the history heads, not subsequently delivered commits or received
items still waiting. Frontier heads that are unknown or outside that history
grant nothing. Signing may change the heads selected by a new signed viewpoint,
but cannot change the history or participation of an already captured
viewpoint.

A viewpoint contains enough information to determine the set of contributing
commits regardless of future changes to the write permissions, commit history,
or available signatures. This means we can define diffs in terms of viewpoints.

`changes(&cursor, &target)` captures the currently delivered history. It yields
one `Change` per commit that was not delivered to this materializer before, or
whose participation differs between the cursor's viewpoint and `target`,
parents first. `next_changes(&mut changes, &mut out, max)` appends up to `max`
records and returns `Batch::More` until it returns `Batch::Complete(cursor)`.
Records carry metadata only: `hash`, `payload` (the logical payload ID),
`newly_delivered`, and `visibility`. Payload bytes are fetched separately, for
example with `Graph::payload(&hash)` or `Graph::decode_item`, which check them
against the commits. The cursor records the
delivery heads separately from the materialized viewpoint, so historical reads
do not rewind delivery; persist it (`to_bytes`) atomically with the materialized
state. Comparing viewpoints determines participation changes, including changes
to previously delivered commits when newly selected history supplies frontier
proofs. The CRDT uses these records to produce document-level changes.

#### Working and Signed Viewpoints

We use the same representation for two views:

* `view_point(&permissions)` selects all delivered history, including our own
  drafts whose signatures are pending.
* `signed_view_point(&permissions)` selects the admitted heads. Its heads and
  history can differ from the working viewpoint. Admitted ancestors underneath
  drafts are retained; simply filtering the working heads would incorrectly
  lose them.

Local materialization uses the working viewpoint without waiting for signing.
Comparing it with the signed viewpoint shows participation changes awaiting
signatures. Comparing their selected histories identifies pending commits even
when their effects aren't visible, such as an overwritten assignment. The signed
history is parent-closed but needn't be a single linear prefix: different
branches may get signed at different times.

When a signature arrives, admitting already delivered drafts lets the signed
view catch up without changing working participation. Admission can also let
waiting received items proceed, adding delivered history that may change
participation in newly captured views. Captured viewpoints stay unchanged. To
preserve historical signing status, save both viewpoints; the working one alone
doesn't record which commits were admitted then.

#### Comparing Viewpoints

`diff(&before, &after)` requires every history head in both viewpoints to be
delivered; an unknown head returns `Error::UnknownCommit(hash)`, regardless of
write permissions. Draft heads are accepted, allowing local materialization and
diffs before signing without making those drafts admitted. Signing changes
neither a captured viewpoint's meaning nor its comparability on a history that
already has its commits.

Calculating participation is separate from validating admission. `diff` checks
history and write permissions only, not admission or signatures. Callers
requiring admitted history can check `is_admitted` for each head; since
admission is parent-closed, that also establishes admission of the selected
ancestors.

For a single-view membership query, prepare `diff(&view, &view)`. On the
resulting `ViewDiff`, `visibility(&commit)` returns `Vis::Both` for a
contributing commit and `Vis::Neither` for a known non-contributing commit. An
unknown commit is `Error::UnknownCommit`, not proof of invisibility.
`changed_commits()` lists the commits whose participation differs, parents
first.

Signature evidence outside the selected history may help admit a commit, but
cannot expand a captured viewpoint or supply a write-frontier bound outside its
history. Missing history heads prevent the comparison rather than making
selected commits invisible.

Any process with the selected history delivered obtains the same commit
participation, regardless of arrival order or additional local knowledge. This
confers process independence without requiring a shared authorization log or DAG.

#### Testing Pending Local Signatures

Tests should hold back a local signature: the edit appears in the working view,
but not the signed view or an export. Their diff shows the pending edit. After
signing, the signed view catches up and export is allowed, without another
change to the working document. Both earlier viewpoints must still mean the
same thing. Also check that admitted ancestors under drafts remain in the signed
history, that a draft cannot supply a permission bound there, and that
exporting a descendant cannot leak an unsigned local ancestor.

## API

In this sketch, `Log`, `OperationCodec`, and `handle` are application-specific.
The codec owns payload encoding and decoding; Stockfoil checks canonical bytes,
verifies signatures, and admits. One update path serves cold starts, local
edits, signing, receipt, and permission changes.

```rust,ignore
use std::collections::HashMap;
use stockfoil::{
    Admission, Batch, Driven, Error, Graph, Heads, MaterializationCursor, MemoryStorage,
    ViewPoint, Vis, WritePermission, WritePermissions,
};

type G = Graph<OperationCodec, MemoryStorage>;

/// Bring `log` and `cursor` from the cursor's view to `target`.
fn materialize(
    graph: &mut G,
    log: &mut Log,
    cursor: &mut MaterializationCursor,
    target: &ViewPoint,
) -> Result<(), Error> {
    let mut changes = graph.history().changes(cursor, target)?;
    let mut batch = Vec::new();
    loop {
        batch.clear();
        let progress = graph.history().next_changes(&mut changes, &mut batch, 256)?;
        for change in &batch {
            if change.newly_delivered {
                // Retain new operations, initially inactive, even if currently denied.
                let bytes = graph.payload(change.payload)?;
                log.add_operations(change.hash, &bytes, false);
            }
            match change.visibility {
                Vis::After => log.set_commit_visibility(change.hash, true),
                Vis::Before => log.set_commit_visibility(change.hash, false),
                Vis::Both | Vis::Neither => {}
            }
        }
        if let Batch::Complete(next) = progress {
            *cursor = next;
            return Ok(());
        }
    }
}

let mut graph = Graph::new(OperationCodec, MemoryStorage::new());
let mut permissions: WritePermissions = HashMap::from([
    (alice, WritePermission::Unbounded),
    (bob, WritePermission::Unbounded),
]);
let mut cursor = MaterializationCursor::new();
let mut waiting: Vec<Admission> = Vec::new();

// A local edit is a delivered draft, visible in the working view at once.
let parents = graph.history().heads(Heads::Delivered).collect();
let draft = graph.commit(alice, parents, operation.encode().as_slice())?;
request_signature(draft.signing_bytes());
let target = graph.history().view_point(&permissions);
materialize(&mut graph, &mut log, &mut cursor, &target)?;

// Later, the signer answers. This admits the draft (and any drafts it
// completes) for export; working participation does not change.
graph.sign(alice, draft.hash, signature)?;

// Received items need not arrive in causal order. A waiting admission keeps
// its verified state; retry it whenever the history's epoch has changed.
let blob = graph.storage_mut().insert(bytes);
let mut op = graph.begin_admit();
match graph.drive(&blob, &mut op)? {
    Driven::Done(_receipt) => {}
    Driven::Waiting(_dependency) => waiting.push((blob, op)),
}
let mut epoch = None;
while epoch != Some(graph.history().epoch()) {
    epoch = Some(graph.history().epoch());
    for (blob, mut op) in std::mem::take(&mut waiting) {
        match graph.drive(&blob, &mut op) {
            Ok(Driven::Waiting(_)) => waiting.push((blob, op)),
            Ok(Driven::Done(_)) => {}
            Err(e) => handle(e),
        }
    }
}
let target = graph.history().view_point(&permissions);
materialize(&mut graph, &mut log, &mut cursor, &target)?;

// Revoke Alice at these heads. Bob stays unbounded. No new delivery is needed.
permissions.insert(alice, WritePermission::Bounded(revocation_heads));
let target = graph.history().view_point(&permissions);
materialize(&mut graph, &mut log, &mut cursor, &target)?;
```

For an admitted-only view, pass `signed_view_point(&permissions)` instead of
`view_point(&permissions)`. Drafts are still reported once as newly delivered
(with `Vis::Neither`), so later signing changes only their visibility and
operations are not stored twice. For a historical read that must not change
live state, copy the materializer together with its cursor, then update the
copy to the requested viewpoint. The MV-register example
(`examples/mv-register`) uses exactly this approach.

Hosts without a native codec use `History` directly: `begin_commit` and
`publish_commit` for local commits, and `History::receive` with
`Admission::next`, `Admission::reply`, and `History::advance` to answer read,
decode, and bind requests. Raw traversal (`traverse`, `traverse_since`) and
participation-only `diff` queries remain available for inspection or
specialized consumers, but are not needed for this update loop.
