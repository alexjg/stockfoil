# Architecture and Technical Reference

`stockfoil` manages one document's authenticated commit graph and its
materializations without performing I/O or depending on an async runtime.
Applications own their payload formats, CRDT state, storage, transport, and
scheduling.

For an introduction to the core concepts, start with the
[crate documentation](../stockfoil/src/lib.rs). The [wire specification](spec.md)
defines the binary format and cryptographic rules; [permissions](permissions.md)
explains write frontiers and view participation.

---

## Layers and ownership

`stockfoil` is split into four layers so that applications with in-memory data get
a simple high-level API, while applications with large on-disk data can manage I/O
and memory buffers directly:

```text
Replica<C, M>                 in-memory document + automatically updated working state
      |
Document<C>                   in-memory canonical payloads + pure bundle Codec
      |
Ledger                        local author, WritePermissions, drafts, dependency parking
      |
History                       DAG topology, PayloadIds, admission, certificate inventory

Materialization               independent view planner; plans against &Ledger
      +-- Batch / Event       application applies, then completes against &Ledger

Host bytes + decoder ---> Admission ---> Ledger::admit + staged PayloadIds
```

- **`History`** is the pure topology and certificate core. It stores no payload
  bytes, codec, permission policy, or storage handles—only graph nodes, authors,
  caller-assigned `u64` `PayloadId`s, signature evidence, and the export
  inventory.
- **`Ledger`** wraps `History` with document-level configuration: the local
  `Author`, the expected `FormatId`, the current `WritePermissions` policy, and a
  bounded `Queue` for verified items waiting on missing dependencies. It still
  stores zero payload bytes.
- **`Document<C>`** wraps `Ledger` with a `Codec` and an in-memory table of
  canonical `Bytes` (`Arc<[u8]>`), automatically staging and binding payloads.
- **`Replica<C, M>`** pairs a `Document<C>` with one in-memory `Materializer` and
  a `Follow::Working` `Materialization`, automatically advancing state after every
  mutation.

To play well with async code and external storage, operations that span caller
I/O—`Admission`, `Batch`, `Checkpoint`, `Ancestry`, `ExportPlan`, and `Draft`—are
**owned values** that do not hold a borrow of `Ledger` or `History`.

### Internal DAG representation and chain-prefix clocks

Inside `History`, nodes are stored append-only in parents-first order, so every
internal `NodeIdx` is also a topological position. (Parent order and repeated
parent edges are preserved because they affect `CommitHash`, even though ancestry
queries treat repeated edges as a single causal link.)

To make causal ancestry and view-diff queries fast without copying node sets,
`History` decomposes the DAG into immutable, single-author causal **chains**:
- Whenever a new commit arrives, `History` tries to extend an existing chain by
  the same author (preferring a direct same-author parent that is currently a
  chain tip, or another same-author chain tip in the parents' causal past), and
  starts a new chain only when an author branches concurrently.
- Any causal down-set (or permission-masked view) can then be represented as a
  **chain-prefix clock** (`Clock`): a persistent, structure-sharing AVL map from
  `ChainId` to the highest included node position on that chain.
- Full ancestry clocks are cached at merges, at commits with nonzero leading-zero
  hash depth, and at least every 128 nodes along long linear runs. Computing
  fragment membership or view differences reduces to fast interval differences
  across per-chain node lists.
- Chain IDs and node indices are strictly internal to one `History` instance and
  are never serialized to disk or sent over the wire.

---

## Local commits and signing

A commit in `stockfoil` passes through distinct lifecycle stages depending on
whether it was created locally or received from a peer:

| Stage | Visible to `Materialization`? | Exportable to peers? |
| --- | --- | --- |
| Received & verified, waiting for dependencies | No | No |
| Delivered local `Draft` (unsigned) | Yes (`Follow::Working` / `Pinned`) | No |
| Admitted commit (authenticated & parent-closed) | Yes (all views) | Yes, via a covering certificate |

Admitted history is always authenticated and parent-closed: a commit cannot be
admitted until all of its parents (and fragment boundaries) are admitted. A local
unsigned `Draft` is delivered to the graph immediately so local views can render
it, but it cannot satisfy a network item's dependency until signed and admitted.

### Creating and signing commits

When using `Ledger` with caller-owned storage:

1. Call `ledger.begin_commit(parents)` to validate that the parent hashes are
   already delivered (`None` selects the current delivered heads) and get a
   `CommitBuilder`.
2. Stream the canonical payload chunks into both `CommitBuilder::update` and your
   storage. Calling `builder.finish()` yields a `Prepared` commit with its final
   `CommitHash`.
3. Call `ledger.commit(prepared, payload_id)` to insert the commit into the graph
   as an unsigned `Draft`.
4. Sign `draft.signing_bytes()` (the raw 32-byte `CommitHash`)—either
   synchronously via `ledger.sign_drafts(&signer, &mut bind)` or externally via
   `ledger.attach_signature(&draft, signature, &mut bind)`.

With `Document` or `Replica`, `document.commit(bytes, parents)` and
`replica.commit(bytes)` handle the in-memory payload table for you. If your
in-memory state prefers to mutate itself directly during an edit and return the
encoded operation bytes, `replica.change(&signer, |state| ...)` (or
`change_draft`) commits the bytes and calls `Materialization::mark_local` so the
new commit is not echoed back through `Event::Deliver`.

### Certificate assembly on signing

Attaching a signature to a draft can complete one or more exportable certificates
(or none yet, if an ancestor is still an unsigned draft):
- A depth-0 draft (whose `CommitHash` does not start with a `0x00` byte) becomes
  admitted as a standalone commit once signed by its author and once all of its
  parents are admitted.
- A nonzero-depth draft first attempts to assemble its canonical **fragment**
  (down to the nearest ancestor commits of at least the same depth), which
  succeeds once every author in that fragment has transitive signature coverage
  within the fragment and all external parents/boundaries are admitted. If an
  earlier commit in that range was admitted via a signature outside the fragment,
  the draft falls back to a standalone certificate if directly signed by its own
  author.

`Signed` reports which drafts became admitted, which parked network items were
released as a result, and any parked items that failed placement once their
dependencies arrived. Because unsigned drafts are not part of the exportable wire
inventory, applications that delay signing across restarts should journal unsigned
drafts separately.

---

## Admission and caller-owned buffers

When using `Ledger`, receiving an untrusted wire item is driven through an owned
`Admission` state machine (`ledger.begin_receive()`, or `begin_receive_with` to
customize buffer sizes, `Limits`, and work quantum).

```text
Read*        parse wire header and verify FormatId
Decode*      stream-decode body, hash canonical members
             verify Ed25519 signatures and author coverage   (Admission::next)
NeedHistory  byte verification complete; stage members       (caller)
             check dependencies/placement, bind, publish     (Ledger::admit)
```

Notice that **all parsing, decoding, BLAKE3 hashing, and Ed25519 signature
verification happen before `Admission` ever touches `Ledger` or `History`**.
Corrupt or unauthenticated items are rejected before reaching the graph.

### The request/reply loop

Calling `admission.next()` advances verification by up to the configured work
quantum and returns one of:
- `Next::Continue`: The work quantum was consumed during signature or coverage
  checking; call `next()` again (yielding to your scheduler if desired).
- `Next::Request(ticket, request)`: Perform the requested host action and pass the
  result to `admission.reply(ticket, reply)`:

  | `Request` | Host action & `Reply` |
  | --- | --- |
  | `Read { offset, max }` | Read up to `max` bytes from the wire item at `offset` and reply with `Reply::Read { bytes, eof }`. |
  | `Decode { meta, input, eof, capacity }` | Pass `input` to your streaming bundle decoder (initialized from `meta`) and reply with `Reply::Decoded { consumed, output, status }`. |
  | `Bind { meta }` | Issued during `Ledger::admit`: return one `PayloadId` per member via `Reply::Bound(ids)`. |

- `Next::NeedHistory`: Byte and signature verification succeeded. Stage the
  decoded canonical payloads in your storage and hand the `Admission` to
  `ledger.admit(op, &mut bind)`.
- `Next::Failed(err)`: Verification failed; discard any staged bytes for this
  item.

During `Decode`, your streaming decoder reports progress via `DecodeStatus`:
`Working` (produced output or consumed input; more output available), `NeedInput`
(all offered input consumed), `EndMember` (the current member's canonical payload
is complete—must occur once per member, even for empty payloads), and `Finished`
(all members complete; no trailing body bytes allowed).

### Dependency parking and publication

When `ledger.admit(op, &mut bind)` runs:
1. If all external parents and fragment boundaries are already admitted, `Ledger`
   verifies canonical fragment placement, calls `bind` (if any member has not been
   delivered before) to get the `PayloadId`s for the members, inserts new commits,
   records the certificate, and drains any parked items that were waiting on the
   newly admitted commits.
2. If an external parent or boundary is not yet admitted, `Ledger` parks the
   verified `Admission` in its bounded `Queue` (default 1024 items, configurable
   with `with_max_parked`) and returns `Received::Waiting(missing_hash)` (or
   `Received::Evicted { waiting, evicted }` if the queue was full and evicted the
   oldest parked item).

Binding is deferred until publication actually succeeds; parked items do not
consume `PayloadId`s while waiting. If your storage stages decoded bytes while an
item is parked, release one staging reference whenever an item is admitted, fails
placement after unparking (`Received::Admitted { failed, .. }`), or is evicted.

---

## Exports and stored payloads

Calling `ledger.export(&cursor)` snapshots the current certificate inventory and
returns an owned `ExportPlan` containing the items that `cursor` (`ExportCursor`)
has not yet acknowledged:

- Only admitted certificates are exported; unsigned drafts and `WritePermissions`
  are never exported.
- When a fragment is admitted, any standalone commit or smaller fragment whose
  members lie entirely within the new fragment is **retired** from the inventory
  (reported in `plan.retired()` for transport reconciliation).
- For each index in `0..plan.len()`, `ledger.export_item(&plan, index)` returns an
  owned `ExportedItem` containing the `ItemId`, the pre-encoded wire v2 `header`,
  the ordered list of `(CommitHash, PayloadId)` members, the `BundleMeta`, and the
  fragment's internal nonzero-depth `checkpoints` (used by Sedimentree/Subduction
  layers).
- Call `plan.acknowledge(index)` only after the destination has durably accepted
  the item, then persist `plan.cursor().to_bytes()`. Unacknowledged items will be
  included again in the next plan.

With in-memory `Document` / `Replica`, `document.export_item(&plan, index)` also
runs your `Codec::encode` and returns the complete wire item bytes.

### Verifying and extracting payloads from storage (`PayloadCheck` and `DecodeItem`)

When using `Ledger`, your storage does not need its own cryptographic integrity
layer:
- If you store unpacked canonical payloads by `PayloadId`, call
  `history.check_payload(&commit_hash)` to get a `PayloadCheck`, feed the bytes
  read from disk into `update`, and call `finish()` to verify that they reproduce
  the commit's `CommitHash`.
- If you store only compressed wire items on disk (so a `PayloadId` points to
  `(stored_item_key, member_index)`), call `history.decode_item(options, wanted)`
  to get a `DecodeItem` state machine. It streams the stored wire item through
  bounded `Read` and `Decode` requests, emits `DecodeNext::Chunk { member, bytes }`
  for just the requested member indices, verifies each member's `PayloadCheck` at
  the end of that member, and stops as soon as the last wanted member finishes.
  (Treat emitted chunks as provisional until the member finishes without
  `Error::CorruptPayload`.)

---

## Queries and canonical traversal

`Ledger` and `History` expose direct topology queries (`contains`, `is_admitted`,
`author_of`, `parents`, `heads`, `drafts`, `is_ancestor`, `traverse`).

To query historical causal membership or compute commit-set diffs without holding
a borrow of the graph:

```rust
use stockfoil::{CommitHash, Commits, Error, Ledger};

fn causal_additions<'h>(
    ledger: &'h Ledger,
    before_heads: &[CommitHash],
    after_heads: &[CommitHash],
) -> Result<Commits<'h>, Error> {
    let before = ledger.ancestry(before_heads)?;
    let after = ledger.ancestry(after_heads)?;
    after.difference(&before, ledger.history())
}
```

- `ledger.ancestry(heads)` captures an owned `Ancestry` handle (backed by `Arc`
  chain-prefix clocks) for `heads` and the current `WritePermissions`.
- `Ancestry::difference` and `ledger.commits_since(heads)` return a lazy,
  exact-sized `Commits` iterator over the causal difference (ignoring permissions
  and including drafts).
- Every traversal in `stockfoil`—whether `Commits` or `Materialization`—emits
  commits in **canonical order**: ancestors before descendants, breaking ties
  among ready commits by smallest `CommitHash`. This order depends only on the
  selected commits and their DAG structure, never on local arrival order or batch
  size.

---

## Materialization

Each `Materialization` has its own `Follow` rule, permanent payload rejections,
and progress `Checkpoint`. Calling `planner.plan(&ledger, budget)` plans up to
`budget` source records into an owned `Batch` of `Event<PayloadId>`s:

| `Event` | Meaning and application obligation |
| --- | --- |
| `Deliver { hash, author, parents, payload, active }` | First time this materialization sees `hash`. Record structural metadata even when `active` is `false`; apply user-visible effects only when `active` is `true`. |
| `Activate { hash }` / `Retract { hash }` | An already-delivered commit entered or left the active view (only emitted when `supports_visibility_changes` is `true`). |
| `Reset` | Clear application state before a clean replay of active history. |
| `Payload { hash, payload }` | Delivers an older commit's payload requested via `Applied::wanted`. |

After applying a `Batch`, pass it along with an `Applied` report (`rejected` and
`wanted`) to `planner.complete(&ledger, batch, applied)`:

```rust
use stockfoil::{Applied, Batch, Checkpoint, Error, Ledger, Materialization};

fn drive<S>(
    ledger: &Ledger,
    planner: &mut Materialization,
    state: &mut S,
    mut apply: impl FnMut(&mut S, &Ledger, &Batch) -> Result<Applied, Error>,
    mut persist: impl FnMut(&mut S, &Checkpoint) -> Result<(), Error>,
) -> Result<(), Error> {
    while let Some(mut batch) = planner.plan(ledger, 64)? {
        loop {
            let applied = apply(state, ledger, &batch)?;
            match planner.complete(ledger, batch, applied)? {
                Some(more) => batch = more, // follow-up Payload events or cascading retractions
                None => break,
            }
        }
        persist(state, planner.checkpoint())?; // persist state + checkpoint atomically
    }
    persist(state, planner.checkpoint())
}
```

If `Applied::rejected` marks a newly delivered commit as rejected with `cascade:
true`, and a descendant of that commit was already activated earlier in the same
batch, `complete` automatically emits a follow-up `Batch` to retract that
descendant (or reset and replay if `supports_visibility_changes` is `false`). You
must finish the inner `complete` loop before planning the next batch or saving the
checkpoint.

---

## Persistence and failure recovery

To make an on-disk materialization crash-recoverable:

1. **Persist wire items before materializing them:** Every commit hash referenced
   by a saved `Checkpoint` must exist in your rebuilt `Ledger` on restart.
2. **Persist application state and `Checkpoint::to_bytes()` atomically:** Only
   save the checkpoint after `complete` returns `Ok(None)`. If an I/O error or
   cancellation interrupts a batch mid-flight, roll back or discard the
   speculative state changes and resume from the last jointly saved state and
   `Checkpoint`. (See [`examples/volume`](../examples/volume) for an example using
   SQLite and a checksummed redo journal to update a raw disk image and its
   checkpoint atomically.)

### Why `Checkpoint` survives reordered history rebuilds

A serialized `Checkpoint` stores:
- the `Follow` rule,
- the last completed `MaterializationCursor` (delivery heads and `ViewPoint`),
- the set of permanently rejected `CommitHash`es and their `cascade` flags, and
- if saved mid-traversal (between outer batches), the `InProgress` target
  `ViewPoint`, the **captured delivery heads** when the traversal began, and the
  number of source records already processed.

Because the checkpoint contains only `CommitHash`es and source-record counts—no
in-memory `NodeIdx`es or `PayloadId`s—and because traversal order is strictly
canonical, a process that restarts after a crash can reload its stored wire items
into a fresh `Ledger` in *any* arrival order, call
`Materialization::resume(checkpoint, supports_visibility_changes)`, and continue
the interrupted traversal from the exact next commit. While `checkpoint.is_partial()`
is true, `checkpoint.heads()` still reports the last *completed* view's heads
rather than the partially applied target.

### Error handling in in-memory drivers (`DriveError`)

For in-memory `Document` and `Replica`, `Materialization::step` / `update` /
`change` return `DriveError<M::Error, E>`, distinguishing `Stockfoil(Error)` from
your materializer's `Apply(M::Error)`, `Checkpoint(M::Error)`, and edit closure
`Edit(E)` errors.

In-memory mutations are not database transactions: if `replica.receive(&item)`
admits an item into the document and your `Materializer::apply` subsequently
returns an error, the commit remains in `replica.document()`, and the
`Materialization` marks itself **poisoned** (`Error::Poisoned`) so you cannot
accidentally continue mutating a half-updated state. Recover by passing a fresh or
restored state to `Replica::rebuild`.
