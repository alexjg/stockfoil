# Verifying the current Stockfoil with Verus

This milestone starts from `main` at
`551d67a5d459f985295c1bb7b12b5bcc26ff3f78`. It targets the current
`stockfoil-core::History`, persistent AVL clocks, interval differences, and
captured materialization batches. **It does not verify the full production
crate.** Shared executable proofs and conditional model theorems are listed
separately below.

The next milestone is tracked in the [authorized visibility plan](AUTHORIZED_VISIBILITY_PLAN.md),
which states the public contract, proof dependencies and completion criteria.

## Run

Install [Verus 0.2026.11.02.8bf68ee](https://github.com/verus-lang/verus/releases/tag/release/0.2026.11.02.8bf68ee)
and its matching Rust toolchain (`1.99.1` for Linux x86-64). The archive includes
Z3 and vstd.

```sh
rustup toolchain install 1.99.1 --profile minimal
export VERUS_BIN=/absolute/path/to/verus-x86-linux/verus
bash verification/verify.sh
python3 verification/check_mutations.py
cargo +1.99.1 test --workspace --locked
```

`VERUS_BIN` defaults to `verus` on PATH. The runner checks the exact version,
verifies every module in `main.rs`, and enables `--no-cheating`; it accepts no
arguments that could skip proofs. The mutation runner requires a passing
baseline, then checks temporary copies with thirty-nine deliberate semantic errors.
Compilation or tool setup failures do not count as mutation rejection.

The checked proof baseline reports **119 verified, 0 errors** and rejects all
thirty-nine mutations. The authorization plan records which stage these results
establish; the public API refinement remains unfinished.

[CI](../.github/workflows/verus.yml) pins the Verus archive and SHA-256, runs the
proof and mutation suites, and tests the Rust workspace. Normal Cargo builds
erase proofs using the pinned `verus_builtin_macros` procedural macro. They do
not need the Verus executable or a runtime vstd dependency. This workspace uses
`std`; this milestone makes no `no_std` compatibility claim.

## Shared production code

The harness includes these exact source files; it does not verify substitute
implementations of their executable bodies:

| Source / operation | Proved contract | Production caller |
| --- | --- | --- |
| [`clock.rs`](../stockfoil-core/src/verified/clock.rs): `NodeIdx::new`, `index` | Preserve the exact `u32` position; the tuple field remains private. | History insertion and node indexing |
| `contains` | Inclusive membership in an optional coordinate; `None` differs from position zero. | `Clock::contains` and `include`'s early return |
| `cap` | Exact prefix intersection, including absent and zero bounds. | `Clock::mask` |
| `upper_bound` | Terminates and returns exactly the first index above a bound in a nondecreasing slice; arithmetic and indexing are safe. | Interval difference |
| `difference_range` | Valid half-open slice containing exactly the nodes in the after-prefix and outside the before-prefix. Reversed bounds give an empty slice. | `clock::difference`, hence fragment membership and materialization planning |
| [`events.rs`](../stockfoil-core/src/verified/events.rs): `classify` | Both visibility bits are represented exactly by `Vis`. | Materialization record emission |
| `batch_end` | Advances by `min(max, remaining)` without overflow. Positive budgets make progress; zero budgets preserve position. | `emit_next_changes` |
| [`tree.rs`](../stockfoil-core/src/verified/tree.rs): `Clock::default`, `clone` | Empty clocks are ordered and have an empty coordinate map; cloning preserves the logical tree using `Arc::clone`. | Clock construction and snapshots |
| `Clock::node`, `height`, `len` | Constructor preserves ordering and the exact coordinate map, and computes exact structural height/size without overflow under explicit child and capacity premises. Accessors return cached metadata, which equals structural values when metadata is valid. | Rotations, updates and reconstruction |
| `Clock::mask` | Exact coordinate-wise intersection with the callback bounds, preserving the tree invariant and never increasing size. Denied keys disappear, zero remains a valid tip, and retained tips are capped. | Permission participation |
| `Clock::from_sorted`, `from_tips` | Sorted reconstruction preserves the exact entry sequence and establishes ordering, metadata and AVL balance. Tip-table construction maps precisely the contiguous chain IDs to their supplied tips. Both prove capacity, indexing safety and termination under an input-length bound. | Permission masks and delivered-history clocks |
| `Clock::union` | Exact union of keys and pointwise maximum tips; preserves the tree invariant and computes exact distinct-key size. The result must fit u32; overlap is counted only once. | ClockBuilder and ancestry summaries |
| `Clock::entries`, `Entries::next` | Actual Rust iterator returns precisely the remaining in-order sequence, terminates and stays exhausted. For ordered trees, the sequence contains every map entry exactly once in strictly increasing key order, with length equal to structural size. | Union, masking and interval differences |
| `Clock::set`, `include` | Terminating path-copying replacement and pointwise maximum update preserve ordering, metadata and balance, with exact size changes and height growth at most one. Only a new coordinate needs size capacity. | Clock builders and union |
| `Clock::balance` | Given balanced children whose heights differ by at most two, restores AVL balance while preserving ordering, the exact map and size, and valid metadata. Output height lies between the taller input child and one more than it. Explicit size/height capacity premises cover every intermediate constructor. | Path-copying insertion |
| `Clock::position`, `contains` | Terminating, exact lookup and inclusive membership in the actual tree, conditional on its BST ordering invariant. | History, permission bounds and interval differences |

`NodeIdx` stays a distinct history-local newtype; chain IDs remain `u32`.
The actual `Clock(Option<Arc<Entry>>)` representation and iterative lookup now
live in the shared `verified/tree.rs` module. `contents` gives its mathematical
coordinate map and `ordered` requires strict BST ordering throughout the tree.
Lookup follows a smaller subtree on each iteration; its termination proof does
not rely on cached height/length metadata. No tree copying or recursive runtime
lookup is introduced. The clone proof uses vstd's trusted `Arc::clone` contract;
a runtime pointer-equality regression also checks structural sharing.

**Whole-clock callers still must establish validity and size capacity.**
The actual `Clock::node` now lives in the shared verified module. Its children
must be ordered, have exact metadata, and have keys strictly below/above the new
root. Their combined size plus the root must fit `u32`, and their heights plus
one must fit `u8`. Under these premises, it preserves ordering and contents and
computes the exact structural size and height. These capacity conditions are
proof obligations, not new runtime checks or a proof of global capacity safety.

`size` and `depth` are independent recursive specifications; `metadata` requires
cached values to equal them throughout the tree. Empty clocks establish this
predicate. The recursive `balanced` predicate independently requires child
heights to differ by at most one at every node. `node` characterizes when its
result is balanced; it also permits temporary unbalanced nodes used by rotations.

The production `balance` routine now lives in the shared module, with helpers
for the left-heavy and right-heavy cases. It verifies all four rotation paths
and the no-rotation path. Its inputs must be ordered, balanced, have valid
metadata, partition keys around the root, differ in height by at most two, and
meet the constructor's total-size and input-height capacity bounds. The proof
establishes each intermediate constructor's premises, safe height comparisons,
and the nonempty children needed by `unwrap`. Returned height is between the
maximum input child height and that height plus one. Unchanged subtrees still
use shallow `Arc::clone`; no whole-tree traversal or copying is added.

Insertion (`set`/`include`) now establishes the balancing premises along every
recursive path. It preserves ordering, metadata and balance, writes exactly the
requested coordinate (`set`) or its maximum with the old tip (`include`), and
increases size exactly when adding a key. Recursion terminates on a smaller
subtree; returned height never decreases and grows by at most one.

A structural counting lemma proves that any balanced tree with valid `u32`
size metadata has height below 65. Thus insertion needs no separate height
assumption. It still requires valid input and capacity for a new coordinate;
whole-clock construction and history callers must establish those premises.
The actual stack-based iterator is now a named `Entries` type in the shared
module. Its `Iterator::next` implementation verifies both its concrete remaining
sequence contract and vstd's standard iterator contract. Each descent terminates
on a smaller subtree, each yielded entry removes exactly the first remaining
item, and exhaustion is stable. A structural lemma connects that sequence to
the coordinate map: every map entry appears exactly once, in strictly increasing
key order, and sequence length equals structural size. No whole-tree staging
allocation is introduced; the traversal retains its stack of borrowed entries.
The production union now composes the verified iterator and inclusion operation.
Its loop accumulates exactly the maximum over the consumed entries and retains
the larger input's structure. The result contains exactly the union of input
keys, each tip is at least both applicable input tips, and every output tip is
one of those inputs. This characterizes the pointwise maximum without inventing
entries. A map-cardinality lemma connects ordered-tree size to distinct keys;
capacity is required only for the union of keys, so overlapping inputs are not
charged twice. The output has exactly that size and preserves the tree invariant.
Sorted reconstruction now establishes the full tree invariant directly. It
preserves the input sequence exactly, computes its exact size and midpoint-tree
height, and proves balance using a height-step lemma. Recursive calls consume
strictly smaller slices. Input length must fit `u32`; derived child height bounds
discharge constructor capacity without an independent height assumption.
`from_tips` constructs the same indexed entries as before, with a separate lemma
connecting each array index to its coordinate and excluding all other keys.
Masking now composes the actual iterator, shared cap operation and sorted
reconstruction. Its contract requires a callable, deterministic callback contract
for the relevant coordinates. It proves a bound result exists for every input
coordinate and returns exactly the keys whose bound is present, with each tip
capped at that bound. Size cannot increase. The implementation uses the existing
filtered-entry vector and visits each input once; no new runtime parameter or
whole-history scan is added. The production permission callback must still be
proved to supply the policy's correct bounds; the mask does not assume this fact.

The callback specification uses a chosen result satisfying its contract;
existence for all visited coordinates is established by the actual calls, and
uniqueness is an explicit premise. No callback outcome is assumed without a
call. Its executable interface remains `FnMut`, allowing the existing bound cache.

The binary search requires a sorted chain-node slice, and `batch_end` requires
`next <= total`. The wrappers maintain these by construction and have regression
tests, but their invariant maintenance remains a formal proof obligation.
Strict ordering and unique chain membership are additionally needed for unique
whole-history enumeration. The helpers add no staging allocation and retain
logarithmic interval search.

## Connected model theorems

These files are verification-only models. Their hypotheses are explicit proof
obligations for the concrete wrappers, not facts established about all runtime
states.

| File | Checked result | Remaining production boundary |
| --- | --- | --- |
| [`graph.rs`](graph.rs) | Independent parent-edge reachability is ordered and transitive; append preserves old reachability; same-author maximal-tip reuse preserves chain invariants; valid summaries imply exact ancestry membership. | Hash-to-index mapping, chain assignment, concrete summary construction, strict-ancestor fast path and unknown hashes. |
| [`walk.rs`](walk.rs) | Starting, expanding parents, revisiting and substituting a complete cache preserve the walk invariant; an empty pending set yields exact history. | Stack/visited bookkeeping and termination, selective cache initialization and thread safety, accumulating the AVL summary. |
| [`authorization.rs`](authorization.rs) | Independent policy laws: missing/denied permissions, exact unbounded permission, inclusive bounds, author isolation, available-frontier equivalence/monotonicity, and fixed-view stability under append. | Mapping commit hashes and stored authors to model nodes; view validation and concrete capture. |
| [`visibility.rs`](visibility.rs) | Coordinate masking equals the independent parent-edge permission policy under explicit bound-compilation contracts. Unavailable frontier heads grant nothing. | Permission-map lookup, actual author labels, filtering frontier hashes, constructing bound clocks, mask iteration and AVL reconstruction. |
| [`intervals.rs`](intervals.rs) | The shared executable interval search selects exactly the supplied chain nodes in one history and outside another, given valid summaries and correct AVL lookup results. Also states the inclusive-head-minus-boundaries fragment rule. | Complete chain tables, whole-clock enumeration, AVL lookup, fragment boundary discovery and placement checks. |
| [`materialization.rs`](materialization.rs) | Candidate sets equal new delivery union visibility change. A batch model uses the shared boundary and classifier, emits the selected pending slice with exact fields, and consecutive slices concatenate without gaps or overlap. Sorted candidates are unique and topological; an unchanged cursor yields no candidates. | Concrete set assembly, sort/dedup, record/hash/payload mapping, history identity checks, captured heads and completed-cursor publication. |
| [`examples.rs`](examples.rs) | Nonempty witnesses for chain summaries, permission masking and batches; zero versus absence, reversed bounds, maximum indices and budgets. | Specification checks, not wrapper proofs. |
| [`authorization_tests.rs`](authorization_tests.rs) | Nonempty policy witnesses with multiple heads, cross-author bounds, denied intermediate authors, unavailable heads, and a frontier becoming known after capture. | Specification checks, not wrapper proofs. |
| [`tree_examples.rs`](tree_examples.rs) | Actual shared tree witnesses exercise both search directions, absent keys, zero/maximum IDs, clones and membership. A malformed tree fails the ordering predicate. | Also uses actual construction and balancing for all four rotations, the no-rotation case, and equal-height grandchildren, with distinct tips and retained snapshots. Update witnesses also check replacement, monotone inclusion, extreme keys and retained snapshots; iteration witnesses check key/tip order, completeness and repeated exhaustion. Union witnesses cover overlapping keys, maximum tips, idempotence, both size-selection branches, empty inputs and retained snapshots. Reconstruction witnesses cover sparse/extreme keys, even sizes, empty inputs and zero tips. Mask witnesses use actual callbacks for denied, unbounded, zero and expanding bounds. The production permission callback and history callers remain unproved. |

[`coordinates.rs`](coordinates.rs) supplies a mathematical sequence view of AVL
entries. It is not an executable vector clock. Model chain labels widen `u32`
IDs to `usize`, and authors are abstract integer labels; neither mapping from
production is proved here. `graph::summarizes` requires each coordinate to be a
real history member and to bound every history member on its chain. Exact
membership is derived from these facts and the chain ancestry invariant.

The graph model follows parent edges independently of clocks. Repeated edges
have set semantics. Bounded permission first intersects the frontier **heads**
with the selected history, then takes their ancestry. Taking all frontier
ancestry first and intersecting afterward would incorrectly grant ancestors of
unavailable heads; a concrete witness and mutation test check this distinction.

The materialization model describes the current captured candidate list, not a
whole-history production scan. `emit_batch` is an executable **model** whose
boolean inputs stand for already computed delivery and visibility membership.
Only `batch_end` and `classify` are shared with the production emitter. A positive
budget decreases the number of candidates remaining; the host must supply such
budgets to finish a nonempty traversal. Later deliveries require a new traversal.

## Validation and trust

Thirty-nine mutations must be rejected: an expanding cap, exclusive membership,
exclusive upper-bound search, dropping position zero for an absent clock,
failure to advance a positive batch, swapped visibility, omitted new invisible
commits, granting an out-of-history frontier, defaulting missing permissions to
unbounded, escaping the selected history, excluding the frontier itself, and
selecting permissions by the wrong author, taking either wrong AVL subtree,
omitting a matching tree root, dropping a tree during cloning, choosing the shorter
child height, omitting the root from the size, swapping constructor children,
using the wrong tip in either rotation direction, skipping either inner double
rotation, skipping the left-heavy balancing case, ignoring replacement tips,
dropping a new coordinate, skipping every inclusion update, descending the wrong
iterator subtree, revisiting the left subtree, dropping a yielded entry,
ignoring the smaller union input, replacing union tips with zero, and discarding
the larger initial tree, replacing reconstructed tips with zero, swapping
reconstructed children, shifting tip-table chain IDs, failing to cap expanding
mask bounds, dropping position zero, and ignoring denied mask coordinates.

The ordinary tests compare interval results with an independent linear filter,
exercise zero/unlimited batches and delivery after capture, and retain the
workspace's randomized parent-walk, visibility, persistence, and replica tests.
The [public authorization contract tests](../stockfoil-core/tests/authorized_visibility.rs)
exercise the policy through `changes`/`next_changes`, including permission
transitions and a frontier becoming known after capture.
These tests complement the proofs; they do not discharge model hypotheses.

There are no project `assume`, `admit`, `external_body`, `assume_specification`,
or custom axioms. The trusted base includes Verus and erasure, vstd's library
specifications, Z3, and Rust. Allocation failure, resource limits and cryptographic
security are outside this milestone. The proof count includes helpers and loop
obligations; it is not a measure of production coverage.

## Next proof obligations

1. **Persistent clocks:** the BST predicate and conditional lookup are checked.
   Node construction now preserves ordering and exact metadata under explicit
   premises. Balancing now restores AVL balance and preserves the map, size and
   metadata under explicit child and capacity premises. `set`/`include` now
   establish those premises, including deriving height capacity from balanced
   structure and size metadata. Complete ordered iteration is now verified too.
   Union now composes iteration and inclusion and checks capacity against the
   distinct result keys. Sorted reconstruction and tip-table construction now
   establish the invariant directly. Masking now proves exact callback-bound
   intersection. Next connect the tree's map to the graph model's coordinate
   view and verify whole-history callers establish the remaining contracts.
   Preserve structural sharing and account for `u32`/`u8` limits rather than
   assuming they cannot overflow.
2. **History refinement:** prove insertion and author-chain selection preserve
   the chain invariant, and that concrete cached walks produce exact summaries.
3. **Views and materialization:** prove permission compilation, complete chain
   enumeration, candidate dedup/order, capture semantics, wrong-history rejection
   and cursor publication establish the model contracts.
4. **Authentication and fragments:** verify incremental per-author coverage,
   exact depth-stopped boundary discovery, fragment placement, and compatibility
   between export and admission. State BLAKE3/Ed25519 assumptions explicitly.
5. **Admission and export:** verify tickets, stale/wrong-kind reply rejection,
   cancellation, atomic publication, epoch waiting, acknowledged inventory and
   cursor durability semantics. Host storage and application rollback remain
   separate responsibilities.
6. **Representation independence:** connect equivalent hash-identified histories
   with different chain labels through an explicit graph isomorphism.
