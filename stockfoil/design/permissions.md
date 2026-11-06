# Permissions and Views

In `stockfoil`, **authentication** proves who authored a commit, while
**permissions** decide which authenticated commits are allowed to affect your
application's materialized state.

The [wire specification](spec.md#6-authentication) defines how cryptographic
signature evidence is verified. The [architecture reference](architecture.md)
covers how commits are admitted, exported, and materialized.

---

## Why Local-First Authorization Uses Write Frontiers

In a centralized client-server system, permissions are checked at write time
against a wall clock: when Bob submits an edit, the server checks if Bob currently
has write access. If Bob's access was revoked at 2:00 PM, the server rejects
anything from Bob after 2:00 PM.

In a local-first system built on a commit graph, there is no central gatekeeper
and no trusted wall clock—peers work offline and can set their system clock to
any value. Even more importantly, **you cannot enforce permissions by deleting or
rejecting unauthorized commits from the commit graph**:

1. **Hash-chain integrity:** Suppose Alice (an authorized writer) syncs with Bob
   while offline and creates her own valid commits on top of Bob's commit before
   learning that Bob was revoked. Because Alice's `CommitHash` commits to her
   parent hashes, deleting Bob's commit from the graph would break the hash chain
   and make Alice's valid commits unverifiable.
2. **CRDT metadata references:** Even when Bob's commit should not contribute a
   visible value to the document, Alice's valid descendant commit might reference
   structural metadata from it (such as an operation ID, key, or causal parent
   link).

Instead of mutating the graph, `stockfoil` keeps the authenticated commit graph
intact for synchronization and filters which commits *participate* when computing
application state.

In a causal graph, the only tamper-proof notion of time is **position in the
DAG**. When an author's write access is revoked, what you actually know is the
point in history where the revocation took effect: a set of commit hashes
`frontier`. Any commit by that author in the causal past of `frontier` happened
before the revocation; any commit outside that causal past (whether created after
the revocation, or created offline on an unmerged fork) did not.

---

## Document Policy (`WritePermissions`)

Because every application stores its access-control grants and revocations
differently (in a separate control document, an external capability service, or
special commits inside the document itself), `stockfoil` does not prescribe a
capability format. Your application evaluates its own authorization rules and
produces a `WritePermissions` policy: a rule per listed author, plus one rule for
every author it does not list.

A `Ledger` (or `Document` / `Replica`) holds one current `WritePermissions` policy
shared by all of its materializations, including historical checkouts. Permissions
are local policy and are never exported over the wire.

By default, `Ledger::new` grants only its local author `Unbounded` permission.
You can replace the policy at any time with `set_permissions`:

| `WritePermission` | Meaning within the selected history |
| --- | --- |
| `Unbounded` | All of this author's commits in the selected history participate. |
| `Bounded(frontier)` | Only commits by this author that are ancestors-or-self of at least one `frontier` head *within the selected history* participate. |
| `Denied` | None of this author's commits participate. |

Authors without their own entry get the policy's unlisted rule, `Denied` unless
set with `WritePermissions::with_unlisted`. Setting it to `Unbounded` admits every
author, for applications where another layer (a sync server's storage policy, a
capability system) already decides who may write. A listed author's own rule
always wins, so individual authors can still be bounded or denied. A `Bounded`
unlisted rule applies one shared frontier to all unlisted authors.

A few boundary rules keep `Bounded(frontier)` well-behaved when history is partial
or pinned:
- An empty frontier (`Bounded(vec![])`) permits nothing.
- Unknown frontier hashes, or frontier hashes that lie outside the materialization's
  selected history, provide no proof of permission. `stockfoil` filters the
  frontier to heads present in the selected history *before* taking their causal
  past, so an out-of-view commit (such as an unsigned draft when viewing signed
  history) cannot smuggle its ancestors into a view that excludes the frontier
  head itself.
- Permissions only filter commits; they never expand the selected history or
  substitute for a cryptographic signature.

```rust
use stockfoil::{
    Author, Codec, CommitHash, DriveError, Materializer, Replica, WritePermission,
};

fn revoke_after<C: Codec, M: Materializer>(
    replica: &mut Replica<C, M>,
    author: Author,
    frontier: Vec<CommitHash>,
) -> Result<(), DriveError<M::Error>> {
    let mut permissions = replica.document().permissions().clone();
    permissions.insert(author, WritePermission::Bounded(frontier));
    replica.set_permissions(permissions)?; // updates policy and refreshes working state
    Ok(())
}
```

Any other independent `Materialization`s attached to the same document will
observe the updated policy on their next `plan` / `update` call.

---

## Selected History (`Follow` and `ViewPoint`)

While all materializations of a document share its `WritePermissions`, each
`Materialization` chooses which heads of the commit graph it tracks via `Follow`:

- **`Follow::Working`**: Tracks all delivered heads, including unsigned local
  `Draft`s. This is the default for a live local working copy.
- **`Follow::Signed`**: Tracks only cryptographically admitted heads. (Admitted
  commits that happen to have unsigned local drafts built on top of them remain
  included; `stockfoil` tracks admitted heads separately rather than merely
  filtering working tips.)
- **`Follow::Pinned(heads)`**: Tracks the causal past of a fixed set of commit
  hashes. Updating the pin switches branches or historical snapshots; updating
  the document's `WritePermissions` still updates which commits within that pinned
  history participate.

Internally, a `ViewPoint` pairs the selected heads with a snapshot of
`WritePermissions`. Attaching a signature to a local draft can advance a
`Follow::Signed` materialization (and may release parked network items waiting on
that dependency) without delivering anything new to a `Follow::Working`
materialization that already saw the draft.

For read-only historical queries that do not need a full `Materialization`,
`ledger.ancestry(heads)` captures an owned `Ancestry` snapshot of the causal past
of `heads` under current permissions:
- `ancestry.contains(history, &hash)` checks whether `hash` is in the causal past
  of `heads`.
- `ancestry.participates(history, &hash)` checks whether `hash` is both in the
  causal past *and* permitted by the captured `WritePermissions`.

Given the same selected heads and `WritePermissions`, participation is completely
deterministic and independent of the order in which items arrived.

---

## Delivery Is Not Visibility

When new commits enter the graph, `Materialization` emits `Event::Deliver { hash,
author, parents, payload, active }` for **every** commit—even commits that lie
outside a pinned view or are disallowed by `WritePermissions` (`active: false`).

Why deliver inactive commits at all?
Because causal history is parent-closed, while participating commits need not be.
If Bob's commit $C_1$ is revoked (`active: false`), and Alice's permitted commit
$C_2$ (`active: true`) has $C_1$ as a parent, $C_2$ may refer to keys, object IDs,
or causal coordinates introduced in $C_1$. Delivering $C_1$ with `active: false`
lets your CRDT record the structural metadata it needs to interpret $C_2$, while
skipping $C_1$'s user-visible effect.

When a later call to `set_permissions` (or `set_follow`) changes whether an
already-delivered commit participates:
- If your state supports incremental visibility changes
  (`Materializer::supports_visibility_changes() == true`), the planner emits
  `Event::Activate { hash }` and `Event::Retract { hash }` for the affected
  commits.
- If your state does not support incremental retraction
  (`supports_visibility_changes() == false`), normal forward deliveries still
  happen incrementally, and whenever a previously active commit is retracted, the
  planner emits `Event::Reset` followed by a clean replay of the history.

---

## Application-Level Payload Validity (`Applied::rejected` and `wanted`)

Cryptographic authentication proves that an author signed a commit, not that its
payload bytes are meaningful to your CRDT.

- **Rejecting invalid payloads (`Applied::rejected`):** If a newly delivered commit
  contains a malformed or out-of-bounds application payload, your materializer
  reports it in `Applied::rejected`. Rejection is permanent *for that
  materialization* (and is saved in its `Checkpoint`); setting `cascade: true`
  also taints any causal descendants of the rejected commit. Rejecting a payload
  in a materialization does not remove the commit from `History` or stop it from
  syncing, so hash chains remain intact. (Note: genuine storage or I/O errors
  should return `Err(...)` to abort the batch rather than reporting a commit as a
  rejected payload.)
- **Requesting fallback payloads (`Applied::wanted`):** When retracting a winning
  write exposes an older, previously overwritten commit whose payload you did not
  keep in RAM, add that older commit's hash to `Applied::wanted`.
  `Materialization::complete` will immediately emit a follow-up `Batch` containing
  `Event::Payload { hash, payload }`. Always finish all follow-up batches before
  persisting your state and `Checkpoint` together.

See the [Rust multi-value register](https://github.com/alexjg/stockfoil/tree/main/examples/mv-register),
[JavaScript multi-value register](https://github.com/alexjg/stockfoil/tree/main/examples/mv-register-js), and
[sparse block volume](https://github.com/alexjg/stockfoil/tree/main/examples/volume) examples for complete implementations of
write-frontier revocation and fallback.
