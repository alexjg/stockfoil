//! # Stockfoil
//!
//! `stockfoil` is a library for building local-first data structures (such as CRDTs,
//! key-value stores, or replicated disk images) on top of an authenticated commit
//! graph.
//!
//! If you are familiar with the basic idea of a commit graph—a directed acyclic
//! graph (DAG) where each edit is a commit with parent hashes, an author, and an
//! operation payload—a naive implementation looks straightforward: keep a map of
//! commits by hash, topologically sort incoming commits, and apply their payloads
//! to your state.
//!
//! In practice, building a real local-first system on a commit graph runs into
//! three hard problems:
//!
//! 1. **[Materialization and incremental view maintenance](#1-materialization-and-incremental-view-maintenance):**
//!    Your application state is a view over the commit graph. As commits arrive
//!    from peers in arbitrary orders, as users inspect historical branches, or as
//!    commits are retroactively excluded, you need to keep that state up to date
//!    incrementally (or know when to reset and replay it).
//! 2. **[Compaction without losing verifiability](#2-compaction-and-implementing-a-codec):**
//!    Fine-grained editing produces tens of thousands of tiny commits. You need to
//!    coalesce ranges of the graph into compressed bundles so storage and sync stay
//!    fast, while still reconstructing the exact individual commits inside.
//! 3. **[Authentication and authorization without a central server](#3-authentication-authorization-and-write-frontiers):**
//!    Peers edit offline and sync peer-to-peer or through untrusted relays. You need
//!    to verify who authored every commit (even inside compacted bundles), and you
//!    need a way to revoke or bound a writer's permissions when there is no central
//!    gatekeeper or trusted wall clock—without deleting commits that later valid
//!    commits causally depend on.
//!
//! `stockfoil` solves these graph-level problems without performing I/O or imposing
//! an async runtime. To use it, your application provides two things:
//! - a **[`Materializer`]** (or an external [`Materialization`] loop) that folds
//!   commits into your data structure, and
//! - a **[`Codec`]** that packs and unpacks your commit payloads inside compacted
//!   bundles (or you can start with the built-in [`Concatenated`] codec).
//!
//! ---
//!
//! ## 1. Materialization and Incremental View Maintenance
//!
//! When you build a data structure on a commit graph, the commit graph is the source
//! of truth and your application state (an in-memory CRDT, a key-value index, or an
//! on-disk image) is a materialized view of that graph.
//!
//! In a simple append-only log, materialization is just `for commit in new_commits {
//! state.apply(commit); }`. In a local-first commit graph, maintaining a view is
//! harder for several reasons:
//!
//! 1. **Peers receive commits in different orders:** Over a network, concurrent
//!    branches and compacted bundles arrive in arbitrary order. Your state needs a
//!    deterministic, canonical traversal order so that any replay or multi-step
//!    catch-up behaves predictably.
//! 2. **You often want multiple views of the same graph:** Your UI usually wants a
//!    live working copy that includes local edits immediately (even before they are
//!    cryptographically signed), while a background task or audit view might want
//!    only signed history, or a checkout pinned to a specific historical commit.
//! 3. **Commits can become inactive after they were already applied:** Whether
//!    because you moved a pinned view to a different branch, rejected a corrupt
//!    payload, or revoked a writer's permissions (covered in
//!    [Section 3](#3-authentication-authorization-and-write-frontiers)), a commit
//!    that is already in the graph may need to have its effect excluded or
//!    retracted—even while later active commits still causally depend on its
//!    metadata.
//!
//! ### Views (`Follow`) and Incremental [`Event`]s
//!
//! A [`Materialization`] tracks one view of a document, configured with a [`Follow`]
//! rule:
//! - [`Follow::Working`]: Follows all delivered commits, including unsigned local
//!   [`Draft`]s. This is what [`Replica`] uses for its working copy.
//! - [`Follow::Signed`]: Follows only cryptographically admitted commits.
//! - [`Follow::Pinned(heads)`](Follow::Pinned): Follows the causal past of a fixed
//!   set of commit hashes (for historical snapshots or branches).
//!
//! Whenever commits arrive or the active view changes, [`Materialization`] diffs
//! your state's current position against the target view and emits [`Event`]s in a
//! deterministic, canonical ancestor-first order (breaking ties among ready commits
//! by smallest [`CommitHash`], regardless of network arrival order):
//!
//! - **[`Event::Deliver { hash, author, parents, payload, active }`](Event::Deliver):**
//!   Emitted once for every new commit entering the graph. Notice the `active:
//!   bool` field: **delivery is separate from visibility**. Even if a commit does
//!   not currently participate in the view (`active: false`—for instance, because
//!   it lies outside a pinned branch or was written by a revoked author), a later
//!   `active: true` commit may have it as a parent and reference structural
//!   metadata introduced in it (such as an operation ID or object key). Your
//!   materializer receives every commit so it can record whatever causal metadata
//!   it needs, and applies the commit's user-visible effect only when `active` is
//!   `true`.
//! - **[`Event::Activate { hash }`](Event::Activate) and
//!   [`Event::Retract { hash }`](Event::Retract) (or [`Event::Reset`]):**
//!   Emitted when a branch switch or permission change alters whether a previously
//!   delivered commit participates in the view:
//!   - **Incremental retraction (`supports_visibility_changes() == true`):** If your
//!     data structure can turn an already-delivered commit's effect on or off by
//!     hash, implement [`Materializer::supports_visibility_changes`] to return
//!     `true`. You will receive `Activate` and `Retract` events for just the
//!     affected commits. If retracting a winning write uncovers an older,
//!     previously overwritten commit whose payload you did not keep in memory,
//!     return [`Applied::want(older_hash)`](Applied::want) and `stockfoil` will
//!     deliver [`Event::Payload { hash, payload }`](Event::Payload) in an immediate
//!     follow-up batch.
//!   - **Reset and replay (`supports_visibility_changes() == false`, the default):**
//!     If writing incremental retraction logic is not worth the complexity for your
//!     data structure, leave `supports_visibility_changes()` as `false`. Normal
//!     forward growth still applies incrementally via `Event::Deliver`, and only
//!     when an already-applied commit actually needs to be retracted will
//!     `stockfoil` emit [`Event::Reset`] followed by re-delivering the active
//!     history from scratch.
//! - **Rejecting invalid application payloads ([`Applied::reject`]):**
//!   A commit can be well-formed at the graph layer and still contain payload bytes
//!   that violate your application's invariants. If [`Event::Deliver`] hands you a
//!   malformed or out-of-bounds payload, report its hash via [`Applied::reject`]
//!   (setting `cascade: true` if descendants building on it should also be
//!   tainted). The materialization permanently treats that commit as inactive
//!   without removing it from the shared commit graph.
//!
//! ---
//!
//! ## 2. Compaction and Implementing a `Codec`
//!
//! Collaborative documents and fine-grained CRDTs quickly accumulate tens of
//! thousands of small commits. Storing and syncing every commit individually has
//! two major costs:
//! 1. **Metadata overhead:** Every commit carries a 32-byte hash, 32-byte parent
//!    hashes, a 32-byte author public key, and a 64-byte signature (~130+ bytes of
//!    framing per edit).
//! 2. **Poor compression:** Similar operations across adjacent commits (such as
//!    repeated map keys, property names, or sequential text edits) cannot be
//!    compressed together if each commit is encoded in isolation.
//!
//! You want to bundle ranges of the commit graph into larger chunks and compress
//! their payloads together, while still being able to reconstruct the individual
//! commits and their hashes on the receiving side.
//!
//! ### Fragments
//!
//! `stockfoil` exchanges history as self-contained wire items ([`ItemId`]), which
//! are either a standalone [`ItemId::Commit`] or a compacted [`ItemId::Fragment`]
//! ([`FragmentRange`]):
//!
//! - **Deterministic fragment boundaries (Sedimentree):** Peers agree on where
//!   fragments start and stop without coordination by counting the leading zero
//!   bytes of each [`CommitHash`] (its *depth*). Most commits have depth 0.
//!   Roughly 1 in 256 commits starts with a `0x00` byte (depth 1), 1 in 65,536
//!   starts with `0x0000` (depth 2), and so on. Whenever a commit has depth
//!   $D > 0$, it defines a canonical fragment starting at that `head` commit and
//!   extending backward through its ancestors until it hits boundary commits of
//!   depth $\ge D$.
//! - **Automatic retirement:** Once a fragment is admitted, `stockfoil`
//!   automatically retires the individual standalone commits and smaller fragments
//!   it covers from future exports.
//!
//! ### What Your [`Codec`] Does
//!
//! Every wire item has two parts:
//! 1. A **header** (managed entirely by `stockfoil`) that compactly encodes the
//!    subgraph topology—the ancestor-first list of $N$ member commits, their
//!    [`Author`]s, their internal or external parent references ([`BundleMeta`]),
//!    and the covering [`Signature`]s.
//! 2. A **body** (managed by your application's [`Codec`]) that encodes the $N$
//!    member payloads as a single byte sequence.
//!
//! Each individual commit in your application has **canonical payload bytes**: the
//! uncompressed logical operation bytes that are hashed into that commit's
//! [`CommitHash`].
//!
//! When exporting an item (whether a 1-commit standalone item or an $N$-commit
//! fragment), `stockfoil` passes the [`BundleMeta`] and the slice of $N$ canonical
//! payloads to [`Codec::encode`], which produces the item's body bytes. When
//! receiving an item, `stockfoil` parses the header into [`BundleMeta`] and passes
//! it along with the body bytes to [`Codec::decode`], which must return the $N$
//! canonical payloads.
//!
//! This gives your application complete freedom over bundle compression—you can
//! extract a shared key dictionary across all $N$ commits, delta-encode, or run a
//! columnar compressor—subject to one invariant:
//!
//! > **The lossless round-trip rule:** [`Codec::decode`] must return the exact,
//! > byte-for-byte canonical payload for each of the $N$ members in [`BundleMeta`].
//!
//! As we will see next, reconstructing the exact canonical payload for each member
//! is what allows a receiver to cryptographically verify a compressed fragment
//! without trusting the peer who compressed it.
//!
//! If you do not need cross-commit compression right away, use the built-in
//! [`Concatenated`] codec, which simply prefixes each member's canonical payload
//! with its length.
//!
//! ---
//!
//! ## 3. Authentication, Authorization, and Write Frontiers
//!
//! In a traditional client-server application, authorization happens at a single
//! choke point in wall-clock time: when Bob submits a write at 2:05 PM, the server
//! checks whether Bob currently has write access. If Bob's access was revoked at
//! 2:00 PM, the server rejects the write and it never enters the history.
//!
//! In a local-first commit graph, there is no central server and no trusted wall
//! clock—an offline or malicious peer can set their clock to whatever they like.
//! Even more importantly, **you cannot enforce permissions by deleting unauthorized
//! commits from the commit graph**.
//!
//! Suppose Alice (who has write access) syncs with Bob while offline and creates
//! her own valid commits on top of Bob's commit before learning that Bob was
//! revoked. Because Alice's [`CommitHash`] is computed over her parents' hashes,
//! dropping Bob's commit from the graph would break the hash chain and make Alice's
//! valid work unverifiable.
//!
//! `stockfoil` therefore separates **authentication** (who wrote a commit, which is
//! an immutable property of the graph) from **authorization** (which commits are
//! allowed to contribute to materialized state, which is a local policy):
//!
//! ### Authentication and Transitive Signatures
//!
//! Every commit has:
//! - an [`Author`] (a 32-byte Ed25519 public key),
//! - an ordered list of parent [`CommitHash`]es, and
//! - canonical payload bytes defined by your application.
//!
//! From these, `stockfoil` computes a 32-byte BLAKE3 [`CommitHash`]. A commit is
//! *admitted* into synchronized history only when it is covered by a valid Ed25519
//! [`Signature`] from its [`Author`] over that [`CommitHash`].
//!
//! What about compacted fragments containing thousands of commits? Because every
//! commit hash commits to its parents' hashes, an author's signature on a commit
//! near the tip of a fragment *transitively authenticates* all of that author's
//! ancestor commits inside the same fragment. When receiving a fragment,
//! `stockfoil` uses your [`Codec::decode`] to reconstruct each member's canonical
//! payload, recomputes every member's [`CommitHash`] up the parent graph, and
//! verifies that every author's members are covered by a signature from that author
//! inside the fragment. If Alice authored 5,000 commits in a fragment and signed
//! the last one, a single 64-byte signature proves she authored all 5,000.
//!
//! ### Authorization: Write Frontiers
//!
//! Because wall clocks cannot be trusted in a decentralized graph, the only
//! tamper-proof notion of time is *causal position in the DAG*. When you revoke an
//! author's write access in a local-first system, what you actually know is the
//! point in history where that revocation took effect: a set of commit hashes
//! $[H_1, H_2]$. Any commit by that author in the causal past of $[H_1, H_2]$
//! happened *before* the revocation; any commit outside that causal past (whether
//! written after the revocation, or written offline on an unmerged fork) did not.
//!
//! `stockfoil` represents this directly with [`WritePermission`]:
//! - [`WritePermission::Unbounded`]: All of this author's commits in the selected
//!   history participate.
//! - [`WritePermission::Bounded(frontier)`](WritePermission::Bounded): Only commits
//!   by this author that are ancestors of (or equal to) at least one commit in
//!   `frontier` participate.
//! - [`WritePermission::Denied`] (or absent from the [`WritePermissions`] map):
//!   None of this author's commits participate.
//!
//! Different applications store permission grants and revocations in different
//! places—a separate access-control document, an external capability service, or
//! special commits inside the document itself. Rather than dictating an
//! authorization format, `stockfoil` takes a [`WritePermissions`] map (`HashMap<Author,
//! WritePermission>`) that you can update at any time via
//! [`Replica::set_permissions`] (or [`Ledger::set_permissions`]).
//!
//! When [`WritePermissions`] change, unauthorized commits remain in the commit
//! graph so that fragments, sync, and child commit hashes keep working, while the
//! [materialization layer](#1-materialization-and-incremental-view-maintenance)
//! delivers them with `active: false` or emits [`Event::Retract`] / [`Event::Reset`]
//! to remove their effects from your application state.
//!
//! ---
//!
//! ## Example: An In-Memory CRDT with [`Replica`]
//!
//! For data structures that fit in memory, [`Replica<C, M>`] bundles a
//! [`Document<C>`], your [`Materializer`] state `M`, and a [`Follow::Working`]
//! [`Materialization`], automatically updating your state after edits, receives,
//! signing, and permission changes:
//!
//! ```rust
//! use std::collections::BTreeMap;
//! use ed25519_dalek::SigningKey;
//! use stockfoil::{
//!     Applied, Author, Bytes, CommitHash, Concatenated, Event, ExportCursor,
//!     FormatId, History, Materializer, Replica, WritePermission, WritePermissions,
//! };
//!
//! // A simple grow-only map CRDT where each commit payload is "key=value".
//! // Because it does not implement incremental retraction, `stockfoil` will
//! // automatically send `Event::Reset` and replay active commits when a
//! // permission revocation removes a previously applied write.
//! #[derive(Default)]
//! struct KvStore {
//!     entries: BTreeMap<String, String>,
//! }
//!
//! impl Materializer for KvStore {
//!     type Error = &'static str;
//!
//!     fn apply(
//!         &mut self,
//!         _history: &History,
//!         events: Vec<Event<Bytes>>,
//!     ) -> Result<Applied, Self::Error> {
//!         let mut applied = Applied::default();
//!         for event in events {
//!             match event {
//!                 Event::Reset => self.entries.clear(),
//!                 Event::Deliver { hash, payload, active, .. } => {
//!                     let Ok(text) = std::str::from_utf8(&payload) else {
//!                         applied = applied.reject(hash, "invalid utf-8");
//!                         continue;
//!                     };
//!                     let Some((k, v)) = text.split_once('=') else {
//!                         applied = applied.reject(hash, "missing '='");
//!                         continue;
//!                     };
//!                     if active {
//!                         self.entries.insert(k.to_owned(), v.to_owned());
//!                     }
//!                 }
//!                 Event::Activate { .. } | Event::Retract { .. } | Event::Payload { .. } => {
//!                     unreachable!("supports_visibility_changes() is false")
//!                 }
//!             }
//!         }
//!         Ok(applied)
//!     }
//! }
//!
//! let alice_key = SigningKey::from_bytes(&[1; 32]);
//! let bob_key = SigningKey::from_bytes(&[2; 32]);
//! let alice = Author::from(&alice_key);
//! let bob = Author::from(&bob_key);
//!
//! let format = FormatId::new("example/kv-v1").unwrap();
//! let mut alice_replica = Replica::new(
//!     Concatenated::new(format.clone()),
//!     alice,
//!     KvStore::default(),
//! );
//!
//! // Alice makes a local commit and signs it.
//! let c1 = alice_replica.commit(b"greeting=hello").unwrap();
//! alice_replica.sign_drafts(&alice_key).unwrap();
//! assert_eq!(alice_replica.state().entries.get("greeting").unwrap(), "hello");
//!
//! // Bob syncs from Alice, then adds two commits of his own.
//! let mut bob_replica = Replica::new(
//!     Concatenated::new(format),
//!     bob,
//!     KvStore::default(),
//! );
//! bob_replica
//!     .set_permissions(WritePermissions::from([
//!         (alice, WritePermission::Unbounded),
//!         (bob, WritePermission::Unbounded),
//!     ]))
//!     .unwrap();
//!
//! let export = alice_replica.export(&ExportCursor::new());
//! for i in 0..export.len() {
//!     let (_item, bytes) = export.item(i).unwrap();
//!     bob_replica.receive(&bytes).unwrap();
//! }
//!
//! let bob_c1 = bob_replica.commit(b"author=bob").unwrap();
//! let _bob_c2 = bob_replica.commit(b"greeting=hijacked").unwrap();
//! bob_replica.sign_drafts(&bob_key).unwrap();
//!
//! // Alice receives Bob's commits, but bounds Bob's write frontier to `bob_c1`.
//! // `bob_c2` remains in the commit graph, but only `bob_c1` participates in state!
//! let bob_export = bob_replica.export(&ExportCursor::new());
//! for i in 0..bob_export.len() {
//!     let (_item, bytes) = bob_export.item(i).unwrap();
//!     alice_replica.receive(&bytes).unwrap();
//! }
//! alice_replica
//!     .set_permissions(WritePermissions::from([
//!         (alice, WritePermission::Unbounded),
//!         (bob, WritePermission::Bounded(vec![bob_c1.hash])),
//!     ]))
//!     .unwrap();
//!
//! assert_eq!(alice_replica.state().entries.get("author").unwrap(), "bob");
//! assert_eq!(alice_replica.state().entries.get("greeting").unwrap(), "hello");
//! ```
//!
//! ---
//!
//! ## Reference and Advanced Topics
//!
//! Once you know the three core concepts above, the rest of the crate provides
//! finer control over storage, signing, queries, and crash recovery.
//!
//! ### Choosing an Entry Point
//!
//! `stockfoil` is layered so you only take on the manual plumbing you actually need:
//!
//! | Layer | When to use it | How payloads are stored |
//! | --- | --- | --- |
//! | [`Replica<C, M>`] | State and commit payloads fit in memory, and you want one primary working state kept current automatically. | In memory as [`Bytes`] (`Arc<[u8]>`) |
//! | [`Document<C>`] + [`Materialization`] | Commit payloads fit in memory, but you want to drive one or more materializations manually (e.g., multiple branches or custom batching). | In memory as [`Bytes`] |
//! | [`Ledger`] + [`Materialization`] | Payloads live on disk, in SQLite, or in an external store, or are too large to hold in RAM (see `examples/volume`). | Caller-assigned `u64` [`PayloadId`] handles |
//! | [`History`] | Low-level graph topology, admission, and certificate engine without document policy or a dependency parking queue. | Caller-assigned `u64` [`PayloadId`] handles |
//!
//! ### Local [`Draft`]s, Zero-Echo Edits, and External Signing
//!
//! Local commits and signing are decoupled so that signing can happen on a hardware
//! key, over WebCrypto, or in batches without blocking local edits:
//!
//! - **Unsigned [`Draft`]s:** [`Replica::commit`], [`Document::commit`], and
//!   [`Ledger::commit`] immediately deliver an unsigned [`Draft`] to the local
//!   graph. A [`Follow::Working`] materialization sees drafts right away, and
//!   subsequent local commits can build on them, but drafts are **never exported**
//!   to peers until signed. (Because drafts are not yet exportable wire items,
//!   applications that delay signing across process restarts must journal unsigned
//!   drafts themselves.)
//! - **Direct state editing without local echo ([`Replica::change`]):** Some in-memory
//!   CRDTs prefer to mutate their state directly during a user edit and emit the
//!   resulting operation bytes, rather than encoding operations first and waiting
//!   for [`Event::Deliver`] to echo them back. [`Replica::change`] (and
//!   [`Replica::change_draft`]) runs an edit closure against your state, commits the
//!   returned canonical bytes on top of the current heads, and calls
//!   [`Materialization::mark_local`] to advance the materialization past that commit
//!   without re-delivering it.
//! - **External signing:** If you have a synchronous [`Signer`] (such as an
//!   `ed25519_dalek::SigningKey`), call `sign_drafts`. If signing is asynchronous
//!   or remote, take the owned [`Draft`], sign [`Draft::signing_bytes`] outside the
//!   library without holding any borrow on the document, and pass the resulting
//!   [`Signature`] to [`Document::attach_signature`] or
//!   [`Ledger::attach_signature`].
//!
//! ### Exporting and Receiving Wire Items
//!
//! - **Exporting ([`ExportPlan`], [`ExportCursor`]):** Calling `export(&cursor)`
//!   plans the minimal set of admitted items (standalone commits and fragments)
//!   that a peer or storage backend has not yet acknowledged. Call
//!   [`ExportPlan::acknowledge`] only after the destination has durably stored an
//!   item, then save [`ExportPlan::cursor`]. [`ExportCursor::to_bytes`] serializes
//!   the cursor in a versioned, graph-independent format.
//! - **Out-of-order arrivals and dependency parking ([`Received`], [`Queue`]):**
//!   Network items can arrive before their parent commits or fragment boundaries.
//!   When an item arrives early, `stockfoil` still verifies its wire format,
//!   reconstructs its member hashes, and verifies all Ed25519 signatures
//!   *immediately*, so invalid bytes are rejected before anything is buffered.
//!   Verified items missing an admitted dependency are parked in a bounded queue
//!   (configurable via [`Ledger::with_max_parked`], defaulting to 1024 items and
//!   returning [`Received::Waiting`] or [`Received::Evicted`]) and are admitted
//!   automatically as soon as their dependencies arrive.
//!
//! ### Historical Queries Without Re-Materializing ([`Ancestry`], [`Commits`])
//!
//! If your application maintains its own index (for example, a table mapping keys
//! or disk blocks to the [`CommitHash`]es that wrote them), you do not need to spin
//! up a new [`Materialization`] just to read historical state:
//! - [`Ledger::ancestry(heads)`](Ledger::ancestry) captures an owned, cheap-to-clone
//!   [`Ancestry`] handle representing the causal past of `heads` and the current
//!   [`WritePermissions`] snapshot.
//! - [`Ancestry::contains`] tests whether a commit is in the causal past of `heads`;
//!   [`Ancestry::participates`] tests whether it is both in the causal past *and*
//!   permitted by the captured [`WritePermissions`].
//! - [`Ancestry::difference`] and [`Ledger::commits_since`] return a lazy,
//!   exact-sized [`Commits`] iterator yielding causal commit differences in
//!   canonical ancestor-first order.
//!
//! ### Out-of-Core / Sans-I/O Operation (`Ledger` + `Materialization`)
//!
//! When commit payloads are too large for RAM or live on disk (as in
//! `examples/volume`), use [`Ledger`] and [`Materialization`] directly:
//!
//! - **Creating commits:** Call [`Ledger::begin_commit`], stream canonical payload
//!   chunks through [`CommitBuilder::update`] while writing them to your storage,
//!   and pass the finished [`Prepared`] commit plus your storage's [`PayloadId`] to
//!   [`Ledger::commit`].
//! - **Receiving items in bounded memory ([`Admission`]):** Call
//!   [`Ledger::begin_receive`] to get an owned [`Admission`] state machine. Drive
//!   [`Admission::next`] and answer its [`Request::Read`] and [`Request::Decode`]
//!   requests using your own fixed-size buffers. Once byte verification finishes
//!   ([`Next::NeedHistory`]), pass the admission to [`Ledger::admit`] along with a
//!   [`Bind`] callback that assigns [`PayloadId`]s to newly delivered members.
//! - **On-demand extraction and integrity checking ([`PayloadCheck`], [`DecodeItem`]):**
//!   Your storage does not need its own cryptographic integrity layer. When reading
//!   canonical payloads back from disk, stream them through
//!   [`History::check_payload`] ([`PayloadCheck`]) to verify they match the commit
//!   hash. If you store only compressed wire items on disk rather than unpacked
//!   member payloads, [`History::decode_item`] ([`DecodeItem`]) streams and
//!   verifies just the requested member payloads out of a stored wire item in a
//!   single bounded-buffer pass.
//! - **Budgeted materialization and crash recovery ([`Batch`], [`Checkpoint`]):**
//!   Drive [`Materialization::plan(&ledger, budget)`](Materialization::plan) and
//!   [`Materialization::complete(&ledger, batch, applied)`](Materialization::complete)
//!   in a loop until `complete` returns `Ok(None)` (meaning all follow-up
//!   [`Event::Payload`] requests and cascading rejections for that batch have
//!   finished):
//!
//!   ```rust,ignore
//!   while let Some(mut batch) = checkout.plan(&ledger, 64)? {
//!       loop {
//!           let applied = image.apply(&batch, &my_storage).await?;
//!           match checkout.complete(&ledger, batch, applied)? {
//!               Some(more) => batch = more,
//!               None => break,
//!           }
//!       }
//!       // Persist application state and checkout.checkpoint().to_bytes()
//!       // together in one atomic transaction.
//!       image.persist(checkout.checkpoint()).await?;
//!   }
//!   ```
//!
//!   Always persist received wire items **before** materializing their commits, and
//!   save your application state and [`Checkpoint::to_bytes`] **atomically** after
//!   each outer batch completes. Because a [`Checkpoint`] records progress using
//!   only [`CommitHash`]es and canonical traversal counts—never in-memory node
//!   indices or [`PayloadId`]s—a process that crashes mid-traversal can rebuild its
//!   [`Ledger`] from stored wire items in *any* order and resume materialization
//!   from the exact point where it left off.
// Compile the Markdown API examples in normal `cargo test --doc` runs without
// adding implementation-only documentation modules to the published API.
#[cfg(doctest)]
mod design_docs {
    #[doc = include_str!("../../design/permissions.md")]
    mod permissions {}
    #[doc = include_str!("../../design/architecture.md")]
    mod architecture {}
}

mod codec;
mod document;
mod drive_error;
mod ledger;
mod materialize;
mod replica;
mod signer;

/// Canonical bytes of one in-memory commit. Cloning does not copy its contents.
pub type Bytes = std::sync::Arc<[u8]>;

pub use codec::{Codec, Concatenated};
pub use document::{Document, Export};
pub use drive_error::DriveError;
pub use ledger::{Bind, Draft, ExportedItem, Ledger, Received, Signed};
pub use materialize::{
    Applied, Batch, Event, Materialization, Materializer, Progress, Rejection, Step,
};
pub use replica::Replica;
pub use signer::Signer;

mod admission;
mod author;
mod commit_hash;
mod decode;
mod error;
mod evidence;
mod history;
mod meta;
mod queue;
mod reader;
mod signature;
mod verified;
mod verify;
#[cfg(feature = "fuzzing")]
pub mod wire;
#[cfg(not(feature = "fuzzing"))]
pub(crate) mod wire;
mod write_permission;

pub use admission::{
    Admission, Counts, DecodeStatus, Next, ReceiveOptions, Reply, ReplyError, Request, Ticket,
};
pub use author::{Author, InvalidAuthor};
pub use commit_hash::{CommitHash, InvalidCommitHash};
pub use decode::{DecodeItem, DecodeNext};
pub use error::Error;
pub use history::{
    Ancestry, Checkpoint, CommitBuilder, CommitInfo, Commits, Delivered, ExportCursor, ExportItem,
    ExportPlan, Follow, Heads, History, PayloadCheck, Prepared, Receipt, ViewPoint,
    WritePermissions,
};
// Raw traversal machinery is internal; the fuzz harness still exercises it
// directly against its independent model.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub use history::{Batch as ChangeBatch, Change, Changes, MaterializationCursor, Vis};
#[cfg(not(feature = "fuzzing"))]
pub(crate) use history::{Batch as ChangeBatch, Changes, MaterializationCursor, Vis};
#[cfg(feature = "fuzzing")]
pub use meta::Evidence;
pub use meta::{
    BundleMeta, FormatId, FragmentRange, ItemId, Limits, MemberShape, ParentRef, PayloadId,
};
pub use queue::{Evicted, Queue, QueueNext};
pub use signature::Signature;
pub use wire::header_len;
pub use write_permission::WritePermission;
