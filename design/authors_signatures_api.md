# Auth and revocation: API sketch

This is the implementation sketch for
[Authors, Signatures, and Write Frontiers](authors_signatures_revocation.md).
Names and signatures below are proposals, not implemented APIs.

## Commit metadata and signatures

* Keep the author in the hashed commit contents; expose `CommitMeta::author()`.
* Remove signatures from `CommitContents`, `CommitMeta`, and the hashed encoding.
  Deliver them separately as `(author, commit_hash, signature)`.
* Add an authored commit builder, roughly `commit_by(author, payload)`. Parents
  are the delivered heads, including commits that don't currently contribute.
* Add `ingest_signature(author, hash, signature) -> Result<(), SignatureError>`.
  Verify real Ed25519 signatures. A signature can arrive before its commit;
  retain it until the graph can establish which ancestors it covers.
* Give `Author` equality, ordering, hashing, and debug output so it works as a
  map key. Keep signing keys outside Stockfoil.

The old unsigned constructor needs an explicit migration decision. Anonymous
commits must not quietly bypass default-deny in the authenticated API.

## Delivery and permissions

* Rename `heads`, `traverse`, and `traverse_since` to `delivered_heads`,
  `traverse_delivered`, and `traverse_delivered_since`. Keep the current causal
  ordering and callback shape. Authentication and permissions don't gate delivery.
* Add `WritePermission::{Unbounded, Bounded(Vec<CommitHash>), Denied}` and
  `set_write_permissions(&HashMap<Author, WritePermission>)`. This replaces the
  whole map; missing authors are denied. Snapshot the input, don't borrow it.
* `merge` exchanges commits and signature evidence, not write permissions.
  Receiving data must not grant access. Revoked commits remain available for
  delivery and sync.

Existing graph tests can still use unsigned commits to test delivery. The demos
will need authored, signed commits and explicit grants in their test fixtures.

## Perspectives and diffs

`Perspective` is an owned, cloneable description containing delivered heads,
write permissions, and authenticated frontiers. It must survive graph mutation.
Give it a `delivered_heads() -> &[CommitHash]` accessor; the same description can
be resolved by another graph with enough evidence.

Proposed graph methods:

* `perspective() -> Perspective` captures the current state.
* `resolve(&Perspective) -> Result<ResolvedPerspective, ResolveError>` prepares
  repeated participation queries. No silent fallback for missing evidence.
* `diff(&Perspective, &Perspective) -> Result<PerspectiveDiff, ResolveError>`
  resolves both sides and prepares their comparison.

Queries on the prepared objects:

* `ResolvedPerspective::visible(&CommitHash) -> Result<bool, QueryError>`.
* `PerspectiveDiff::visibility(&CommitHash) -> Result<Vis, QueryError>`, where
  `Vis` is `Before`, `After`, `Both`, or `Neither`.
* `PerspectiveDiff::changed_commits()` iterates `CommitDiff` values with
  `commit_id` and `visibility_delta`. The latter is `VisDiff::Before` or
  `VisDiff::After`; unchanged commits never appear. Each changed commit appears
  once. Leave traversal order unspecified for now.

A known commit outside a perspective's history is invisible there. An unknown
hash is a query error, not proof of invisibility. Newer signatures may validate
an older captured authentication frontier without expanding it. Missing graph
or signature evidence is a resolution error.

A commit diff isn't a document patch. A commit can participate in both views
while its visible value changes because another commit stopped overwriting it.

## MVR demo

The current `State` drops overwritten values. That won't work for revocation.
Keep every delivered assignment (the graph already retains its payload), and
select the causally maximal *participating* assignments for each key. Revoking
an assignment removes both its value and its overwriting effect. An allowed
assignment still overwrites its ancestors through revoked intermediate commits.

For the first implementation, rebuilding the small demo's state from retained
assignments is fine. Later we can use the commit diff to pick affected keys.
In either case, compare perspectives after commit ingestion, signature ingestion,
and permission updates, not just after explicit revocation.

Small demo API changes used by the contract test:

* `author() -> Author` exposes the replica's public signing key.
* `set(key, value) -> CommitHash` creates and signs an assignment; callers may
  ignore the returned hash as they do today.
* `set_write_permissions(&HashMap<Author, WritePermission>)` updates the graph
  and reevaluates the document.
* `get_at(&Perspective, key) -> Result<BTreeSet<String>, ResolveError>` reads a
  captured perspective without changing the current one.

`new()` generates a signing key and explicitly grants that author unbounded
access. This is demo setup, not a Stockfoil default. `fork()` creates a new
signing key but copies the old permission map; tests must grant the new author
on both replicas before expecting its writes to contribute. `merge()` transfers
signature evidence as well as commits, and leaves local permissions alone.

## First test and follow-ups

`tests/mv_register/revocation.rs` is a red contract test against these proposed
APIs. It is wired into the normal smoke test target, so that target deliberately
won't compile until the APIs and demo changes exist. No fake authentication or
reference implementation stands in for Stockfoil.

It covers inclusive bounds, restored overwritten values, concurrent values,
allowed descendants of revoked commits, default-deny, explicit denial, empty
bounds, restoring unbounded access, stable old perspectives, backwards and
cross-branch comparisons, and resolving a perspective on another replica.
Revocation must not change delivered heads or redeliver old commits.

The MVR can also expose delayed-signature and delayed-frontier-proof behavior,
but those need a lower-level fixture that delivers commits and signatures
separately. Add focused tests for:

* A signature arriving without new commits changes participation, not old
  perspectives or delivered heads.
* A queued frontier commit proves nothing until all its parents are delivered;
  its delivery can activate older assignments without redelivering them.
* Invalid signatures, wrong signers, signature-before-commit, and duplicates.
* Resolving old perspectives using newer signatures after pruning old ones;
  insufficient evidence reports an error.
* Multiple heads in both write and authentication frontiers, including branches
  signed by different authors.

Keep byte-format and signature-verification tests at the graph/metadata layer.
The MVR has no nested objects, so it cannot test hiding an object whose creation
was revoked; that needs a richer CRDT demo.
