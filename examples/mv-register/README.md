# Multi-value register

An example CRDT on Stockfoil's native `Graph<C, S>`: a map from keys to
multi-value registers. Concurrent writes to a key are all kept; a write
replaces every write it causally follows.

```sh
cargo run -p stockfoil-mv-register    # two replicas, concurrent writes, revocation
cargo test -p stockfoil-mv-register
```

## Code

- [`src/lib.rs`](src/lib.rs): `MvRegisterMap`.
  - **Writes.** `prepare_set` commits an unsigned draft on every delivered head.
    `publish` signs the drafts, parents first, with the replica's Ed25519 key,
    which admits them. `set` does both. Drafts are visible locally but never
    exported.
  - **Materialization.** One path serves edits, receipt, permission changes,
    historical reads (`get_at`, on a copy), and cold starts (`rematerialize`).
    It runs `History::changes` from the saved `MaterializationCursor`, then
    `next_changes` batches. Newly delivered assignments are fetched and kept
    even if they do not participate now. A later permission change can enable
    them without delivering them again. Payloads are fetched with
    `Graph::payload` for canonical blobs. Members of one stored item are fetched
    with a single `Graph::decode_item` pass. `Vis::After` adds a write to the
    participating set and `Vis::Before` removes it. Each affected key is then
    recomputed from its participating writes, keeping only the causally maximal
    ones. Hiding a write can reveal an older one that produced no change record.
  - **Permissions.** These are local configuration (`set_write_permissions`),
    never taken from the network. `Unbounded`, `Bounded(frontier)` and `Denied`
    (or a missing author) select which authenticated writes participate.
  - **Sync.** `sync_to` exports what a peer has not acknowledged. The cursor is
    an `ExportCursor` per peer, keyed by the peer's author. The peer stores and
    admits each item. The cursor advances only over items the peer stored. Items
    it just received are also marked as known for the reverse direction, so
    they are not echoed back. Plans list items in head delivery order, which puts
    most dependencies first, but an item can still arrive before one of them. Such
    an item is kept as a verified `Admission` and driven again when history
    changes, with no re-reading or re-verification.
    `export_items` and `receive` are the byte-level halves for a real transport.
- [`src/codec.rs`](src/codec.rs): `RegisterCodec` (format
  `stockfoil/mv-register/v2`). The canonical assignment is
  `uleb(key.len) key uleb(value.len) value`. The encoded body writes each key
  once per bundle: later members refer to it by its index in a dictionary built
  as the body is read. Both directions stream with bounded state: keys are
  limited to 1 KiB, the dictionary to 4096 keys or 256 KiB, and values pass
  through in caller-sized pieces. The encoder reads payloads from storage by
  `PayloadLocation`. A `Member` location is decoded from the stored item that
  holds it. Consecutive members of one item share a single decoding pass.

Tests cover register semantics, concurrency and three-way merges, drafts and
signing, tampering, revocation and restoration, and frontiers that arrive later.
They also check cold-start against incremental materialization across several
change batches, persisted cursors, fragments relayed from stored members,
out-of-order delivery, per-peer cursors, and randomized convergence.

## Why there is no Subduction transport

The previous version synchronized through a Subduction WebSocket server. That
adapter was built on Stockfoil APIs the rebuild removed:

- `TransportMetadata` gave commit parents and fragment checkpoint lists for
  Subduction's `LooseCommit` and `Fragment`.
- `CommitGraph::ingest` held a pending queue that admitted items automatically
  when their dependencies arrived.

Porting it would mean deriving checkpoints from `BundleMeta` and rebuilding
the pending queue. The async client and server and the JavaScript interop
harness would also have to be ported. That is most of the old example's code,
and it is not about the register. This version keeps the transport-independent
part: export cursors, encoded items as plain bytes, and admissions that wait
for dependencies. Any byte transport can carry `export_items` output to
`receive`.

Not covered: persistence of the graph or materialized state, encryption, and
resource limits beyond Stockfoil's default `Limits`.
