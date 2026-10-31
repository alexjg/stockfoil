# JavaScript API

Implemented in [`javascript/`](../javascript) as a handwritten TypeScript wrapper
over thin `wasm-bindgen` bindings, packaged with
[wasm-bodge](https://github.com/alexjg/wasm-bodge). The
[JavaScript register](../examples/mv-register-js) interoperates with the Rust
example through a real Subduction WebSocket server.

## Design

- Graph operations are synchronous after Wasm initialization. Signing and
  networking stay in application code.
- Bind `CommitGraph<Vec<u8>>` initially, rather than storing arbitrary JS values
  inside Rust. Applications own payload encoding and decoding.
- Expose ordinary JS records for metadata, viewpoints, and transport items.
- Keep materialization and export cursors explicit and immutable.
- Return batches rather than invoke JS callbacks while Rust borrows the graph.
- Keep verification inside Rust; a TypeScript type must not establish trust.

This does not require moving payload storage out of the Rust graph first. The
binding adapts the existing payload callbacks internally without exposing aligned
metadata/payload arrays to JS callers.

## Values

Use validated, branded hexadecimal strings for commit hashes and author IDs.
These have value equality and work naturally as JS map keys. An author ID encodes
an Ed25519 public key. Payloads, signatures, and signing inputs are `Uint8Array`s.

```ts
export type CommitHash = string & { readonly __brand: "CommitHash" };
export type AuthorId = string & { readonly __brand: "AuthorId" };

export type WritePermission =
  | "unbounded"
  | "denied"
  | { readonly through: readonly CommitHash[] };

export interface ViewPoint {
  readonly heads: readonly CommitHash[];
  readonly writePermissions: ReadonlyMap<AuthorId, WritePermission>;
}

export interface Commit {
  readonly hash: CommitHash;
  readonly author: AuthorId;
  readonly parents: readonly CommitHash[];
  readonly payloadHash: string;
  readonly payload: Uint8Array;
}

export interface SignatureEvidence {
  readonly author: AuthorId;
  readonly commit: CommitHash;
  readonly signature: Uint8Array;
}

export type ItemId =
  | { readonly kind: "commit"; readonly hash: CommitHash }
  | {
      readonly kind: "fragment";
      readonly head: CommitHash;
      readonly boundary: readonly CommitHash[];
    };

export type TransportItem =
  | {
      readonly kind: "commit";
      readonly commit: Commit;
      readonly signatures: readonly SignatureEvidence[];
    }
  | {
      readonly kind: "fragment";
      readonly head: CommitHash;
      readonly boundary: readonly CommitHash[];
      readonly commits: readonly Commit[];
      readonly signatures: readonly SignatureEvidence[];
      readonly borrowed?: readonly CommitHash[]; // Usually empty.
    };
```

`borrowed` lists the tips of fragment history that other items authenticate;
see [borrowed authentication](authors_signatures_revocation.md#borrowed-authentication).
Omitting it means the fragment authenticates all of its members itself.

`authorId(bytes)` and `commitHash(bytes)` convert raw public-key and hash bytes to
canonical lowercase identifiers. Validate identifiers at the binding boundary;
branding alone is not validation. `describeCommit(author, parents, payload)`
reconstructs derived hashes for application decoders without authenticating the
result. Preserve parent order and multiplicity because both are part of the
authenticated commit contents.

Every transport commit carries its payload alongside its metadata. There are no
public parallel arrays or payload lookup callbacks. Input bytes are copied into
owned storage; returned byte arrays are copies, not views into growable Wasm
memory. Captured viewpoints do not borrow mutable application permission maps.

## Graph and cursors

```ts
export declare class CommitGraph {
  constructor();

  // Adds a local draft using the current working heads.
  commit(author: AuthorId, payload: Uint8Array): {
    hash: CommitHash;
    signingBytes: Uint8Array;
    report: IngestReport;
  };

  addSignature(evidence: SignatureEvidence): IngestReport;
  ingest(item: TransportItem): IngestReport;

  viewPoint(): ViewPoint;
  signedViewPoint(): ViewPoint;
  setWritePermissions(
    permissions: ReadonlyMap<AuthorId, WritePermission>
  ): void;

  changes(
    since: MaterializationCursor,
    view?: ViewPoint
  ): {
    changes: readonly MaterializationChange[];
    cursor: MaterializationCursor;
  };

  exportChanges(since: ExportCursor): {
    added: readonly TransportItem[];
    retired: readonly ItemId[];
    cursor: ExportCursor;
  };

  hasCommit(hash: CommitHash): boolean;
  isAncestor(ancestor: CommitHash, descendant: CommitHash): boolean;
  pendingItems(): number;
  missingDependencies(): readonly CommitHash[];
  missingAuthentication(): readonly CommitHash[];

  dispose(): void;
}

export declare class MaterializationCursor {
  constructor(); // Empty.
  clone(): MaterializationCursor;
}

export declare class ExportCursor {
  constructor(); // Empty.
  clone(): ExportCursor;
}
```

`hasCommit` means admitted, not merely delivered. Received items can wait in the
pending queue, while local unsigned drafts are already delivered working history.
Neither is exportable until admitted. Missing dependencies are not exceptions.

`ingest` checks the supplied hashes against the exact payload and metadata bytes,
verifies signatures, and then admits or queues the item. It does not accept a
caller-asserted "verified" flag or expose an unchecked verified-item constructor.

`setWritePermissions` replaces the permission map. Missing authors are denied;
receiving or signing history never grants permissions.

The wrapper hides raw Wasm handles and provides idempotent graph disposal. Later
graph calls throw `Disposed`. Cursors retain owned Rust snapshots through private
handles, reclaimed by wasm-bindgen finalizers. Snapshots, cursors, and batches do
not borrow the graph. Root/slim wrappers share cursor registries through their
common raw constructors, so library and application imports interoperate.

## Editing and signing

Applications choose an encoding. Stockfoil authenticates exact bytes; it does not
interpret or canonicalize JSON.

```ts
const payload = new TextEncoder().encode(
  JSON.stringify({ key: "title", value: "Hello" })
);

graph.setWritePermissions(new Map([[alice, "unbounded"]]));

const draft = graph.commit(alice, payload);
handleRejections(draft.report.rejected);
update(); // Show the local draft immediately.

const signature = await signer.sign(draft.signingBytes);
const report = graph.addSignature({
  author: alice,
  commit: draft.hash,
  signature,
});
handleRejections(report.rejected);
update();
```

Here `alice` is an `AuthorId`; `signer`, `update`, and `handleRejections` are
application code. `signingBytes` contains the raw 32-byte commit hash, not its
hexadecimal text. No Rust borrow is held while signing awaits. This supports
synchronous signers, WebCrypto, and remote signing without embedding key
management in Stockfoil.

## Materialization

```ts
export interface MaterializationChange {
  readonly commit: Commit;
  readonly newlyDelivered: boolean;
  readonly visibility: "before" | "after" | "both" | "neither";
}

let cursor = new MaterializationCursor();

function update() {
  const batch = graph.changes(cursor);
  for (const change of batch.changes) {
    materializer.apply(change);
  }
  cursor = batch.cursor;
}
```

`changes` defaults to the current working viewpoint. Passing `signedViewPoint()`
restricts participation to admitted history; passing a captured viewpoint supports
historical reads. Delivery and participation retain their Rust semantics:

- Changes are ancestors-before-descendants, with each commit visited at most once.
- Newly delivered commits are included even if they cannot currently participate.
- A historical or signed target does not rewind the delivery frontier or hide
  newly delivered local drafts from storage.
- Historical materialization uses a copy of the application state and its cursor,
  rather than moving the live materializer backwards.

Build the batch in Rust and return before application callbacks run. This avoids
one Wasm-to-JS callback per commit and permits application code to query the graph
while applying the batch without retaining a traversal borrow.

The input cursor does not change. If application processing throws, it can replay
from the old cursor, but partial application-side effects are not rolled back.
Copying the cursor alone does not copy the materialized state.

## Publication

```ts
let exportCursor = new ExportCursor();

async function publish() {
  const batch = graph.exportChanges(exportCursor);
  for (const item of batch.added) {
    await transport.store(item);
  }
  exportCursor = batch.cursor;
}
```

Keep one export cursor per destination inventory and advance it only after the
batch is successfully stored. A partial failure may cause duplicate publication
on retry. Calls sharing the cursor should be serialized.

The empty cursor exports the complete current certified cover. Subsequent calls
compare that cover with the cursor, not with delivery heads or a historical event
log. Changes to permissions or sufficient signature evidence for an unchanged
item identity do not require republication.

`retired` identifies superseded inventory entries, not invalid history or commits
to delete. A simple publisher can ignore it. Applications maintaining an exact
inventory should publish additions before retiring superseded entries.

Transport items own their metadata, signatures, and payload bytes, so a returned
batch remains usable after later graph changes or certificate pruning. Item order
is unspecified; receivers use the pending ingestion queue.

## Errors

```ts
export declare class StockfoilError extends Error {
  readonly code: string;
}

export interface IngestRejection {
  readonly item: ItemId;
  readonly error: StockfoilError;
}

export interface IngestReport {
  readonly admitted: readonly ItemId[];
  readonly rejected: readonly IngestRejection[];
  readonly pending: number;
}
```

Immediate input or validation failures throw `StockfoilError`, with stable codes
rather than requiring message parsing. The exported `ErrorCode` union lists them:
`InvalidInput`, `PayloadHashMismatch`, `CommitHashMismatch`, `InvalidSignature`,
`InvalidPublicKey`, `MissingSignature`, `InvalidItem`, `InvalidFragment`,
`NotPendingLocalCommit`, `MissingCommit`, `Disposed`, and `InternalError`.

Admission is atomic per item. A valid dependency can expose an invalid queued
fragment without itself being rejected. Those deferred failures appear in the
report of the operation that unblocked them, including signing. `admitted` lists
processed certificates, possibly duplicates; it is not a materialization stream.
`pending` counts received items still waiting, not local drafts.

Binding code should translate expected Rust errors, not use panic recovery as
normal validation or promise rollback after arbitrary panics.

## Packaging

Use wasm-bodge's [TypeScript wrapper mode](https://github.com/alexjg/wasm-bodge#typescript-wrappers):

```json
{
  "wasm-bodge": {
    "wrapper": {
      "entry": "./src/index.ts",
      "slimEntry": "./src/slim.ts",
      "tsconfig": "./tsconfig.json",
      "exposeBindings": false
    }
  }
}
```

The wrapper imports generated Rust bindings through `#wasm-bodge/bindings`.
Consumers normally import from `"stockfoil"`; wasm-bodge handles environment-specific
initialization. `"stockfoil/slim"` provides the same API for manual initialization
and library integrations. Its dedicated `slimEntry` exposes `initSync(module)` and
an asynchronous default `init(input)`, returning no raw Wasm exports. The public
declarations use ordinary web platform types rather than private binding aliases.

Keep raw generated bindings private so their handle layout and marshaling details
are not part of the supported JS API.

## Deferred work

- A public binary format and `load`/`save`: structured transport items are enough
  for the first JS bindings.
- Moving payload storage out of the Rust graph: the byte-oriented JS surface
  need not wait for that refactor.
- Networking adapters, key stores, automatic JSON codecs, and lower-level graph
  inspection APIs are outside the library surface. The separate JS register
  example supplies its own signer, private BCS codec, and Subduction adapter.
