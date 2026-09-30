# Stockfoil for JavaScript

A synchronous TypeScript/JavaScript API over the Rust authenticated commit graph,
packaged with [wasm-bodge](https://github.com/alexjg/wasm-bodge). Applications own
payload codecs, materialization, signing, and transport. Stockfoil owns the binary
envelope, graph metadata compression, authentication, and dependency queue.

## Build and test

Prerequisites: Node.js with `structuredClone`, nightly Rust with `rust-src`,
`wasm-bindgen-cli` **0.2.127**, `wasm-opt`, and wasm-bodge with TypeScript wrapper
support. Tested wasm-bodge revision:
`ee3bca139cea45a38aab78a2f2fcc136432ce25f` (0.5.0).

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
# Optional browser smoke tests:
npx playwright install chromium
npm run test:browser
```

The build uses wasm-bodge's unwind pipeline. Expected errors are not Rust panics.
Generated artifacts are ignored; `npm pack` includes Wasm, JS, and declarations.
Tests install the tarball into an isolated consumer and check its public types.

## Required application codec

```ts
import { CommitGraph, type PayloadCodec } from "stockfoil";

interface Assignment { key: string; value: string }
const codec: PayloadCodec<Assignment> = {
  formatId: "my-application/assignment/v1",
  encodeCommit(payload) { /* return canonical Uint8Array */ },
  decodeCommit(bytes, limits) { /* return an Assignment */ },
  encodeFragment(shape, payloads) { /* encode the entire batch together */ },
  decodeFragment(shape, bytes, limits) { /* return Assignment[] in member order */ },
};
const graph = new CommitGraph(codec, { maxItemBytes: 8 * 1024 * 1024 });
```

The method bodies above are placeholders. See the complete
[register codec](../examples/mv-register-js/codec.mjs), which uses canonical BCS
assignment bytes and a key dictionary/run representation for fragments. There is
**no default fragment encoder**. Both application encodings are required; no
application needs to implement a Stockfoil envelope.

`shape.members` aligns positionally with payloads. Each member has an `author`
and ordered `parents`: `{kind: "member", index}` refers to an earlier member;
`{kind: "external", hash}` refers outside the fragment. Parent order and duplicates
are significant. The shape and bytes supplied to decoders are not authenticated
yet: decoding precedes hash reconstruction and signature verification.

Codec laws:

- Commit encoding is deterministic. Standalone decoding and re-encoding must
  reproduce the received bytes exactly.
- Fragment decoding must produce exactly one payload per member, with the same
  individual canonical encodings as the original payloads.
- `formatId` is 1–128 UTF-8 bytes and comes from trusted local configuration.
  Mismatched incoming format IDs are rejected before application decoding.
- Methods are synchronous. Promises, thenables, and encoder results other than
  `Uint8Array` are rejected. External codec side effects are not rolled back.

### Ownership and reentrancy

Payloads must support `structuredClone`. Commit inputs, decoded results, encoder
arguments, and materialized values are cloned: mutating any caller-visible object
cannot alter retained authenticated history. Shared memory (`SharedArrayBuffer`,
including nested buffers and views) is rejected because cloning it retains an
alias. Functions and other non-cloneable values are rejected. Custom class
prototypes are not preserved by structured cloning; use cloneable data values.

Byte arrays, shapes, limits, transport metadata, and callback batches are owned
copies, not borrowed views of Wasm memory. Encoder arguments may be retained or
mutated, but the codec must still obey its canonical encoding laws. Export checks
canonical hashes again and rejects disagreement rather than publishing changed
history. The configured codec object itself is application-owned; changing its
behavior after construction can violate these laws.

Every synchronous call back into the same graph from a codec, **including reads
and disposal**, throws `ReentrantCall`. Calls on unrelated graphs are permitted.
Codec exceptions throw `StockfoilError` with code `CodecError` and the original
thrown value in `cause`. Immediate native failures preserve distinct codes such
as `WrongCodec`, `InvalidEncoding`, `NonCanonicalPayload`, `LimitExceeded`, and
signature/admission errors.

### Resource budgets

`new CommitGraph(codec, limits?)` accepts partial overrides:

| Option | Default |
| --- | ---: |
| `maxItemBytes` | 8 MiB |
| `maxMembers` | 65,536 |
| `maxParents` | 1,000,000 total references |
| `maxSignatures` | 65,536 |
| `maxPayloadBytes` | 8 MiB total canonical payload bytes |

Limits are nonnegative integers representable as Wasm `usize` (32 bits). Native
framing/count validation happens before application decoding. Decoders receive
the limits and must enforce them **before expanding untrusted counts**. Core also
checks decoded canonical totals, but cannot sandbox allocations or CPU consumed
inside application code. These are **inbound per-item** budgets, not pending-queue
quotas. They do not restrict local `commit`/`describeCommit` or `exportChanges`;
a locally produced envelope can exceed a receiver's budgets.

`graph.setDecodeLimits(options)` replaces the inbound limits, merging the supplied
partial options with the same defaults as the constructor, **not the previous
limits**. `{}` restores all defaults. It also updates the JS `maxItemBytes`
precheck before copying input into Wasm. Invalid options leave the previous
limits intact; reentrant calls are rejected like other graph operations.

A budget rejection does not retain the item for automatic retry. After deciding
that a larger inbound budget is safe, explicitly retry the same bytes:

```ts
graph.setDecodeLimits({ maxItemBytes: 16 * 1024 * 1024, maxPayloadBytes: 16 * 1024 * 1024 });
const retried = graph.ingest(encodedBytes);
```

Changing limits does not re-decode or remove already admitted or queued items.

## Editing, signing, and receipt

```ts
import { authorId } from "stockfoil";

const author = authorId(publicKeyBytes); // 32 Ed25519 public-key bytes
const permissions = new Map([[author, "unbounded" as const]]);
graph.setWritePermissions(permissions);
const draft = graph.commit(author, { key: "title", value: "Hello" });
const signature = await signer.sign(draft.signingBytes);
const report = graph.addSignature({ author, commit: draft.hash, signature });
if (report.rejected.length) throw new AggregateError(report.rejected.map(r => r.error));

const received = graph.ingest(encodedBytes); // Uint8Array, not a structured item
```

Sign the raw 32-byte `signingBytes`, not the hexadecimal hash text. Unsigned local
drafts are immediately materializable but not exportable; `hasCommit` reports
authenticated admission only. Missing authors are denied. Receiving or signing
history does not grant permissions.

`graph.describeCommit(author, payload)` prepares metadata against current working
heads without inserting a draft, useful for deterministic preparation/mining.
Identifiers are canonical lowercase hexadecimal strings.

Ingestion validates framing, canonical payloads, hashes, signatures, and fragment
placement before admission. Missing dependencies queue automatically. Immediate
codec/validation errors leave that item’s graph and queue state unchanged. Inspect
**every** operation’s `IngestReport.rejected` for deferred failures: admitting a
valid dependency may reject an older queued item without rolling back the valid
admission.

## Materialization

```ts
import { MaterializationCursor } from "stockfoil";

let cursor = new MaterializationCursor();
function update() {
  const batch = graph.changes(cursor);
  for (const change of batch.changes) {
    // change.commit.payload is Assignment, not bytes.
    materializer.apply(change);
  }
  cursor = batch.cursor;
}
```

`materializer` is application code. Store newly delivered commits even when
`visibility` is `"neither"`. Enable participation for `"after"`, disable it for
`"before"`, and retain old operations needed for revocation. Changes arrive in
ancestors-before-descendants order, in an owned batch with no live Rust borrow.

Pass `graph.signedViewPoint()` or a captured viewpoint as the second argument to
`changes`. Historical reads copy both application state and cursor. Cursors are
immutable, cloneable, and garbage-collected; reuse an old cursor to replay.
Partial materializer effects are not rolled back if application code throws.

## Publication

```ts
import { ExportCursor } from "stockfoil";

let published = new ExportCursor();
async function publish() {
  const batch = graph.exportChanges(published);
  for (const item of batch.added) {
    await transport.store(item.metadata, item.bytes);
  }
  published = batch.cursor;
}
```

`exportChanges(cursor)` is the output flow for both synchronization and persistence;
`ingest(bytes)` is the input flow. It returns `{added, retired, cursor}`. Every
addition is an `EncodedItem` containing complete native envelope `bytes` and
derived `metadata`:

- Commit: `{kind: "commit", hash, parents}` (transport parent set).
- Fragment: `{kind: "fragment", head, boundary, checkpoints}`; checkpoints are
  sorted, unique, owned 12-byte `Uint8Array` hints, not authentication evidence.

Use one cursor per destination and serialize publication. Advance it only after
storing the entire batch; failed encoding leaves the original cursor reusable.
Retries can duplicate items. Retired IDs are not revocations or unconditional
instructions to delete history. Exported bytes remain usable after graph edits or
disposal. An empty cursor exports the complete signed inventory. To persist it,
store framed item bytes durably before advancing the cursor; reload with
`ingest(bytes)`. Append-only logs can ignore retirements. Drafts, queued received
items, permissions, and keys are not included. There is no separate snapshot or
single-item export API. `FragmentShape`, `MemberShape`, and `ParentRef` remain
public codec context, not an alternative transport representation.

Call `graph.dispose()` to promptly release Wasm memory. Disposal is idempotent;
subsequent operations throw `Disposed`. Owned batches, viewpoints, and cursors
remain usable independently.

## Initialization

`"stockfoil"` initializes automatically. Libraries can import `"stockfoil/slim"`;
a root import initializes the shared instance, or initialize explicitly:

```ts
import init, { initSync, CommitGraph } from "stockfoil/slim";
await init(wasmBytes); // Also accepts URL or Response.
// Or: initSync(wasmBytes);
const graph = new CommitGraph(codec);
```

The raw asset is exported at `"stockfoil/wasm"`. Root/slim cursors interoperate
within the same module system. Binding handles are not public exports. Bundler
loading uses top-level await.

`CommitGraph<P>(codec)` owns payload encoding, authentication, storage, and
queries in one native graph. Receipt accepts only native envelope bytes; there
is no structured transport or unchecked graph API. The standalone
`describeCommit(author, parents, bytes)` utility derives byte-payload metadata
but does not authenticate it. Use `graph.describeCommit(author, payload)` to
prepare typed metadata using the graph's codec and current working heads.

## Rust interoperability

The [JavaScript register example](../examples/mv-register-js) uses the same native
Stockfoil envelope as Rust, with compatible application commit and compressed
fragment payload codecs. Neither Stockfoil package has a networking dependency.

```sh
# From the repository root, after building this package:
npm --prefix examples/mv-register-js ci
npm --prefix examples/mv-register-js test
npm --prefix examples/mv-register-js run test:interop
```
