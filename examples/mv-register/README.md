# Multi-value register over Subduction

An example CRDT built on Stockfoil, with its original register, revocation,
signature, and materialization tests. It is a separate workspace package: the
networking dependencies are not dependencies of the Stockfoil library.

## Run

The example temporarily uses Rust path dependencies from a sibling Subduction
checkout at `../subduction` (relative to the Stockfoil repository root). All four
Subduction/Sedimentree crates use that checkout, including their transitive links;
Stockfoil core still has no Subduction dependency. Keep the sibling checkout
available even when running workspace tests.

From the repository root:

```sh
cargo run -p stockfoil-mv-register
```

This starts an in-memory **real Subduction WebSocket server** on an ephemeral
loopback port. Two independent clients:

1. Share an initial assignment.
2. Make concurrent assignments.
3. Synchronize and print `{"Hello", "Howdy"}` on both replicas.

Fragment transfer, historical reads, revocation, unsigned drafts, and fresh-replica
catch-up are exercised in the tests rather than in the runnable demo.

No register `merge()` or cloned graph transfers history between clients. The
server stores opaque blobs; it does not run the register or interpret its
permissions. The demo exits after checking convergence and drains its tasks.
Each run uses fresh author keys, transport keys, and a random document ID.

### Existing server

With a compatible `subduction_cli` supporting `--auth open`, start a test server
in another terminal:

```sh
subduction_cli server --auth open --ephemeral-key --socket 127.0.0.1:8080 \
  --data-dir /tmp/stockfoil-demo-server --ready-file /tmp/stockfoil-demo-ready
```

Copy the `peer_id` (64 hex characters) from the ready file or server log:

```sh
cargo run -p stockfoil-mv-register -- \
  --server ws://127.0.0.1:8080 --server-peer <PEER_ID>
```

The peer ID is the server's **public** identity, not its private key seed. The
client explicitly checks the authenticated server identity. This demo requires
an open-policy server: it does not implement Keyhive delegation, so a server
using Keyhive's default access policy will reject its requests. Open policy is
for local testing, not deployment on the public Internet. Stockfoil author
verification remains enabled regardless of the server's storage policy.

See the [Subduction server documentation](https://github.com/inkandswitch/subduction/blob/main/subduction_cli/README.md)
for installation and server configuration. Build the CLI from the same sibling
checkout as the clients; this is not a stable cross-version protocol.

## Code

- [`src/lib.rs`](src/lib.rs): register assignments, cached values, and one
  `visit_changes` materializer for edits, signing, ingestion, historical reads,
  and permission changes.
- [`src/sync.rs`](src/sync.rs): uses `export_changes` and an `ExportCursor` to
  publish new certified cover items with `store_built_batch`, advancing the cursor
  only after successful storage. `Fragment::from_parts` accepts Stockfoil's
  checkpoint prefixes directly. Publication is local-only; the separate sync step
  requests sync/subscription and loads locally stored blobs into Stockfoil.
  Stockfoil owns pending items and
  payloads, automatically admitting them when dependencies arrive or are signed
  locally.
- [`src/wire.rs`](src/wire.rs): **example-only** versioned BCS envelopes for
  individual commits and fragments. Original author signatures are retained;
  the transport peer cannot replace them with its own signatures. Commit hashes
  are reconstructed from exact payload bytes before signature verification and
  payload decoding. Parent order and multiplicity are preserved in the envelope,
  even though Subduction's transport parents are sets.
- [`src/transport.rs`](src/transport.rs): authenticated WebSocket connections and
  background-task ownership/shutdown.

Permissions are explicit application configuration, never imported from network
payloads. Revocation changes local participation, not what authenticated history
may be relayed. Unsigned drafts are not exported. Fragment checkpoints use
Subduction's leading-zero-byte depth metric and 12-byte checkpoint IDs; complete
heads, boundaries, and authenticated contents remain in the envelope.

This minimal adapter assumes peers advertise transport metadata correctly; it
provides blobs to Stockfoil without cross-checking their transport heads, parents,
or fragment metadata. Incorrect advertisements can prevent convergence, but do
not bypass Stockfoil's payload, signature, or graph-admission checks.

Admission is atomic per item, not per batch. Network `load()` reports immediate
and deferred admission errors and still materializes valid items admitted before
the error. The register wrapper ignores deferred rejection reports from local
operations rather than logging them; applications can inspect `IngestReport` for
diagnostics. Queued received items are invisible to materialization until admitted,
unlike local unsigned drafts.

This deliberately small adapter polls rather than exposing a production event
loop. It caps blobs at 8 MiB and WebSocket messages at 16 MiB, but the graph's
pending-item queue currently has no limits. It has no encryption/compression or
client-side persistence and does not provide a general resource-budget policy.
It is not Stockfoil's proposed public binary `load`/`save` API.

## Test

```sh
cargo test                         # both default workspace members
cargo test -p stockfoil-mv-register # just this example
```

The WebSocket tests bind only `127.0.0.1:0` and need no external service. Other
checks cover malformed envelopes, payload/signature tampering, wrong server
identity, deferred dependencies, fragment relay, historical views, revocation,
and draft publication. Network tests and fixtures live in
[`src/sync/tests.rs`](src/sync/tests.rs).

For real Rust/JavaScript interoperability, see
[`../mv-register-js`](../mv-register-js/README.md). Its `npm run test:interop`
starts this binary with the test-only `--interop` JSON-lines control protocol
([`src/sync/interop.rs`](src/sync/interop.rs)). Control messages carry configuration
and commands; all history travels through the loopback Subduction server. The
JavaScript replica uses Stockfoil Wasm and the same sibling checkout's Subduction
Wasm WebSocket client directly. The default native demo is unchanged.
