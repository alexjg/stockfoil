# stockfoil (JavaScript)

Transport-neutral authenticated commit DAGs and batched CRDT materialization.
Topology, policy, verification, parking and materialization planning run in Rust.
Applications own CRDT state, storage, codecs, transport and scheduling.

## In memory

```js
import { Document, Materialization, acknowledgeExport } from "stockfoil";

const document = new Document(codec, { author }); // pure, synchronous codec
const checkout = new Materialization({ supportsVisibilityChanges: true });

const draft = document.commit(payloadBytes); // synchronous; parents default to working heads
document.attachSignature(draft, sign(draft.hash)); // Ed25519 over the raw hash

while (!checkout.isCurrent(document.ledger)) {
  let batch = checkout.plan(document.ledger, 64);
  while (batch) {
    // Resolve copies of payload IDs without modifying the completion token.
    const events = batch.events.map(event =>
      "payload" in event
        ? { ...event, payload: document.payloadById(event.payload) }
        : event);
    const applied = state.apply(events); // { rejected?: [...], wanted?: [...] }
    batch = checkout.complete(document.ledger, batch, applied);
  }
  // Persist state and checkout.checkpoint atomically, after all follow-ups.
}

const plan = document.export(peerCursor ?? []);
for (let i = 0; i < plan.items.length; i++) {
  await peer.store(document.exportItem(plan.items[i]));
  acknowledgeExport(plan, i); // only after durable acceptance
}
peerCursor = plan.cursor;

const result = document.receive(itemBytes); // synchronous; parks missing dependencies
// result.kind: "admitted", "waiting", or "evicted"
```

The codec has `format`, `encode(meta, payloads)`, and
`decoder(meta).step(input, eof, capacity)`. Encoding collects a whole body;
the decoder reports `{ consumed, output, status }`, with status `"working"`,
`"needInput"`, `"endMember"`, or `"finished"`. Canonical output is limited by
capacity. See [the runnable register](../examples/mv-register-js).

## Plain data at the boundary

Drafts, receipts, views, events, batches and export descriptions are plain
JavaScript objects. Hashes/authors/signatures are owned `Uint8Array` copies
(32/32/64 bytes); payload IDs, tickets, epochs and batch tokens are `bigint`.
Checkpoints are versioned byte arrays; `inspectCheckpoint(bytes)` returns a
plain description. Export cursors are item-ID arrays; use `encodeExportCursor`
and `decodeExportCursor` for canonical persistent bytes. Treat cursors as sets.

Data can be structured-cloned, including an unchanged batch before completing
it. Batch tokens are process-local and validated against events and rebuild
flags: do not edit them, replay them, or use them with another planner.
They are not durable checkpoints.

Generated Wasm classes are **not exported**. `Ledger`, `Materialization` and
`Ancestry` are ordinary JS facades over stateful engines. Streaming operations
are JS objects with methods. Call `dispose()` when done; consuming an operation
also releases its engine. Repeated disposal is safe. Data objects need no
disposal.

## Caller-owned bounded-buffer I/O

Use `Ledger`, not `Document`, for large/sparse payloads:

```js
import { Ledger, verify } from "stockfoil";

const ledger = new Ledger({ author, format: codec.format, maxParked: 1024 });
const admission = ledger.beginReceive({ readSize: 65536, decodeCapacity: 65536 });
try {
  await verify(admission, {
    read: (offset, max) => storedItem.read(offset, max), // empty = EOF
    decoder: meta => stagingDecoder(meta), // stage canonical output with your own I/O
  }, { signal });
  // Item/canonical bytes must be durable before materializing.
  const result = ledger.admit(admission, (id, hashes) => stagedIdsFor(id, hashes));
} finally {
  admission.dispose();
}
```

`verify` stops after byte verification; it neither waits for dependencies nor
publishes. Its read/decode requests hold no ledger borrow across awaits.
Cancellation is checked between requests; the host owns cancellation of its I/O.
Binding is synchronous and must return one previously assigned u64 payload ID
per member, including duplicates. **Do not reenter the ledger from binding.**
Parking never calls binding. Keep staging for parked copies; release one
reference for each admitted/failed/evicted item, including released receipts in
`Signed.released` and `Received.items`. Duplicate parked copies can share bytes.

Local commits feed the same chunks into `ledger.beginCommit(parents?)` and your
storage, then `ledger.commit(builder, payloadId)` consumes the builder and returns
a draft. `builder.hash()` inspects its current hash without consuming it.
Attach externally obtained signatures with `ledger.attachSignature(draft,
signature, bind)`.

`ledger.export(cursor)` captures plain descriptions: wire header, canonical
member hashes/IDs, metadata and sedimentree checkpoints. Stream body encoding
from your storage; no export handle or ledger borrow survives that I/O.
`ledger.decodeItem(wanted, options)` with `driveDecode` checks wanted canonical
member chunks from stored bundles. Visitor output is provisional until decoding
succeeds; stage effects until then. `ledger.checkPayload(hash)` checks canonical
bytes directly using `update`/`finish`.

The graph and graph-sized metadata remain in memory; export descriptions and
`Ancestry.difference` collect metadata, never payloads. A planner budget limits
source records, **not bytes or work inside one huge commit**.

## Views, rejection and recovery

Each independent `Materialization` follows `"working"`, `"signed"`, or
`{ kind: "pinned", heads }`. Permissions belong to the document's ledger, not
individual checkouts: `ledger.setPermissions([[author, permission], ...])`,
where permission is `"unbounded"`, `"denied"`, or a frontier of hashes.

Deliveries include inactive commits: descendants may reference their metadata.
Later visibility changes are `activate`/`retract`, or `reset` and replay for
states that cannot retract. Report permanent payload rejection with
`{ rejected: [{ hash, reason, cascade }] }`; request older payloads with
`{ wanted: [hash] }`. Complete every correction/payload follow-up before saving
state and checkpoint together. A checkpoint read while a batch is outstanding
does not include its speculative application effects.

On application failure or cancellation, discard the planner and restore
application state with its last jointly saved checkpoint. Resume via
`new Materialization({ checkpoint, supportsVisibilityChanges })` after rebuilding
the ledger from stored items. Unsigned drafts require a separate local journal.
Errors from Rust have `name: "StockfoilError"` and a stable `code`.

## Development and initialization

Node 22+, nightly Rust with `rust-src`, matching `wasm-bindgen-cli` (0.2.127),
`wasm-opt`, `esbuild`, TypeScript, and `wasm-bodge` 0.5:

```sh
cargo install wasm-bodge --version 0.5.0 --locked
cargo install wasm-bindgen-cli --version 0.2.127 --locked
rustup toolchain install nightly --component rust-src
npm install
npm test
npm run typecheck
```

For stable Rust/abort builds, add the `wasm32-unknown-unknown` target and run
`wasm-bodge build --panic abort`; `--no-wasm-opt` skips optimization.

Root imports initialize Wasm automatically. For manual initialization, import
`initialize` from `stockfoil/slim` and pass Wasm bytes or a `WebAssembly.Module`
before creating engines. `stockfoil/wasm` exposes the binary.

`tests/fixture.rs` validates native wire interoperability.
Regenerate fixtures only deliberately with
`STOCKFOIL_UPDATE_FIXTURES=1 cargo test -p stockfoil-wasm`.
