# JavaScript multi-value register ↔ Rust

A small JavaScript materializer using **Stockfoil Wasm** for the authenticated
history graph, Node's Ed25519 signer, and the sibling Subduction checkout's
`@automerge/subduction` **Wasm WebSocket client** for transport. No native transport
proxy runs for JS.

## Run

Requires Node 22+, Rust, and sibling checkouts of `stockfoil` and `subduction`.
Both JavaScript packages must be built locally. From the Stockfoil root:

```sh
# See javascript/ for Stockfoil's Wasm build prerequisites.
npm --prefix javascript install
npm --prefix javascript run build

# Build Subduction's actual package exports (dist/), not the older pkg*/ outputs.
# Requires wasm-bodge and the Subduction checkout's Wasm build prerequisites.
(
  cd ../subduction
  wasm-bodge build \
    --crate-path "$PWD/subduction_wasm" \
    --package-json "$PWD/subduction_wasm/package.json" \
    --out-dir "$PWD/subduction_wasm/dist"
)

npm --prefix examples/mv-register-js ci
npm --prefix examples/mv-register-js test
npm --prefix examples/mv-register-js run test:interop
```

The npm `file:` dependency links `../subduction/subduction_wasm`; it does **not**
build Wasm automatically. Rebuild `dist/` after changing Subduction's Rust source
and restart the Node process to load it. No reinstall is needed for changes to
that linked build output. In Subduction's development shell, `bodge subduction_wasm`
also builds this layout, including its optional debug variant.

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
  It uses `storeBuiltBatch` for local-only publication and
  `Fragment.fromCheckpointPrefixes` with Stockfoil's exported prefixes. Its export
  cursor advances only after a complete batch is stored; retired cover entries
  can remain in storage. Callers must serialize calls sharing this cursor.
- `wire.mjs`: the exact private BCS version-1 envelope from
  [`../mv-register/src/wire.rs`](../mv-register/src/wire.rs), implemented without
  another codec dependency. Assignments are two length-prefixed UTF-8 strings.
  Commit metadata hashes are reconstructed by Stockfoil's `describeCommit`,
  preserving parent order/multiplicity and exact payload bytes; **decoding alone
  does not verify an item**. `CommitGraph.ingest` verifies and admits it.
- `interop.mjs`: automated native/Wasm integration scenario.

Both examples temporarily depend on the sibling Subduction checkout rather than
registry releases, so native and Wasm clients can exercise the same unreleased
APIs. Checkpoint-prefix construction and mixed batch storage are now used directly;
no full checkpoint IDs are reconstructed from fragment members. The private BCS
envelope remains unchanged and is not a stable cross-version format.

These APIs remove the compatibility workaround, not all integration boilerplate.
Managed client lifecycle, central expected-peer enforcement, incremental stored-blob
delivery, and round-scoped sync completion remain upstream work. The adapters keep
their manual identity checks, cleanup, and bounded convergence polling. See
[`SLIM_THE_API.md`](../../SLIM_THE_API.md) for the current API review.

This demo trusts peers to advertise correct Subduction transport metadata (as the
native example does); Stockfoil still independently authenticates the original
history and payloads. OpenPolicy is for local testing, not public deployment.
There is no persistence, encryption, or production pending-queue/resource policy.
See the [native example](../mv-register/README.md) for these limitations and the
[JS API design](../../design/js_api.md) for graph/cursor semantics.
