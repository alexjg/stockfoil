# Verifying Stockfoil with Verus

This first milestone connects parent-edge reachability, chain clocks, permission
visibility, and materialization events in a checked mathematical model. It also
verifies several executable kernels used directly by Stockfoil. **The full
production crate is not yet verified.** The distinction between shared code and
model contracts below is part of the verification claim.

The starting revision was `39c22a1147706313729db5dd38c20387bfa5e53d`.

## Run

Install [Verus 0.2026.11.02.8bf68ee](https://github.com/verus-lang/verus/releases/tag/release/0.2026.11.02.8bf68ee)
and its matching Rust toolchain. The Linux x86-64 release requires Rust `1.99.1`.
The archive includes Z3 and vstd.

```sh
rustup toolchain install 1.99.1 --profile minimal
export VERUS_BIN=/absolute/path/to/verus-x86-linux/verus
bash verification/verify.sh
python3 verification/check_mutations.py
```

`VERUS_BIN` defaults to `verus` on PATH. The runner checks the exact version,
verifies every module in `main.rs`, and enables `--no-cheating`. It does not
forward arguments that could skip verification. A successful run reports:

```text
verification results:: 43 verified, 0 errors
```

The count includes helpers and loop obligations; it does not measure production
coverage. Mutation checks first require a passing baseline, then verify temporary
copies with five deliberate errors: an expanding cap, an exclusive lookup bound,
a minimum in place of union's maximum, omitted new invisible commits, and an
incorrect permission specification accepting out-of-history frontier heads.
Compiler or setup errors do not count as successful mutation rejection.

[CI](../.github/workflows/verus.yml) pins the Verus archive and SHA-256, runs these
checks, and tests the production crate with and without `std`, including an
embedded `thumbv7em-none-eabi` compile check.

## Shared production code

[`src/verified/clock.rs`](../src/verified/clock.rs) and
[`src/verified/events.rs`](../src/verified/events.rs) are compiled by ordinary
Cargo and included directly by the verification harness. There is no copied or
generated implementation for the following kernels:

| Kernel | Proved contract | Production caller |
| --- | --- | --- |
| `max_position` | Exact maximum; its prefix is the union of both inputs' prefixes. | `ClockBuilder::include` |
| `position`, `contains` | Sound and complete binary search on strictly sorted chain IDs; termination, bounds and arithmetic safety. | `AncestorClock` lookup/membership |
| `cap` | Exact intersection of the history prefix and an optional bound; absence is distinct from zero. | `AncestorClock::mask` |
| `classify` | Both visibility bits are represented exactly by `Vis`. | `ViewPointDiff::visibility_at` |
| `should_emit` | Emit iff newly delivered or visibility changed. | `CommitGraph::visit_changes` |

Ordinary builds use the pinned `verus_builtin_macros` procedural macro to erase
specifications and proofs. They require neither the Verus executable nor vstd as
a Cargo dependency. `alloc` and the existing `no_std` API remain supported.

The clock file also retains verification-only executable models of coordinate
union and whole-vector masking, guarded by `verus_keep_ghost`. Production masking
uses the shared `cap` inside its existing callback/iterator implementation, so it
does not allocate an additional vector of bounds. The whole-vector masking proof
does **not** by itself verify that iterator or callback.

## Connected model theorems

The model uses topological integer node IDs, parent sets, and integer author IDs.
Repeated parent edges have set semantics for reachability. It does not model the
hash map, metadata serialization, or cryptographic identity mapping.

| File | Results | Boundary still to discharge in production |
| --- | --- | --- |
| [`graph.rs`](graph.rs) | Reachability is transitive and ordered; append preserves old reachability; reusing a same-author maximal tip in parent history preserves chain invariants; exact chain summaries imply exact membership, composed with the shared executable lookup. | Insertion establishes the model mapping and invariants; greedy selection satisfies reuse premises; builders produce exact summaries. |
| [`walk.rs`](walk.rs) | Initial state, parent expansion, revisits, and full-cache substitution preserve a walk invariant; an empty pending set gives exactly the requested history. | Concrete stack/visited bookkeeping, termination, cache initialization, and mapping accumulated coordinates to the covered set. |
| [`visibility.rs`](visibility.rs) | The mask model yields exactly the independent reachability-based permission policy; unavailable frontier heads grant nothing. | `participation` computes the stipulated bound clocks and lookup results, validates delivery heads, and associates each chain with its actual author. |
| [`materialization.rs`](materialization.rs) | A terminating scan model emits exactly the required events, without duplicates, in topological order; classifications are correct; an unchanged fully processed cursor emits nothing. | Production graph scan, callbacks, payload borrows, cursor construction, error ordering, and panic behavior. |
| [`examples.rs`](examples.rs) | Nonempty witnesses instantiate chain and visibility contracts. An out-of-history bound cannot authorize a delivered ancestor; visibility may skip a denied intermediate author. | Specification checks, not proofs of the wrappers. |

`graph::reaches` is defined independently of clocks by following earlier parent
IDs. Its recursive specification enumerates candidate parent IDs to make the
induction explicit. `unfold_reaches` proves the usual rule: a node reaches itself
or an ancestor of one of its parents. The `dag` condition ensures every parent
is a valid earlier node, so this enumeration omits no edge.

The summary contract is deliberately weaker than exact lookup: every stored
coordinate is a real history member, and every history member is bounded by its
chain's coordinate. The representation theorem derives exact membership from
that contract and the ancestry-order invariant. The bound-compilation contract
spells out the three permission branches; it is not a proof that the production
maps and iterators satisfy them.

## Intended public properties and remaining work

1. **Exact ancestry:** `is_ancestor(a, b)` iff a nonempty parent path connects
   `b` to `a`. Next, verify builders and insertion/caching against the model
   contracts, then the wrapper's strictness and unknown-hash handling.
2. **Exact visibility:** delivered membership plus the author's permission.
   Bounded permissions use only frontier heads in the captured history. Finish
   concrete bound compilation and query-wrapper proofs.
3. **Authentication coverage:** every introduced commit is covered by its
   author's valid signature on an appropriate descendant within supplied
   metadata. Prove maximal-author-frontier coverage and connect it to signature
   checks. This has not been proved in this milestone.
4. **Exact fragments:** members equal inclusive head history minus the union of
   boundary histories; boundaries match depth-stopped discovery. Prove
   export/verify/admit compatibility. This remains unproved.
5. **Admission safety and progress:** admitted history is authenticated and
   parent-closed; queued items are isolated; invalid items cannot partially
   admit or suppress valid candidates; draining a finite queue reaches a fixed
   point. Concrete transitions and termination remain unproved.
6. **Exact materialization:** finish the wrapper and cursor proofs, including
   validation before callbacks. Application-specific CRDT state and rollback of
   callback side effects are outside the event-stream contract.
7. **Representation independence:** equivalent hash-identified histories and
   viewpoints give equivalent ancestry, visibility, and cursor meaning despite
   different chain assignments. The model policy is independent of chains, but
   a cross-replica theorem still needs a graph isomorphism and concrete
   refinement. Admission convergence additionally requires matching evidence
   and dependencies; payload equality is caller-supplied.

## Trust and limitations

There are no project `assume`, `admit`, `external_body`, `assume_specification`, or
custom axioms. The runner enforces Verus's `--no-cheating` mode. Proofs still rely
on Verus, its erasure and Rust integration, vstd's trusted specifications, Z3,
and the Rust toolchain. Model preconditions are explicit obligations, not yet
verified facts about every production call.

Memory exhaustion, unbounded-input resource limits, concurrent cache behavior,
cryptographic security, serialization, and application payload correctness are
not established here. Future authentication proofs should assume appropriate
BLAKE3 and Ed25519 security properties explicitly. Caching correctness must
include the shared-prefix/divergent-clone boundary: graph-local indices and
clocks cannot be freely transferred between replicas.

The parent-walk oracle in [`tests/ancestor_vectors.rs`](../tests/ancestor_vectors.rs)
remains an independent regression check for production behavior. It complements,
but cannot close, the remaining proof boundaries.
