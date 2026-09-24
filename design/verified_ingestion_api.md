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
    fn verify_fragment(
        range: FragmentRange,
        signatures: Vec<SignatureEvidence>,
        commits: impl IntoIterator<Item = Result<CommitMeta, VerificationError>>,
    ) -> Result<VerifiedItem, VerificationError>;
}

impl VerifiedItem {
    fn id(&self) -> &ItemId;
    fn dependencies(&self) -> impl Iterator<Item = &CommitHash>;
    fn commits(&self) -> impl Iterator<Item = &CommitMeta>;
    fn signature_boundary(&self) -> &BTreeMap<Author, BTreeSet<CommitHash>>;
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

`ItemId` names a standalone commit (at any depth) or canonical fragment.
`MinimalCover` contains `loose_commits` and `fragments`, in deterministic order.
An `UnverifiedItem` carries metadata and signature evidence; payloads travel
separately. `VerifiedItem` has private fields and no unchecked constructor.

The owned item is an in-memory convenience, not a wire representation.
`verify_fragment` drives an application-supplied fallible decoder incrementally;
callers need not materialize a decoded `Vec<CommitMeta>` first. Compression can
omit commit hashes and encode parent references however the application chooses.
The decoder reconstructs each envelope with `CommitMeta::new` and yields it;
malformed compressed data can return `VerificationError::InvalidItem`.
Verification retains metadata for structure checks and admission, not payload
bytes. A decoder error aborts without producing a certificate.

## Stateless verification and causal admission

Verification checks only the item:

* Consume canonically constructed metadata; reject duplicates and cycles.
  `CommitMeta` has private fields and hashes its contents at construction, so
  duplicate identity needs no second metadata comparison or rehashing.
* Require exact canonical fragment membership and inclusive boundaries.
* Require standalone commits at any depth to have their own author signature.
* Check every signature, with targets and coverage paths confined to members.
  A signer authenticates only that signer's authored ancestors.
* Compute each author's causally latest **introduced** commits, keeping all
  concurrent tips. The author's signature boundary must dominate every one of
  these tips through supplied metadata. Signed targets may be later members
  authored by somebody else. Inclusive boundaries are authenticated assumptions,
  not repeated signature obligations.
* Order members lexicographically among parents-first choices by full hash.

The result is conditional: **if the declared dependencies are authenticated,
this item authenticates the history it introduces**. Dependencies are parent
hashes for a standalone commit and boundary hashes for a fragment. Verification has
no dependency provider and does not query graph state.

Admission checks those dependencies against authenticated, parent-closed history
at the destination. Bare local metadata does not suffice. Missing dependencies
return `IngestError::MissingDependency` before mutation or payload callbacks;
verified items can wait in an external buffer without being verified again.

Admission prepares all new payload values before mutation. Canonical hash identity
binds metadata under collision resistance. Duplicate receipts preserve existing
payloads, including local copies.
A payload callback panic leaves the graph unchanged. Neither metadata verification
nor ingestion checks opaque payload bytes against their declared hashes.

## Where certificates live

Every node stores signatures **over its own hash**, keyed by signer, without a
redundant target hash. A fragment stores a per-author signature boundary: references
to the signed member nodes whose signatures establish the required frontier
coverage. `SignatureEvidence` adds the explicit target only for transport.

A received fragment indexes its introduced members and attaches the verified
signature-boundary references to its canonical record. A standalone commit does
not index a fragment, even at nonzero depth: its own author signature authenticates
that commit, not a whole fragment's multi-author contents. It stays loose until
covered by a certified fragment. A later fragment can promote the same admitted
head or index a formerly bare checkpoint as an interior constituent.

The existing structural reduction determines the minimal cover. Contained and
auxiliary fragments can drop their signature-boundary references; topology remains
for construction and late forks. Pruning keeps the union of signatures referenced
by current cover fragments, loose nodes' own-author signatures, and pending local
signatures. Shared evidence is stored once on its target node, not copied onto
every fragment that references it.

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

A pending depth-zero commit waits for its own signature and authenticated parents.
Local nonzero-depth heads assemble a canonical fragment using signatures stored
on the actual member nodes. Signatures at the head replace older evidence from
the same author. To admit a high-depth local edit as a standalone announcement
instead, verify and ingest an explicit `UnverifiedItem::Commit`; automatic local
fragment assembly does not prematurely admit it before co-signatures can arrive.
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
index. `export()` reads current cover items, following stored signature-boundary
references and gathering ordered metadata through indexed fragment traversal.
It does not search for signature coverage or perform cryptographic verification.
Payloads are not cloned by metadata export.

A current cover fragment's head may also be exported as `ItemId::Commit(head)`:
a single envelope and its own-author signature. This announces the head and its
parent links, but the receiver must still admit its parents and must not mistake
the announcement for a fragment certificate. Receiving the complete fragment
later preserves already-admitted payloads.

Other non-current items return `ExportError::NotInMinimalCover`, even when still
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
