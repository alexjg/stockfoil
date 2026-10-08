# stockfoil (JavaScript)

Wasm bindings for Stockfoil's `History` and `Admission`, plus an async host
driver. The binding is thin: hashing, verification, placement, publication, and
export planning all run in the Rust core; JavaScript supplies storage and codecs.

```js
import { Graph, MemoryStorage, ExportCursor } from "stockfoil";

const graph = new Graph(codec, new MemoryStorage());
const draft = await graph.commit(author, parents, payloadBytes);
graph.sign(author, draft.hash, sign(draft.signingBytes)); // Ed25519 over the hash

const plan = graph.export(new ExportCursor());
for (let i = 0; i < plan.length; i++) {
  await peer.send(await graph.writeItem(plan, i));
  plan.acknowledge(i);
}

const receipt = await peer.graph.receive(itemBytes); // waits for dependencies
```

Hashes, authors, and signatures are `Uint8Array`s (32, 32, and 64 bytes).
Payload IDs, tickets, and epochs are `BigInt`. Every value crossing the boundary is
an owned copy. Errors are `StockfoilError`s whose `code` is the core error's
`Error::code()`.

## Driving admissions yourself

`drive(history, admission, host)` runs one admission against any host:

```ts
interface Host {
  read(item: Uint8Array, offset: number, max: number): MaybePromise<Uint8Array>; // empty = end
  bind(item: Uint8Array, memberCount: number): MaybePromise<readonly bigint[]>;
  decoder(meta: BundleMeta): MaybePromise<Decoder>; // step(input, eof, capacity)
}
```

It yields after each work quantum, never holds borrowed data across an `await`,
and retries a waiting admission when `history.epochChanged()` resolves. Many
admissions and local commits can be in flight on one `History`.

The lower-level objects mirror the Rust API: `History` (`beginCommit`,
`publishCommit`, `sign`, `receive`, `advance`, `export`, `exportItem`,
`viewPoint`, `changes`, `nextChanges`, …), `Admission` (`next`, `reply`,
`request`, `counts`), `ExportPlan`, and the `ExportCursor` and
`MaterializationCursor` classes with `toBytes`/`fromBytes`.

The convenience `Graph`'s `Codec.encode(meta, payloads)` encodes a whole bundle
in memory; streaming encoders can be built on `History.exportItem` directly.

## Development

```sh
npm install
npm test        # builds with wasm-bodge, then runs node --test
```

`tests/fixture.rs` regenerates `test/fixtures/rust-items.json` (items produced by
the native Rust graph) with `STOCKFOIL_UPDATE_FIXTURES=1 cargo test -p stockfoil-wasm`.
