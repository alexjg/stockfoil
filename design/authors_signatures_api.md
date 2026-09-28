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
  value. The MVR demo hashes its local assignments using BCS. Its example-only
  Subduction adapter reconstructs hashes from exact received payload bytes,
  verifies the original author signatures, then decodes assignments.
* Remove signatures from `CommitContents`, `CommitMeta`, and the hashed encoding.
  Deliver them separately as `(author, commit_hash, signature)`.
* Add an authored commit builder, roughly `commit_by(author, payload)`. Parents
  are the graph heads, including unsigned drafts and commits that don't
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

* `visit_changes` is the standard materialization entry point, combining delivery
  and participation changes. The lower-level `heads`, `traverse`, and
  `traverse_since` APIs expose causally delivered history, including local drafts.
  Received items must be verified before admission and delivery; permissions
  don't gate delivery.
  Admitted commits are exactly delivered commits not marked `pending_local`.
  Their history is verified and parent-closed by invariant, so `CommitGraph`
  needs neither a separate authenticated set nor per-author
  `signature_frontiers`.
* `CommitGraph` has no causal queue. Received verified items first check that
  external dependencies are admitted, then insert their commits parents-first;
  every insertion is causally ready by invariant. Range boundaries and external
  parents derived from member metadata must all be admitted. Admission also checks
  normalized range placement against their history before payload callbacks or mutation.
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

Both examples sign their local commits and materialize through `visit_changes`.
The OR-set uses delivery only; the MVR also exercises participation and explicit
permission grants.

## Incremental materialization

Use `CommitGraph::visit_changes(&cursor, &after, callback)` as the standard way
to build or update a CRDT view. It combines delivery and participation changes in
one callback and returns `Result<MaterializationCursor, DiffError>`. Use the same
path after local edits, signing, receipt/merge, or permission changes, rather than
coordinating a delivery traversal with a separate diff.

Start an empty materializer with `MaterializationCursor::new()` (or `default()`).
The cursor owns both the previously processed delivery heads and the previous
viewpoint. These are deliberately separate: a historical or signed viewpoint may
exclude delivered commits that the materializer has already stored. Read-only
`delivery_heads()` and `view_point()` accessors expose the snapshots.

Each callback receives a `MaterializationChange<'_, P>` with:

* `meta: &CommitMeta` and `payload: &P`, borrowed without cloning.
* `newly_delivered: bool`, indicating that the commit lies outside the cursor's
  processed delivery history. Store this data even when it does not participate.
* `visibility: Vis`, classifying participation in the old and requested views.

A commit is visited once if newly delivered or if its participation changed,
never twice when both conditions hold. Newly delivered denied commits are visited
with `Vis::Neither`. Already stored commits whose participation is unchanged are
omitted. Additions and removals are visited in ancestors-before-descendants order,
including ancestry through omitted commits; concurrent ordering is unspecified.

```rust
let next_cursor = graph.visit_changes(&cursor, &after, |change| {
    if change.newly_delivered {
        log.add_operations_from(change.meta, change.payload, false);
    }
    match change.visibility {
        Vis::After => log.set_commit_visibility(*change.meta.hash(), true),
        Vis::Before => log.set_commit_visibility(*change.meta.hash(), false),
        Vis::Both | Vis::Neither => {}
    }
})?;
cursor = next_cursor;
```

The returned cursor captures all currently delivered history, including pending
local drafts, and the requested viewpoint, even if no callbacks ran. Keep it
with the successfully updated materializer state. The input cursor is not mutated;
reusing it replays changes. The callback runs to completion with no early-break
protocol, and its side effects are not rolled back if it panics.

All history heads in the cursor and target viewpoint are validated before any
callback. A missing head returns `DiffError::MissingCommit`; unknown permission
bounds grant nothing. A cursor can be reused on another graph with its delivered
history, regardless of graph-local chain assignments, provided the caller retains
the corresponding materializer state. Admission and signatures are not checked
by this traversal.

This remains a commit-level update stream, not a document patch. For example,
revoking an overwriting assignment may restore an older value without revisiting
that older commit. Keep the operations needed to recompute such effects.
`examples/mv-register/src/lib.rs` demonstrates this with a cached materialized
view; `tests/orset/mod.rs` shows the simpler delivery-only case using the same API.

## Viewpoints and lower-level queries

`ViewPoint` is an owned, cloneable description containing only `heads`
and `write_permissions`. Give it a `heads() -> &[CommitHash]` accessor
and a read-only `write_permissions()` map.
`ViewPoint::from_parts(heads, write_permissions)` reconstructs the
snapshot after transport or storage; it doesn't validate its claims. The
viewpoint survives graph mutation and can be compared elsewhere once its history
has been delivered.

Working and signed viewpoints have the same shape but can select different
heads and histories. No authentication frontier or extra local-changes field is
needed. Authentication is an admission check, not a participation predicate;
actual transport signature evidence is kept independently of viewpoints.

Select a target using `view_point()` or `signed_view_point()`, then pass it to
`visit_changes`. For specialized queries that do not maintain materialized state,
`diff` remains available as a participation-only primitive:

* `view_point() -> ViewPoint` captures all delivered history, including local
  drafts, and the current write permissions.
* `signed_view_point() -> ViewPoint` captures the maximal admitted commits as
  heads and the current write permissions. Admitted ancestors underneath drafts
  are retained, not lost by merely filtering the working heads.
* `diff(&ViewPoint, &ViewPoint) -> Result<ViewPointDiff<'_, P>, DiffError>`
  calculates participation from delivered graph history and write permissions,
  including pending local drafts. A missing history head in either viewpoint
  yields `DiffError::MissingCommit(hash)`, even under total permission denial.
  Admission and signatures are not checked.

Diffing isn't signature verification. Any viewpoint whose history is delivered
can be compared, so local editing doesn't wait for signing IO. Callers requiring
admitted history can check each head with `has_commit`; parent-closed admission
then guarantees that the selected ancestors are admitted too.

Queries on the prepared diff:

* `ViewPointDiff::visibility(&CommitHash) -> Result<Vis, QueryError>`, where
  `Vis` is `Before`, `After`, `Both`, or `Neither`.
* `ViewPointDiff::changed_commits()` iterates `CommitDiff` values with
  `commit_id` and `visibility_delta`. The latter is `VisDiff::Before` or
  `VisDiff::After`; unchanged commits never appear. Each changed commit appears
  once, in topological (ancestors-before-descendants) order for both additions
  and removals, including ancestry through omitted commits. Concurrent commits
  have unspecified relative order, which may differ between replicas.

A known commit outside a viewpoint's history is invisible there. An unknown
hash is a query error, not proof of invisibility. Permission bounds are evaluated
only within the selected history: a pending draft cannot serve as a bound in a
signed viewpoint that excludes it. Unknown permission bounds grant nothing;
only missing history heads prevent diff construction. Pending local heads are
valid for diffing, and later signing changes neither their selected history nor
participation. For a single-view membership query, use `diff(&view, &view)`:
`Vis::Both` means visible and `Vis::Neither` means invisible.

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

The implementation lives in `examples/mv-register/src/lib.rs`. Its materializer
owns a cursor, all delivered assignments, their participation, and cached values.
One `visit_changes` callback stores new assignments and updates participation,
then affected keys are recomputed from the retained writes. `get` reads the cache.
Local edits, signing, permission changes, merge, and network ingestion all call
that same update path.

`get_at` clones the materialized state together with its cursor and updates that
private copy to the requested viewpoint. Historical reads therefore use the same
materialization logic without changing live state or redelivering stored writes.
They return `DiffError` if a history head is missing. Delivered working snapshots
remain readable before signing; admission is not validated on reads. Merge still
transfers only authenticated history.

The example owns its signing key and has no external-signer wrapper. Its
Subduction demo uses a private versioned BCS wire envelope, not a stable Stockfoil
format; see `examples/mv-register/README.md`. Its document API is:

* `new()` and `author()` create and identify a replica, initially permitted to
  read its own writes.
* `prepare_set(key, value)` makes a local draft; `publish()` signs pending work.
  `set(key, value)` does both.
* `set_write_permissions(...)` changes participation without discarding history.
* `view_point()`, `get(key)`, and `get_at(view, key)` support current and historical
  reads.
* `merge(&other)` transfers authenticated history, not permissions or drafts.
  `fork()` creates a new key, copies permissions, and merges that history.

The register integration tests live in `examples/mv-register/tests` and cannot
inspect the private graph. They cover
concurrent register values, revocation restoring overwritten values, and one
prepare/publish/relay flow preserving historical drafts. Additional tests cover
denied delivery followed by permission grants, late frontier proofs, and isolated
historical reads. The OR-set example in `tests/orset/mod.rs` uses `visit_changes`
as well, processing only `newly_delivered` events and retaining surviving add IDs.
It intentionally models delivery only, not revocation: its operations ignore
`visibility`. Neither example needs raw traversal or a separate participation diff.

Signature errors, causal admission, fragment certificates, and viewpoint diffs
are tested directly in the graph suites rather than repeated through the examples.
The sedimentree structural oracle runs causal, reverse, and shuffled schedules
with two seeds by default; `SEDIMENTREE_TEST_SEED` and `SEDIMENTREE_TEST_CASES`
allow longer reproducible runs without slowing the normal suite.
