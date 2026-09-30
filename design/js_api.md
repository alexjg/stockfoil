# JavaScript API guide

The [`stockfoil` package](../javascript) exposes one typed `CommitGraph<P>` through
Wasm. The native graph directly owns its codec, payloads, history, and received
queue; generated Wasm handles are private implementation details, not another
mutable graph API. Rust and Wasm share the [v1 envelope](codec-wire-v1.md), hash
reconstruction, signature verification, and admission rules.

## Codec and ownership

```ts
import { CommitGraph, ExportCursor, MaterializationCursor } from "stockfoil";

// registerCodec is an application PayloadCodec<Assignment> implementation.
const graph = new CommitGraph(registerCodec);
```

A `PayloadCodec<P>` supplies:

- `formatId`: trusted application format/version, never selected by incoming data.
- `encodeCommit(payload)`: deterministic canonical `Uint8Array` bytes.
- `decodeCommit(bytes, limits)`: reconstruct a payload with identical canonical bytes.
- `encodeFragment(shape, payloads)`: compress the complete batch together.
- `decodeFragment(shape, bytes, limits)`: return one payload per member in order.

Shape members are ancestors-first, with an author and ordered parent references
(`{ kind: "member", index }` or `{ kind: "external", hash }`). Parent order and
multiplicity are authenticated. Fragment packing must preserve each member's
canonical encoding; it need not be identical across replicas.

Callbacks are synchronous; Promise/thenable results are rejected. Payloads are
structured-cloned at ownership boundaries, including encoder inputs and returned
materialization batches. Shared memory and non-cloneable values are rejected;
custom prototypes are not preserved. Bytes, shapes, limits, and snapshots are
owned values, not views into growable Wasm memory. Codec callbacks cannot re-enter
the same graph, including queries or disposal (`ReentrantCall`). Callback errors
become `StockfoilError` with code `CodecError` and the original exception as cause.

## Editing, permissions, and signing

```ts
graph.setWritePermissions(new Map([[alice, "unbounded"]]));
const draft = graph.commit(alice, { key: "title", value: "Hello" });
handleReport(draft.report);
update(); // Local drafts are already delivered working history.
const signature = await signer.sign(draft.signingBytes);
handleReport(graph.addSignature({ author: alice, commit: draft.hash, signature }));
update();
```

The codec, signer, materializer, and report handler are application code.
`AuthorId` and `CommitHash` are validated branded hexadecimal strings;
`authorId(bytes)` and `commitHash(bytes)` convert 32-byte values. Signing input is
the raw 32-byte commit hash, not its hexadecimal text. `describeCommit(author,
payload)` on the graph previews metadata using current working heads without
inserting a draft. Signing and networking remain outside Stockfoil.

Permissions are `"unbounded"`, `"denied"`, or `{ through: hashes }`.
`setWritePermissions` replaces the map; absent authors are denied. Receiving or
signing history never grants permission. A `ViewPoint` owns `heads` and a
`writePermissions` map. `viewPoint()` selects delivered working history;
`signedViewPoint()` selects admitted history.

## Materialization

```ts
let cursor = new MaterializationCursor();
function update() {
  const batch = graph.changes(cursor);
  for (const change of batch.changes) materializer.apply(change);
  cursor = batch.cursor;
}
```

Each change has `commit` (metadata and typed `payload`), `newlyDelivered`, and
`visibility` (`"before"`, `"after"`, `"both"`, or `"neither"`). Store newly delivered
commits even if denied; use visibility changes to update participation. Changes
are ancestors-before-descendants, once per commit. Keep older operations needed
when revocation reveals overwritten values.

`changes(cursor, view)` accepts a signed or captured historical viewpoint. This
does not rewind the delivery frontier or hide newly delivered drafts from
storage. For historical reads, copy both materializer state and cursor. Cursors
are immutable snapshots with `clone()`; keeping the input cursor allows replay,
not rollback of application side effects. Batches return before application code
runs, so applying them holds no graph traversal borrow.

## Encoded synchronization

```ts
let published = new ExportCursor();
async function publish() {
  const batch = graph.exportChanges(published);
  await transport.store(batch.added);
  published = batch.cursor; // Only after the complete batch is stored.
}

const report = graph.ingest(receivedBytes);
handleReport(report);
update();
```

Each `exportChanges` addition is an `EncodedItem` with opaque
`bytes: Uint8Array` and `metadata`. Commit metadata contains `kind`, `hash`, and `parents`; fragment
metadata contains `kind`, `head`, `boundary`, and sorted unique 12-byte
`checkpoints`. Checkpoints are reconciliation hints derived locally, not evidence
transmitted in the envelope. Adapters do not decode outgoing bytes.

`exportChanges(cursor)` returns `added`, `retired`, and a captured `cursor`. The
empty cursor exports the complete certified cover. Keep one cursor per destination
and serialize publications sharing it. Partial storage failures permit duplicate
publication on retry. `retired` denotes superseded inventory, not invalid history
or an instruction to delete commits. Batches survive later graph edits, pruning,
and disposal. Neither unsigned drafts nor queued received items are exported.
Synchronization and persistence use this same byte flow; there is no single-item
export or structured inventory API. The `*Shape` types remain public codec context.

`ingest(bytes)` decodes and authenticates before admitting or queueing. Missing
dependencies are not exceptions. Queued items own payloads but are not delivered,
visible, or exportable. Admission (including local signing) wakes waiters.
`hasCommit(hash)` tests admission; `pendingItems()` and `missingDependencies()`
expose received queue progress. `isAncestor(ancestor, descendant)` queries history.
There is no public structured insertion or caller-asserted verification flag.

## Reports, failures, and limits

Local commits include an `IngestReport`; `addSignature` and `ingest` return one:

- `admitted`: processed certificates, possibly duplicates; not a delivery stream.
- `rejected`: deferred failures with `item: ItemId` and `error: StockfoilError`.
- `pending`: received items still waiting, excluding local drafts.

Always inspect rejections: a valid dependency can expose an invalid queued
fragment without being rolled back itself. Admission is atomic per item, not per
batch. Materialize successful progress even when a later item fails.

Immediate failures throw `StockfoilError` with an `ErrorCode`, rather than requiring
message parsing. Codes cover codec/encoding failures, wrong format, canonical-byte
mismatch, budgets, signatures, fragment placement, invalid inputs, re-entry, and
disposal; see [`types.ts`](../javascript/src/types.ts) for the complete union.

Constructor limits and `setDecodeLimits(options)` govern inbound items only:
8 MiB envelopes, 65,536 members, 1,000,000 parent references, 65,536 signatures,
and 8 MiB reconstructed canonical payload bytes by default. Omitted fields on
replacement reset to defaults. Decoders must cooperate before allocating; core
cannot sandbox codec CPU or allocations. Local commits/exports are not constrained
by these budgets. Explicitly raise limits and retry an oversized item if trusted;
there is no authenticated chunking or aggregate pending-queue budget.

## Packaging and lifetime

Import from `"stockfoil"` for automatic environment-specific initialization.
`"stockfoil/slim"` exposes asynchronous default `init(input)` and `initSync(module)`
for manual initialization and library integrations. Both expose the same API and
share cursor/error identity. Generated bindings are not public exports.

`graph.dispose()` is idempotent except during codec callbacks. Later graph calls
throw `Disposed`; owned snapshots/batches remain usable. Cursor handles are
reclaimed by finalizers and do not borrow the graph.

See the [package guide](../javascript/README.md) for build instructions and exact
signatures, [`CODEC.md`](../CODEC.md) for codec laws, and the
[JS register example](../examples/mv-register-js) for matching BCS codecs,
external signing, revocation, drafts, and real Rust/JS WebSocket synchronization.
Whole-document persistence, networking, and key management remain application
concerns.
