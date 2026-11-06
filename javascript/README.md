# stockfoil (JavaScript / TypeScript)

`stockfoil` is a WebAssembly-powered library for building local-first data
structures (CRDTs, key-value stores, collaborative documents, or out-of-core
indexes) on top of an authenticated commit graph.

If you are familiar with the basic idea of a commit graph—a directed acyclic
graph (DAG) where each edit is a commit with parent hashes, an author, and an
operation payload—building a real local-first system on top of one quickly runs
into three hard problems:

1. **[Materialization and incremental view maintenance](#1-materialization-and-incremental-view-maintenance):**
   Your application state is a view over the commit graph. As commits arrive from
   peers in arbitrary orders, as users inspect historical branches, or as commits
   are retroactively excluded, you need to keep that state up to date
   incrementally (or know when to reset and replay it).
2. **[Compaction without losing verifiability](#2-compaction-and-implementing-a-codec):**
   Fine-grained editing produces tens of thousands of tiny commits. You need to
   bundle ranges of the commit graph into compressed chunks so storage and sync
   stay fast, while still reconstructing the exact individual commits inside.
3. **[Authentication and authorization without a central server](#3-authentication-authorization-and-write-frontiers):**
   Peers edit offline and sync peer-to-peer or through untrusted relays. You need
   to verify who authored every commit (even inside compacted bundles), and you
   need a way to revoke or bound a writer's permissions when there is no central
   gatekeeper or trusted wall clock—without deleting commits that later valid
   commits causally depend on.

`stockfoil` runs the commit graph, incremental materialization planner,
compaction metadata, signature verification, and dependency parking queue in
Rust/Wasm without performing I/O or imposing a networking stack. Your JavaScript
code provides two things:
- how commits fold into your application state (by applying batches of `Event`s
  from a `Materialization`), and
- a **`Codec`** that packs and unpacks your commit payloads inside wire bundles.

---

## Getting Started

When your data structure and commit payloads fit in memory, you pair a `Document`
(which holds the commit graph and canonical payloads) with a `Materialization`
(which plans incremental state updates).

### Step 1: Fold events into your state (`Materialization`)

A `Materialization` plans batches of `Event`s against `document.ledger`. Write a
small `update()` helper that drains planned batches, applies them to your state,
and calls `checkout.complete(...)`:

```js
import { Document, Materialization } from "stockfoil";

class KvStore {
  entries = new Map();

  apply(document, events) {
    const rejected = [];
    for (const event of events) {
      if (event.kind === "reset") {
        this.entries.clear();
      } else if (event.kind === "deliver") {
        const text = new TextDecoder().decode(document.payloadById(event.payload));
        const eq = text.indexOf("=");
        if (eq === -1) {
          // Flag malformed payloads without aborting or breaking sync.
          rejected.push({ hash: event.hash, reason: "expected key=value" });
          continue;
        }
        if (event.active) {
          this.entries.set(text.slice(0, eq), text.slice(eq + 1));
        }
      }
    }
    return { rejected };
  }
}

function update(document, checkout, state) {
  const { ledger } = document;
  while (!checkout.isCurrent(ledger)) {
    let batch = checkout.plan(ledger, 64);
    while (batch) {
      const applied = state.apply(document, batch.events);
      batch = checkout.complete(ledger, batch, applied);
    }
  }
}
```

### Step 2: Create a `Document` and commit local edits

Create a `Document` with your [bundle `Codec`](#2-compaction-and-implementing-a-codec)
and local `author` (a 32-byte Ed25519 public key as a `Uint8Array`), then commit
an operation payload and sign the resulting `Draft`'s 32-byte `hash`:

```js
import { generateKeyPairSync, sign } from "node:crypto";

function makeIdentity() {
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  const author = new Uint8Array(
    Buffer.from(publicKey.export({ format: "jwk" }).x, "base64url"),
  );
  return {
    author,
    signHash: (hash) => new Uint8Array(sign(null, hash, privateKey)),
  };
}

const alice = makeIdentity();
const aliceDoc = new Document(kvCodec, { author: alice.author });
const aliceView = new Materialization();
const aliceState = new KvStore();

const draft = aliceDoc.commit(new TextEncoder().encode("title=Hello"));
aliceDoc.attachSignature(draft, alice.signHash(draft.hash));
update(aliceDoc, aliceView, aliceState);

console.log(aliceState.entries.get("title")); // "Hello"
```

### Step 3: Set permissions and sync with another peer

By default, a new document only permits commits written by its own local author.
To collaborate with a peer, configure write permissions on `document.ledger` and
exchange wire items with `export`, `exportItem`, and `receive`:

```js
import { acknowledgeExport } from "stockfoil";

const bob = makeIdentity();
const bobDoc = new Document(kvCodec, { author: bob.author });
const bobView = new Materialization();
const bobState = new KvStore();

bobDoc.ledger.setPermissions([
  [alice.author, "unbounded"],
  [bob.author, "unbounded"],
]);

// Export unacknowledged items from Alice and receive them on Bob's document.
let aliceToBobCursor = [];
const plan = aliceDoc.export(aliceToBobCursor);
for (let i = 0; i < plan.items.length; i++) {
  const wireBytes = aliceDoc.exportItem(plan.items[i]);
  bobDoc.receive(wireBytes);
  acknowledgeExport(plan, i); // call only after durable delivery
}
aliceToBobCursor = plan.cursor;

update(bobDoc, bobView, bobState);
console.log(bobState.entries.get("title")); // "Hello"
```

Looking at those three steps raises three natural questions, which are covered in
the next three sections:
1. **In `KvStore.apply`:** Why does a `"deliver"` event have an `active` boolean,
   why did we handle `"reset"`, and what are the other event kinds?
   ([Section 1](#1-materialization-and-incremental-view-maintenance))
2. **In `new Document(kvCodec, ...)`:** What are the wire items produced by
   `exportItem`, and how do you implement a `Codec`?
   ([Section 2](#2-compaction-and-implementing-a-codec))
3. **In `setPermissions`:** Why did `bobDoc` need explicit permissions, how are
   compacted items authenticated, and how do you revoke a writer after a specific
   commit? ([Section 3](#3-authentication-authorization-and-write-frontiers))

---

## Core Concepts

### 1. Materialization and Incremental View Maintenance

When you build a data structure on a commit graph, the commit graph is the source
of truth and your application state is a materialized view of that graph.

Each `Materialization` tracks one view of a document's `Ledger`, configured via
`follow`:
- `"working"` (default): Follows all delivered commits, including unsigned local
  `Draft`s.
- `"signed"`: Follows only cryptographically admitted commits.
- `{ kind: "pinned", heads: Uint8Array[] }`: Follows the causal past of a fixed
  set of commit hashes (for historical snapshots or branches).

Whenever commits arrive or the active view changes, `checkout.plan(ledger, budget)`
diffs your state's current position against the target view and returns a `Batch`
of `Event`s in a deterministic, canonical ancestor-first order (breaking ties
among ready commits by smallest commit hash, regardless of network arrival order):

- **`{ kind: "deliver", hash, author, parents, payload, active }`:**
  Emitted once for every new commit entering the graph. Notice `active: boolean`:
  **delivery is separate from visibility**. Even if a commit does not currently
  participate in the view (`active: false`—for example, because it lies outside a
  pinned branch or was written by a revoked author), a later `active: true`
  commit may have it as a parent and reference structural metadata introduced in
  it (such as an operation ID or key). Your state always receives the commit so
  it can record whatever causal metadata it needs, and applies the user-visible
  effect only when `active` is `true`.
- **`{ kind: "activate", hash }` and `{ kind: "retract", hash }` (or `{ kind: "reset" }`):**
  Emitted when a branch switch or permission change alters whether a previously
  delivered commit participates in the view:
  - **Incremental retraction (`supportsVisibilityChanges: true`):** If your data
    structure can turn an already-delivered commit's effect on or off by hash (as
    in [`examples/mv-register-js`](../examples/mv-register-js)), pass
    `{ supportsVisibilityChanges: true }` to `new Materialization(...)`. You will
    receive `"activate"` and `"retract"` events for just the affected commits. If
    retracting a winning write uncovers an older, previously overwritten commit
    whose payload you did not keep in memory, return `{ wanted: [olderHash] }` to
    `checkout.complete(...)` and `stockfoil` will immediately return a follow-up
    batch containing `{ kind: "payload", hash, payload }`.
  - **Reset and replay (`supportsVisibilityChanges: false`, the default):** If
    incremental retraction is not worth the complexity for your data structure
    (as in `KvStore` above), leave `supportsVisibilityChanges` as `false`. Normal
    forward growth still applies incrementally via `"deliver"`, and only when an
    already-applied commit actually needs to be retracted will `stockfoil` emit
    `{ kind: "reset" }` followed by re-delivering the active history from scratch.
- **Rejecting invalid application payloads (`{ rejected }`):**
  If a commit is cryptographically valid at the graph layer but its payload bytes
  are malformed for your application, return
  `{ rejected: [{ hash, reason, cascade }] }` from your batch handler. The
  materialization permanently treats that commit (and its descendants, if
  `cascade: true`) as inactive without removing it from the shared commit graph.

---

### 2. Compaction and Implementing a `Codec`

Storing and syncing every commit individually carries ~130+ bytes of hashes,
public keys, and signatures per edit and prevents compressing redundant data
across commits.

`stockfoil` exchanges history as wire items (`ItemId`), which are either a
standalone `{ kind: "commit", head }` or a compacted **Sedimentree fragment**
`{ kind: "fragment", head, boundary }`:
- **Deterministic fragment boundaries:** Commit hashes with $D > 0$ leading zero
  bytes (their *depth*) deterministically define a fragment starting at that
  `head` commit and extending backward through its ancestors until hitting
  boundary commits of depth $\ge D$. Once admitted, a fragment automatically
  retires the standalone commits and smaller fragments it covers.
- **Header vs. body:** Every wire item has a header (managed by `stockfoil`,
  encoding the $N$ members' authors, parent topology, and covering signatures)
  and a body (managed by your `Codec`).

#### The `Codec` Interface

Each individual commit in your application has **canonical payload bytes**—the
logical operation bytes that are hashed into that commit's `CommitHash`. Your
`Codec` packs $N$ canonical payloads into one bundle body on encode, and streams
them back out on decode:

- `format: string`: A 1–128 byte UTF-8 identifier for your application format.
- `encode(meta: BundleMeta, payloads: Uint8Array[]): Uint8Array`: Packs all
  `payloads` (one per member in `meta.members`, in ancestor-first order) into a
  single body buffer. You can compress across all $N$ commits together (e.g.,
  building a shared key dictionary or delta-encoding), as long as decoding
  reconstructs the exact canonical bytes for each member.
- `decoder(meta: BundleMeta): { step(input, eof, capacity): DecodeStep }`:
  Returns a streaming decoder for one item body. On each `step(input, eof,
  capacity)` call, it consumes up to `input.length` bytes and produces up to
  `capacity` bytes of canonical output, returning `{ consumed, output, status }`
  where `status` is:
  - `"needInput"`: All `input` was consumed (`consumed === input.length`) and
    more input is needed.
  - `"working"`: Filled `capacity` (or made progress) and can produce more output
    before needing new input.
  - `"endMember"`: The `output` of this step completes the current member's
    canonical payload (must be emitted once per member in `meta.members`, even
    for empty payloads).
  - `"finished"`: All `meta.members.length` members have ended and no body bytes
    remain.

Here is a complete, reusable length-prefixed `Codec` (used by `kvCodec` in
Getting Started and in [`examples/mv-register-js/codec.mjs`](../examples/mv-register-js/codec.mjs)):

```js
function concat(parts) {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const p of parts) {
    out.set(p, offset);
    offset += p.length;
  }
  return out;
}

class LengthPrefixedDecoder {
  constructor(memberCount) {
    this.memberCount = memberCount;
    this.ended = 0;
    this.prefix = new Uint8Array(4);
    this.have = 0;
    this.remaining = -1;
  }

  step(input, eof, capacity) {
    let consumed = 0;
    let produced = 0;
    const chunks = [];
    const result = (status) => ({ consumed, output: concat(chunks), status });

    for (;;) {
      if (this.ended === this.memberCount) {
        if (consumed < input.length) throw new Error("trailing body bytes");
        return result("finished");
      }
      if (this.remaining < 0) {
        const n = Math.min(4 - this.have, input.length - consumed);
        this.prefix.set(input.subarray(consumed, consumed + n), this.have);
        this.have += n;
        consumed += n;
        if (this.have === 4) {
          this.remaining = new DataView(this.prefix.buffer).getUint32(0, true);
          this.have = 0;
        }
      }
      if (this.remaining >= 0) {
        const n = Math.min(this.remaining, input.length - consumed, capacity - produced);
        chunks.push(input.subarray(consumed, consumed + n));
        consumed += n;
        produced += n;
        this.remaining -= n;
        if (this.remaining === 0) {
          this.remaining = -1;
          this.ended++;
          return result("endMember");
        }
        if (produced === capacity && produced > 0) return result("working");
      }
      if (consumed === input.length) {
        if (eof) throw new Error("truncated body");
        return result("needInput");
      }
    }
  }
}

export const kvCodec = {
  format: "example/kv-v1",
  decoder: (meta) => new LengthPrefixedDecoder(meta.members.length),
  encode(_meta, payloads) {
    const parts = [];
    for (const payload of payloads) {
      const len = new Uint8Array(4);
      new DataView(len.buffer).setUint32(0, payload.length, true);
      parts.push(len, payload);
    }
    return concat(parts);
  },
};
```

---

### 3. Authentication, Authorization, and Write Frontiers

In a centralized server app, permissions are checked against a wall clock when a
write arrives: if Bob's access was revoked at 2:00 PM, the server rejects any
write from Bob after 2:00 PM.

In a local-first commit graph, there is no central gatekeeper and no trusted wall
clock. Worse, **you cannot enforce permissions by deleting unauthorized commits
from the commit graph**: if Alice (who has write access) synced with Bob while
offline and created her own valid commits on top of Bob's commit before learning
Bob was revoked, dropping Bob's commit from the graph would break the hash chain
and make Alice's valid commits unverifiable.

`stockfoil` separates **authentication** (who wrote a commit, which is an
immutable property of the graph) from **authorization** (which commits
participate in your materialized state, which is a local policy):

- **Authentication and transitive signatures:** Every commit is identified by a
  32-byte BLAKE3 hash over its ordered parent hashes, its 32-byte Ed25519
  `author` public key, and the hash of its canonical payload. Inside a compacted
  fragment, because each commit hash commits to its parents' hashes, an author's
  Ed25519 signature near the tip of the fragment *transitively authenticates* all
  of that author's ancestor commits inside the fragment. Reconstructing the exact
  canonical payloads via your `Codec` lets `stockfoil` recompute every commit
  hash and verify all authors' signatures before admitting the fragment.
- **Authorization via write frontiers (`ledger.setPermissions`):** In a causal
  graph, the only tamper-proof notion of time is *position in the DAG*. When an
  author's write access is revoked, what you actually know is the point in
  history where the revocation took effect—a list of commit hashes called a
  **write frontier**:
  - `"unbounded"`: All of the author's commits in the selected history
    participate.
  - `Uint8Array[]` (a frontier of commit hashes): Only commits by this author
    that are ancestors of (or equal to) at least one commit in the frontier
    participate. Anything written causally after that frontier—or on an unmerged
    offline fork—is excluded.
  - `"denied"`: None of the author's commits participate.

  Authors not listed get the unlisted rule, the optional second argument to
  `setPermissions` (or `unlistedPermission` in the ledger options). It defaults to
  `"denied"`; pass `"unbounded"` to admit every author.

```js
// Permit Alice unconditionally, but bound Bob to commits at or before `bobLastGoodHash`:
aliceDoc.ledger.setPermissions([
  [alice.author, "unbounded"],
  [bob.author, [bobLastGoodHash]],
]);
update(aliceDoc, aliceView, aliceState);
```

When permissions change, disallowed commits remain in the commit graph so that
fragments, sync, and child commit hashes keep working, while `Materialization`
delivers them with `active: false` or emits `"retract"` / `"reset"` events to
remove their effects from your state.

---

## Reference and Advanced Topics

### Memory Model: Plain JS Data vs. Disposable Engines

Generated `wasm-bindgen` pointer classes are **never exposed**. Everything
crossing the Wasm boundary falls into one of two categories:

| Category | Types | Lifecycle |
| --- | --- | --- |
| **Plain JS data** | `Draft`, `Receipt`, `Received`, `Signed`, `Event`, `Batch`, `ExportPlan`, `ExportedItem`, `ExportCursor`, `View`, `BundleMeta`, `CheckpointInfo` | Ordinary JS objects and owned `Uint8Array` copies. Safe to log, keep, or `structuredClone`. No `.dispose()` needed. |
| **Stateful engines & operations** | `Document`, `Ledger`, `Materialization`, `Ancestry`, `Admission`, `DecodeItem`, `CommitBuilder`, `checkPayload` | Hold live Wasm state. Call `.dispose()` when finished (consuming an operation via `ledger.commit` or `ledger.admit` also disposes it; repeated `.dispose()` calls are safe). |

Payload IDs, request tickets, epochs, and `Batch.token` values are `bigint`s. Do
not mutate a `Batch`'s `events`, `rebuild`, or `token` before passing it back to
`checkout.complete(ledger, batch, applied)`.

### Out-of-Core / Async Streaming I/O (`Ledger`, `verify`, `driveDecode`)

When commit payloads live in IndexedDB, OPFS, SQLite, or the filesystem and are
too large to hold in memory, use `Ledger` directly instead of `Document`.
`Ledger` tracks topology, permissions, and caller-assigned `u64` `bigint` payload
IDs without storing payload bytes in memory:

- **Receiving items with bounded buffers (`beginReceive` + `verify` + `admit`):**
  `verify` drives an `Admission`'s read and decode requests asynchronously without
  holding a borrow of the `Ledger` across `await`s:

  ```js
  import { Ledger, verify } from "stockfoil";

  const ledger = new Ledger({ author, format: codec.format, maxParked: 1024 });
  const admission = ledger.beginReceive({ readSize: 65536, decodeCapacity: 65536 });
  try {
    await verify(
      admission,
      {
        read: (offset, max) => storedItem.read(offset, max), // empty Uint8Array = EOF
        decoder: (meta) => createStreamingStagingDecoder(meta),
      },
      { signal },
    );
    // Stage decoded canonical bytes durably before admitting:
    const result = ledger.admit(admission, (itemId, memberHashes) =>
      assignPayloadIds(itemId, memberHashes),
    );
    // result.kind: "admitted" | "waiting" | "evicted"
  } finally {
    admission.dispose();
  }
  ```

  Note that the `bind` callback passed to `ledger.admit` (and
  `ledger.attachSignature`) is synchronous and is only called when an item is
  actually published (never when it is parked waiting on a dependency). It must
  return one `bigint` payload ID per member without re-entering the `Ledger`.
- **Creating local commits (`beginCommit` + `commit`):**
  Stream canonical payload chunks into both `builder.update(chunk)` and your
  storage, then call `ledger.commit(builder, payloadId)` to get a `Draft`.
- **Verifying or extracting stored payloads (`checkPayload` and `decodeItem`):**
  Use `ledger.checkPayload(hash)` (`update` / `finish`) to verify unpacked
  canonical bytes read from disk against their commit hash, or
  `ledger.decodeItem(wanted, options)` with `driveDecode` to stream and verify
  specific member payloads out of a compressed wire item on disk.

### Persistence: Checkpoints and Export Cursors

- **Materialization checkpoints (`checkout.checkpoint`):**
  A versioned `Uint8Array` representing the planner's progress (including its
  follow rule, completed view cursor, permanent rejections, and any in-progress
  canonical traversal). Always persist wire items *before* materializing them, and
  persist your application state and `checkout.checkpoint` **atomically** after
  `checkout.complete(...)` returns `undefined`. Resume after a restart with
  `new Materialization({ checkpoint, supportsVisibilityChanges })`. Use
  `inspectCheckpoint(bytes)` to inspect a checkpoint's metadata in JS.
- **Export cursors (`ExportCursor`):**
  `plan.cursor` is a plain `ItemId[]` array. Use `encodeExportCursor(cursor)` and
  `decodeExportCursor(bytes)` to serialize and restore it from durable storage.

### Wasm Initialization (`stockfoil` vs. `stockfoil/slim`)

Importing from `"stockfoil"` initializes the WebAssembly module automatically in
Node, browsers, and Cloudflare Workers (`workerd`).

If your environment requires manual Wasm instantiation, import from
`"stockfoil/slim"` and call `initialize(wasmBytesOrModule)` once before creating
any engines (the raw `.wasm` binary is exported at `"stockfoil/wasm"`).

---

## Development

Requires Node 22+, nightly Rust with `rust-src`, `wasm-bindgen-cli` (`0.2.127`),
`wasm-opt`, `esbuild`, TypeScript, and `wasm-bodge` (`0.5.0`):

```sh
cargo install wasm-bodge --version 0.5.0 --locked
cargo install wasm-bindgen-cli --version 0.2.127 --locked
rustup toolchain install nightly --component rust-src
npm install
npm test
npm run typecheck
```

`tests/fixture.rs` validates wire interoperability between the Rust crate and JS
bindings. To intentionally regenerate wire fixtures after a spec change, run
`STOCKFOIL_UPDATE_FIXTURES=1 cargo test -p stockfoil-wasm`.
