# Auth and revocation: API sketch

This is the implementation sketch for
[Authors, Signatures, and Write Frontiers](authors_signatures_revocation.md).
The in-memory graph and MVR demo implement this API. This is an experimental
sketch: no compatibility APIs or format migrations.

## Commit metadata and signatures

* Every commit has an author in its hashed contents. `CommitMeta::author()`
  returns `&Author`; `payload_hash()` exposes the payload hash. `CommitGraph<P>`
  cannot encode an arbitrary `P`: callers must bind received payloads to this
  hash before ingestion or use. Signature
  verification authenticates the envelope and hash, not an unchecked decoded
  value. The MVR receive path checks its canonical BCS bytes before storing them.
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
  publication. Invalid signatures do neither. Older signatures are pruned once a
  covering signature and its ancestry are publishable; until then we retain the
  older proof so exported history can still be authenticated.
* Give `Author` equality, ordering, hashing, and debug output so it works as a
  map key. Keep signing keys outside Stockfoil.

The flow is prepare, show locally and request signing, then publish once signed.
The application owns the signing IO and pending requests; Stockfoil stays
synchronous. A later frontier signature may cover a batch of local commits. We
don't need a signature per keystroke or a resolver API listing pending requests.
Signing never grants write permission.

`commit_by(author, payload)` is the commit builder. An author is mandatory even
when signatures haven't arrived yet.

## Delivery and permissions

* `delivered_heads`, `traverse_delivered`, and `traverse_delivered_since` expose
  causally delivered history. Authentication and permissions don't gate delivery.
* Add `WritePermission::{Unbounded, Bounded(Vec<CommitHash>), Denied}` and
  `set_write_permissions(&HashMap<Author, WritePermission>)`. This replaces the
  whole map; missing authors are denied. Snapshot the input, don't borrow it.
* `minimal_cover` selects authenticated transport items; `export` supplies their
  metadata and stored signature evidence. Pending local work is excluded. A
  descendant export must not leak unsigned local ancestors. Publication is
  independent of the working frontier and document visibility. See
  [verified ingestion](verified_ingestion_api.md) for the transport contract.
* `merge` exchanges those items, verifies them, and admits them in dependency
  order. Write permissions and unverified queues do not travel through merge.
  Receiving data must not grant access. Revoked, admitted commits remain
  available for delivery and sync.

Graph tests and the delivery-only OR-set example use authored commits without
signatures. The MVR demo uses signatures and explicit permission grants.

## Viewpoints and diffs

`ViewPoint` is an owned, cloneable description containing delivered heads,
write permissions, and authentication frontiers. Those frontiers say which commits
the viewpoint treats as authenticated, not whether signatures are available.
Give it a `delivered_heads() -> &[CommitHash]` accessor, plus read-only
`write_permissions()` and `authentication_frontiers()` maps. `ViewPoint::from_parts`
reconstructs the description after transport or storage; it doesn't validate its
claims. The viewpoint survives graph mutation and can be resolved elsewhere once enough
signature evidence is available.

Working and signed viewpoints have the same shape. The working frontier also
covers our own prepared commits pending signing; the signed frontier uses only
verified coverage. No extra local-changes field is needed. The graph must keep
actual signature evidence separate from the working frontier.

Proposed graph methods:

* `view_point() -> ViewPoint` captures the current working state.
* `signed_view_point() -> ViewPoint` captures the same delivered history and
  write permissions, but with signature-backed authentication frontiers.
* `resolve(&ViewPoint) -> Result<ResolvedViewPoint, ResolveError>` checks
  signature evidence and prepares repeated participation queries. It always
  requires actual signatures; pending ones aren't a substitute. It doesn't
  return a signing-task list or offer a "trust this frontier" flag.
* `diff(&ViewPoint, &ViewPoint) -> Result<ViewPointDiff, DiffError>`
  calculates participation under both descriptions, without requiring signatures
  to have arrived. Missing graph data can still prevent the comparison.

Diffing isn't signature verification. Local working viewpoints can be used
immediately, but a received viewpoint must pass `resolve` before driving a
materialized view. Keep participation calculation separate from that check so
local editing doesn't wait for signing IO.

Queries on the prepared objects:

* `ResolvedViewPoint::visible(&CommitHash) -> Result<bool, QueryError>`.
* `ViewPointDiff::visibility(&CommitHash) -> Result<Vis, QueryError>`, where
  `Vis` is `Before`, `After`, `Both`, or `Neither`.
* `ViewPointDiff::changed_commits()` iterates `CommitDiff` values with
  `commit_id` and `visibility_delta`. The latter is `VisDiff::Before` or
  `VisDiff::After`; unchanged commits never appear. Each changed commit appears
  once. Leave traversal order unspecified for now.

A known commit outside a viewpoint's history is invisible there. An unknown
hash is a query error, not proof of invisibility. Newer signatures may validate
an older captured authentication frontier without expanding it. Missing graph
or signature evidence is a resolution error, not a change to the viewpoint's
meaning. A working viewpoint can exist and be used locally before that evidence
is available.

Comparing working and signed viewpoints gives the document changes awaiting
signatures. Also allow comparison of their captured per-author authentication
frontiers: pending commits may have no visible effect, so document patches alone
aren't enough for signing-status UI. Save both viewpoints if we need historical
signing status. Signature arrival may leave the working state unchanged while the
signed state catches up; neither captured viewpoint changes.

A commit diff isn't a document patch. A commit can participate in both views
while its visible value changes because another commit stopped overwriting it.

## MVR demo

The graph retains every delivered assignment. The MVR selects the causally
maximal *participating* assignments for each key. Revoking an assignment removes
both its value and its overwriting effect. An allowed
assignment still overwrites its ancestors through revoked intermediate commits.

The small demo rebuilds its state from retained assignments. Later we can use
the commit diff to pick affected keys. It also retains its locally captured viewpoints
so it can recognize historical local reads that don't yet have signatures.
In either case, compare viewpoints after local creation, commit ingestion,
signature ingestion, and permission updates, not just after explicit revocation.
The displayed state uses the working viewpoint; signing status compares it with
the signed one.

The implementation lives in `tests/mv_register/implementation.rs`. The tests are
sibling modules, so Rust privacy keeps them out of its graph and assignment store.
They exercise document reads, historical views, publication, and receive behavior
through the MVR API:

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
* `view_point()` and `signed_view_point()` capture working and signature-backed
  document views.
* `export_assignments()` returns messages with opaque metadata and encoded MVR
  payloads, withholding unsigned local work. `receive_assignment(message)` checks
  the payload hash, decodes the assignment, and ingests it as received data.
  Signatures travel separately; receipt alone doesn't make an assignment visible.
* `get_at(&ViewPoint, key) -> Result<BTreeSet<String>, ResolveError>` reads a
  captured viewpoint without changing the current one. Local captures can be
  read before signing; imported viewpoints must be resolved first.

`new()` generates a signing key and explicitly grants that author unbounded
access. This is demo setup, not a Stockfoil default. `fork()` creates a new
signing key, copies the old permission map, and imports only publishable history.
It doesn't copy unsigned local drafts or the source's local viewpoint exemptions. Tests
must grant the new author on both replicas before expecting its writes to
contribute. `merge()` transfers publishable commits and signature evidence, and
leaves local permissions alone.

## First test and follow-ups

`tests/mv_register/revocation.rs` and the signature-flow test run in the normal
smoke target against the MVR API, backed by the real graph and Ed25519 verification.

The revocation test covers inclusive bounds, restored overwritten values,
concurrent values, allowed descendants of revoked commits, default-deny, explicit
denial, empty bounds, restoring unbounded access, stable historical reads, and
reading a captured view on another replica.

`tests/view_point_diff.rs` tests the graph API directly: bidirectional and cross-branch
diffs, visibility classification, changed-commit enumeration, unchanged delivery
under revocation, strict resolution, and working-versus-signed participation.
Those assertions don't belong in the MVR tests.

`tests/mv_register/signatures.rs` exercises the split
prepare/sign flow. It creates an Ed25519 key from a fixed test seed and signs the
actual 32-byte commit hashes with `ed25519-dalek`. The test controls when those
signatures reach Stockfoil, so it can exercise delayed signing without fixed
signature fixtures or a second implementation of the commit encoding.

The flow test covers:

1. Capture the initial working and signed viewpoints.
2. Prepare a local edit without delivering its signature. It appears in the
   working document immediately; the signed view still shows the old value.
3. Read the working and signed views. Confirm another replica cannot receive the
   local assignment through export or merge, or read the captured pending view.
4. Deliver the real signature. The signed view catches up, export is allowed,
   and the working document doesn't change again.
5. Recheck the captured views: their values stay fixed. Another replica can now
   read the earlier working view using the published signature evidence.

It also checks a batch covered by one later signature, an overwritten pending
edit, a valid signature supplied for the wrong hash, duplicate delivery, and
holding back unsigned descendants and permissions changing while signing is
pending. The MVR receive tests also cover tampered assignment payloads and
receiving our own author identity without granting local trust. `tests/auth.rs`
covers a signed descendant by another author that mustn't leak an unsigned local
ancestor.

The graph-level tests in `tests/auth.rs` deliver commits and signatures separately
to cover:

* A remote signature arriving without new commits changes participation, not old
  viewpoints or delivered heads.
* A queued frontier commit proves nothing until all its parents are delivered;
  its delivery can activate older assignments without redelivering them.
* Invalid signatures, wrong signers, signature-before-commit, and duplicates.
* Resolving old viewpoints using newer signatures after pruning old ones;
  insufficient evidence reports an error.
* Multiple heads in both write and authentication frontiers, including branches
  signed by different authors.

Keep byte-format and signature-verification tests at the graph/metadata layer.
The MVR has no nested objects, so it cannot test hiding an object whose creation
was revoked; that needs a richer CRDT demo.
