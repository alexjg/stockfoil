# Stockfoil

A library for building CRDTs on top of authenticated commit DAGs.

## Payload codecs and encoded history

Construct `CommitGraph::new(codec)` with an application `PayloadCodec`. Both
canonical commit encoding and **whole-fragment compression** are required: the
application can use columns, dictionaries, or runs across commits. Stockfoil
owns the envelope, compressed graph metadata, signature verification, admission,
and encoded exports. Signing keys, permissions, materialization, and transport
remain application concerns.

```rust
let mut graph = CommitGraph::new(RegisterCodec);
let draft = graph.commit(author, assignment)?;
// Sign draft.signing_bytes externally; inspect reports for deferred rejections.
let report = graph.ingest_signature(author, draft.hash, signature)?;

let (changes, next) = graph.export_changes(&published)?;
adapter.store(changes.added).await?; // Each item has metadata and opaque bytes.
published = next;                  // Only after successful batch storage.
let received = graph.ingest(&encoded_bytes)?;
```

See [`CODEC.md`](CODEC.md) for the contract and
[`design/codec-wire-v1.md`](design/codec-wire-v1.md) for the interoperable v1
format. [Implementation notes](design/codec-implementation.md) cover the current
Hexane integration and compression measurements. Payload compression is still
application-defined. The [Rust register codec](examples/mv-register/src/codec.rs)
and [JS codec](examples/mv-register-js/codec.mjs) implement matching BCS assignment
bytes and key-dictionary/run fragment packing.

`CommitGraph<C: PayloadCodec>` is the single graph implementation: it owns the
codec, typed payloads, history, and received queue directly. Structured insertion
and export helpers are private. Both synchronization and persistence use
`export_changes(cursor)` → store/send `EncodedItem.bytes` → `ingest(bytes)`.
An empty cursor exports the full current signed inventory; advance a destination's
cursor only after successful storage. `ItemId` identifies inventory entries in
reports and retirements, while `TransportMetadata` supplies the extra parents and
checkpoint hints adapters need. Structural summaries and cover-selection types
are not public APIs. The `*Shape` types remain public inputs to application codecs.
No Subduction dependency is introduced into Stockfoil core.

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
* [Observed-remove set](src/commit_graph/tests/orset/mod.rs): the delivery-only case, processing
  `newly_delivered` events with the same cursor API.

See the [materialization API guide](design/authors_signatures_api.md#incremental-materialization)
for the full contract. `diff`, `traverse`, and `traverse_since` are lower-level
options for graph inspection and specialized consumers, not prerequisites for
materialization.

The register examples temporarily depend on a sibling Subduction checkout at
`../subduction`. The Rust example uses `subduction_tokio` for managed outbound
connections and task ownership. See their READMEs for local Rust/Wasm setup.

Run the register demo (starts a local Subduction server automatically):

```sh
cargo run -p stockfoil-mv-register
```

See its [README](examples/mv-register/README.md) for connecting to an existing
server. `cargo test` runs the library and example tests.

## JavaScript

The [JavaScript package](javascript) exposes the same graph through Wasm, with
TypeScript declarations, typed payload codecs, and cursor-based change batches.
`new CommitGraph(codec)` uses the native envelope; payloads are structured-cloned
and synchronous codec re-entry is rejected. It uses wasm-bodge for Node.js/browser
packaging; signing and transport stay outside the library.

The [JS register example](examples/mv-register-js) synchronizes with the Rust
register through a real Subduction WebSocket server. After building the package
(see its [build instructions](javascript/README.md)):

```sh
npm --prefix examples/mv-register-js ci
npm --prefix examples/mv-register-js test
npm --prefix examples/mv-register-js run test:interop
```

## Receiving out-of-order history

`CommitGraph::ingest` accepts encoded bytes in any arrival order. It decodes and
verifies before mutation, then queues items whose dependencies are not yet admitted.
Queued received items are not delivered history: they do not affect viewpoints,
materialization, or export. Local unsigned drafts remain a separate concept.

Admission, including local signing, automatically wakes waiting items.
`ingest` and `ingest_signature` return an `IngestReport`; `commit` includes one
in its `Draft`. Native `merge` returns a report on success and retains partial
progress in `MergeError::report` on failure. Inspect `rejected`: a valid dependency
can expose an invalid queued fragment without itself being rejected. Admission
is atomic per item, not per batch. `pending_items()` and `missing_dependencies()`
expose queue progress.

`DecodeLimits` bounds each inbound envelope and its reconstructed canonical
payloads. It does not limit trusted local commits/exports, the whole graph, or
the pending queue. Certified fragments can grow beyond the defaults; a receiver
may explicitly increase its budget and retry with `set_decode_limits` (JS:
`setDecodeLimits`). Application decoders must cooperate with these budgets.
There are currently no aggregate queue limits or authenticated fragment chunking.

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

Implement `PayloadCodec` for a data structure meeting these requirements. You get:

* A shared binary envelope for individual commits and compressed fragments.
* Authenticated certified covers and transport metadata for Sedimentree.
* An in-memory synchronous graph with out-of-order admission, drafts, revocation,
  materialization cursors, and retry-safe export snapshots.
* The same encoding and verification pipeline in Rust and Wasm/JavaScript.

## Fixed memory: future direction

The current implementation is in-memory, not a fixed-memory or asynchronous
storage engine. The following sections describe future streaming goals, not
properties supplied by the current `Vec`-based codec API.

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
