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
  Preparation fixes the author, parents, payload, and hash before any signing IO.
* Add a local ingestion path, roughly `ingest_local_commit(meta, payload)`. It
  records our prepared commit, advances the working authentication frontier, and
  marks it unpublished. Ordinary incoming ingestion must not do this.
* Add `ingest_signature(author, hash, signature) -> Result<(), SignatureError>`.
  Verify real Ed25519 signatures. A signature can arrive before its commit;
  retain it until the graph can establish which ancestors it covers. Verified
  signatures advance the signed frontier and release covered local commits for
  publication. Invalid signatures do neither.
* Give `Author` equality, ordering, hashing, and debug output so it works as a
  map key. Keep signing keys outside Stockfoil.

The flow is prepare, show locally and request signing, then publish once signed.
The application owns the signing IO and pending requests; Stockfoil stays
synchronous. A later frontier signature may cover a batch of local commits. We
don't need a signature per keystroke or a resolver API listing pending requests.
Signing never grants write permission.

The old unsigned constructor needs an explicit migration decision. Anonymous
commits must not quietly bypass default-deny in the authenticated API.

## Delivery and permissions

* Rename `heads`, `traverse`, and `traverse_since` to `delivered_heads`,
  `traverse_delivered`, and `traverse_delivered_since`. Keep the current causal
  ordering and callback shape. Authentication and permissions don't gate delivery.
* Add `WritePermission::{Unbounded, Bounded(Vec<CommitHash>), Denied}` and
  `set_write_permissions(&HashMap<Author, WritePermission>)`. This replaces the
  whole map; missing authors are denied. Snapshot the input, don't borrow it.
* Add `traverse_publishable`, with the same callback shape as delivered traversal,
  for the export path.
  Withhold our unpublished local commits until covered by verified signatures.
  A descendant export must not leak unsigned local ancestors. Publication uses
  actual signature coverage, not the working frontier or document visibility.
* `merge` uses that export path to exchange commits and signature evidence, not
  write permissions. Receiving data must not grant access. Received commits can
  still be forwarded pending verification; the holdback is for unpublished local
  work. Revoked, published commits remain available for delivery and sync.

Existing graph tests can still use unsigned commits to test delivery. The demos
will need authored, signed commits and explicit grants in their test fixtures.

## Perspectives and diffs

`Perspective` is an owned, cloneable description containing delivered heads,
write permissions, and authentication frontiers. Those frontiers say which commits
the perspective treats as authenticated, not whether signatures are available.
Give it a `delivered_heads() -> &[CommitHash]` accessor. It must survive graph
mutation and can be resolved elsewhere once enough signature evidence is available.

Working and signed perspectives have the same shape. The working frontier also
covers our own prepared commits pending signing; the signed frontier uses only
verified coverage. No extra local-changes field is needed. The graph must keep
actual signature evidence separate from the working frontier.

Proposed graph methods:

* `perspective() -> Perspective` captures the current working state.
* `signed_perspective() -> Perspective` captures the same delivered history and
  write permissions, but with signature-backed authentication frontiers.
* `resolve(&Perspective) -> Result<ResolvedPerspective, ResolveError>` checks
  signature evidence and prepares repeated participation queries. It always
  requires actual signatures; pending ones aren't a substitute. It doesn't
  return a signing-task list or offer a "trust this frontier" flag.
* `diff(&Perspective, &Perspective) -> Result<PerspectiveDiff, DiffError>`
  calculates participation under both descriptions, without requiring signatures
  to have arrived. Missing graph data can still prevent the comparison.

Diffing isn't signature verification. Local working perspectives can be used
immediately, but a received perspective must pass `resolve` before driving a
materialized view. Keep participation calculation separate from that check so
local editing doesn't wait for signing IO.

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
or signature evidence is a resolution error, not a change to the perspective's
meaning. A working perspective can exist and be used locally before that evidence
is available.

Comparing working and signed perspectives gives the document changes awaiting
signatures. Also allow comparison of their captured per-author authentication
frontiers: pending commits may have no visible effect, so document patches alone
aren't enough for signing-status UI. Save both perspectives if we need historical
signing status. Signature arrival may leave the working state unchanged while the
signed state catches up; neither captured perspective changes.

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
In either case, compare perspectives after local creation, commit ingestion,
signature ingestion, and permission updates, not just after explicit revocation.
The displayed state uses the working perspective; signing status compares it with
the signed one.

Small demo API changes used by the contract test:

* `author() -> Author` exposes the replica's public signing key.
* `with_author(Author)` creates a replica whose signer lives elsewhere. It uses
  the public key as the assignment's actor ID; no private key is required.
* `prepare_set(key, value) -> CommitHash` creates and locally ingests an assignment
  without signing it. `ingest_signature(author, hash, signature)` delivers a
  signer response and refreshes the working and signed views.
* `set(key, value) -> CommitHash` remains a convenience that prepares a local
  assignment and signs it immediately with the demo's in-memory key. Split those
  steps in the fixture so tests can hold back real signatures while the local
  edit is already visible. Callers may still ignore the returned hash.
* `set_write_permissions(&HashMap<Author, WritePermission>)` updates the graph
  and reevaluates the document.
* `get_at(&Perspective, key) -> Result<BTreeSet<String>, ResolveError>` reads a
  captured perspective without changing the current one. Local captures can be
  read before signing; imported perspectives must be resolved first.

`new()` generates a signing key and explicitly grants that author unbounded
access. This is demo setup, not a Stockfoil default. `fork()` creates a new
signing key but copies the old permission map; tests must grant the new author
on both replicas before expecting its writes to contribute. `merge()` transfers
publishable commits and signature evidence, and leaves local permissions alone.

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

`tests/mv_register/signatures.rs` adds a second red contract test using the split
prepare/sign flow. It creates an Ed25519 key from a fixed test seed and signs the
actual 32-byte commit hashes with `ed25519-dalek`. The test controls when those
signatures reach Stockfoil, so it can exercise delayed signing without fixed
signature fixtures or a second implementation of the commit encoding.

The flow test covers:

1. Capture the initial working and signed perspectives.
2. Prepare a local edit without delivering its signature. It appears in the
   working document immediately; the signed view still shows the old value.
3. Diff working against signed and check the pending change. Confirm another
   replica cannot receive the local commit through export or merge, and that
   signature-backed resolution of the working perspective fails for now.
4. Deliver the real signature. The signed view catches up, export is allowed,
   and the working document doesn't change again.
5. Recheck the captured perspectives: their values and frontiers stay fixed.
   The earlier working perspective can now resolve against signature evidence.

It also checks a batch covered by one later signature, an overwritten pending
edit, a valid signature supplied for the wrong hash, duplicate delivery, and
holding back unsigned descendants. Still add a case where permissions change
while signing is pending, and one where a signed descendant by a different author
mustn't leak an unsigned local ancestor.

The MVR can expose delayed remote signatures and frontier proofs too. These need
a fixture that delivers commits and signatures separately. Add focused tests for:

* A remote signature arriving without new commits changes participation, not old
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
