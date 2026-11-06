# Multi-value register map

A CRDT on Stockfoil's `Replica<RegisterCodec, Register>`. Concurrent writes to a
key coexist; causally later writes
supersede them. Revoking a write can reveal an earlier value again.

- [`src/lib.rs`](src/lib.rs): `Register` implements `Materializer`, retaining
  inactive writes as metadata and updating affected keys once per batch.
  `MvRegisterMap` provides application methods and per-peer export cursors.
- [`src/codec.rs`](src/codec.rs): pure byte codec, deduplicating keys across a
  bundle. Canonical payloads encode one assignment without the dictionary.
- [`src/main.rs`](src/main.rs): offline edits, direct in-memory synchronization,
  and write-frontier revocation.

History, permissions, delivery progress, retraction and signing orchestration
belong to Stockfoil, not the register. Historical reads create independent
pinned materializations under the document's current permissions.

```sh
cargo run -p stockfoil-mv-register
cargo test -p stockfoil-mv-register
```

The main example demonstrates offline edits, synchronization, concurrent values,
and bounding a writer's permitted history. The tests additionally cover drafts,
relay, revocation/reactivation, cold rebuilds, and out-of-order items.

## Sync with Subduction

See [Syncing the register with Subduction](SUBDUCTION.md) for exporting
Sedimentree metadata with Stockfoil's optional `sedimentree` feature and driving
storage, synchronization, and incoming updates with an application-owned node.
