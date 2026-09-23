# Verified sedimentree ingestion: API scaffold

Received history has one admission boundary:

```text
UnverifiedItem -> verify(explicit verified dependencies) -> VerifiedItem -> ingest
```

Dependency discovery, verification, admission, graph dependency resolution, and
peer merging currently use `todo!()`. The APIs and their callers compile; these
paths are not yet runnable. This does not specify a serialization format.

## Untrusted items

`UnverifiedItem` represents decoded metadata, not payload bytes:

* `Commit { meta, signatures }` represents an individual commit.
* `Fragment { range, commits, signatures }` represents a canonical fragment.

`SignatureEvidence { author, commit, signature }` is untrusted evidence, not a
verification witness. Constructing an item or evidence establishes no trust.
A future decoder may reconstruct hashes omitted on the wire; the current
in-memory API uses complete commit envelopes.

`dependencies()` returns external `DependencyId`s (commit hashes or fragment
ranges). The caller can fetch or queue missing prerequisites before verification;
raw received items must not enter the graph's verified history while waiting.

## Verification without the whole graph

```rust
trait VerifiedDependencies {
    fn get(&self, id: &DependencyId) -> Option<VerifiedDependency<'_>>;
}

impl UnverifiedItem {
    fn dependencies(&self) -> std::vec::IntoIter<DependencyId>;
    fn verify(
        self,
        dependencies: &(impl VerifiedDependencies + ?Sized),
    ) -> Result<VerifiedItem, VerificationError>;
}
```

A provider can be a graph, storage, or a small collection of already verified
items. `VerifiedItem::dependency` supplies borrowed witnesses for dependencies
established by that item. A witness exposes the dependency identity and its
verified commit metadata, not arbitrary unverified envelopes or a trust boolean.

`VerifiedItem` and `VerifiedDependency` have private representations and no
unchecked public constructors. Only successful verification can establish their
evidence. Neither unsigned local metadata nor publication eligibility alone is
a verification witness.

Verification establishes identity, structure, and sufficient author-signature
coverage relative to the declared verified prerequisites. It authenticates the
payload hashes, not the actual payload bytes. Byte integrity remains a separate,
potentially streaming check. The accepted item must retain evidence sufficient
for later export, not just record that verification once succeeded.

`VerificationError` distinguishes missing dependencies, invalid item
identity/structure, invalid signatures, and missing author-signature coverage.

## Admission

```rust
impl<P: Clone> CommitGraph<P> {
    fn ingest(
        &mut self,
        item: VerifiedItem,
        payload: impl FnMut(&CommitMeta) -> P,
    ) -> Result<(), IngestError>;
}
```

The destination checks its own verified dependencies and conflicts before
mutation or callbacks. Verification against an external provider does not imply
that those dependencies exist in the destination graph. Admission may therefore
return `IngestError::MissingDependency` even for a successfully verified item.

The callback supplies data or a storage handle for each newly admitted commit.
Duplicates retain their original payloads; overlapping fragments must be safe to
admit repeatedly. A metadata-first application can use the declared payload hash
as its handle and obtain bytes later. The graph cannot verify an arbitrary `P`.

Public raw `ingest_commit` is removed. Its causal-delivery machinery is private
`insert_commit`, still used by local editing. `ingest_local_commit` and
`ingest_signature` remain separate local operations: unsigned local work is
withheld and cannot satisfy a verified dependency merely by being present.

`merge` retains its signature as a convenience API placeholder, but its former
raw forwarding implementation is removed. It must eventually exchange and verify
sedimentree items, then call `ingest`, rather than bypass verification.

## Tests and migration status

`tests/ingest.rs` describes the new contracts using real signature evidence:
metadata-only admission, isolated dependency providers, destination dependency
checks before callbacks, and atomic fragment admission. Compile-fail doctests
check that unverified items cannot be passed to `ingest` and that witnesses cannot
be fabricated through public struct literals.

Legacy tests and examples have been migrated off public raw insertion so all
callers compile. Their signature-evidence fixtures are explicitly stubbed where
they previously assumed unsigned receipt. Tests involving remote unsigned data,
forwarding detached commits, or in-graph received queues need behavioral updates
when the new transport implementation is written. They are not silently routed
through local trust, given fake verified tokens, ignored, or claimed to pass.

For this scaffold use:

```sh
cargo check --all-targets
cargo test --no-run
cargo test --doc
```

Running the full behavior suite will encounter intentional `todo!()` panics.
