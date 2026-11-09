# Stockfoil

`stockfoil` is a library for building local-first data structures (CRDTs, key-value
stores, replicated disk images) on top of an authenticated commit graph.

If you already know the basic idea of a commit graph—a DAG where each edit is a
commit with parent hashes, an author, and an operation payload—building a real
local-first system on top of one quickly runs into three problems:

1. **Incremental view maintenance (materialization):** Your application state is a
   view over the commit graph. As commits arrive from peers in arbitrary orders,
   as users inspect historical branches, or as commits are retroactively excluded,
   you need to update that state incrementally (or know when to reset and replay
   it).
2. **Compaction without losing verifiability:** Fine-grained editing produces tens
   of thousands of tiny commits. You need to bundle ranges of the commit graph
   into compressed chunks so storage and sync stay fast, while still reconstructing
   the exact individual commits inside.
3. **Authentication and authorization without a central server:** Peers edit
   offline and sync peer-to-peer or through untrusted relays. You need to verify
   who authored every commit (even inside compacted bundles), and you need a way
   to revoke or bound a writer's permissions when there is no central gatekeeper or
   trusted wall clock—without deleting commits that later valid commits causally
   depend on.

`stockfoil` handles the commit graph, incremental materialization planning,
compaction metadata, cryptographic verification, and permission filtering without
performing I/O or imposing an async runtime. Your application provides two things:
- a **`Materializer`** (or an external `Materialization` loop) that folds commits
  into your data structure, and
- a **`Codec`** that packs and unpacks commit payloads inside compacted bundles
  (or you can start with the built-in `Concatenated` codec).

---

## Getting Started

Requires **Rust 1.91 or newer**.

```toml
[dependencies]
stockfoil = "0.1"
```

Enable `features = ["sedimentree"]` for optional Sedimentree export metadata.

When your data structure and commit payloads fit in memory, you can get a working,
synchronized replica in three steps using `Replica<C, M>`:

### 1. Implement `Materializer` for your state

Your state can be any Rust struct. Implementing `Materializer` requires a single
method, `apply`, which receives batches of `Event`s as the commit graph changes:

```rust
use std::collections::BTreeMap;
use stockfoil::{Applied, Bytes, Event, History, Materializer};

#[derive(Default)]
struct KvStore {
    entries: BTreeMap<String, String>,
}

impl Materializer for KvStore {
    type Error = &'static str;

    fn apply(
        &mut self,
        _history: &History,
        events: Vec<Event<Bytes>>,
    ) -> Result<Applied, Self::Error> {
        let mut applied = Applied::default();
        for event in events {
            match event {
                Event::Reset => self.entries.clear(),
                Event::Deliver { hash, payload, active, .. } => {
                    let Some((k, v)) = std::str::from_utf8(&payload)
                        .ok()
                        .and_then(|s| s.split_once('='))
                    else {
                        applied = applied.reject(hash, "expected key=value");
                        continue;
                    };
                    if active {
                        self.entries.insert(k.to_owned(), v.to_owned());
                    }
                }
                _ => {}
            }
        }
        Ok(applied)
    }
}
```

### 2. Create a `Replica` and make local commits

To start without writing a custom bundle `Codec`, use the built-in `Concatenated`
codec along with a `FormatId` and an `Author` (derived from an Ed25519 signing
key):

```rust,ignore
let alice_key = SigningKey::from_bytes(&[1; 32]);
let alice = Author::from(&alice_key);
let format = FormatId::new("example/kv-v1")?;

let mut alice_doc = Replica::new(
    Concatenated::new(format.clone()),
    alice,
    KvStore::default(),
);

alice_doc.commit(b"title=Hello")?;
alice_doc.sign_drafts(&alice_key)?;
assert_eq!(alice_doc.state().entries["title"], "Hello");
```

### 3. Sync with another peer

By default, a new replica only permits commits by its own local author. Grant
peers write permission via `set_permissions` and exchange wire items with `export`
and `receive`:

```rust,ignore
let mut bob_doc = Replica::new(Concatenated::new(format), bob, KvStore::default());
bob_doc.set_permissions(WritePermissions::from([
    (alice, WritePermission::Unbounded),
    (bob, WritePermission::Unbounded),
]))?;

let export = alice_doc.export(&ExportCursor::new());
for i in 0..export.len() {
    let (_meta, item_bytes) = export.item(i)?;
    bob_doc.receive(&item_bytes)?;
}
assert_eq!(bob_doc.state().entries["title"], "Hello");
```

Those three steps raise three natural questions, which are the focus of the core
concepts below:
1. Why does `Event::Deliver` have an `active: bool` flag, why did `KvStore` handle
   `Event::Reset`, and what are the other `Event` variants?
2. What are the wire items produced by `export.item(i)`, and when should you
   replace `Concatenated` with a custom `Codec`?
3. Why did `bob_doc` need explicit `WritePermissions`, how are compacted items
   verified, and how do you revoke an author after a specific commit?

---

## Core Concepts

### 1. Materialization and Incremental View Maintenance

A `Materialization` maintains one view of the commit graph, following a `Follow`
rule:
- `Follow::Working`: All delivered commits, including unsigned local `Draft`s.
- `Follow::Signed`: Strictly authenticated history.
- `Follow::Pinned(heads)`: A fixed historical snapshot or branch.

As commits arrive or the active view changes, `Materialization` diffs your state's
current position against the target view and emits `Event`s in canonical
ancestor-first order:
- **`Event::Deliver { hash, author, parents, payload, active }`:** Emitted once
  for every new commit entering the graph. **Delivery is separate from
  visibility:** even when a commit does not participate in the current view
  (`active: false`), a later active commit may have it as a parent and reference
  structural metadata introduced in it (such as an operation ID or key). Your
  state always receives the commit so it can index whatever metadata it needs, and
  applies the user-visible effect only when `active` is `true`.
- **`Event::Activate` and `Event::Retract` (or `Event::Reset`):** When a branch
  switch or permission change alters whether an already-delivered commit
  participates, data structures that set
  `Materializer::supports_visibility_changes() == true` receive incremental
  `Activate` and `Retract` events (and can request older overwritten payloads via
  `Applied::want` -> `Event::Payload`). Data structures that leave
  `supports_visibility_changes()` as `false` still apply forward edits
  incrementally via `Deliver`, and automatically receive `Event::Reset` followed by
  a clean replay whenever a commit needs to be retracted.
- **Rejecting invalid payloads (`Applied::reject`):** If a commit contains a
  corrupt application payload, your materializer can mark it permanently rejected
  (optionally cascading to descendants) without breaking the shared commit graph.

### 2. Compaction and Implementing a `Codec`

Storing and syncing every commit individually carries ~130+ bytes of hashes, keys,
and signatures per edit and prevents compressing redundant data across commits.

`stockfoil` exchanges history as wire items (`ItemId`), which are either a
standalone `Commit` or a compacted **Sedimentree `Fragment`**:
- **Deterministic boundaries:** Commit hashes with $D > 0$ leading zero bytes
  (their *depth*) deterministically define a fragment from that `head` commit back
  to the nearest ancestor commits of depth $\ge D$ (the fragment's `boundary`).
  Once admitted, a fragment automatically retires the standalone commits and
  smaller fragments it covers.
- **Your `Codec`:** Every wire item has a header (managed by `stockfoil`, encoding
  the $N$ members' authors, parent topology, and signatures) and a body (managed
  by your `Codec`). Your `Codec::encode` packs the $N$ canonical commit payloads
  into the item's body—for example, deduplicating keys or columnar-compressing
  across all $N$ commits. The only requirement is a **lossless round-trip**:
  `Codec::decode` must reconstruct each member's exact canonical payload bytes. If
  you don't need cross-commit compression yet, use `Concatenated`.

### 3. Authentication, Authorization, and Write Frontiers

In a centralized server app, permissions are checked against a wall clock when a
write arrives: if Bob's access was revoked at 2:00 PM, the server rejects any
write from Bob after 2:00 PM.

In a local-first commit graph, there is no central server and no trusted wall
clock. Worse, **you cannot enforce permissions by deleting unauthorized commits
from the commit graph**: if Alice (who has write access) synced with Bob while
offline and created her own valid commits on top of Bob's commit before learning
Bob was revoked, dropping Bob's commit would break the hash chain and make Alice's
valid commits unverifiable.

`stockfoil` separates **authentication** (an immutable property of the graph) from
**authorization** (which commits participate in your materialized state):

- **Authentication and transitive signatures (`Author`, `CommitHash`, `Signature`):**
  Every commit is identified by a 32-byte BLAKE3 `CommitHash` over its ordered
  parent hashes, its `Author` (a 32-byte Ed25519 public key), and the hash of its
  canonical payload. Inside a compacted fragment, because each commit hash commits
  to its parents' hashes, an author's signature near the tip of the fragment
  *transitively authenticates* all of that author's ancestor commits inside the
  fragment. Reconstructing the exact canonical payloads via `Codec::decode` lets
  the receiver recompute every `CommitHash` and verify all authors' signatures
  without keeping a separate signature on every commit.
- **Authorization via write frontiers (`WritePermissions`, `WritePermission`):**
  In a causal graph, the only tamper-proof notion of time is *position in the DAG*.
  When an author's write access is revoked, what you actually know is the point in
  history where the revocation took effect—a set of commit hashes called a **write
  frontier**:
  - `WritePermission::Unbounded`: All of the author's commits participate.
  - `WritePermission::Bounded(frontier)`: Only commits by this author in the causal
    past of `frontier` (ancestors-or-self) participate. Anything written causally
    after that frontier—or on an unmerged fork—is excluded.
  - `WritePermission::Denied`: None of the author's commits participate.

  Authors not listed get the policy's unlisted rule: `Denied` by default, or for
  example `WritePermissions::new().with_unlisted(WritePermission::Unbounded)` to
  admit everyone when another layer decides who may write.

Your application evaluates its own access-control rules and hands the resulting
`WritePermissions` map to `set_permissions`. Disallowed commits stay in the graph
so fragments, sync, and child commit hashes remain valid, while the
materialization layer delivers them with `active: false` or retracts their effects
from your state.

---

## Out-of-Core / Sans-I/O Operation (`Ledger` + `Materialization`)

When payloads live on disk or are too large to hold in RAM (see
[`examples/volume`](https://github.com/alexjg/stockfoil/tree/main/examples/volume)), use `Ledger` and `Materialization`
directly. `Ledger` tracks graph topology, permissions, and caller-assigned `u64`
`PayloadId`s without holding payload bytes in memory:

- Stream local commits into storage and a `CommitBuilder` simultaneously, then call
  `ledger.commit(prepared, payload_id)`.
- Drive an owned `Admission` state machine (`ledger.begin_receive()`) with
  fixed-size read/decode buffers, then call `ledger.admit(op, bind)`.
- Plan and complete materialization batches against `&Ledger`, persisting your
  state and `checkout.checkpoint().to_bytes()` atomically after all follow-up
  batches finish:

```rust,ignore
while let Some(mut batch) = checkout.plan(&ledger, 64)? {
    loop {
        let applied = image.apply(&batch, &my_storage).await?;
        match checkout.complete(&ledger, batch, applied)? {
            Some(more) => batch = more,
            None => break,
        }
    }
    image.persist(checkout.checkpoint()).await?; // state + checkpoint, atomically
}
```

---

## Documentation and Examples

- [Crate documentation](https://docs.rs/stockfoil): full guide
  with a runnable in-memory CRDT example.
- [Permissions and views](https://github.com/alexjg/stockfoil/blob/main/stockfoil/design/permissions.md): deep dive into write frontiers,
  selected history, inactive delivery, and payload rejection.
- [Architecture and reference](https://github.com/alexjg/stockfoil/blob/main/stockfoil/design/architecture.md): layered ownership,
  chain-prefix clocks, streaming admission, exports, and crash recovery.
- [Wire specification](https://github.com/alexjg/stockfoil/blob/main/stockfoil/design/spec.md): wire v1 encoding, commit identity,
  authentication rules, and fragment invariants.
- [Multi-value register (Rust)](https://github.com/alexjg/stockfoil/tree/main/examples/mv-register) and
  [(JavaScript)](https://github.com/alexjg/stockfoil/tree/main/examples/mv-register-js): in-memory CRDT with key-deduplicating
  bundle codec, synchronization, and revocation.
- [Sparse block volume](https://github.com/alexjg/stockfoil/tree/main/examples/volume): out-of-core streaming I/O, SQLite index,
  lazy historical reads, and crash-recoverable redo journaling.
- [JavaScript/Wasm bindings](https://github.com/alexjg/stockfoil/tree/main/javascript).
- [Sedimentree export](https://docs.rs/stockfoil/latest/stockfoil/sedimentree/): enable the optional
  `sedimentree` feature to convert exports into `LooseCommit` / `Fragment`
  metadata. Storage and networking remain application-owned; see the
  [Subduction integration guide](https://github.com/alexjg/stockfoil/blob/main/examples/mv-register/SUBDUCTION.md).

- [Verus verification](verification/README.md)

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo doc -p stockfoil --no-deps --open
cargo bench -p stockfoil --bench history
cargo check --manifest-path fuzz/Cargo.toml --bins
cargo test --manifest-path fuzz/Cargo.toml --lib
./scripts/ci/test-package
```

`test-package` builds the crate archive and tests its extracted contents outside
this repository, including doctests and the optional `sedimentree` feature.
It does not publish anything. CI uses Rust 1.91, the minimum supported version.

## License

Licensed under the [MIT License](https://github.com/alexjg/stockfoil/blob/main/LICENSE).
