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
  requests sync/subscription and loads locally stored blobs with `graph.ingest`.
  Publication forwards each `ExportedItem`'s `metadata` and opaque `bytes`.
  Stockfoil owns pending items and
  payloads, automatically admitting them when dependencies arrive or are signed
  locally.
- [`src/codec.rs`](src/codec.rs): `RegisterCodec`, selected by
  `CommitGraph::new(RegisterCodec)`. Assignments remain canonical BCS pairs of
  UTF-8 strings. Fragment payloads use a first-occurrence key dictionary,
  `Vec<(u32, u32)>` key runs (fixed little-endian index/count pairs), and values
  in member order. Decoders check counts and canonical byte budgets before
  expansion. Encoders have no fixed 8 MiB cap: `DecodeLimits` applies only to
  inbound items, not local edits or exports. Receivers can raise their graph's
  limits before retrying larger items. The format ID is `stockfoil/mv-register/v1`.
  Stockfoil owns its binary envelope, hashes, original signatures, and admission;
  the example never parses graph metadata from bytes.
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

`prepare_set`, `set`, `publish`, `fork`, and `merge` return `eyre::Result`; callers
must propagate or handle codec, signing, and inbound admission failures. For example:

```rust
let mut alice = MvRegisterMap::new();
alice.set("title", "Hello")?;
let mut bob = alice.fork()?;
bob.merge(&alice)?;
```

Admission is atomic per item, not per batch. Network `load()` reports immediate
and deferred admission errors and still materializes valid items admitted before
the error. `merge()` likewise materializes earlier admissions on failure and keeps
Stockfoil's `MergeError<std::io::Error>` (including its partial `IngestReport`) in
the `eyre` error chain, available via `downcast_ref`. The register wrapper ignores
deferred rejection reports from successful local
operations rather than logging them; applications can inspect `IngestReport` for
diagnostics. Queued received items are invisible to materialization until admitted,
unlike local unsigned drafts.

This deliberately small adapter polls rather than exposing a production event
loop. The default inbound graph budget is 8 MiB per item/canonical payload, and
WebSocket messages are capped at 16 MiB, but the graph's pending-item queue
currently has no limits. Successful local encoding does not imply that a receiver
with default budgets or a transport with smaller limits can accept the item. It has no encryption or client-side
persistence and does not provide a general resource-budget policy.
It uses Stockfoil's public codec-owned encoded API. Codec limits are cooperative;
applications must choose a trusted codec rather than selecting executable code
from untrusted incoming data.

## Test

```sh
cargo test                         # both default workspace members
cargo test -p stockfoil-mv-register # just this example
```

The WebSocket tests bind only `127.0.0.1:0` and need no external service. Other
checks cover malformed envelopes, payload/signature tampering, wrong server
identity, deferred dependencies, fragment relay, historical views, revocation,
and draft publication. Shared Rust/JS golden payload vectors test canonical
encoding below, at, and above 8 MiB, with exact and raised decode budgets; these
tests use no network. Failed-merge tests check retained partial reports and live
materialization. Network tests and fixtures live in
[`src/sync/tests.rs`](src/sync/tests.rs).

For real Rust/JavaScript interoperability, see
[`../mv-register-js`](../mv-register-js/README.md). Its `npm run test:interop`
starts this binary with the test-only `--interop` JSON-lines control protocol
([`src/sync/interop.rs`](src/sync/interop.rs)). Control messages carry configuration
and commands; all history travels through the loopback Subduction server. The
JavaScript replica uses Stockfoil Wasm and the same sibling checkout's Subduction
Wasm WebSocket client directly. The default native demo is unchanged.
