# Application-defined payload codecs and encoded graph APIs

Status: implemented in Rust, Wasm/JavaScript, and both register examples. This document records the design and implementation sequence; [`design/codec-wire-v1.md`](design/codec-wire-v1.md) specifies interoperable bytes and acceptance rules. Current dependencies, resource defaults, and compression measurements are in the [implementation notes](design/codec-implementation.md). Public contracts live in [`src/codec.rs`](src/codec.rs), [`src/commit_graph.rs`](src/commit_graph.rs), and [`javascript/src/types.ts`](javascript/src/types.ts).

Implementation decisions: required application `format_id`, cooperative `DecodeLimits` on both decoders, inbound-only limits with explicit retry/reconfiguration, structured-cloned JS payloads, guarded synchronous callbacks, and encoded native merges with partial-error reports. `CommitGraph<C: PayloadCodec>` directly owns the codec, payloads, history, and queue. There is one graph implementation; structured insertion/export helpers are private.

## Decision and motivation

Stockfoil should own its wire envelope and the complete export/ingestion pipeline. Applications must still define **both individual commit payloads and compressed fragment payloads**.

Compression across commits is a central requirement, not an optional optimization. Requiring applications to supply opaque per-commit byte strings and then concatenating or generically compressing those strings would prevent important application-specific encodings, such as columns, dictionaries, and runs of operations.

The intended boundary is:

| Stockfoil owns | Application owns |
| --- | --- |
| Versioning and framing of the Stockfoil envelope | Canonical commit payload encoding |
| Compact graph-structure encoding | Compressed fragment payload encoding |
| Signature evidence and verification | Decoding those application payloads |
| Hash reconstruction and graph admission | CRDT materialization |
| Transport metadata and export cursors | Signing keys and write-permission policy |

There is one Stockfoil envelope, not a pluggable whole-envelope codec. An application supplies a required payload codec when constructing its graph. Synchronization and persistence call `export_changes` and `ingest` without implementing framing, signature handling, or payload-lookup callbacks.

This refines the codec proposal in [`SLIM_THE_API.md`](SLIM_THE_API.md): do not replace application-specific bundle compression with a default codec or make all graphs exclusively byte-oriented.

## Goals

- Preserve application-specific compression across an entire fragment.
- Make `CommitGraph::export_changes` return complete encoded items and their transport metadata.
- Make `CommitGraph::ingest` accept encoded bytes and coordinate decoding, authentication, and admission.
- Retain typed application payloads, including suitable immutable storage handles.
- Preserve existing commit identities for unchanged canonical payload bytes and metadata.
- Preserve certified-cover selection, out-of-order ingestion, drafts, revocation, and cursor semantics.
- Use the same Stockfoil envelope implementation in Rust and Wasm.
- Remove the example-owned Stockfoil wire protocols without hiding their responsibilities in another example helper.

Non-goals for this change:

- A Subduction dependency in Stockfoil core.
- Moving signing keys or permission decisions into the codec.
- Defining a single schema for every CRDT's operations.
- Replacing application-specific fragment compression with a loop over independent commit encodings.
- Settling whole-document persistence, encryption, or every future streaming API.

## 1. Two representations, one commit identity

Separate these concepts explicitly:

1. **Canonical commit payload bytes.** Together with the existing authored commit metadata, these determine the commit hash and signing bytes.
2. **Compressed fragment payload bytes.** These encode a batch of application payloads efficiently. Decoding must reconstruct payloads with exactly the same canonical commit encodings.

Fragment packing, compression settings, and envelope layout must not change commit identities. Keep the current hash construction in [`src/commit_meta.rs`](src/commit_meta.rs): ordered parents, author, and the hash of canonical payload bytes. Signatures remain detached evidence over commit hashes.

Two semantically equivalent application values are not necessarily interchangeable if they produce different canonical bytes. The contract is byte preservation, not merely preservation of the displayed document state.

Likewise, a standard envelope does not require every replica to emit an identical blob for the same `ItemId`. Replicas can retain different sufficient signature evidence, and a payload codec can support more than one valid fragment packing. Item identity and physical blob identity remain distinct.

## 2. Application codec

Start with an in-memory interface matching the current graph's use case:

```rust
pub trait PayloadCodec {
    type Payload: Clone;
    type Error: std::error::Error + 'static;

    /// Trusted application format/version; incoming IDs never select code.
    fn format_id(&self) -> &str;

    /// Canonical bytes used to compute a commit's payload hash.
    /// Also the payload body of a standalone commit.
    fn encode_commit(
        &self,
        payload: &Self::Payload,
    ) -> Result<Vec<u8>, Self::Error>;

    fn decode_commit(
        &self,
        bytes: &[u8],
        limits: &DecodeLimits,
    ) -> Result<Self::Payload, Self::Error>;

    /// Encode the whole batch together, exploiting application structure.
    fn encode_fragment(
        &self,
        shape: &FragmentShape,
        payloads: &[&Self::Payload],
    ) -> Result<Vec<u8>, Self::Error>;

    /// Return exactly one payload per member, in the supplied order.
    fn decode_fragment(
        &self,
        shape: &FragmentShape,
        bytes: &[u8],
        limits: &DecodeLimits,
    ) -> Result<Vec<Self::Payload>, Self::Error>;
}
```

There is no default fragment encoder that simply calls `encode_commit` repeatedly. Implementing batch encoding is an explicit responsibility of an application codec.

The graph owns the codec. Keeping it separate from the payload type permits application configuration or access to immutable payload storage without requiring every payload value to contain that machinery.

These methods are synchronous, like the current graph API. Asynchronous disk access or remote encoding would need a separate preparation/continuation design; do not introduce hidden blocking I/O or retain graph borrows across asynchronous work.

### Structural context

Supply the same logical shape to the fragment encoder and decoder:

```rust
pub struct FragmentShape {
    pub members: Vec<MemberShape>,
}

pub struct MemberShape {
    pub author: Author,
    pub parents: Vec<ParentRef>,
}

pub enum ParentRef {
    /// Index of an earlier member in this fragment.
    Member(usize),

    /// Parent outside this fragment's membership.
    External(CommitHash),
}
```

Contract:

- Members are ordered ancestors before descendants.
- Payloads and members correspond positionally, with identical counts.
- An internal parent reference must refer to an earlier member.
- Parent order and multiplicity are preserved: they are part of the signed hash.
- An external parent is not necessarily a fragment boundary. Keep the existing distinction between the declared boundary and all external-parent admission dependencies.
- The decoder can inspect authors and topology without needing full member hashes that have not yet been reconstructed.
- Received shape data is untrusted until verification completes.

These are logical API structures, not the physical wire layout. In particular, `MemberShape` does not mean transmitting a full author key and full parent hashes for every commit.

Before fixing the interface, exercise it with a genuinely compressed codec. If the application needs more context, add that deliberately rather than making it decode the Stockfoil envelope itself.

### Codec laws

Implementations must satisfy:

1. `encode_commit` is deterministic for a fixed payload and agreed codec configuration.
2. Decoding an emitted standalone payload and re-encoding it reproduces the original canonical bytes.
3. Decoding an emitted fragment yields one payload per member, in the same order, whose individual canonical encodings match those supplied to the fragment encoder.
4. Fragment decoding does not require unavailable boundary-history payloads. A received fragment must be decodable and statelessly verifiable before its dependencies are admitted.
5. Retained payloads and their canonical representations remain immutable. A storage handle must keep referring to the same authenticated contents.

The graph need not require equality on application payload values; the relevant equality is of canonical encodings. Codec laws need property and interoperability tests. The library cannot prove an arbitrary application encoder is lossless merely from its Rust trait implementation.

## 3. Graph API

The primary graph becomes generic over the codec rather than only the stored payload type:

```rust
pub struct CommitGraph<C: PayloadCodec> {
    // codec, graph metadata, C::Payload values, pending items, ...
}
```

Selected public signatures (see `src/commit_graph.rs` and its private codec module for the complete implementation):

```rust
impl<C: PayloadCodec> CommitGraph<C> {
    pub fn new(codec: C) -> Self;
    pub fn with_limits(codec: C, limits: DecodeLimits) -> Self;
    pub fn set_decode_limits(&mut self, limits: DecodeLimits);
    pub fn decode_limits(&self) -> &DecodeLimits;

    /// Encode, hash, and insert an unsigned local draft.
    pub fn commit(
        &mut self,
        author: Author,
        payload: C::Payload,
    ) -> Result<Draft, Error<C::Error>>;

    pub fn ingest_signature(
        &mut self,
        author: Author,
        commit: CommitHash,
        signature: Signature,
    ) -> Result<IngestReport, SignatureError>;

    pub fn export_changes(
        &self,
        since: &ExportCursor,
    ) -> Result<(ExportChanges, ExportCursor), Error<C::Error>>;

    /// Decode, authenticate, then admit or queue.
    pub fn ingest(
        &mut self,
        bytes: &[u8],
    ) -> Result<IngestReport, Error<C::Error>>;

    /// Earlier successful admissions/rejections survive a later error.
    pub fn merge(
        &mut self,
        other: &Self,
    ) -> Result<IngestReport, MergeError<C::Error>>;
}

pub struct Draft {
    pub hash: CommitHash,
    pub signing_bytes: [u8; 32],
    pub report: IngestReport,
}
```

`Error<C::Error>` distinguishes codec, framing, canonical-encoding, verification, resource-limit, and admission failures. `MergeError<E>` carries both the accumulated `report: IngestReport` and underlying `error: Error<E>`; inspect reports on failure as well as success.

`commit` combines canonical encoding, metadata construction, and local draft insertion. The caller still signs externally, possibly asynchronously. Unsigned local work retains its existing materialization and publication semantics.

`export_changes` becomes fallible because application encoding can fail. An error must not advance publication state. The returned cursor describes the captured exported inventory, not whatever edits happen later while an application awaits storage.

Materialization remains based on `visit_changes`, with a borrowed `C::Payload` in each event. Permission changes and historical reads do not require a new materialization model.

Codec orchestration and graph algorithms operate on the same `CommitGraph<C>` fields. Structural summaries, cover selection, individual-item selection, and structured verification/admission are internal implementation details, not alternate graph APIs. Applications cannot bypass canonical encoding or authentication through structured insertion.

The public transfer flow is `export_changes(cursor)` → store/send each `EncodedItem.bytes` → `ingest(bytes)`. There is no public `export(id)`, `as_sedimentree`, or `minimal_cover`. The `Fragment`, `LooseCommit`, summary/cover, `UnverifiedItem`, and `VerifiedItem` types are internal. `ItemId` identifies admitted/rejected/retired inventory entries; `TransportMetadata` additionally supplies parents and checkpoint hints needed by transport adapters.

`FragmentShape`, `MemberShape`, and `ParentRef` remain public codec context for topology- and author-aware application compression. They do not expose a second transport representation. `CommitMeta` remains available to materializers.

Native cloning clones the codec, payloads, drafts, queue, and permissions; shared handles still obey immutability laws. Native `merge` checks format IDs and transfers encoded exports through the receiver's codec, rather than trusting Rust type equality. It transfers neither sender permissions nor unsigned/queued history.

## 4. Encoded exports and transport metadata

```rust
pub struct EncodedItem {
    pub metadata: TransportMetadata,
    pub bytes: Vec<u8>,
}

pub enum TransportMetadata {
    Commit {
        hash: CommitHash,
        parents: BTreeSet<CommitHash>,
    },
    Fragment {
        range: FragmentRange,
        checkpoints: BTreeSet<Checkpoint>,
    },
}

pub struct ExportChanges {
    pub added: Vec<EncodedItem>,
    pub retired: Vec<ItemId>,
}
```

Exported metadata uses Stockfoil types and is derived locally from the certified item. A Subduction adapter can construct its records without decoding the outgoing bytes or scanning fragment members to recover checkpoints.

The transport parent set is deliberately separate from the ordered, possibly repeated parents used in the authenticated representation.

Preserve existing export behavior:

- Select the certified cover, not every structurally discoverable fragment.
- Do not export unsigned drafts or queued received history.
- Derive checkpoint hints locally; hints are not authentication evidence.
- Keep exported bytes and metadata valid after later graph edits or certificate pruning.
- Advance a destination's export cursor only after its whole batch is stored successfully.
- Permit retry after partial storage; retirement is not an unconditional request to delete transport history.

## 5. Stockfoil envelope and compression

Specify one versioned outer format for standalone commits and fragments. It carries the information needed for:

- Identifying the format version and item kind.
- Checking the configured application payload format/version.
- Representing commit structure or fragment range and member shape.
- Delimiting the application-encoded payload body.
- Retaining sufficient original-author signature evidence.

Do not simply standardize the current example BCS envelope without examining its metadata overhead. The structural encoding must exploit internal parent references and repeated authors, avoid unnecessary member-hash duplication, and preserve the existing signature elision/coverage rules.

### Use Hexane for fragment metadata

The implementation uses pinned [`hexane =1.0.0-alpha.5`](https://docs.rs/hexane/1.0.0-alpha.5/hexane/) for Stockfoil's columnar fragment-metadata encoding, rather than building new RLE and delta codecs. Hexane is maintained as part of Automerge by the Stockfoil maintainer, so it is an appropriate core dependency. Its RLE, delta, boolean, and raw-byte facilities fit this metadata well.

Keep Hexane behind Stockfoil's wire module: `PayloadCodec`, `FragmentShape`, and `EncodedItem` should not expose Hexane types. The same Rust implementation will compile into Wasm; JavaScript applications should not need a separate metadata codec.

The implemented v1 column layout (all columns explicitly use LEB128):

| Data | Representation |
| --- | --- |
| Author keys | A dictionary of unique full keys, written once per fragment |
| Author for each member | RLE-encoded author-dictionary indices |
| Parent count for each member | RLE-encoded counts; a linear chain becomes a run of ones |
| Parent-reference kind | Boolean column over flattened parent entries: internal member or external hash |
| Internal parent references | RLE-encoded backward distances from the current member |
| External parent hashes | A dictionary of full hashes, with an RLE-encoded index column for external parent entries |
| Signature signers and targets | RLE-encoded author-dictionary indices and member indices |
| Actual signatures | Raw fixed-size bytes for the retained evidence, not a signature per member |

Flatten parents in member order and preserve each member's original parent order and multiplicity. Counts and kind tags reconstruct that sequence while consuming the internal/external reference columns. Dictionary deduplication must not deduplicate or sort the logical parent lists. The declared fragment boundary remains distinct from its external parents, even if their physical encoding shares a hash table.

Do not add a full member-hash or payload-hash column: reconstruct these from the shape and canonical application payload bytes. Full external hashes, author keys, and retained signatures are essentially incompressible; the gain comes from not repeating them and from compressing the integer structure around them.

For a long same-author chain, author indices, parent counts, reference kinds, and backward distances collapse into runs. Tests measure **complete** metadata, including framing and dictionaries: a 10,000-member single-author chain is 106 bytes without evidence/body, and a 1,000-member mixed-author branching case is 3,942 bytes. A complete signed 101-member test-codec envelope is 256 bytes versus 4,617 canonical payload bytes. Backward distances beat absolute-index deltas on chains, but not every branching shape; [Implementation notes](design/codec-implementation.md#compression-measurements) record that tradeoff and the regression ceilings.

Prefer Hexane's sequential encoders for one-shot exports; this does not require replacing Stockfoil's in-memory graph with mutable Hexane columns. On receipt, use its validating/fallible loading APIs. Stockfoil must still check exact column cardinalities, checked sums of parent counts, dictionary bounds, reference validity, and resource budgets. A tiny RLE stream can describe an enormous number of entries: do not blindly expand it into `FragmentShape` before enforcing limits. Fully finish validation before graph mutation.

Hexane is pinned to `=1.0.0-alpha.5`. Column values and framing lengths are constrained to u32 on native and Wasm; v1 explicitly selects LEB128, not optional Bijou64 or a dependency default. A checked preflight scan rejects oversized/overflowing runs and malformed integers before Hexane's fallible validating loader expands them. This also protects against unchecked arithmetic in the inspected prerelease loader. No delta columns are accepted in v1.

Stockfoil still defines the column schema, framing, and format version. Hexane supplies the column encoding primitives, not the Stockfoil protocol. Golden byte fixtures must keep dependency upgrades from accidentally changing that protocol. This change is to fragment transport encoding only, not the canonical commit-hash encoding.

References: [Hexane's versioned documentation](https://docs.rs/hexane/1.0.0-alpha.5/hexane/) and [upstream README](https://github.com/automerge/automerge/blob/main/rust/hexane/README.md).

### Application payload compression stays independent

Application fragment encoding remains free to use columns, dictionaries, runs, or other domain-specific transforms. Applications may also use Hexane, but are not required to use it. Stockfoil's use of Hexane for metadata does not replace application-specific payload compression or change the `PayloadCodec` contract. General-purpose compression may supplement that work but is not a replacement for it.

### Application format identity

The envelope starts with `STKF`, version 1 and item kind. The separately framed application format ID is 1–128 UTF-8 bytes and must exactly match trusted local configuration. It is not an additional commit-hash domain; existing identities remain unchanged.

The application configures the expected codec. An unauthenticated identifier in an incoming envelope must not select arbitrary code or silently change how the application interprets history. Reject unsupported or mismatched formats.

Changing the canonical commit payload encoding changes commit identities. Changing only fragment packing need not. A format upgrade requires deliberate codec configuration; the format ID does not cryptographically bind application semantics. Applications requiring that binding must include it in their canonical payload bytes.

## 6. Encoding, authentication, and mutation order

### Local commit

1. Encode the application payload canonically.
2. Compute its payload hash and the existing authored commit hash using current working heads.
3. Insert the payload as local working history only after encoding succeeds.
4. Return the hash, signing bytes, and operation report.
5. On a later signature, verify it and run the existing certification/admission machinery.

### Standalone export and ingestion

Export obtains canonical payload bytes from the codec and wraps them with structural metadata and retained evidence. If reconstructed bytes disagree with an existing stored payload hash, fail rather than silently changing the authenticated history.

On receipt, parse the envelope, decode the application payload, and require canonical re-encoding to equal the received payload body. Then reconstruct metadata, verify original-author evidence, and admit or queue.

Rejecting noncanonical standalone encodings is important: otherwise Stockfoil could admit a signed payload which cannot later survive fragment compression and reconstruction.

### Fragment export

1. Select a certified fragment and enumerate its exact members in topological order.
2. Construct its logical shape and aligned borrowed payload list.
3. Call the application's batch encoder.
4. Encode compact structural metadata, the compressed payload body, and retained signature evidence in the Stockfoil envelope.
5. Return complete bytes and derived transport metadata.

The encoder is responsible for its lossless-compression contract; mandatory decode-and-verify of every locally encoded fragment is not implied by this sketch.

### Fragment ingestion

1. Parse and validate envelope framing, counts, versions, and shape references.
2. Call the configured fragment decoder on the bounded payload body.
3. Require exactly one decoded payload per member.
4. Canonically encode each reconstructed payload and hash it.
5. Resolve internal parent references to reconstructed hashes and derive each full `CommitMeta` in order.
6. Check item identity, membership, signature coverage, and the existing stateless fragment invariants.
7. Perform graph-dependent admission checks, or queue the verified item and its owned payloads until dependencies arrive.

Application fragment decoding necessarily runs before author verification is complete. Codecs must treat both their shape and bytes as hostile input; receiving a signed envelope does not make parsing safe.

### Atomicity and reports

Framing, codec, canonicalization, and stateless verification errors must leave the graph and received queue unchanged for that input item. Ready items with invalid graph placement must also fail without admission.

Preserve existing deferred-rejection semantics: a valid dependency can be admitted while exposing an invalid queued fragment. That rejection is reported in `IngestReport`; valid earlier admissions are not rolled back. This is item-level processing, not a transactional batch-ingestion API.

Decode into temporary owned payloads before graph mutation. Do not claim to roll back a codec's external side effects, such as writes it independently makes to application storage.

## 7. Application usage

```rust
let mut graph = CommitGraph::new(RegisterCodec);

let draft = graph.commit(
    author,
    Assignment {
        key: "title".into(),
        value: "Hello".into(),
    },
)?;
handle_report(draft.report)?;

let signature = signer.sign(&draft.signing_bytes);
handle_report(graph.ingest_signature(author, draft.hash, signature)?)?;

// Materialize with visit_changes as before.

let (changes, next) = graph.export_changes(&published)?;
adapter.store(changes.added).await?;
published = next;

handle_report(graph.ingest(&blob)?)?;
// Materialize again, including participation changes.
```

The signer, adapter, and `handle_report` are application-level placeholders. In particular, reporting a deferred rejection must not suggest that all admissions were rolled back.

A register codec can keep its existing canonical assignment encoding initially, preserving commit hashes. Its fragment encoding should demonstrate actual batch compression—for example, shared key dictionaries and runs of repeated keys—rather than wrapping a list of independent assignment byte strings.

## 8. Rust/Wasm/JavaScript boundary

The Wasm build must use the same Stockfoil envelope parser, encoder, hash reconstruction, and verifier as Rust. Do not replace the JS example's handwritten envelope with a second handwritten implementation in the JS package.

Expose the same conceptual flow in JS:

```javascript
const graph = new CommitGraph(registerCodec);
const draft = graph.commit(author, assignment);
// Sign draft.signingBytes externally, then add the signature.

const batch = graph.exportChanges(published);
await adapter.store(batch.added);
published = batch.cursor;

const report = graph.ingest(blob);
```

The application supplies `formatId`, `encodeCommit`, `decodeCommit`, `encodeFragment`, and `decodeFragment`; both decoders receive cooperative limits. The wrapper maps these into native orchestration:

- Callbacks are synchronous; Promise/thenable results are rejected. A JS busy guard and native nonblocking lock reject re-entry into the same graph, including reads and disposal.
- Payloads are structured-cloned at ownership boundaries, including encoder inputs and materialization output. Shared memory and non-cloneable values are rejected; custom prototypes are not preserved.
- Callback exceptions remain the `cause` of `StockfoilError("CodecError")`. Native validation failures retain distinct codes.
- Byte arrays, shapes, and limits are owned copies, not invalidatable Wasm-memory views; fragment shape/batches cross as complete values.

See the [JavaScript package guide](javascript/README.md) for exact declarations, ownership rules, and `setDecodeLimits` replacement semantics.

Rust and JS implementations of an application codec must agree on canonical commit bytes and decode each other's fragment bodies. They need not always choose identical fragment packing.

## 9. Streaming and resource budgets

The `Vec` signatures are an initial in-memory sketch, not a promise of fixed-memory decoding. The current graph already retains application payloads, but a codec must not introduce unbounded intermediate allocations unnecessarily.

`DecodeLimits` defaults to 8 MiB envelope bytes, 65,536 members, 1,000,000 parent references, 65,536 signatures, and 8 MiB total reconstructed canonical payload bytes. Core bounds framing/columns before expansion; application decoders receive the limits and must cooperate before allocating. Core checks canonical totals afterward, but cannot sandbox codec CPU or allocations.

These limits govern **inbound ingestion only**, including native merge. They do not constrain trusted local commits or exports: an automatically growing certified fragment can exceed a receiver's budget after older certificates have been pruned. Blocking export at that point would strand retained history. A receiver may explicitly increase limits and retry the same bytes; no authenticated chunking or smaller fallback certificate is promised. Wire field widths and export-time canonical hash checks still apply.

Possible evolutions include:

- Encoding into a writer or hash sink instead of constructing canonical byte vectors solely to hash them.
- Yielding decoded payloads incrementally into a temporary verified-item builder.
- Retaining immutable payload-storage handles rather than large decoded objects.

These should preserve the codec laws and verification-before-admission contract. A streaming callback that mutates the live graph before final verification is not an acceptable shortcut.

The received pending-item queue's resource policy remains a separate concern; adding envelope limits does not solve an unbounded queue.

## 10. Implementation sequence

The following phases are implemented; the list records the original work breakdown. Native/Wasm golden fixtures, browser loaders, codec failure/budget cases, and real Rust/JS WebSocket synchronization are covered by tests.

### Phase 1: settle the contracts

- Specify canonical commit encoding, fragment round-trip laws, ordering, and immutability requirements.
- Exercise `FragmentShape` with a compressed application codec.
- Decide application format identification/versioning and initial decoder-budget handling.
- Record how cloning, direct `merge`, and codec configuration interact. A direct merge must not import payloads under an incompatible codec configuration merely because their Rust types match.

### Phase 2: implement the envelope and codec orchestration

- Add the codec and shape types and one Stockfoil wire module.
- Add a pinned Hexane dependency for metadata columns, verifying native and Wasm builds.
- Specify column framing, dictionaries, integer types/encoding, and defaults; implement the Hexane-backed structural encoding with fixed byte-level test vectors.
- Compare backward-distance RLE and absolute-index delta encoding on chain and branching fixtures before fixing the parent-reference representation.
- Validate column lengths, count sums, table indices, and resource limits before expanding untrusted metadata.
- Reconstruct metadata and route decoded items through the existing verification/admission engine.
- Keep low-level structured admission/export helpers private, preserving graph verification.

### Phase 3: expose the graph API

- Introduce codec ownership and storage of `C::Payload`.
- Add the combined local `commit` operation.
- Change export results to encoded items plus transport metadata.
- Make incremental export fallible while retaining cursor behavior.
- Add byte-oriented `ingest` with unchanged admission/report semantics.
- Preserve typed materialization and existing graph queries.

The codec and limits live directly on the existing graph alongside nodes, payloads, pending items, and history. Codec methods may be organized in a private module, but there is no inner graph or forwarding layer.

### Phase 4: migrate the Rust example

- Implement an assignment codec with a real compressed fragment representation.
- Remove example-owned Stockfoil envelopes, signature serialization, metadata reconstruction, and temporary payload maps.
- Feed encoded exports and their metadata into the existing Subduction batch adapter.
- Leave permissions, signing, materialization, and transport lifecycle visibly separate.

### Phase 5: expose the API through Wasm and migrate JS

- Bridge application codec operations while retaining the shared native envelope implementation.
- Implement the same application payload format in the JS example.
- Remove JS Stockfoil-envelope reader/writer logic; keep only application-codec responsibilities.
- Update declarations and examples to use encoded exports and ingestion.

### Phase 6: validate and document

- Run the tests below and inspect actual encoded-size results.
- Update the API guides, package declarations, READMEs, and the earlier default-codec proposal.
- Keep structured mutation/export helpers crate-private and algorithm fixtures test-only.
- Treat the old example wire format as experimental; do not add a compatibility layer without a concrete need.

## 11. Test and acceptance criteria

### Codec and identity

- Standalone canonical round trips and rejection of noncanonical encodings.
- Fragment round trips preserving every member's canonical payload bytes.
- Payload hashing independent of fragment packing/compression choices.
- Parent order and duplicate parents remain hash-bound.
- Existing commit-hash vectors remain unchanged for unchanged canonical payloads.
- Encoder/decoder errors, wrong member counts, and invalid references leave the graph unchanged.
- Detect canonical-byte disagreement with stored metadata rather than exporting altered history.

### Authentication and admission

- Payload, author, parent, signature, and range tampering are rejected.
- Missing/wrong-author evidence cannot be replaced by transport signatures.
- Invalid placements, bypass edges, boundary ancestry, and out-of-order dependencies retain their current behavior.
- Deferred rejection reports preserve valid admissions and materialization semantics.
- Unsigned drafts and queued received history remain non-exportable.
- Revocation and historical views do not alter authenticated export eligibility.

### Compression

- Test a repetitive workload where the application fragment codec demonstrably saves space over independent commit payloads.
- Test Hexane-backed structural metadata on long same-author chains and on branching/mixed-author graphs, including complete column-framing and dictionary overhead.
- Round-trip ordered/repeated parents through flattened counts, kinds, and reference columns.
- Pin golden metadata bytes across native Rust, Wasm, and Hexane dependency upgrades.
- Reject malformed columns, inconsistent column counts, oversized runs, arithmetic overflow, out-of-domain deltas, and invalid dictionary/member references without graph mutation.
- Ensure full member hashes, author keys, or signatures are not accidentally repeated where the chosen format is intended to elide them.
- Set concrete size-regression expectations from the chosen format and fixtures; do not treat successful decoding alone as evidence that compression works.

### Export, transport, and interoperability

- Encoded export metadata agrees with reconstructed item contents, including checkpoint prefixes.
- Failed encoding/publication leaves the prior export cursor reusable.
- Retrying after partial transport storage does not lose history.
- Rust and JS exchange commits and compressed fragments in both directions.
- Fresh replicas catch up in reverse arrival order without external payload maps.
- Malformed/truncated envelopes, unsupported versions, and resource-limit violations fail predictably.

Success means an application implements **its payload codec and its materializer**, while Stockfoil owns the surrounding authenticated-history protocol. Shorter examples must be the result of that ownership change, not of giving up application-specific compression or removing validation.
