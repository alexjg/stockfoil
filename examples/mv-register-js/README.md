# JavaScript multi-value register ↔ Rust

A small JavaScript materializer using **Stockfoil Wasm** for the authenticated
history graph, Node's Ed25519 signer, and the published
[`@automerge/subduction`](https://www.npmjs.com/package/@automerge/subduction)
**Wasm WebSocket client** for transport. No native transport proxy runs for JS.

## Run

Requires Node 22+, Rust, and the built local `stockfoil` JavaScript package:

```sh
# From the repository root (see javascript/ for Wasm build prerequisites):
npm --prefix javascript install
npm --prefix javascript run build
cd examples/mv-register-js
npm install
npm test
npm run test:interop
```

The interop command builds the Rust example, starts its real in-memory OpenPolicy
Subduction server on `127.0.0.1:0`, and connects independent Rust and JS replicas.
It asserts:

- a connection with the wrong expected server identity is rejected;
- Rust → JS initial history and JS → Rust shared history;
- concurrent `Hello` / `Howdy` converge on both sides;
- certified fragments travel in both directions;
- fresh JS and Rust replicas recover from the server, including reverse blob
  arrival on JS;
- historical reads leave current state unchanged;
- changed payloads and forged signatures are rejected.

It prints both final values and exits. Deadlines, explicit disconnects, and a
last-resort child-process kill bound cleanup on success and failure. No installed
Subduction CLI or external service is needed. Keys and document IDs are fresh on
each run. The test-only native `--interop` JSON-lines control channel carries
commands and trusted author/server configuration, **not history blobs**.

## Files and format

- `register.mjs`: retained topologically ordered assignments, participation set,
  ancestry-based maximal values, external signing, and copied historical state
  **and cursor**. Write permissions come from application configuration only.
- `transport.mjs`: Subduction publication, sync, and verified graph ingestion.
  Its export cursor advances only after a complete batch is stored; retired
  cover entries can remain in storage. Calls sharing this cursor are serialized.
- `wire.mjs`: the exact private BCS version-1 envelope from
  [`../mv-register/src/wire.rs`](../mv-register/src/wire.rs), implemented without
  another codec dependency. Assignments are two length-prefixed UTF-8 strings.
  Commit metadata hashes are reconstructed by Stockfoil's `describeCommit`,
  preserving parent order/multiplicity and exact payload bytes; **decoding alone
  does not verify an item**. `CommitGraph.ingest` verifies and admits it.
- `interop.mjs`: automated native/Wasm integration scenario.

The npm transport is pinned to `@automerge/subduction` **0.21.2**, matching Rust
`sedimentree_core` **0.14.2**, `subduction_core` **0.18.2**,
`subduction_crypto` **0.9.2**, and `subduction_websocket` **0.12.2**.
Its API and this private example envelope are not stable cross-version formats.

This demo trusts peers to advertise correct Subduction transport metadata (as the
native example does); Stockfoil still independently authenticates the original
history and payloads. OpenPolicy is for local testing, not public deployment.
There is no persistence, encryption, or production pending-queue/resource policy.
See the [native example](../mv-register/README.md) for these limitations and the
[JS API design](../../design/js_api.md) for graph/cursor semantics.
