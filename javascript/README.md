# Stockfoil for JavaScript

An alpha TypeScript/JavaScript API over the Rust authenticated commit graph,
packaged with [wasm-bodge](https://github.com/alexjg/wasm-bodge). Graph operations
are synchronous; applications own payload encoding, signing, and transport.

## Build and test

Prerequisites: Node.js, nightly Rust with `rust-src`, `wasm-bindgen-cli` **0.2.127**,
`wasm-opt`, and wasm-bodge with TypeScript wrapper support. The tested wasm-bodge
revision is `ee3bca139cea45a38aab78a2f2fcc136432ce25f` (0.5.0); older 0.3.x builds
are insufficient.

```sh
rustup toolchain install nightly --component rust-src
cargo install wasm-bindgen-cli --version 0.2.127 --locked
cargo install wasm-opt --version 0.116.1 --locked
cargo install --git https://github.com/alexjg/wasm-bodge \
  --rev ee3bca139cea45a38aab78a2f2fcc136432ce25f --locked

# From javascript/:
npm ci
npm run build
npm run typecheck
npm test
```

The build uses wasm-bodge's default unwind pipeline. Expected validation errors
are translated to `StockfoilError`, not implemented as Rust panics. Generated
artifacts are ignored; `npm pack` includes the built Wasm, JS, and declarations.
The tests also install the tarball into an isolated consumer and typecheck its
public API without private source aliases.

Browser smoke tests use Chromium and WebCrypto, exercising automatic loading,
standalone asset loading, and both manual initializers:

```sh
npx playwright install chromium
npm run test:browser
```

## Materialization

```ts
import { CommitGraph, MaterializationCursor } from "stockfoil";

const graph = new CommitGraph();
let cursor = new MaterializationCursor();

function update() {
  const batch = graph.changes(cursor);
  for (const change of batch.changes) materializer.apply(change);
  cursor = batch.cursor;
}
```

`materializer` is application code. Store newly delivered commits even when
`visibility` is `"neither"`. Enable participation for `"after"`, disable it for
`"before"`, and retain old operations needed for revocation. Changes arrive in
ancestors-before-descendants order, as an owned batch with no live Rust borrow.

Pass `graph.signedViewPoint()` or a captured viewpoint as the second argument to
`changes`. Historical reads copy both application state and cursor. Cursors are
immutable and garbage-collected; reuse an old cursor to replay. Partial effects
in application code are not rolled back if processing throws.

## Editing, signing, and receipt

```ts
import { authorId } from "stockfoil";

const author = authorId(publicKeyBytes); // 32 Ed25519 public-key bytes
const permissions = new Map([[author, "unbounded" as const]]);
graph.setWritePermissions(permissions);
const draft = graph.commit(author, payload); // Uint8Array; immediately delivered
update();
const signature = await signer.sign(draft.signingBytes);
const report = graph.addSignature({ author, commit: draft.hash, signature });
if (report.rejected.length) throw new AggregateError(report.rejected.map(r => r.error));
update();
```

The key, payload, and signer belong to the application. Sign the raw 32-byte
`signingBytes`, not the hexadecimal hash text. `hasCommit` checks admission, so it
returns false for an unsigned draft. Missing authors are denied; receiving or
signing history does not grant permissions.

`graph.ingest(item)` accepts structured `TransportItem`s with payload bytes beside
each commit's metadata. Rust checks exact payload hashes, metadata hashes,
signatures, and fragment placement. Missing dependencies queue automatically.
Inspect every operation's `IngestReport.rejected` for deferred failures; valid
admissions are not rolled back when another queued item fails.

Immediate failures throw `StockfoilError` with a stable `code` (see the exported
`ErrorCode` type). `describeCommit(author, parents, payload)` derives hashes for
application decoders; it does **not** authenticate the result. Identifiers are
canonical lowercase hexadecimal strings. Byte arrays are copied, never borrowed
views of Wasm memory.

## Publication

```ts
import { ExportCursor } from "stockfoil";

let published = new ExportCursor();
async function publish() {
  const batch = graph.exportChanges(published);
  for (const item of batch.added) await transport.store(item);
  published = batch.cursor;
}
```

Use one cursor per destination and serialize calls sharing it. Advance only after
the entire batch is stored; retries can duplicate items. Retired inventory IDs
are not revocations or instructions to delete commits. The default publisher can
ignore them. A public binary `load`/`save` format is not implemented.

Additions use the same `TransportItem` type as ingestion. Fragments include
`checkpoints`, a sorted, deduplicated array of owned 12-byte `Uint8Array` prefixes.
Stockfoil derives exports from interior nonzero-depth commits, excluding the head
and boundary history. Standalone commits have no checkpoint field. Ingestion
validates hint byte lengths but does not trust their contents; canonical metadata
is derived locally. Decoders can supply an empty list when their format carries
no hints (omitted JS fields also default to empty).

Call `graph.dispose()` to promptly release its Wasm memory. Disposal is idempotent;
subsequent graph operations throw `Disposed`. Owned batches, viewpoints, and
cursors remain usable independently of the disposed graph.

## Initialization

`"stockfoil"` initializes automatically for the environment. Library integrations
can import `"stockfoil/slim"`; a root import initializes its shared Wasm instance,
or callers can initialize explicitly:

```ts
import init, { initSync, CommitGraph } from "stockfoil/slim";

await init(wasmBytes); // Also accepts a URL or Response.
// Or: initSync(wasmBytes);
const graph = new CommitGraph();
```

The raw asset is exported at `"stockfoil/wasm"`. Root/slim cursors interoperate
within the same module system. Raw binding handles are not a public export.
Bundler loading uses top-level await; see wasm-bodge's README for Vite/Webpack,
CSP, and other environment requirements.

## Rust interoperability

The [JavaScript register example](../examples/mv-register-js) uses this package and
the published Subduction Wasm WebSocket client to synchronize with the existing
Rust register through a real loopback server:

```sh
# From the repository root, after building this package:
npm --prefix examples/mv-register-js ci
npm --prefix examples/mv-register-js test
npm --prefix examples/mv-register-js run test:interop
```

Its private BCS envelope matches the Rust example. Neither Stockfoil package
contains a networking dependency. See [the API design](../design/js_api.md) for
semantics and intentionally deferred work, including queue resource limits.
