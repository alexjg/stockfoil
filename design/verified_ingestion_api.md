# Verified ingestion and minimal-cover synchronization

Peers exchange the structural minimal cover of the authenticated fragment index.
There is no separate cover index, candidate cover, or publication-repair pass.
Pending local edits can be visible in the working graph without entering the
fragment index.

## API

```rust
impl UnverifiedItem {
    fn id(&self) -> ItemId;
    fn dependencies(&self) -> impl Iterator<Item = CommitHash>;
    fn verify(self) -> Result<VerifiedItem, VerificationError>;
}

impl VerifiedItem {
    fn id(&self) -> &ItemId;
    fn dependencies(&self) -> impl Iterator<Item = &CommitHash>;
    fn commits(&self) -> impl Iterator<Item = &CommitMeta>;
}

impl<P: Clone> CommitGraph<P> {
    fn minimal_cover(&self) -> MinimalCover;
    fn export(&self, id: &ItemId) -> Result<UnverifiedItem, ExportError>;
    fn has_admitted_commit(&self, hash: &CommitHash) -> bool;
    fn ingest(
        &mut self,
        item: VerifiedItem,
        payload: impl FnMut(&CommitMeta) -> P,
    ) -> Result<(), IngestError>;

    fn ingest_local_commit(&mut self, meta: CommitMeta, payload: P);
    fn ingest_signature(
        &mut self, author: Author, commit: CommitHash, signature: Signature,
    ) -> Result<(), SignatureError>;
}
```

`ItemId` names a loose commit or canonical fragment. `MinimalCover` contains
`loose_commits` and `fragments`, in deterministic order. An `UnverifiedItem` is
one of those items with its metadata and signature evidence; payloads travel
separately. `VerifiedItem` has private fields and no unchecked constructor.

## Stateless verification and causal admission

Verification checks only the item:

* Reconstruct every metadata hash; reject duplicates and cycles.
* Require exact canonical fragment membership and inclusive boundaries.
* Require loose commit items to have depth zero and their own author signature.
* Check every signature, with targets and coverage paths confined to members.
  A signer authenticates only that signer's authored ancestors.
* Require coverage for every introduced member. Boundaries are assumptions,
  not repeated signature obligations.
* Order members lexicographically among parents-first choices by full hash.

The result is conditional: **if the declared dependencies are authenticated,
this item authenticates the history it introduces**. Dependencies are parent
hashes for a loose commit and boundary hashes for a fragment. Verification has
no dependency provider and does not query graph state.

Admission checks those dependencies against authenticated, parent-closed history
at the destination. Bare local metadata does not suffice. Missing dependencies
return `IngestError::MissingDependency` before mutation or payload callbacks;
verified items can wait in an external buffer without being verified again.

Admission checks metadata conflicts and prepares all new payload values before
mutation. Duplicate receipts preserve existing payloads, including local copies.
A payload callback panic leaves the graph unchanged. Neither metadata verification
nor ingestion checks opaque payload bytes against their declared hashes.

## Where certificates live

A loose node stores its signature certificate. A canonical fragment stores its
signature certificate directly on the fragment record. Internal, structurally
contained fragments need not retain certificates.

A received item already has everything needed for admission: insert its new
members into the authenticated index and attach its certificate to its node or
fragment. The existing structural reduction determines the minimal cover.
Certificates for covered pieces can then be discarded; topology remains for
future construction and late forks. Shared evidence needed by two cover
fragments stays on both fragments.

Authentication is durable. Local viewpoint hash frontiers remain valid after
signature-byte pruning, but are not transport certificates.

## Pending local work

Local metadata is visible optimistically and may be causally queued. It is not
in the authenticated fragment index until it has a complete certificate and
admitted dependencies.

Separate signature ingestion is **only for known, unadmitted local commits**.
Unknown hashes, received history, and already-admitted local commits return
`SignatureError::NotPendingLocalCommit`. Signatures can arrive while known local
metadata is still waiting for its parents, but not before the metadata is known.

A pending loose commit waits for its own signature and authenticated parents.
A pending fragment head combines its signatures with certificates on its
constituents. Signatures at the head replace older evidence from the same author.
Once the item is complete, it is verified and admitted exactly like a received
item. All newly ready local items can reuse constituent evidence before covered
certificates are discarded. Incomplete local work never blocks unrelated items.

A later loose signature does not silently authenticate an unsigned batch. Either
sign the loose edits individually or complete a canonical fragment containing
them. A late mixed-author fork may require another author to co-sign its pending
head if older constituent certificates have already been discarded. Historical
commits cannot be re-signed through the separate-signature API.

## Reading and exchanging the cover

`minimal_cover()` uses structural reduction over the authenticated fragment
index. `export()` checks current cover membership, reads the item's stored
signatures, and gathers its metadata using the indexed fragment traversal. It
does not search for signature coverage or perform cryptographic verification.
Payloads are not cloned by metadata export.

Non-current items return `ExportError::NotInMinimalCover`, even when still
readable locally. An inventory snapshot does not pin old exports; refresh the
inventory after a stale request. There is no historical export fallback.

`merge` exchanges complete covers, verifies items statelessly, and admits them
in dependency order. Every published cover is closed and can be admitted into
an empty receiver. Dependencies may be introduced inside another cover fragment;
they do not require standalone exports. Permissions, pending local trust, and
unverified queues do not travel through merge.

See [Sedimentree summaries and reads](sedimentree_sync_api.md) for canonical ranges,
structural containment, and deterministic ordering. Wire encoding, network IO,
payload storage, and application permission policy remain separate concerns.
