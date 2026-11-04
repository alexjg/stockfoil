# Stockfoil

Authenticated commit histories for CRDTs, independent of what the commits contain.

Stockfoil keeps the commit DAG of a document: who wrote each commit, how commits
relate, which ones are authenticated, and the signatures needed to prove it to
others. Payloads stay in your storage, encoded by your codec, addressed by
logical IDs that Stockfoil hands back to you. It handles

- **hashing and signing** local commits, including assembling signed history into
  [sedimentree](design/wire-v2.md#7-fragment-membership-and-dependency-history)
  fragments;
- **receiving** items of any size with bounded memory, verifying canonical
  contents, signatures, and fragment placement before anything becomes visible;
- **exporting** what a peer or store lacks, tracked by a durable cursor;
- **materialization**: telling a CRDT which commits were delivered or changed
  participation as permissions or views change, without reading payloads.

## Using it from Rust

```rust,ignore
use stockfoil::*;

// Your codec and storage implement `Codec` and `Storage`; `MemoryStorage` is included.
let mut graph = Graph::new(MyCodec::new(), MemoryStorage::new());

// Local edits are drafts until signed.
let draft = graph.commit(author, parents, payload_bytes)?;
graph.sign(author, draft.hash, sign(&key, &draft.signing_bytes()))?;

// Send a peer whatever its cursor lacks.
let mut plan = graph.export(&peer_cursor);
for i in 0..plan.len() {
    let mut item = Vec::new();
    graph.write_item(&plan, i, &mut item)?;
    peer.send(item)?;            // ...and once the peer has stored it:
    plan.acknowledge(i)?;
}
peer_cursor = plan.cursor();

// Receive: verified and admitted atomically, or rejected.
let receipt = graph.receive(incoming.as_slice())?;
```

To update a materialized CRDT, keep a `MaterializationCursor` next to its state
and run the same loop after any local edit, receipt, signature, or permission
change:

```rust,ignore
let target = graph.history().view_point(&permissions);
let mut changes = graph.history().changes(&cursor, &target)?;
let mut records = Vec::new();
let next = loop {
    match graph.history().next_changes(&mut changes, &mut records, 1024)? {
        Batch::More => {}
        Batch::Complete(next) => break next,
    }
};
for change in records {
    if change.newly_delivered {
        state.store(change.hash, graph.payload(&change.hash)?); // even if not visible
    }
    match change.visibility {
        Vis::After => state.enable(change.hash),
        Vis::Before => state.disable(change.hash),
        Vis::Both | Vis::Neither => {}
    }
}
// Persist `next` atomically with `state`.
cursor = next;
```

## Using it from other runtimes

`Graph` is a thin synchronous driver. Underneath, receiving is an `Admission`
operation that asks its host to `Read` stored bytes, `Decode` with the
application codec, and `Bind` payload IDs, while it hashes and verifies everything
itself. Storage keys are the host's own: nothing assumes they are content hashes,
so an end-to-end encrypted store that names blobs by ciphertext digest works as is,
and payloads read back later are checked against their commits. A host can answer those requests from any runtime, worker, or language,
with several admissions and local edits in flight at once. The
[JavaScript package](javascript) does exactly that with promises.

## Documentation

- [Architecture and contracts](design/history-operations-api.md)
- [Wire format v2](design/wire-v2.md)
- [Authors, signatures, and write frontiers](design/authors_signatures_revocation.md)
- [Ancestor clocks](design/ancestor_vectors.typ)
- [Implementation status](IMPLEMENTATION_PROGRESS.md)
- [Verus verification: proved contracts and remaining boundaries](verification/README.md)

## Examples

- [`examples/mv-register`](examples/mv-register): a multi-value register with a
  bundle-compressing codec, signing, revocation, and replica sync.
- [`examples/file_storage.rs`](examples/file_storage.rs): a file-backed store,
  moving a 64 MiB payload between graphs with bounded memory
  (`cargo run --release --example file_storage`).
- [`examples/mv-register-js`](examples/mv-register-js): the register in JavaScript.
- [`subduction`](subduction): storage over Subduction blobs and the mapping between
  items and sedimentree metadata. It builds against a sibling `../subduction`
  checkout.

## Testing

```sh
cargo test --workspace                                       # unit, reference-model, fake-host tests
cargo test --release --test large -- --ignored --nocapture   # >4 GiB payload, bounded memory
cargo bench --bench history                                  # million-commit timings
(cd javascript && npm test)                                  # Wasm binding and JS driver
cargo +nightly fuzz list --fuzz-dir fuzz                     # fuzz targets; see fuzz/README.md
```
