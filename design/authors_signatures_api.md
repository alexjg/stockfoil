# Auth and revocation: API sketch

This is the implementation sketch for
[Authors, Signatures, and Write Frontiers](authors_signatures_revocation.md).
The in-memory graph and MVR demo implement this API. This is an experimental
sketch: no compatibility APIs or format migrations.

`CommitGraph<C: PayloadCodec>` directly owns canonical hashing, encoded exports,
byte ingestion, typed payloads, and graph history; see [`CODEC.md`](../CODEC.md).
Structured insertion, admission, and export are private helpers on this same
graph, not a separate public API. No caller-side payload binding is required.

## Commit metadata and signatures

* Every commit has an author in its hashed contents. `CommitMeta::author()`
  returns `&Author`; `payload_hash()` exposes the payload hash.
  `CommitGraph<C>` binds received payloads to this hash through its codec,
  requires exact standalone canonical re-encoding, and reconstructs fragment
  hashes before verifying signatures. The MVR codec uses canonical BCS
  assignments; its transport adapter passes opaque bytes to Stockfoil.
* Remove signatures from `CommitContents`, `CommitMeta`, and the hashed encoding.
  Deliver them separately as `(author, commit_hash, signature)`.
* `commit(author, payload)` canonically encodes a payload and inserts an authored
  draft. Parents are the graph heads, including unsigned drafts and commits that
  don't currently contribute. `describe_commit(author, &payload)` computes the
  same metadata without inserting it. Preparation fixes the author, parents,
  payload, and hash before any signing IO.
* Private local insertion requires every parent to be already delivered. Parents
  may be unsigned local drafts; they need not be admitted. It immediately delivers
  the prepared commit as `pending_local`, advancing working heads but leaving it
  unpublished. `pending_local` means delivered but unsigned/unadmitted local work
  only, never work awaiting causal dependencies. Byte ingestion cannot introduce
  unauthenticated drafts through this path.
* Add `ingest_signature(author, hash, signature) -> Result<IngestReport, SignatureError>`
  for known, already delivered pending local commits only; a signature cannot be
  ingested before its local commit. Verify real Ed25519 signatures; verified
  coverage releases covered local commits from `pending_local` for admission and
  publication, while preserving parent closure. Invalid signatures do neither.
  Received items are verified before graph ingestion; verified items awaiting
  admitted dependencies are queued inside `CommitGraph`, separately from local
  drafts. Unverified input stays outside the graph, and neither kind of received
  input goes through this local signature API. Older signatures are pruned once a
  covering signature and its ancestry are publishable; until then we retain the
  older proof so exported
  history can still be authenticated.
* Give `Author` equality, ordering, hashing, and debug output so it works as a
  map key. Keep signing keys outside Stockfoil.

The flow is prepare, show locally and request signing, then publish once signed.
The application owns the signing IO and pending requests; Stockfoil stays
synchronous. A later frontier signature may cover a batch of local commits. We
don't need a signature per keystroke or a resolver API listing pending requests.
Signing never grants write permission.

`CommitGraph::commit(author, payload)` combines canonical encoding and draft
insertion, returning a fallible `Draft` containing hash, signing bytes, and report.
An author is mandatory even when signatures haven't arrived yet.

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
* `CommitGraph` owns a queue of verified received items awaiting admitted
  dependencies. Queued items own their payloads but are not delivered, admitted,
  materialized, or exportable. They affect neither heads nor viewpoints nor raw
  delivery traversals. Local drafts remain separate delivered history and do not
  satisfy received admission dependencies. Range boundaries and external parents
  derived from member metadata must all be admitted before an item can enter
  delivered history, parents-first. Normalized range placement is then checked
  against that admitted history. Every insertion remains causally ready.
* Add `WritePermission::{Unbounded, Bounded(Vec<CommitHash>), Denied}` and
  `set_write_permissions(&HashMap<Author, WritePermission>)`. This replaces the
  whole map; missing authors are denied. Snapshot the input, don't borrow it.
* Internal cover selection chooses authenticated transport items;
  `export_changes` supplies their encoded bytes and transport metadata. Structured
  verification types and structural summaries remain private. Pending local work and queued received
  items are excluded. A
  descendant export must not leak unsigned local ancestors. Publication is
  independent of the working heads and document visibility. Actual transport
  evidence is retained independently for export and verification.
* `CommitGraph::merge(&other)` exchanges encoded items through the receiver's
  codec and limits, verifies them, and admits them as dependencies become ready,
  draining the destination's queue too. It returns a report or a `MergeError`
  preserving partial progress.
  The sender's queue, local drafts, and write permissions do not travel through
  merge. Cloning a graph, in contrast, clones its queued items and owned payloads.
  Receiving data must not grant access. Revoked, admitted commits remain
  available for delivery and sync.

Both examples sign their local commits and materialize through `visit_changes`.
The OR-set uses delivery only; the MVR also exercises participation and explicit
permission grants.

## Received queue and operation reports

`CommitGraph::ingest(&[u8]) -> Result<IngestReport, Error<C::Error>>` accepts
encoded items in any arrival order. It decodes, reconstructs canonical hashes,
and verifies signatures before private admission. Missing dependencies are not
an error. Per-item decode limits do not impose aggregate queue quotas.

Application decoding runs before mutation, even when an item must wait. The graph
owns the returned payloads. Exact queued duplicates reuse retained payloads,
though each encoded input still undergoes decoding and verification; distinct
member sets or signature evidence claiming the same `ItemId` are not conflated.
Codec external side effects are not rolled back. Existing delivered payloads
always win, including copies delivered by local insertion or another item after
queueing.

A directly received, ready fragment with invalid placement returns an admission
error carrying `IngestError::InvalidFragment` before graph/queue mutation. For a queued
fragment, placement can only be checked once its dependencies are admitted. If
invalid then, it is removed and reported as a per-item rejection by the operation
that unblocked it, without delivering any of its members. Successful dependencies
and independent ready items remain admitted; there is no batch rollback.
Dependents of the rejected item remain pending unless another certificate admits
the commits they need.

Dependency-indexed waiters wake on every admitted commit, including all members
of an admitted fragment and admissions caused by local signing. Ready received
items and newly certifiable local work drain automatically until neither can make
progress; callers need not resubmit queued items.

Received ingestion and local signing return `Result<IngestReport, ...>`; local
commits return a fallible `Draft` containing `Draft::report`, and encoded native
merges return `Result<IngestReport, MergeError<C::Error>>`. A failed
merge retains earlier admissions/rejections in `MergeError::report` alongside
its typed `error`. Inspect the report on every operation that can admit history,
including partial merge failures:

* `admitted: Vec<ItemId>` lists admitted certificates, including duplicate and
  local certificates, not newly delivered commits. Use `visit_changes` for
  materialization.
* `rejected: Vec<IngestRejection>` lists deferred failures, each with
  `item: ItemId` and `error: IngestError`.
* `pending: usize` counts received items still queued after the call, excluding
  local drafts.

`pending_items() -> usize` exposes the same count; exact queued duplicates count
once. `missing_dependencies()` iterates distinct unadmitted prerequisite hashes
in unspecified order. These may already be delivered as unsigned local drafts.

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

Each callback receives a `MaterializationChange<'_, C::Payload>` with:

* `meta: &CommitMeta` and `payload: &C::Payload`, borrowed without cloning.
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
local drafts, and the requested viewpoint, even if no callbacks ran. Queued
received items are excluded until admission delivers them. Keep it
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
view; `src/commit_graph/tests/orset/mod.rs` shows the simpler delivery-only case
using the same materialization API with an internal fixture codec.

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
* `diff(&ViewPoint, &ViewPoint) -> Result<ViewPointDiff<'_, C>, DiffError>`
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
enough. Save both viewpoints if we need historical signing status. Admitting
already delivered drafts alone changes newly captured signed heads, not working
participation. Signing can also unblock received items, delivering new history
that may change participation in newly captured views. Neither effect changes
the meaning of either previously captured viewpoint.

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
Subduction demo uses Stockfoil's [shared v1 envelope](codec-wire-v1.md), with an
application codec for BCS assignments and key-dictionary/run fragment compression;
see `examples/mv-register/README.md`. Its document API is:

* `new()` and `author()` create and identify a replica, initially permitted to
  read its own writes.
* `prepare_set(key, value)` makes a local draft; `publish()` signs pending work.
  `set(key, value)` does both. These operations return `eyre::Result`.
* `set_write_permissions(...)` changes participation without discarding history.
* `view_point()`, `get(key)`, and `get_at(view, key)` support current and historical
  reads.
* `merge(&other)` transfers authenticated history, not permissions or drafts.
  `fork()` creates a new key, copies permissions, and merges that history.
  Both are fallible; merge refreshes materialization even on partial failure.

The register integration tests live in `examples/mv-register/tests` and cannot
inspect the private graph. They cover
concurrent register values, revocation restoring overwritten values, and one
prepare/publish/relay flow preserving historical drafts. Additional tests cover
denied delivery followed by permission grants, late frontier proofs, and isolated
historical reads. The internal OR-set fixture in
`src/commit_graph/tests/orset/mod.rs` uses `visit_changes` as well, processing only
`newly_delivered` events and retaining surviving add IDs.
It intentionally models delivery only, not revocation: its operations ignore
`visibility`. Neither example needs raw traversal or a separate participation diff.

Signature errors, causal admission, fragment certificates, and viewpoint diffs
are tested directly in the graph suites rather than repeated through the examples.
The sedimentree structural oracle runs causal, reverse, and shuffled schedules
with two seeds by default; `SEDIMENTREE_TEST_SEED` and `SEDIMENTREE_TEST_CASES`
allow longer reproducible runs without slowing the normal suite.
