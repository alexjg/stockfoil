# History, admission, and adapters: contracts

This describes the implemented architecture. The wire format is specified
separately in [wire-v2.md](wire-v2.md); author and permission semantics in
[authors_signatures_revocation.md](authors_signatures_revocation.md).

```text
Native Rust Graph<C, S>        JavaScript Graph (javascript/)
            \                     /
     host requests: Read, Decode, Bind
                      |
          Admission (owned operation)
                      |
   payload-independent History (topology, admission, evidence)
```

There is one history implementation and one receive state machine. Adapters
execute requests with application objects; they contain no verification,
publication, or export logic.

## 1. History

`History` owns delivered topology, authorship, logical payload IDs, admission
state, and the signature evidence needed to re-export what it admitted. It holds
no payload bytes, codec, storage handle, callback, runtime, or permission policy,
and it never calls application code. It is `Send + Sync`; callers serialize
mutation (bindings wrap it in a lock).

- Nodes are append-only and parents-first: a node index is a topological position.
- Every node belongs to one single-author **chain**, ordered by ancestry. Ancestry,
  views, and change sets are chain-prefix clocks; differences between clocks are
  enumerated as per-chain intervals, so incremental queries cost about the size of
  their answer rather than of the history.
- A commit is **delivered** (in the working history, possibly an unsigned local
  draft) or **admitted** (authenticated). Admission is parent-closed.
- `epoch()` increments on every delivery or admission.

Queries: `lookup` (retained payload ID, author, admitted), `parents` (ordered,
repeats kept), `heads(Delivered | Admitted)`, `drafts`, `is_ancestor`, `traverse`,
`traverse_since` (unknown hashes ignored).

## 2. Local commits and signing

```rust,ignore
let mut builder = history.begin_commit(author, parents)?; // parents checked first
builder.update(canonical_bytes);                           // any chunking
let prepared = builder.finish();                           // hash, signing_bytes()
history.publish_commit(prepared, payload_id)?;             // an unadmitted draft
let admitted = history.sign(author, hash, signature)?;     // may admit several drafts
```

`sign` verifies the signature, stores it on the draft, then admits every signed
draft whose certificate is complete, in topological order until nothing changes:

- At depth 0 the certificate is the standalone commit. Its author must have signed
  it and its parents must be admitted.
- At depth `d > 0` the certificate is the draft's canonical fragment: boundary =
  the depth-`d` stops below it, members = its ancestry minus the boundaries'.
  Boundaries and external parents must be admitted, and every member must be
  covered by a signature of its author on itself or a member descendant (the same
  rule as received items). Other authors' drafts inside the range block assembly
  until they sign.
- If the fragment cannot be certified only because an already admitted member's
  covering signature lies outside it (that member was admitted when its author
  signed a descendant), a draft signed by its own author with admitted parents is
  certified as a standalone commit instead, which any depth allows.

Signing an admitted commit is `Error::NotDraft`. A signature by another author on
a draft counts toward that author's own commits below it. A certificate keeps only
the signatures that cover its members: per author, those that no later signature
by the same author already reaches. Signatures that cover nothing, such as a
co-signature by a key with no commit in the item, are not kept or exported.

## 3. Admission (receiving)

A received item is one stored blob: wire header plus codec body. `History::receive`
creates an `Admission` that owns its buffers and progress and borrows nothing. It
does not know where the item is stored: the host that answers its requests does.
Storage keys are the host's business and need not be content hashes (an
end-to-end encrypted store knows only ciphertext digests), and nothing about the
stored representation is checked beyond the canonical payloads it decodes to.

```text
Read*        parse the header
Decode*      hash canonical members; boundaries, EOF, codec Finished
             signatures and per-author coverage            — Admission::next
dependencies admitted? fragment placement                  — History::advance
Bind         payload IDs for the members (skipped if all are already delivered)
atomic publication                                         — History::advance
```

The operation parses the header itself from the bytes it hashes, checks the
format ID against trusted configuration before any decoding, and treats decoder
output only as canonical bytes to hash: no host reply means "verified".

**Driving.** `next()` does at most one work quantum without the history and returns
exactly one of `Continue`, `Request(ticket, request)`, `AwaitReply`, `NeedHistory`,
`Waiting(hash)`, `Done(receipt)`, `Failed(error)`. `History::advance(&mut op)`
does the history-dependent step and then reports the next state (possibly the
`Bind` request). `request()` returns the outstanding request again.

**Requests.** At most one is outstanding; its inputs do not change until answered.

| Request | Reply |
| --- | --- |
| `Read { offset, max }` | `Read { bytes, eof }` — empty only with `eof` |
| `Decode { meta, input, eof, capacity }` | `Decoded { consumed, output, status }` |
| `Bind { meta }` | `Bound(ids)` — one per member, bound to (item, member) |
| any | `Failed(message)` — terminal |

Decoder statuses: `Working` (must have made progress), `NeedInput` (consumed all
input), `EndMember` (output completes the current member; every member ends once,
empty ones included), `Finished` (no body bytes may remain). The host creates one
decoder per operation from `meta` on the first `Decode`.

**Replies.** A reply with the wrong ticket (`Stale`) or kind (`WrongKind`) is
rejected without consuming anything or affecting the operation. A reply that
breaks the protocol (more consumed than offered, output over capacity, a stalled
`Working`, too many members, …) fails the operation (`ReplyError::Invalid`).
Tickets are unique across all operations in the process, so a reply meant for
another operation is always stale. They never wrap; exhaustion fails.

**Waiting.** `Waiting(hash)` means a dependency is not admitted. The operation
records the epoch; `advance` returns immediately until `epoch()` changes. Hosts
simply retry every waiting operation after any publication — there is no waiter
registry.

**Publication** inserts undelivered members, admits delivered drafts among them,
records the certificate with only the evidence that covers it (as for local
certificates), and admits any local drafts this completes,
in one `advance` call (cost proportional to the item). The first publication of
a hash fixes its payload ID; `Receipt.members[i].newly_delivered` is relative to
the history, not to any materializer.

**Cancellation** is dropping the operation. It holds no state inside the history,
so nothing needs releasing; late replies to a dropped operation go nowhere.

**Counts.** `counts()` reports exact bytes read, encoded bytes decoded, canonical
bytes hashed, and requests issued, also after failure.

**Resources.** Payload buffers are bounded by `read_size` and `decode_capacity`
regardless of item size (items over 4 GiB are tested). Header metadata is bounded
by `Limits` and scales with the item; the defaults admit fragments of up to 2^20
members. Canonical and stored byte totals are unbounded by default so payloads
of any size stream; applications that need to bound hashing work per item set
them. Work per `next()` is bounded by `quantum` (bytes hashed; one signature
counts as 16 KiB). History steps (placement and publication) are not budgeted:
they cost about the size of the item and of the history between its external
parents and its boundary.

## 4. Export

The inventory holds one certificate per admitted item (received or assembled),
minus those a later fragment covers: a fragment retires the standalone
certificates of its members and any smaller fragment entirely inside it.

```rust,ignore
let mut plan = history.export(&cursor);       // items the destination lacks
for i in 0..plan.len() {
    let item = history.export_item(&plan, i)?; // meta + (hash, payload ID) members
    let header = wire::encode_header(&item.meta, &format);
    // write header, then the codec's encoder output, to the destination
    plan.acknowledge(i)?;                      // the destination durably has it
}
cursor = plan.cursor();                        // advances only over acknowledged items
```

Plans are captured: later admissions belong to a later plan. Items are ordered by
head delivery order, which puts most dependencies first, but receivers must still
tolerate waiting. `retired()` lists cursor items no longer in the inventory; they
are not deletion commands. `ExportCursor` is a set of item IDs with a durable
encoding (`to_bytes`/`from_bytes`), independent of node indices and admission
order. Exported fragment members are in canonical order (parents first, smallest
hash first among ready members). There is no sender rehash: encoders read payloads
by ID from application storage, and may reuse an earlier stored body.

## 5. Views and changes

A `ViewPoint` is heads plus `WritePermissions` (`HashMap<Author, WritePermission>`).
A commit participates if it is in the heads' ancestry and its author's permission
allows it: `Unbounded`; `Bounded(frontier)` — ancestor-or-self of a frontier head
that is itself in the view's history; `Denied` or absent — never.
`view_point(&permissions)` captures delivered heads (drafts included);
`signed_view_point` captures admitted heads.

```rust,ignore
let mut changes = history.changes(&cursor, &target)?;      // captured now
loop {
    match history.next_changes(&mut changes, &mut records, 1024)? {
        Batch::More => {}
        Batch::Complete(next) => { cursor = next; break }
    }
}
```

A record (`hash`, retained `payload` ID, `newly_delivered`, `visibility`) is
produced once for each commit that the cursor's delivered history lacks or whose
participation differs between the cursor's view and the target. Newly delivered
commits that do not participate are still reported. Records are parents-first. All
heads named by the cursor and target must be delivered; this is checked before any
record is produced. Nothing reads payloads; fetch them by ID separately.

`MaterializationCursor` has a durable encoding. Persist it atomically with the
application state after a **complete** traversal. Mid-traversal checkpoints are
deferred: an interrupted traversal restarts from the last saved cursor.

`diff(before, after)` gives participation changes only.

## 6. Native adapter

`Graph<C: Codec<S>, S: Storage>` owns a history, codec, and storage and drives
the same requests synchronously.

- `Storage`: blobs under keys of the storage's choosing (`type Key`;
  `create`/`finish` returns the key, `read_at`), and logical IDs
  (`bind(PayloadLocation<Key>)`, `location`). `PayloadLocation::Member { item,
  index }` means "decode member `index` of stored item `item`"; there is no
  per-member canonical object on receive. Nothing read back is trusted.
- `Codec`: `format`, `decoder(meta)`, `encoder(item)`; `Decoder::step(input, eof,
  output)` and `Encoder::step(storage, output)`.
- `receive(reader)` stores then admits; `admit(&key)`; `begin_admit` +
  `drive(&key, op)` keep a verified operation across `Driven::Waiting`. `commit`,
  `sign`, `export`, `write_item`, `store_item`, `payload(&commit)`,
  `read_payload(&commit, out)`, and `decode_item(&key, wanted, visit)` (one
  decoding pass for many members of a stored item).
- `MemoryStorage` is a complete in-memory store keyed by BLAKE3 hashes.

**Checked reads.** `History::check_payload(commit)` returns a `PayloadCheck`: feed
it canonical bytes, and `finish` recomputes the commit hash from them and the
parents and author recorded in history, failing with `Error::CorruptPayload` on
any difference. `Graph` reads payloads only through it, so storage needs no
integrity guarantees of its own: a lost, corrupted, or substituted blob makes reads
fail rather than return wrong bytes. Streamed bytes precede the check's verdict;
on `CorruptPayload` discard what was received. Encoders still read storage
directly for export; a corrupt payload there produces an item that receivers
reject. Bindings expose `checkPayload` for hosts that read payloads themselves.

## 7. Bindings

`javascript/` (wasm-bindgen) exposes the history and admission objects with owned
copies at the boundary (BigInt payload IDs) and an async TypeScript driver whose
host reads storage and runs codecs with promises. Other languages (Python, Kotlin,
Swift) are expected to use one UniFFI crate over the same objects; not yet built.

## 8. Departures from the earlier sketches

- Publication is a single pass over the item's members, not a constant-time
  activation over privately interned claims. Operations therefore hold nothing in
  the history, and abandonment needs no queue or explicit call.
- No dependency-waiter registry: waiting operations are retried on epoch change.
- Signing is synchronous and assembles certificates for drafts, including
  fragments; it is not a request-driven operation.
- Local commits and export are plain calls; only receiving uses host requests.
- One exclusive `Next` state instead of independent readiness flags; publication
  happens inside `History::advance`.
- Received items are read back from storage instead of being staged from a
  stream. Storage keys are opaque; payload reads are checked against commits
  instead of trusting content-addressed keys.
- One wire format (v2); no probe, no v1 reader, no SFSE envelope.
- Export inventory is "every certificate not covered by a fragment", not a
  recomputed minimal cover.
- Mid-traversal materialization checkpoints are deferred.
