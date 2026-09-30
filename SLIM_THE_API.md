# Slimming the Stockfoil synchronization examples

## Motivation

The Rust and JavaScript multi-value-register examples demonstrate a useful idea: implement a CRDT's payload and materializer, then use Stockfoil for authenticated history and Subduction for synchronization. Unfortunately, the amount of surrounding code makes that idea look much harder than it ought to be.

The examples originally implemented several reusable library responsibilities: a wire protocol, conversion to Subduction records, asynchronous task ownership, connection cleanup, and replication-progress observation. Stockfoil now owns the envelope; the remaining transport responsibilities are still visible. They also demonstrate advanced register features and, especially in JavaScript, act as comprehensive integration tests.

The goal should be that an application author writes **their payload and materializer, plus a small publication/ingestion loop**. They should not need to implement a wire protocol or supervise Subduction's runtime tasks.

This document examines three ways to get there:

1. Improve the Subduction API.
2. Improve the Stockfoil API.
3. Remove unnecessary scope and defensive complexity from the examples.

Subduction API sketches below remain proposals. The follow-ups record adopted APIs and the implemented Stockfoil codec work.

## Follow-up: implemented payload codecs

[`CODEC.md`](CODEC.md) refines and implements the original default-codec proposal: one Stockfoil envelope, with **required application commit and whole-fragment codecs** owned by the graph. Rust and Wasm share framing, Hexane metadata columns, authentication, and admission. Both examples now ingest opaque bytes and export transport metadata without implementing a Stockfoil wire protocol.

The former `wire.rs` and `wire.mjs` are removed. Application `codec.rs`/`codec.mjs` retain only canonical BCS assignments and genuine key-dictionary/run fragment compression. Permissions, external signing, deferred-rejection reports, and retry-safe publication remain explicit. The APIs are `CommitGraph::new(codec)` / `new CommitGraph(codec)`. One graph directly owns codec orchestration and history; structured insertion/export helpers are private rather than a second public API.

## Follow-up: using the local Subduction checkout

Both examples now temporarily use `../subduction`: Rust path dependencies for the Subduction/Sedimentree crates, and an npm `file:` dependency on `subduction_wasm`. The JS package must be built into its `dist/` directory; linking the source package does not build Wasm. See the [JS example setup](examples/mv-register-js/README.md#run).

At the earlier inspected checkout (`0e697e07`; see the managed-client follow-up below):

| Capability | Status |
| --- | --- |
| Mixed store-only batches | Already implemented; now used by both examples |
| Fragment construction from 12-byte checkpoint prefixes | Already implemented; now used by both examples, replacing member scans |
| Managed Tokio client and async JS runtime close | Still needed |
| Real Tokio WebSocket disconnect | Still needed; JS transport disconnect already closes its socket |
| Central expected-peer enforcement | Still needed; retain the manual identity comparisons |
| Incremental post-storage blob delivery | Still needed; retain snapshot loading |
| Explicit round-scoped sync completion | Still needed; retain bounded convergence checks |
| Raw metadata-plus-blob batch convenience | Still a potential simplification; current batches require wrapper construction |

No new Subduction implementation was needed for the batch/prefix switch. Publication now stores a batch locally and leaves network propagation to the explicit sync step. Rust tests and the real Rust/JS WebSocket interoperability scenario pass with that change. Batch atomicity, cursor advancement, and retry rules remain unchanged.

Subduction already sends `HeadsUpdate` acknowledgements for newly inserted pushed items. The outstanding proposal is a round-correlated completion guarantee callers can await, not inventing acknowledgements from scratch. Likewise, JS already has a named sync-result wrapper; the missing piece is richer outcome/completion semantics, not merely a class name.

## Follow-up: adopted managed Tokio client

The Rust example now uses `subduction_tokio::TokioClient` and its bundled
`WebSocketConnector`. This replaces the concrete core/handler/transport aliases,
manual task spawning and tracking, connection deadline orchestration, identity
comparison, abort/drain procedure, and custom `Drop`. The example explicitly
selects memory storage, an ephemeral signer, and open policy; Subduction enforces
`ExpectedPeer::Known` and owns connection/task lifetime. Reconnection remains off.

The managed handle supplies every runtime operation the example needs:
`store_built_batch`, `sync_with_peer`, and `get_blobs`. Shutdown reports are checked
for forced termination, unfinished tasks, and fatal failures. Cleanup continues
across all clients and the server even if an earlier operation or close failed.
Checkpoint assertions use a test-owned storage handle, not raw-core access.

This resolves the native lifecycle work below, not all replication ergonomics.
The loopback server helper stays separate; inbound server management is not part
of `subduction_tokio`. Snapshot loading and bounded convergence checks remain
because there is no incremental stored-item feed or stronger sync completion
contract. Export cursors still advance only after successful local storage.
JavaScript still uses the Wasm client; this Tokio-specific migration does not
provide a managed JavaScript runtime.

## Where the code goes

At the original review, before adopting the local checkout, excluding the register implementations and standalone test files:

| Responsibility | Rust | JavaScript |
| --- | ---: | ---: |
| Wire encoding/decoding | 186 lines, excluding embedded tests | 123 lines |
| Connection and task management | 187 lines | Included in transport below |
| Publication, ingestion, and sync orchestration | 230 lines | 74 lines |

Current files:

- [`examples/mv-register/src/codec.rs`](examples/mv-register/src/codec.rs): application payloads only; the original `src/wire.rs` is removed.
- [`examples/mv-register/src/transport.rs`](examples/mv-register/src/transport.rs)
- [`examples/mv-register/src/sync.rs`](examples/mv-register/src/sync.rs)
- [`examples/mv-register-js/codec.mjs`](examples/mv-register-js/codec.mjs): application payloads only; the original `wire.mjs` is removed.
- [`examples/mv-register-js/transport.mjs`](examples/mv-register-js/transport.mjs)

The JS runnable scenario is also a 146-line integration test, [`interop.mjs`](examples/mv-register-js/interop.mjs), backed by a 107-line [Rust control-protocol helper](examples/mv-register/src/sync/interop.rs). Neither is representative of the code an application needs simply to synchronize.

Line counts are only a rough indication. Moving the same complexity into an unexplained example helper does not solve the problem. Moving reusable responsibilities into supported library APIs does.

## 1. Improve the Subduction API

The Subduction-specific proposal, with more background, is in [`../subduction/IMPROVING_API.md`](../subduction/IMPROVING_API.md).

### 1.1 Provide a managed Rust client

Nearly all of Rust's `transport.rs` is infrastructure:

- A large concrete `Subduction<…>` type and its transport/handler aliases.
- Construction of a signer, storage, policy, timer, spawner, nonce cache, and depth metric.
- Starting a core listener and manager.
- Starting three WebSocket tasks.
- Tracking spawned work.
- Deadlines, shutdown, task draining, cancellation, and `Drop`.

Subduction should provide a high-level Tokio client/session that owns these responsibilities:

```rust
// Proposed API, not current code.
let client = TokioClient::builder()
    .storage(MemoryStorage::default())
    .signer(MemorySigner::generate())
    .connect(url, expected_peer)
    .await?;

// Publish and receive history.

client.close().await?;
```

Keep the existing lower-level construction API for custom runtimes and transports. The high-level layer should supply ordinary defaults without hiding application authorization policy behind an unsafe default.

The managed client should own core, transport, keepalive, and dispatch tasks; surface background failures; and provide documented shutdown behavior. Drop can initiate cancellation, while explicit async close waits for completion. The server's `stop_and_drain()` is already a useful precedent.

The inspected local Tokio WebSocket client's `disconnect()` is still a no-op. This needs to become actual socket shutdown that wakes blocked I/O, rather than forcing applications to abort task handles.

**Expected benefit:** most of the 187-line transport module disappears from the example.

### 1.2 Enforce expected-peer identity inside connection authentication

Both examples compare the authenticated peer ID with the expected peer ID themselves. Passing `Audience::known` does not currently enforce that comparison on the normal initiator success path.

Subduction should enforce the expectation centrally, on every successful handshake path, and return a typed identity-mismatch error before registering the connection. Discovery should remain explicitly different from connecting to a known peer.

Do not remove the example checks until this is fixed. This is a correctness improvement as well as an ergonomics improvement.

### 1.3 Offer incremental access to incoming stored blobs

Both adapters repeatedly call `get_blobs()` and decode the entire local collection. Stockfoil can handle duplicates, but that does not make repeated reads, decoding, and verification a good default.

Provide either a cursor-based API:

```text
read_blobs_since(cursor) -> blobs + next_cursor
```

or a snapshot-plus-watch API for newly available local items. Initial catch-up and live delivery need a race-free handoff. The API should distinguish storage availability from application-level admission: Stockfoil still verifies and may queue an item whose dependencies have not arrived.

Existing remote-head notifications are not sufficient. They describe a peer's advertisement and can be emitted before local ingestion completes.

**Expected benefit:** a natural receive loop replaces whole-store polling and repeated decoding.

### 1.4 Make synchronization completion explicit

The current convergence loops poll until the register values match. Those assertions belong in tests, but applications also need a usable definition of synchronization progress.

An important distinction: `sync_with_peer()` already waits for local ingestion of its response. It does not establish that the other peer has ingested the return-direction data, or that all subscription pushes have finished.

Expose clearly scoped milestones, such as:

- The response for this round has been stored locally.
- The remote peer has acknowledged storing this round's advertised state.

Use a named report rather than a tuple/boolean combination, and distinguish missing connections, authorization outcomes, storage failures, and return-direction transfer failures. A remote acknowledgement may require protocol work; it is not just a renamed method.

Do not promise global convergence while arbitrary writers continue making edits. A completion guarantee should refer to a specific round or captured state.

### 1.5 Adopt existing batching and improve its input types

Batch storage existed even in the original pinned dependencies and is now used by both examples:

- Rust: `store_built_batch`.
- JavaScript: `storeBuiltBatch`.

Publication is store-only, followed by explicit sync rather than add-and-sync combinators. The remaining API opportunity is accepting mixed metadata-plus-blob records without requiring callers to construct multiple wrapper objects and corresponding blob metadata.

Both examples also now consume Stockfoil's canonical 12-byte checkpoint prefixes directly:

- Rust uses `Fragment::from_parts`, which was already available in the previously pinned crate.
- JavaScript uses `Checkpoint.fromBytes` and `Fragment.fromCheckpointPrefixes` from the local checkout; these were not exposed by the previously pinned JS 0.21.2 package.

The fragment-member scans are removed. All Rust Subduction dependencies now resolve to sibling source, not registry copies. The JS dependency points to the same checkout's built Wasm package; rebuild that package after upstream source changes.

Batch storage is not universally transactional. Preserve the rule that the publication cursor advances only after the batch has been stored successfully; retries must tolerate partial prior writes. Retired Stockfoil cover entries are not automatically requests to delete transport history.

## 2. Improve the Stockfoil API

### 2.1 Stockfoil envelope and required payload codecs — implemented

The original “default codec” proposal is superseded by [`CODEC.md`](CODEC.md). Applications supply **both** canonical commit encoding and compressed whole-fragment encoding. The graph owns this codec, so callers use:

```text
graph.export_changes(cursor) -> encoded_items + next_cursor
graph.ingest(bytes) -> ingest_report
```

Exports contain opaque bytes plus commit ID/parent metadata or fragment range/checkpoint prefixes. Adapters do not decode their outgoing blobs. Stockfoil owns framing, exact canonical-byte authentication, ordered-parent reconstruction, author evidence, and admission. Hexane-backed metadata compression is internal; application compression stays independent.

Rust and JS examples no longer implement envelope framing, signature serialization, member-hash reconstruction, or temporary payload maps. Their small BCS readers/writers remain for **application payloads**, not Stockfoil envelopes.

The implemented trust boundary preserves parent order/multiplicity, original-author signatures, explicit local permissions, and deferred rejection reporting. Inbound per-item limits bound parsing and cooperative application decoding, not aggregate queue size. Local export remains possible when a certified fragment grows beyond those limits; a receiver must explicitly approve a larger budget and retry. Whole-document persistence and fixed-memory streaming remain separate future work.

### 2.2 Provide a thin optional Stockfoil–Subduction adapter

Even with encoded exports, applications would repeatedly write the same integration:

1. Convert identifiers.
2. Distinguish loose commits from fragments.
3. Construct Subduction records.
4. Store a batch.
5. Advance the export cursor.
6. Feed received blobs into Stockfoil.

Put that in an optional crate/package, rather than making networking dependencies part of Stockfoil core.

The adapter should:

- Own an export cursor per publication destination.
- Serialize calls sharing that cursor.
- Advance it only after successful local storage.
- Retain safe retry behavior after partial failures.
- Not turn cover retirement into indiscriminate history deletion.
- Expose admission reports rather than silently swallowing deferred rejections.
- Leave permissions, signing policy, and materialization under application control.

Start with small publisher/receiver helpers, not a large `SyncedDocument` abstraction. The adapter should compose the two libraries, not own all application state.

### 2.3 Add small conveniences selectively

Secondary improvements include:

- Additional low-level borrowed export views, if needed; ordinary encoded exports already avoid caller payload lookups.
- A convenience for checking deferred rejections in `IngestReport`.
- An immediate-signing convenience for simple applications, while retaining external/asynchronous signing and the draft workflow.

A rejection-checking convenience must not imply transactionality. A valid dependency may have been admitted even if it exposes an invalid queued item.

Do not redesign materialization just to reduce this example's line count. Its retained assignments and participation tracking are necessary to support revocation correctly. A revoked write can reveal an older value that was not revisited in the latest change batch.

## 3. Simplify the examples themselves

### 3.1 Separate the JS demo from the interoperability test

`interop.mjs` builds Rust, launches a native helper, implements a control protocol, checks wrong identities, manufactures fragments, corrupts signatures, creates fresh replicas, and manages watchdogs.

That is useful integration coverage, not a minimal getting-started example.

Create a separate demo that only:

1. Connects two replicas to a server.
2. Configures author permissions.
3. Shares initial history.
4. Makes concurrent assignments.
5. Synchronizes and prints the result.

Keep the existing interoperability scenario and native control protocol under tests. Keep server startup support clearly separate from the application sync path. Requiring an existing local server can shorten the visible example, but has a setup-cost trade-off; an optional launcher can preserve the one-command experience.

### 3.2 Narrow the basic register's scope

The register wrapper supports unsigned drafts, delayed publication, historical reads, forks, merges, and revocation. A basic sync demo need not present all of these at once.

Possible reductions:

- Sign immediately in `set` and omit the pending-signature collection.
- Move delayed signing and historical reads to advanced examples or test fixtures.
- Use a single register rather than a map if map behavior is not the lesson.

Keep permission configuration explicit. Receiving an author's history must never implicitly authorize that author.

Do not silently simplify away the materializer's revocation semantics while continuing to claim revocation support. Narrow the advertised scope or retain the correct implementation.

### 3.3 Fail fast rather than recover

For a short-lived demonstration, propagating errors with `?` or uncaught exceptions is generally sufficient.

In particular, the Rust ingestion helper and JS `finally` block materialize valid earlier admissions even when a later item fails. A demo that immediately exits on that failure can omit this recovery path. A reusable document that continues operating cannot simply leave its materialized state stale.

The Rust client's cancellation-safe detached cleanup is disproportionate for a tiny executable but appropriate for a reusable client. Move it into Subduction rather than deleting it from shared infrastructure also exercised by tests.

Use a single bounded convergence helper where possible instead of layering application-level timeout wrappers around every operation. However, `Promise.race` only bounds waiting; it does not cancel the underlying operation. Real operation cancellation belongs in the transport API, and automated tests still need an outer deadline.

### 3.4 Keep correctness checks, but put them at the right layer

Do not remove:

- Original-author signature and payload verification.
- Expected server identity verification before upstream enforcement exists.
- Deferred admission rejection reporting.
- Export-cursor advancement after successful publication.
- Parser validation and relevant size/count limits.
- Bounded waits in automated tests.

Likewise, do not replace convergence polling with a fixed sleep. It makes the example shorter by making it unreliable.

The Stockfoil codec now removes the duplicated envelope schema and verification glue. A maintained BCS dependency could still replace the JS application's small payload reader/writer, but that is separate from the library ownership change.

## Recommended implementation order

1. **Separate the JS demo from the integration test.** Immediate readability improvement without API changes or lost coverage.
2. **Stockfoil-owned envelope with required application payload codecs — complete.** Shared Rust/Wasm implementation and both examples migrated; see `CODEC.md`.
3. **Add Subduction's managed Rust client, expected-peer enforcement, and proper close semantics.** The largest Rust-specific reduction; identity enforcement is independently worth fixing promptly.
4. **Extract a thin integration adapter.** Both examples now use existing batch storage and checkpoint-prefix constructors; the remaining conversion and cursor management can be shared.
5. **Add incremental receive notifications and explicit sync-completion semantics.** Replace repeated whole-store scans and application-specific progress polling.

Success means the example visibly explains the CRDT and how authenticated history flows between replicas. Robust parsing, lifecycle management, and retry invariants should still exist—but once, in the libraries responsible for them.
