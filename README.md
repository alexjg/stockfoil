# Stockfoil

A library for building CRDTs on top of authenticated commit DAGs.

## `no_std`

The default `std` feature preserves the standard-library API. Disable it for
`no_std` + `alloc` environments:

```toml
stockfoil = { version = "0.1.0", default-features = false }
```

A global allocator and pointer-width atomics are required. All graph operations,
including signature verification, remain available. Use
`stockfoil::collections::HashMap` for permission maps: it re-exports
`std::collections::HashMap` with `std`, and `hashbrown::HashMap` without it. The
no-std default hasher is not HashDoS-resistant; applications accepting untrusted
input should bound resource usage. No OS randomness is required. Ancestry caches
remain thread-safe; without `std`, concurrent readers may redundantly compute a
cache entry before one result wins publication.

Test the library in both configurations (selecting the package avoids enabling
`std` through the example workspace members):

```sh
cargo test -p stockfoil
cargo test -p stockfoil --no-default-features
cargo check -p stockfoil --no-default-features --lib --target thumbv7em-none-eabi
```

## Materializing a CRDT

Use `CommitGraph::visit_changes` as the standard update path. Keep a
`MaterializationCursor` alongside your materialized state, and run the same
callback after local edits, ingestion/merge, signing, or permission changes:

```rust
use stockfoil::MaterializationCursor;

// Initialize once, together with an empty application-specific materializer.
let mut cursor = MaterializationCursor::new();

// Repeat this update after changing the graph or its permissions.
let after = graph.view_point();
cursor = graph.visit_changes(&cursor, &after, |change| {
    materializer.apply(change);
})?;
```

Inside `apply`, store `change.meta` and `change.payload` when `newly_delivered` is
true, even if the commit is currently denied. Use `visibility` to enable
participation for `Vis::After` or disable it for `Vis::Before`. Each commit is
visited at most once per traversal, in ancestors-before-descendants order.
Keep any older operations needed to recompute document-level effects when
revocation reveals a previously overwritten value.

Pass `signed_view_point()` to restrict participation to admitted history; newly
delivered drafts still arrive for storage. Pass a captured `ViewPoint` for a
historical view. To read history without moving the live view, copy the
materialized state **and** its cursor before updating that copy.

Executable examples:

* [Multi-value registers](examples/mv-register): one callback for new assignments,
  revocation, cached current values, and historical reads, with a runnable
  Subduction WebSocket synchronization demo.
* [Observed-remove set](tests/orset/mod.rs): the delivery-only case, processing
  `newly_delivered` events with the same cursor API.

See the [materialization API guide](design/authors_signatures_api.md#incremental-materialization)
for the full contract. `diff`, `traverse`, and `traverse_since` are lower-level
options for graph inspection and specialized consumers, not prerequisites for
materialization.

Run the register demo (starts a local Subduction server automatically):

```sh
cargo run -p stockfoil-mv-register
```

See its [README](examples/mv-register/README.md) for connecting to an existing
server. `cargo test` runs the library and example tests.

## JavaScript

The [JavaScript package](javascript) exposes the same graph through Wasm, with
TypeScript declarations, byte payloads, and cursor-based change batches. It uses
wasm-bodge for Node.js/browser packaging; signing and transport stay outside the
library.

The [JS register example](examples/mv-register-js) synchronizes with the Rust
register through a real Subduction WebSocket server. After building the package
(see its [build instructions](javascript/README.md)):

```sh
npm --prefix examples/mv-register-js ci
npm --prefix examples/mv-register-js test
npm --prefix examples/mv-register-js run test:interop
```

## Receiving out-of-order history

`CommitGraph::ingest` accepts verified items in any arrival order. It captures
payloads immediately and queues items whose dependencies are not yet admitted.
Queued received items are not delivered history: they do not affect viewpoints,
materialization, or export. Local unsigned drafts remain a separate concept.

Admission, including local signing, automatically wakes waiting items.
`ingest`, `ingest_signature`, `ingest_local_commit`, and `merge` return an
`IngestReport` describing admitted certificates, deferred rejections, and the
remaining pending-item count. Inspect `rejected`: a valid dependency can expose
an invalid queued fragment without itself being rejected. Admission is atomic
per item, not per batch. `pending_items()` and `missing_dependencies()` expose
queue progress. There are currently no queue resource limits.

## A history lesson

This library grew out of the work I have been doing on
[Automerge](https://github.com/automerge/automerge). Automerge is implemented
by storing and transmitting the operations of the underlying CRDT in a commit
DAG. We've been working on authentication and authorization for Automerge
documents. The basic idea is that we sign every commit in the document and
then encrypt the commits and synchronize the encrypted commit graph.

This is conceptually very simple. What complicates it is that Automerge 
documents rely heavily on compressing the commit graph. We produce commits at
a very granular level (e.g. per-keystroke). To avoid generating and storing
a hash for every commit we encode bundles of commits together using a
run-length encoding and re-derive the hashes when we need them. Naively syncing
the entire commit graph will cause a 32x blow up in bandwidth usage (in typical
scenarios) due to having to transmit the commit hash.

To solve this we introduced
[sedimentree](https://github.com/inkandswitch/subduction/blob/main/design/sedimentree.md)
which works a little like a distributed LSM tree where peers which have the
cleartext can produce larger compressed runs for others to use. This design
allows us to incrementally sync encrypted CRDTs whilst retaining the low
bandwidth usage the compression affords us. 

Sedimentree solves the problem of metadata overhead due to commit hashes.
Unfortunately the requirements of the particular authentication mechanism we
have chosen introduce a new overhead. Recall that we are authenticating by 
signing every commit. The problem with this is that we now have to transmit
the signature for every commit. This is similar to the problem with hashes in
that it massively increases bandwidth usage for small, granular commits.

Happily, we can take advantage of typical editing patterns here. Typically
small granular commits (like keystrokes) are issued in long chains with
occasional concurrency. In a long chain of commits the hash of the final commit
in the chain covers everything previously in the chain. This means that we
only need to retain the signatures on commits where the author changes. 

Thus, in Automerge  we encode an "author" field on every commit. This is an
arbitrary byte array set by the user. We also add a signature to each commit,
but it is outside the bytes covered by the hash of the commit. This means that
when compressing ranges of commits together we can discard signatures on
commits where the parent commits have the same author and in this way we can
verify the author of every commit.

## Extracting the idea

All of this work is really quite independent of the Automerge CRDT and should
be useful anytime you have the following things:

* A data structure which can be expressed as a DAG of deltas encoded in some
  binary fashion
* Where the deltas themselves can be encoded more efficiently if encoded in
  batches

## Stockfoil

If you have a data structure which meets these requirements then you can use
stockfoil. You have to provide a trait which looks like this:

```rust
trait Commit {

}
```

And then you get out of it:

* An efficient binary format for encoding the commit graph as both individual
  commits and bundles of commits
* An implementation of the "fragments" structure that sedimentree expects
* A sans-IO state machine based API to the commit graph which works in a fixed
  amount of memory and spools everything to/from disk/network.
* An in-memory synchronous API to the commit graph
* An asynchronous API to the commit grap

## Fixed Memory

One of the goals of stockfoil is to make it possible to perform all interactions
with the commit graph in a constant amount of memory. The idea here is to make
it easy to avoid DoS attacks. We structure everything so that given a small
buffer, all work can be performed by spooling from disk/network into the buffer
and incrementally writing computations out again.

There are a few operations which are important from this perspective:

* Validating the hash graph of incoming changes
* Materializing the state of the CRDT

### Validating the hash graph

When we say "validate the hash graph" what we mean is that there are some invariants
we want to maintain:

* The hash graph linking every commit in the stockfoil DAG has been verified (i.e.
  the referenced parent exists)
* If we are validating signatures, then every commit which changes author (i.e. has
  a parent with a different author to the current author) has a signature and the
  signature is valid for the given author.

Now, `stockfoil` ships commits around as sedimentree fragments. This means that we
can ensure that we have verified the dependencies of a a fragment before we verify
the fragment itself. We design the fragment layout so that it can be verified in a
streaming fashion. This covers verifying the hash graph.

### Materializing the view

Cold starts and incremental updates use the same `visit_changes` callback. An
empty cursor starts a new materialization; a retained cursor includes newly
delivered commits and participation changes since the previous update. See
[Materializing a CRDT](#materializing-a-crdt) above.

Stockfoil does not interpret application payloads. The materializer decides how
commit participation affects CRDT values and retains the data needed for
revocation and historical views.

### Encoding Commits

Assume that a single commit is larger than the memory we want to use. Verifying this
in an incremental fashion is fine, how do we actually work with it at the application
level. Let's say we have something like this:

```rust
struct DocSet {
    docs: HashSet<DocumentId>,
}
```

And let's say each commit is basically just a list of document IDs. What do we do if
we encounter a commit which is just 5 billion document IDs? In principle we should 
be able to scan through the commit and incrementally pass the document IDs out to 
some separate process which can write it to a database. We never need to have the 
whole thing in memory.

Likewise when we _write_ the commit we can write it out in pieces to storage and when
we seal the commit we can write the header.

What might this look like:

```rust
struct DocSet {
    storage: Storage,
}

impl DocSet {
    fn encode_commit(hash: CommitHash) -> CommitEncoder {
        CommitEncoder { storage: self.storage, hash}
    }

    async fn has_doc(&mut self, doc: DocumentId) -> Result<bool, Error> {
        
    }
}

struct CommitEncoder {
    storage: Storage,
    hash: CommitHash,
}

impl CommitEncoder {
    fn encode(out: Buf) -> Result<Status, Error> {
        
    }
}
```
