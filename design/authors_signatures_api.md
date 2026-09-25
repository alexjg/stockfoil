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
  value. The MVR demo hashes its local assignments using BCS, but only merges
  in-memory replicas; it does not implement an untrusted payload decoder.
* Remove signatures from `CommitContents`, `CommitMeta`, and the hashed encoding.
  Deliver them separately as `(author, commit_hash, signature)`.
* Add an authored commit builder, roughly `commit_by(author, payload)`. Parents
  are the delivered heads, including unsigned drafts and commits that don't
  currently contribute, so normal prepared work meets the local ingestion
  precondition. Preparation fixes the author, parents, payload, and hash before
  any signing IO.
* Add a local ingestion path, roughly `ingest_local_commit(meta, payload)`. It
  asserts that every parent is already delivered before any mutation, panicking
  on misuse rather than buffering causally unready work. Parents may be unsigned
  local drafts; they need not be admitted. It immediately delivers our prepared
  commit as `pending_local`, advancing the working heads but leaving it
  unpublished. `pending_local` means delivered but unsigned/unadmitted local
  work only, never work awaiting causal dependencies. Ordinary incoming
  ingestion must not do this.
* Add `ingest_signature(author, hash, signature) -> Result<(), SignatureError>`
  for known, already delivered pending local commits only; a signature cannot be
  ingested before its local commit. Verify real Ed25519 signatures; verified
  coverage releases covered local commits from `pending_local` for admission and
  publication, while preserving parent closure. Invalid signatures do neither.
  Received items are verified before admission; missing dependencies and
  unverified items are buffered outside `CommitGraph`, not through this local
  signature API. Older signatures are pruned once a covering signature and its
  ancestry are publishable; until then we retain the older proof so exported
  history can still be authenticated.
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
  causally delivered history, including local drafts. Received items must be
  verified before admission and delivery; permissions don't gate delivery.
  Admitted commits are exactly delivered commits not marked `pending_local`.
  Their history is verified and parent-closed by invariant, so `CommitGraph`
  needs neither a separate authenticated set nor per-author
  `signature_frontiers`.
* `CommitGraph` has no causal queue. Received verified items first check that
  external dependencies are admitted, then insert their commits parents-first;
  every insertion is causally ready by invariant. Fragment boundaries and explicit
  additional dependencies must all be admitted. Admission also checks normalized
  range placement against their history before payload callbacks or mutation.
  All missing-dependency buffering stays outside the graph, for both local work
  and received items.
* Add `WritePermission::{Unbounded, Bounded(Vec<CommitHash>), Denied}` and
  `set_write_permissions(&HashMap<Author, WritePermission>)`. This replaces the
  whole map; missing authors are denied. Snapshot the input, don't borrow it.
* `minimal_cover` selects authenticated transport items; `export` supplies their
  metadata and stored signature evidence. Pending local work is excluded. A
  descendant export must not leak unsigned local ancestors. Publication is
  independent of the working heads and document visibility. Actual transport
  evidence is retained independently for export and verification.
* `merge` exchanges those items, verifies them, and admits them in dependency
  order. Write permissions and unverified queues do not travel through merge.
  Receiving data must not grant access. Revoked, admitted commits remain
  available for delivery and sync.

Both examples sign their local commits. The OR-set uses delivery only; the MVR
also exercises participation and explicit permission grants.

## Viewpoints and diffs

`ViewPoint` is an owned, cloneable description containing only `delivered_heads`
and `write_permissions`. Give it a `delivered_heads() -> &[CommitHash]` accessor
and a read-only `write_permissions()` map.
`ViewPoint::from_parts(delivered_heads, write_permissions)` reconstructs the
snapshot after transport or storage; it doesn't validate its claims. The
viewpoint survives graph mutation and can be resolved elsewhere once its history
has been admitted.

Working and signed viewpoints have the same shape but can select different
heads and histories. No authentication frontier or extra local-changes field is
needed. Authentication is an admission check, not a participation predicate;
actual transport signature evidence is kept independently of viewpoints.

Proposed graph methods:

* `view_point() -> ViewPoint` captures all delivered history, including local
  drafts, and the current write permissions.
* `signed_view_point() -> ViewPoint` captures the maximal admitted commits as
  heads and the current write permissions. Admitted ancestors underneath drafts
  are retained, not lost by merely filtering the working heads.
* `resolve(&ViewPoint) -> Result<ResolvedViewPoint, ResolveError>` validates that
  each head exists and is admitted, then prepares repeated participation queries.
  Heads absent from the graph yield `ResolveError::MissingCommit(hash)`;
  delivered pending local heads yield `ResolveError::PendingLocalCommit(hash)`.
  Parent closure guarantees that the selected history is admitted too. There is
  no separate signature-frontier coverage check, signing-task list, or "trust
  this frontier" flag.
* `diff(&ViewPoint, &ViewPoint) -> Result<ViewPointDiff, DiffError>` optimistically
  calculates participation from graph history and write permissions only,
  without checking admission or requiring signatures to have arrived. Missing
  graph data can still prevent the comparison.

Diffing isn't signature verification. Local working viewpoints can be used
immediately, but a received viewpoint must pass `resolve` before driving a
materialized view or being used in its diffs. Keep participation calculation
separate from that check so local editing doesn't wait for signing IO.

Queries on the prepared objects:

* `ResolvedViewPoint::visible(&CommitHash) -> Result<bool, QueryError>`.
* `ViewPointDiff::visibility(&CommitHash) -> Result<Vis, QueryError>`, where
  `Vis` is `Before`, `After`, `Both`, or `Neither`.
* `ViewPointDiff::changed_commits()` iterates `CommitDiff` values with
  `commit_id` and `visibility_delta`. The latter is `VisDiff::Before` or
  `VisDiff::After`; unchanged commits never appear. Each changed commit appears
  once. Leave traversal order unspecified for now.

A known commit outside a viewpoint's history is invisible there. An unknown
hash is a query error, not proof of invisibility. Permission bounds are evaluated
only within the selected history: a pending draft cannot serve as a bound in a
signed viewpoint that excludes it. Missing graph data or pending local heads are
resolution errors, not changes to the viewpoint's meaning. A working viewpoint
can exist and be used locally before its history is admitted; later signing can
make it resolvable without changing its selected history or participation.

Comparing working and signed viewpoints gives the participation changes awaiting
signatures. Compare their selected histories as well for signing-status UI:
pending commits may have no visible effect, so document patches alone aren't
enough. Save both viewpoints if we need historical signing status. Signing
changes newly captured signed heads, not the working document or the meaning of
either previously captured viewpoint.

A commit diff isn't a document patch. A commit can participate in both views
while its visible value changes because another commit stopped overwriting it.

## MVR demo

The graph retains every delivered assignment. The MVR selects the causally
maximal *participating* assignments for each key. Revoking an assignment removes
both its value and its overwriting effect. An allowed
assignment still overwrites its ancestors through revoked intermediate commits.

The implementation lives in `tests/mv_register/implementation.rs`. It computes
values on reads rather than maintaining a second state cache. Locally captured
working viewpoints remain readable before signing; imported viewpoints must
resolve against admitted history first.

The example owns its signing key and has no external-signer wrapper or wire
protocol. Its document API is:

* `new()` and `author()` create and identify a replica, initially permitted to
  read its own writes.
* `prepare_set(key, value)` makes a local draft; `publish()` signs pending work.
  `set(key, value)` does both.
* `set_write_permissions(...)` changes participation without discarding history.
* `view_point()`, `get(key)`, and `get_at(view, key)` support current and historical
  reads.
* `merge(&other)` transfers authenticated history, not permissions or drafts.
  `fork()` creates a new key, copies permissions, and merges that history.

The tests are sibling modules and cannot inspect the private graph. They cover
concurrent register values, revocation restoring overwritten values, and one
prepare/publish/relay flow preserving historical drafts. The OR-set example in
`tests/orset/mod.rs` instead applies each delivered operation once with
`traverse_since`, retaining only surviving add IDs.

Signature errors, causal admission, fragment certificates, and viewpoint diffs
are tested directly in the graph suites rather than repeated through the examples.
The sedimentree structural oracle runs causal, reverse, and shuffled schedules
with two seeds by default; `SEDIMENTREE_TEST_SEED` and `SEDIMENTREE_TEST_CASES`
allow longer reproducible runs without slowing the normal suite.
