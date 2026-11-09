# Explicit standard-library assumptions

Standard-library assumptions are an accepted part of this verification effort.
Stockfoil's application logic must still be verified. This document enumerates
**38 trusted contracts/declarations, not 38 proved functions**. None assumes ancestry,
permission correctness, candidate completeness, or any public API guarantee.

## Enabled contracts

The adapters, key-model declarations and generic write-once cell live in
[`stdlib.rs`](../stockfoil/src/verified/stdlib.rs). Their complete source is
SHA-256 pinned in [`trusted_stdlib.json`](trusted_stdlib.json). Changing a body,
contract, or dependency import requires a visible manifest change and review.
The derived author and commit-hash representations are separately pinned in
`author_key.rs`, `commit_hash.rs` and `node_key.rs` (NodeIdx); checked index projection
and author comparison bodies remain
outside that trusted boundary.

| ID | Actual body | Assumed postcondition |
| --- | --- | --- |
| EARLIER-TYPE-1 | Private generic `Peekable<btree_map::Range<NodeIdx, V>>` wrapper | Opaque immutable iterator with a logical remaining sequence. |
| EARLIER-NEW-1 | `map.range(..limit).peekable()` and one peek | Exact map entries with keys below the exclusive numeric limit, in strictly increasing key order; boolean reports whether any exist. Values are generic; no pending-chain property is assumed. |
| EARLIER-NEXT-1 | `entries.next()` | None iff exhausted, otherwise borrowed key/value equal the logical head and the remaining sequence is exactly its tail. |
| READY-ORDER-1 | Rust-derived hash/index comparison and tuple ordering | The fixed `(CommitHash, NodeIdx)` key obeys vstd comparison laws, enabling BTreeSet insertion specifications. No queue invariant is assumed. |
| NODE-ORDER-1 | Rust-derived `Ord`/`Eq` on pinned `NodeIdx(u32)` | The ordinary vstd comparison laws, enabling BTreeMap insert/remove specifications. No frontier, selection or map invariant is assumed. |
| NODE-KEY-1 | Rust-derived `Hash`, `Eq` and `Clone` on pinned `NodeIdx(u32)` | The ordinary vstd hash-key laws, enabling waiter-map operations. No dependency or scheduling property is assumed. |
| READY-POP-1 | `BTreeSet<(CommitHash, NodeIdx)>::pop_first()` | None exactly when empty; otherwise returns and removes precisely the minimum pair under lexicographic hash bytes then numeric node index, preserving every other entry. |
| RANGE-SORT-1 | `ranges.sort_unstable_by_key(\|range\| range.start)` on `Range<usize>` | Nondecreasing starts, the same set of complete ranges, and unchanged length. No union, disjointness, selection, count or traversal guarantee is assumed. |
| HASH-NORMALIZE-1 | `hashes.sort_unstable(); hashes.dedup()` on pinned `CommitHash([u8; 32])` | Strict lexicographic byte order, no duplicates, exactly the same set, and no length increase. No graph or authorization fact. |
| ARC-BYTES-1 | `Vec<u8>` into `Arc<[u8]>` | Preserves the complete canonical byte sequence used by local change. |
| ARC-SLICE-1 | `Vec<CommitHash>::into()` to `Arc<[CommitHash]>` | Preserves the exact element sequence on normal return. |
| SORT-1 | `nodes.sort_unstable()` | Nondecreasing numeric node indices; output is a permutation of the input, preserving multiplicities and membership. |
| DEDUP-1 | `nodes.dedup()` | For nondecreasing input, output has strictly increasing numeric indices, preserves membership, and cannot grow. |
| AUTHOR-KEY-1 | Rust-derived `Hash`, `Eq`, and `Clone` on `Author([u8; 32])` | Satisfies vstd’s collection key model: deterministic hashing, equality agrees with logical identity, and cloning preserves the key. |
| COMMIT-KEY-1 | The same derives on `CommitHash([u8; 32])` | The same key laws. This makes no claim about cryptographic collision resistance. |
| COMMIT-ORDER-1 | Rust-derived `Eq`, `PartialOrd`, and `Ord` on the same pinned `CommitHash` wrapper | Satisfies vstd’s comparison laws, enabling its existing `BTreeSet` specifications. No head membership or graph property is assumed. |
| ONCE-TYPE-1 | Private `OnceValue<T>` containing `std::sync::OnceLock<T>` | Opaque cell with a fixed logical publication predicate. Its raw cell is private; no reset, mutable access or unchecked publication is exposed. |
| ONCE-NEW-1 | `OnceLock::new()` | An empty cell establishes the invariant for any supplied fixed predicate; construction cannot unwind. |
| ONCE-GET-1 | `cell.get()` | Every returned value satisfies that cell's predicate; `None` is always permitted by this conservative specification. |
| ONCE-INIT-1 | `cell.get_or_init(init)` | Given a callable initializer whose every normal return satisfies the predicate, the returned value satisfies it, including when another initializer won or the cell was already initialized. |
| ATOMIC-TYPE-1 | Private `RelaxedAtomic` containing `std::sync::atomic::AtomicU64` | Opaque cell identity governed by vstd's linear `PermissionU64`; no public raw cell or unchecked writes. |
| ATOMIC-NEW-1 | `AtomicU64::new(value)` | Returns the new cell and its unique initial permission at `value`. The ghost output slot is erased. |
| ATOMIC-LOAD-1 | `load(Ordering::Relaxed)` | Atomic, non-unwinding read with the matching permission. The result is deliberately unconstrained; no freshness, publication or ordering guarantee is used. |
| ATOMIC-CAS-1 | `compare_exchange(current, next, Relaxed, Relaxed)` | Success matches the prior cell value, returns it, and installs `next`. Failure preserves the cell; its reported observation is deliberately unconstrained. Cell identity is preserved. |
| STATIC-REF-1 | `&identity::ALLOCATOR` | Every call returns the same reference, named by `global_allocator_ref`. It asserts no allocator validity, uniqueness or Stockfoil correctness property. The actual static initializer is verified. |
| EXPECT-1 | `value.expect(message)` | On normal return, the input was `Some` and its value is returned. `None` can unwind/abort; this is not a panic-freedom claim. |
| VEC-EMPTY-1 | `Vec::new()` | Empty sequence; no allocation or unwinding. |
| VEC-RESERVE-1 | `Vec::try_reserve` | Preserves elements on success and failure; success supplies the requested spare capacity; cannot unwind with the standard global allocator. |
| VEC-ROW-RESERVE-1 | `try_reserve` on an in-bounds nested vector | Preserves all row sequences and outer capacity; success supplies spare capacity in the selected row; cannot unwind. |
| VEC-PUSH-1 | `Vec::push` with spare capacity | Appends exactly one value, consumes one spare slot, and cannot unwind: it neither allocates nor drops the element. |
| VEC-SET-1 | In-bounds replacement of `Copy` elements | Changes exactly that element and preserves capacity; `Copy` excludes a user destructor. Cannot unwind. |
| VEC-ROW-PUSH-1 | Reserved push into an in-bounds nested vector | Appends to the selected row, preserves other rows and outer capacity, and cannot unwind. |
| HASH-RESERVE-1 | `HashMap<CommitHash, NodeIdx>::try_reserve(1)` | Preserves the map; success provides a spare entry; fixed derived keys cannot panic during rehashing. |
| HASH-INSERT-1 | Reserved insertion of `CommitHash`/`NodeIdx` | Exact map insertion without unwinding; keys and values have no user callbacks or destructors. |
| AUTHOR-RESERVE-1 | `try_reserve` on an author map and its existing `Vec<u32>` row | Preserves key domain and all row sequences, including on failure; success provides map/row capacity. Cannot unwind. |
| AUTHOR-PUSH-1 | `entry(key).or_insert(empty).push(value)` with reserved capacity | Exact append to that key's row, preserving every other row; cannot unwind with the pinned key type. |
| HEAD-REMOVE-1 | `BTreeSet<CommitHash>::remove` | Exact removal, without unwinding for the pinned comparison and destructor-free key. |
| HEAD-INSERT-1 | `BTreeSet<CommitHash>::insert` | Exact insertion, without unwinding: comparison cannot panic and fixed-size B-tree node allocation uses Rust's standard aborting allocation-failure behavior. |

The pinned atomic erasure macro uses the same operation-body tokens in both
builds. Verus checks them inside its atomic-invariant macro; Cargo passes them
through `verus_exec_expr!`, erasing only proof scaffolding. Its source is part of
the same pinned library file, and an audit regression rejects a changed erasure
body. No runtime vstd dependency or permission object is introduced.

The atomic assumptions intentionally do not treat Relaxed observations as
sequentially consistent loads or as memory publication. The identity proof relies
on successful compare-exchange and preserved state on failure. Only application
code chooses `observed + 1`, checks exhaustion, registers a fresh key, chooses the
global counter and compares identities. No atomic contract assumes those facts.
The stable-static declaration exists because the pinned Verus does not permit an
exec static in an arbitrary specification; it names a Rust reference, not an
application invariant. Allocator construction/type validity and binding agreement
are checked independently. The expectation contract preserves the existing
exhaustion panic; no liveness/fairness or panic-recovery guarantee is made.

The scheduler adapters concern only the fixed key representations and standard
collections. The pop contract is included because the pinned vstd lacks a usable
specification for this production `pop_first` call. It says nothing about whether
an entry is causally ready, belongs to a pending chain or has the correct commit
hash. Checked queue updates prove their bookkeeping contracts; the outer
scheduler derives ancestry dependencies from the pending-chain premises;
construction and advancement must maintain those premises. Runtime tests
check all hash byte positions, large index bits, hash/index tie ordering and
mixed insertion/removal/wakeup operations. These tests support review of the
library adapters; they are not proofs of the standard library implementation.

The range adapter preserves the existing lazy scan and peek behavior without
collecting records or computing ancestry when the range is empty. The pinned
vstd does not specify this BTreeMap range iterator. Its generic value parameter
and private iterator field keep the three contracts at the collection boundary.
A runtime test compares empty, exclusive-boundary and large-key ranges with an
independent numeric filter, including exact values and repeated exhaustion.

The range-sort adapter preserves the existing lazy traversal's library sort.
Its comparator reads only the standard Range's `usize` start field. Application
code separately proves exact union normalization and distinct-position cardinality.
VecDeque construction, indexing, replacement, push and pop use the pinned vstd
specifications directly. The checked cursor consumes and returns its owned deque
when advancing; no extra allocation or no-unwind library declaration is needed.
The finite cardinality proof also uses vstd's integer-range and disjoint-union
lemmas. No contract assumes a valid graph or correct ready/blocked scheduler.

The hash normalization contract also links the derived order to lexicographic
order over all 32 bytes. Its runtime regression distinguishes every byte position
and checks duplicates, extremes and empty input. The shared-slice adapter preserves
the original production representation; Arc slice dereference and clone are
already specified by the pinned vstd and need no additional project contract.
Neither adapter asserts that the heads are known or authorized: checked capture
and the public Ancestry invariant establish those properties.

The sorting contracts include the bridge from `NodeIdx`'s derived `Ord` and `PartialEq`
to its one private `u32` field. The field's projection is checked in `clock.rs`;
the Rust-derived trait implementations are not verified by this harness. There
is no custom comparator, callback or Stockfoil policy logic in these adapters.
The wrappers preserve the existing production sorting/deduplication calls.

Rust documents these behaviors for
[`slice::sort_unstable`](https://doc.rust-lang.org/std/primitive.slice.html#method.sort_unstable)
and [`Vec::dedup`](https://doc.rust-lang.org/std/vec/struct.Vec.html#method.dedup).
The contracts describe normal completion. Allocation failure and process abort
are outside the existing milestone. No sorting performance bound is proved.

The pinned vstd already specifies `HashMap` construction, lookup, insertion and
entry updates, conditional on these key laws. The project declarations discharge
only that library requirement for the actual derived types. They do not assert
that a Stockfoil map contains the correct nodes or complete author lists.

The OnceValue contracts concern a generic predicate, not a Stockfoil clock.
The invariant follows from private empty construction and requiring the predicate
of every initializer before it can publish. The external datatype declaration
hides the unsupported standard-library representation; it is counted separately
from the three method contracts. It has no unchecked application initializer.
The abstract predicate does not depend on the cell's changing initialization state.

These contracts cover normal returns. They do not promise that an initializer
runs, expose a precise initialized/uninitialized state machine, or prove scheduler
progress or deadlock freedom for arbitrary callbacks. The getter is intentionally
weaker than the standard library's full stability guarantee. Runtime tests check
single publication under contention, repeated-read identity, panic/retry behavior,
and unchanged cell size. Those tests support the adapter review; they do not
verify Rust's OnceLock implementation. See Rust's
[`OnceLock`](https://doc.rust-lang.org/std/sync/struct.OnceLock.html) documentation.

## Checked application layer

`indexes::HashIndex` proves exact hash resolution, including input order, repeated
hashes and unknown-hash errors. `indexes::AuthorIndex` proves ordered and complete
author chain registration. The combined production append preserves both maps
against actual stored nodes and owners. Caller freshness, topology, chain-selection
and capacity requirements remain explicit. Runtime regressions distinguish keys
at every one of their 32 byte positions.

`candidates::normalize` composes these contracts and proves exact preservation of
the input set and strict numeric ordering. The production `history_clock` uses
this function to normalize head indices. The rewritten lazy Traversal does not
call the eager assembler. `candidates::assemble` and its batch-model refinement
are retained only under `cfg(verus_keep_ghost)` as a reference set model, without
claiming the new canonical traversal's ordering or memory behavior.
The runtime regression checks the actual adapters on duplicate and extreme node
indices against an independent set of numeric indices. Tests support review;
they do not prove the trusted standard-library implementations.

The actual ancestry walk now uses vstd's existing `Vec`, primitive-key `HashSet`,
and `Arc` contracts. Its stack, visited set, clock accumulation, cache shortcuts,
and multi-head combination are checked in the strict pass. No new project
contract is enabled for that walk itself. The checked `ancestry_cache` application
module now connects it to OnceValue. Append captures a logical ancestry set and
label prefix from the actual next graph, binds the cell predicate to a valid exact
summary, and preserves every older binding after append. The actual initializer
runs the checked walk and proves its result satisfies the predicate; actual reads
transport that summary to the current graph. No cached value's ancestry correctness
is supplied by a library axiom. The metadata invariant is established and preserved
by checked append and now maintained by the private owned Topology. The rest of
History's admitted-head and publication state remains outside that invariant.
Captured state and the identity binding are now checked separately.
Logical snapshots are erased: they allocate no runtime graph or ancestry copy.

The actual delivered-head `BTreeSet` now lives in the private Topology. The
comparison-law declaration enables vstd's existing set operations and cardinality
contracts for the derived commit-hash key. The incremental removal/insertion,
exact correspondence to vertices without children, duplicate-aware shortcut guard,
and graph coverage theorem are all checked application code. No trusted
contract asserts that a head set or delivered clock is correct. The public
iterator wrapper and admitted-head bookkeeping are not verified here.

The lineage proof uses the pinned vstd resource-map library's authoritative and
persistent tokens. Application code proves registration, state extension and
receipt agreement; no contract asserts that a Stockfoil ID denotes a lineage.
These are erased proof resources, not runtime maps. They use Verus/vstd's existing
resource logic, which is part of the verification trusted base, without adding
any project external body or assumption. `Captured`'s type invariant is checked
at construction and cloning. Topology now also has a checked type invariant.
`append_plan::prepare` proves that successful or failed reservations preserve
logical contents. Its checked `ready` predicate accounts for every vector, nested
row and hash-map capacity needed by the append. A checked `no_unwind` publication
step updates storage, indexes, delivered heads and the erased lineage together;
its postcondition reestablishes the complete topology invariant. The library
contracts do not assert any of these application facts.

The spare-capacity functions name allocation metadata separately from sequence
contents. They do not assume allocation succeeds. Failure makes preparation return
`None`, and the existing audited `expect_some` contract permits a panic before any
logical mutation. This includes fallible allocation errors that an ordinary
`Vec::push` would previously have aborted on. B-tree node allocation may still
abort. The proof uses the standard global allocator's non-unwinding contract and
Rust's default aborting allocation-error behavior for infallible allocations;
it does not cover a custom panic-on-OOM runtime configuration. Liveness, successful
allocation and recovery of the rest of History's publication state are not claimed.

The original storage/index append helpers remain available to tests and the strict
reference suite. Production uses the separately checked reserved append path;
no verification-only implementation is substituted for that production path.

## Enforcement and reporting

`verify.sh` first runs `check_trusted.py`, then runs:

1. The original proof suite with Verus's `--no-cheating` flag. The adapters and
   their dependent application modules are excluded from this pass.
2. The full suite with the explicit contracts enabled. This pass cannot use
   `--no-cheating`, because that flag rejects the external bodies and opaque cell declaration.

The runner prints the contract count separately from Verus's verified count.
The full count includes the original suite; the two pass counts must not be
added. Existing shared helpers are rechecked in the full suite as well.

The source audit pins the entire trusted file, rejects additional known proof
escape declarations in the audited Rust source directories, and rejects explicit
path imports outside those directories. Regression checks demonstrate rejection
of a modified adapter, exposed raw cell or derived-key representation, application
assumptions, unchecked application bodies,
and out-of-scope imports. It is a review guardrail, not a Rust parser or proof of
the build system. Review remains necessary for changes to the manifest, audit,
harness, compiler configuration, or dependencies.

A separate [`public_api_wiring.json`](public_api_wiring.json) pins the production
public re-exports, module wiring and the production traversal-to-cursor call sites. Its hashes are not trusted application
contracts and do not enter the 38-contract count. The audit rejects changes until
the manifest is deliberately updated; tests redirect History and Ancestry exports
to demonstrate detection. This prevents an unnoticed split between the checked
bodies and the published API, subject to the same source-review limitations.

Semantic mutation checks first run both baseline passes. Each mutation then runs
the audit and complete expanded suite using `verify.sh --mutation-only`; it does
not repeat the subset pass. This retains all application checks while avoiding
redundant subset verification for each mutation.

Semantic mutation checks run against checked application logic. Corrupting an
external body cannot be detected by its assumed contract; the source audit and
runtime checks address accidental changes there. The other trusted components
remain Rust, Verus and erasure, vstd, Z3, and the existing toolchain/build chain.

## Future contracts

Signature contracts are not enabled. The hashing boundary is documented
separately below. The collection key laws concern Rust map keys only. Introduce further contracts only alongside
actual consumers and explicit contract documentation. The enabled atomic/static
contracts do not assume identity correctness; the checked allocator and binding
agreement supply those application facts.

The separately approved [BLAKE3 boundary](TRUSTED_BLAKE3.md) adds five explicitly
trusted declarations. They are not counted as standard-library contracts.
