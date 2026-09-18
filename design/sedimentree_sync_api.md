# Sedimentree sync: API sketch

A starting point for [Sedimentree Sync](sedimentree_sync.md), not a settled API.
Names and Rust signatures below are sketches. No implementation changes yet.

Decisions so far:

* Use Subduction's leading-zero-byte depth metric.
* Implement the sedimentree algorithms in Stockfoil, without `sedimentree_core`.
* Start with unbounded memory and whole-buffer codecs; streaming comes later.
* Make loose commits and fragments independently verifiable, with their author
  signatures inside their blobs. Index the graph around fragment boundaries.

Three pieces:

* Stockfoil maintains boundary indexes and selects a cover of sealed items.
* The CRDT supplies a codec for individual commits and compressed runs.
* Stockfoil compares summaries and produces wants, needs, and fixups. The caller
  does the IO, with or without Subduction.

Keep these separate. Producing a fragment shouldn't require a connection, and
comparing summaries shouldn't require decoding the CRDT.

## What gets synchronized

Use publishable history, not the current `ViewPoint`'s participating commits.
Revocation mustn't change the sedimentree or discard the data needed to restore
an older view. Each advertised item must be sealed: its contents and included
signatures suffice to verify it without reading another item.

* A loose commit carries its author's signature over that commit's own hash.
* A fragment carries enough signatures to authenticate every enclosed commit,
  including the boundary commits. All proof targets and ancestry paths used for
  that authentication must be inside the fragment.

This is stronger than the current prototype's `traverse_publishable` and `merge`
rules. Global signature coverage doesn't make an unsigned loose commit a valid
sync item. Unsealed data can remain stored, and our own pending edits can still
appear optimistically. Neither makes it a complete sedimentree item.

For the first cut, export a causally complete cover of delivered history using
sealed items. Don't publish a local descendant whose dependencies can only be
served as unsigned or incomplete items. A received fragment can be verified
before its external parents arrive; causal delivery still waits for them.

A summary describes available representations of history. It is neither a
`ViewPoint` nor proof of author permissions. Permission snapshots and local
unsigned-edit allowances do not travel with sync.

## Describing the pieces

Own the public types in Stockfoil. Adapt them to Subduction rather than exposing
Subduction's types throughout the CRDT API. Each summary is for one document;
the caller supplies document identity and agrees on the codec. The depth metric
is fixed: count leading zero bytes in commit hashes.

```rust
struct FragmentRange {
    head: CommitHash,
    boundary: BTreeSet<CommitHash>,
}

struct BlobMeta {
    hash: BlobHash,
    size_bytes: u64,
}

struct LooseCommit {
    commit: CommitHash,
    parents: BTreeSet<CommitHash>,
    blob: BlobMeta,
}

struct Fragment {
    range: FragmentRange,
    checkpoints: BTreeSet<Checkpoint>,
    blob: BlobMeta,
}

struct SedimentreeSummary {
    loose_commits: Vec<LooseCommit>,
    fragments: Vec<Fragment>,
}
```

`Checkpoint` represents the interior checkpoint identifiers needed for
Sedimentree's covering checks. Upstream currently truncates these to 12 bytes.
They are reconciliation hints, not a substitute for validating decoded history.

Keep three hashes distinct: a commit's identity, its canonical payload hash,
and the hash of a transport blob. Recompressing a fragment can change the last
one without changing any commits or the fragment's causal range.

Likewise, don't identify a fragment by its head alone. Its causal identity is
`(head, boundary)`. A concrete representation also has a blob hash. Two encoders
may produce different blobs for the same range without either peer missing
history. Both representations must independently carry complete author evidence;
an incomplete blob isn't another valid representation of the same range.

## Viewing the graph as a sedimentree

```rust
impl<P> CommitGraph<P> {
    fn as_sedimentree(&self) -> SedimentreeView<'_, P>;
}

impl<P> SedimentreeView<'_, P> {
    fn loose_commits(&self) -> impl Iterator<Item = SignedCommitRef<'_, P>>;
    fn fragments(&self) -> impl Iterator<Item = FragmentRef<'_, P>>;
    fn encode<C: SyncCodec<P>>(
        &self, codec: &mut C,
    ) -> Result<EncodedBatch, SyncError<C::Error>>;
}
```

`SedimentreeView` borrows the boundary indexes; it doesn't decompose the entire
graph afresh on every sync. It selects a cover of sealed items and supplies
commit iterators; it doesn't choose the CRDT's compressed format.
`SignedCommitRef` provides metadata, a payload reference, and the direct author
signature. `FragmentRef` provides its range, checkpoints, fragment-local proof
set, and selected commits in deterministic parent-before-child order. Neither
lets an encoder accidentally traverse unpublished work. Unsealed candidates stay
in the local index, not in this export view or its summary.

Proposed range convention: include the head and boundary commits, but don't
walk through boundaries to their parents. At a head's nonzero depth, stop each
backwards walk at an ancestor of at least that depth, or at a root. Reaching a
root with no qualifying checkpoint includes that root without inventing a
boundary hash. Merges can produce several boundaries. Confirm these rules
against the Subduction adapter before implementing them.

Overlaps are normal; ingestion deduplicates by commit hash. Build candidate
fragments at qualifying heads, then choose a cover and leave uncovered commits
loose. Don't include every interior commit hash in the exported summary: that
would undo the point of compression.

Use Subduction's `CountLeadingZeroBytes` rule: `00 00 12 ...` has depth 2.
The earlier description in terms of hex digits was a mistake. Test-only depth
fixtures can make small examples practical without making the live metric
configurable.

## Boundary indexes and proof retention

Make boundaries part of the storage model, not just something we discover while
exporting. The sketch needs:

* A boundary index keyed by full commit hash, recording depth and the indexed
  ranges starting or ending there. Truncated wire checkpoints aren't index keys.
* A range index keyed by `FragmentRange`, locating its commit run, lower-level
  ranges, and proof requirements. Track open/unsealed candidates separately from
  sealed fragments. A loose tail holds commits not yet covered by a sealed range.
* A persistent signature store keyed by `(Author, CommitHash)`, indexed by the
  loose items and canonical fragment ranges that require each proof. Fragment
  proofs remain required even when their range isn't in the current sync cover.

Update these indexes as commits and signatures arrive. Producing a fragment
walks indexed runs; applying a verified fragment records its range and proof set
as a unit, deduplicating overlaps. The first implementation can retain the
per-commit index for exact ancestry and `ViewPoint` queries. Boundaries don't
coarsen permissions or make all commits in a fragment participate together.

The current per-author authentication frontier remains useful for viewpoints,
but is no longer the signature retention policy. A signature can be redundant
for the document and still be necessary for a particular fragment.

A simple sufficient proof set is a signature on each causally maximal commit
by each author *within the fragment*. A proof may cover other commits only by
that same author, using paths wholly inside the fragment. One author's signature
on the fragment head does not authenticate other authors' commits. Retained
proofs may provide a smaller equivalent set; there is no need to minimize it in
the first implementation.

For example, in an Alice-authored chain `a -> b -> c -> d`, a signature on `d`
authenticates the whole chain in memory. It cannot replace a signature on `b`
for an indexed fragment containing only `a, b`. That fragment must retain its
own usable proof. A concurrent Alice branch may require another signature too.

**Never prune proofs needed for fragments.** This isn't conditional on a fragment
being in today's cover. Sedimentree is intended to be the on-disk format too:
load a fragment and its causal predecessors, and it must contain everything
needed to verify and apply it, without contacting an author for fresh signatures.
Initially, retaining all verified signatures is fine.

Compacting smaller fragments into a larger one must preserve their required
proofs, not just the minimum set needed to authenticate the outer range. Carry
those proofs in the larger blob so a fresh replica can reconstruct and save the
contained canonical fragments too. Deduplicate identical proof records, but
never drop a required proof just because a later document-wide signature covers
its commit. This also strengthens what counts as a complete incoming fragment:
its proof set must support the canonical subfragments wholly contained in it.

### Signing follows sedimentree boundaries

Stockfoil determines signing requirements while preparing local, unpublished
data. The boundary indexes identify the loose-commit signatures and range-local
author tips that must be signed. The application supplies those signatures,
possibly asynchronously; it shouldn't have to plan fragments or choose proof
sets itself. Don't publish until those requirements are satisfied, and preserve
the resulting fragment proofs permanently.

Batched signing works when it supplies the proofs for the canonical fragments
being published, including their contained ranges. A still-loose tail needs
direct signatures on its commits. A later tip signature alone isn't enough if
it skips proofs required at intervening fragment boundaries.

Once data has been published or accepted as a complete fragment, saving,
forwarding, compacting, or restoring it must never require new signatures.
Only new local, unpublished data can generate signing work. Missing proofs in
received data are an incomplete-item error, not a request to re-sign history.
This replaces the prototype's global-frontier-only pruning and signing model.

## The CRDT codec

`encode_fragment` is called once for the **whole fragment**, not once per commit.
Make the input explicit:

```rust
struct CommitRef<'a, P> {
    meta: &'a CommitMeta,
    payload: &'a P,
}

struct SignedCommitRef<'a, P> {
    commit: CommitRef<'a, P>,
    signature: &'a Signature,
}

impl<P> FragmentRef<'_, P> {
    fn range(&self) -> &FragmentRange;
    fn checkpoints(&self) -> &BTreeSet<Checkpoint>;
    fn commits(&self) -> impl Iterator<Item = CommitRef<'_, P>>;
    fn signatures(&self) -> impl Iterator<Item = &CommitSignature>;
}
```

`commits()` traverses the entire selected batch and can be called more than once.
The encoder can collect it, share author/string tables, encode columns, or
run-length encode operations across commit boundaries. It does **not** have to
call `encode_commit` or concatenate individually encoded commits. The input's
parent-before-child order doesn't constrain the physical layout of the bytes.
The signature iterator includes the proofs that must survive for contained
canonical ranges.
A codec can also borrow the CRDT's own operation store when `P` is a handle into
that store; it must still encode exactly the selected commits.

```rust
struct DecodedCommit<P> {
    contents: CommitContents,
    payload: P,
}

struct CommitSignature {
    author: Author,
    commit: CommitHash,
    signature: Signature,
}

struct DecodedLooseCommit<P> {
    commit: DecodedCommit<P>,
    signature: Signature, // By this commit's author, over its own hash.
}

struct DecodedFragment<P> {
    commits: Vec<DecodedCommit<P>>,
    signatures: Vec<CommitSignature>,
}

trait SyncCodec<P> {
    type Error;

    fn encode_commit(&mut self, commit: SignedCommitRef<'_, P>)
        -> Result<Vec<u8>, Self::Error>;
    fn encode_fragment(&mut self, fragment: FragmentRef<'_, P>)
        -> Result<Vec<u8>, Self::Error>;

    fn decode_commit(&mut self, bytes: &[u8])
        -> Result<DecodedLooseCommit<P>, Self::Error>;
    fn decode_fragment(&mut self, bytes: &[u8])
        -> Result<DecodedFragment<P>, Self::Error>;

    fn canonical_payload(&mut self, payload: &P)
        -> Result<Vec<u8>, Self::Error>;
}
```

The codec can compress metadata as well as operations. It must reconstruct the
exact commit contents, including author and parent order, not just an equivalent
DAG. Stockfoil defines commit hashing; the codec doesn't get to replace it.
`canonical_payload` supplies the bytes used for each payload hash, regardless of
how those bytes were compressed in a fragment. These are hashing inputs, not
mandatory per-commit records in the wire format. `decode_fragment` expands the
joint representation back into logical commits for verification. The codec is
trusted application code; incoming blobs aren't trusted.

The blob includes its signature or proof set. These signatures remain outside
individual commit hashes: bundling a proof with a commit doesn't change the
commit's identity. "Detached" means outside the hashed commit contents, not a
separate sync channel.

A fragment codec must reconstruct everything needed for verification from that
blob. Compression dictionaries, author tables, and metadata can't depend on a
neighbouring fragment. Operations can still refer to older CRDT objects; checking
their application semantics is separate from verifying the fragment's hash graph
and author coverage.

`EncodedBatch` contains metadata paired with these self-contained blobs. It
exposes `summary()` and lookup by item reference, with no sideband signature list.
Blob hashes and sizes cover the actual bytes, including proofs. Keep the batch
or persist its bytes before advertising it. Alternative encodings are fine as
long as each is independently verifiable.

Start with unbounded memory and whole-buffer codecs. Fixed-memory verification
and streaming come later; this first API doesn't promise bounded resource use.
Keep encoding, verification, and ingestion separate so that change is possible.

## Receiving pieces

```rust
fn verify_item<P, C: SyncCodec<P>>(
    item: &EncodedItem,
    codec: &mut C,
) -> Result<VerifiedItem<P>, SyncError<C::Error>>;

impl<P> CommitGraph<P> {
    fn ingest_verified(&mut self, item: VerifiedItem<P>) -> IngestReport;
}
```

`EncodedItem` pairs loose-commit or fragment metadata with blob bytes.
`VerifiedItem` is opaque and constructible only by verification. Crucially,
`verify_item` takes no graph: the same fragment must verify in an empty process
as in a replica with the whole document. Given the format and any decryption
keys, neither the codec nor verifier may fetch another part of the tree.

Verification checks:

1. Blob size/hash and well-formed encoding.
2. Every canonical payload hash and commit hash, including boundary commits.
3. The advertised range, depth rules, and checkpoints against the decoded graph.
   Reject missing interior commits, unrelated extras, and cycles. Only the
   declared cut can have parent edges leading outside the fragment; a loose
   commit may likewise reference parents not present in its item.
4. The loose commit's direct author signature, or the fragment's whole proof set.
   Verify each signature over its named hash and check coverage of every enclosed
   commit by its own author, following only included ancestry. A proof target
   outside the item, or an uncovered commit, makes the item incomplete. Also
   check that the retained proofs suffice for each canonical subfragment wholly
   contained in this range, using only that subfragment's ancestry. Otherwise
   compaction could produce a blob that verifies now but can't be saved back
   into its smaller fragments.

External parent hashes are authenticated references, not assertions that their
contents have been checked. Independence means **internal consistency and full
author coverage**, not complete ancestry, causal delivery, or write permission.
A valid fragment can therefore await external parents during ingestion.

Missing required signatures are an item verification error, even if the local
graph happens to have a later signature which would cover the data. Local
signing responses still use `ingest_signature`; that can seal local candidates,
but incoming sedimentree items must carry their own proofs.

Only after verification does ingestion update the boundary/range indexes, retain
the item's proof set, and make causally ready commits available. An invalid item
must leave no partial graph or index changes. `IngestReport` distinguishes new
delivery from items waiting for external parents. Overlaps are idempotent and
never confer local unsigned-edit trust. The report isn't a document patch; the
CRDT still compares viewpoints and materializes in its own order.

## Wants, needs, and fixups

```rust
enum ItemId {
    Commit(CommitHash),
    Fragment(FragmentRange),
}

struct ItemRef {
    id: ItemId,
    blob: BlobMeta,
}

struct SyncPlan {
    wants: Vec<ItemRef>, // They have this; we want it.
    needs: Vec<ItemRef>, // We have this; they need it.
    fixups: Vec<Fixup>,
}

struct Fixup {
    side: Side, // Local or remote inventory.
    redundant: ItemRef,
    covered_by: Vec<ItemRef>,
}

impl SedimentreeSummary {
    fn reconcile(&self, remote: &Self) -> Result<SyncPlan, SummaryError>;
}
```

All directions are relative to `self`. Reconciliation is pure: receiving a
summary doesn't insert commits, change permissions, or delete anything.
Compare coverage in the combined summaries, not just differences of blob hashes.
A peer with a larger fragment needn't receive every loose commit inside it.
With incomplete covering information, prefer redundant transfer over silently
assuming that history is present. Serve requests by lookup in the published
batch/inventory, never by unrestricted lookup into the working graph.

A fixup is conditional advice. If the only covering fragment is remote, first
fetch it, verify it, and make it durably available. Only then can local storage
drop a redundant representation. This removes redundant encoded bytes, not
required fragment proofs: preserve those in the replacement and in the boundary
indexes. Don't delete operation history or a blob referenced elsewhere. A remote
summary alone is never authority to delete local data. Plans are snapshots:
recheck before applying a fixup if the inventory has changed.

For example, if we have loose commits and the remote has a fragment covering
them, the plan can request that fragment and suggest dropping our loose blobs
*after* receiving it. It doesn't send those same loose commits back as needs.

## Signatures and Subduction

Use the same self-contained items with and without Subduction. There is no
separate signature-sync channel in this design. The earlier question about
"detached author evidence" meant how to transport the signatures already kept
outside Stockfoil's commit hashes. The answer is: inside each item's blob.

This changes the invariant behind reconciliation. A summary advertises a
complete, verifiable item, not merely knowledge of some commit hashes. Proof
arrival can make an unsealed candidate advertisable for the first time; it
mustn't be required to repair an item that was previously advertised as complete.
Thus causal-identity reconciliation needn't fetch newer proofs for the same
range just to authenticate that range. A peer holding only unverified metadata
or partial data must not count that item as available in its summary.

For Subduction, the adapter maps our metadata and blobs into its loose commits
and fragments. It supplies document identity, outer metadata signing, and any
transport/encryption setup. The blob hash covers the actual bytes handed to
Subduction, including enclosed author proofs. A Subduction signature on the
outer metadata authenticates the packager; it doesn't replace any of those
commit-author signatures. Opaque relays can store and forward complete blobs
without decoding them. That doesn't produce a `VerifiedItem`: the receiving
Stockfoil endpoint still verifies the inner contents after decryption.

Implement the covering and reconciliation algorithms in Stockfoil. We aren't
using `sedimentree_core` internally; its types and algorithms remain useful
references for checking the adapter.

## First slice and remaining questions

Start with boundary/range indexes and a whole-buffer MVR codec. Seal and verify
individual loose commits and fragments before adding reconciliation between two
in-memory replicas. No sockets, async runtime, or fixed-memory machinery yet.
Keep MVR tests against its document API and index/verification/reconciliation
tests against Stockfoil.

First tests:

* Verify a fragment with no graph or other blobs available, including multi-author
  histories and concurrent branches by the same author.
* Reject missing author coverage, wrong-author proofs, and proofs whose targets
  or required ancestry lie outside the fragment. Cached document-wide evidence
  must not change that result.
* Accept internal verification with external boundary parents absent, but delay
  causal delivery until they arrive. Check boundary payloads and signatures too.
* Advance the global signature frontier, compact and save a larger fragment,
  then load it in a fresh replica. Verify and re-export its canonical smaller
  fragments without a signer or the old blobs. Reject a larger fragment whose
  proof set would make that impossible.
* Keep direct signatures while commits remain loose. After publication, sync
  and persistence must not generate any new signing requests.
* Make a pending item advertisable only after sealing; never advertise an
  unsigned loose commit merely because a later global signature covers it.
* Check chains, merges, arrival-order independence, overlaps, alternate complete
  encodings, corrupt blobs/range claims, and conditional fixups.

Remaining details:

* Confirm inclusive boundary/root conventions against the adapter. They determine
  exactly which commits and proofs each fragment must contain.
* What is the local preparation/signing API? Stockfoil must identify all required
  signing targets from the boundary indexes, and the application supplies the
  signatures. Keep the signer and its IO outside Stockfoil.
* Try a genuinely batched MVR codec (shared tables or columns), not just a list
  of independently encoded commits. Round trips must preserve individual commit
  identities and all required fragment proofs.

## Upstream references

Checked against the linked upstream sources while writing this sketch; the
adapter should target a specific revision before implementation:

* [Sedimentree overview](https://github.com/inkandswitch/subduction/blob/main/design/sedimentree.md)
* [Depth metrics](https://github.com/inkandswitch/subduction/blob/main/sedimentree_core/src/depth.rs)
* [Fragment metadata and covering](https://github.com/inkandswitch/subduction/blob/main/sedimentree_core/src/fragment.rs)
* [Loose commit metadata](https://github.com/inkandswitch/subduction/blob/main/sedimentree_core/src/loose_commit.rs)
* [Batch sync and causal fingerprints](https://github.com/inkandswitch/subduction/blob/main/design/sync/batch.md)
