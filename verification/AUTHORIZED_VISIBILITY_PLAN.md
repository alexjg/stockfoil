# Authorized visibility: public API verification plan

> Rebased onto the rewritten `main`. [REBASE_STATUS.md](REBASE_STATUS.md)
> supersedes historical API and production-coverage claims below: raw History
> changes are now internal, the public boundary includes Materialization and
> Ledger, and the eager candidate assembly is reference-model code only.

Current implementation progress and the first API target are tracked in
[API_TARGET.md](API_TARGET.md).

## Current status

The public captured ancestry portion is now verified: `History::ancestry`,
`Ancestry::contains`, `participates`, `heads` and clone, plus History construction
and strict ancestry. The harness checks the actual exported types and errors,
with no caller validity preconditions. Head normalization retains exactly the
requested set, including redundant ancestors. Membership equals the frozen
parent-edge/policy set, with wrong-history rejection and unknown hashes excluded.

Public materialization `plan`, `complete`, `is_current`, `step` and `update` now
also compose captured authorization through actual callback arguments, feedback,
checkpoint publication and exact cumulative progress. Public `mark_local` now
checks local acknowledgement and its metadata/causal query dependencies. Authorization is relative
to recorded authors and supplied policy; this does not establish authentication,
payload confidentiality, concrete application effects or durable storage.
`change`, commit publication and general lifecycle reachability remain open. The chronological
progress notes retain earlier counts and boundaries; [API_TARGET.md](API_TARGET.md)
and [README.md](README.md) give current results.

## What we want to verify

A commit participates in a view exactly when it belongs to the selected history
and its author's permission allows it. The selected history includes the view's
heads and every commit reachable by following their parents.

- Denied permissions allow nothing. Unlisted authors use the configured fallback
  rule, denied by default; an explicit author entry always overrides that rule.
- Unbounded permission allows that author's commits within the selected history.
- Bounded permission allows that author's commits reachable from at least one
  available frontier head, including the frontier head itself.
- A frontier head is available only if it belongs to the selected history.
  Unknown frontier heads and known heads outside that history grant nothing,
  even when they have ancestors inside the history.
- Frontier heads may have a different author. The permission entry determines
  whose commits are authorized; the frontier determines how far it reaches.
- Denying an intermediate author does not cut off ancestry traversal to an
  earlier authorized commit. Multiple heads and duplicate frontier entries use
  union/set semantics.

Both safety and completeness matter: unauthorized commits never participate,
and authorized commits are never accidentally excluded. Unbounded permission
does not authorize commits outside the selected history. Changing one author's
permission cannot change another author's participation.

Visibility is participation, not confidentiality or delivery. Materialization
must report a newly delivered commit even when it is invisible. For each record
emitted by `History::next_changes`, `Before`, `After`, `Both`, and `Neither` must
encode the exact policy for the cursor's previous view and the target view
captured by `History::changes`. Later deliveries must not change the captured
authorization result. A later traversal may have different heads or permissions.

Unknown view heads are errors; unknown permission frontier heads are ignored.
A traversal must not be used with another history. Signature validity is part
of the later attestation milestone: stored authors and supplied permissions are
inputs to this policy, rather than an assertion that all delivered drafts are
authenticated. Persisting application state and its cursor atomically remains
the application's responsibility.

## Strategy and reviewable stages

The dependency order is policy, persistent clocks, history refinement,
permission compilation, then the public API connection. Each stage must state
which production facts remain hypotheses. Adding another model theorem alone
does not close a production boundary.

### 1. Independent policy and contract tests

Separate the parent-edge policy from coordinate/clock refinement. State the
rules above using graph nodes, authors, heads and permissions, without AVL
operations or chain labels. Prove denial and unbounded behavior, inclusive
frontiers, irrelevance of unavailable frontier heads, author isolation, and
stability under graph append with fixed view heads. Use nonempty witnesses to
exercise multiple heads, cross-author bounds and paths through denied authors.

Exercise the same rules through the public materialization API, including
before/after transitions and graph append during a captured traversal. These
tests establish the intended behavior and catch integration regressions; they
are not proofs of the wrappers. Extend mutation checks to reject policy errors.

### 2. Actual persistent AVL clock operations

Verify the production tree representation and operations against a mathematical
coordinate map: BST ordering, height/length metadata, rotations, lookup,
insertion, union, sorted iteration, masking and reconstruction from sorted
entries. Prove arithmetic bounds for `u8` heights and `u32` lengths/chain IDs.
Retain persistence and structural sharing. A copied vector/tree implementation
in the harness is not a substitute for verifying the actual source.

### 3. History and cache invariants

Define the mapping from stored hashes and `NodeIdx` values to the parent-edge
graph, and from chain labels to authors. Prove single-author chains, ancestry
ordering, complete sorted chain-node tables, and exact summary construction.
Verify the concrete stack/visited walk and its termination, selective cache
initialization, and safe reuse after append. Cover all construction paths,
including local drafts and received history; do not assume an unproved valid
history at public entry points. Identify standard-library trust boundaries
explicitly, including immutable sharing and once-cell behavior.

### 4. Concrete permission compilation

Verify `history/view.rs::participation`: resolve valid view heads; obtain their
exact ancestry; select permissions by the actual chain author; filter frontier
heads before constructing frontier ancestry; compute bounds and mask the clock.
Prove its output represents exactly the independent authorized set, in both
directions. The existing `compiled_bounds` hypothesis must be discharged by this
implementation, rather than retained as a public theorem precondition.

### 5. Public API connection

Specify `History::changes` and `History::next_changes` over reachable states.
Connect the cursor's prior view and the captured target to actual record fields.
Verify history-identity and invalid-head errors, immutable capture of the policy
inputs, and stability as the history grows between batches. Combine with the
materialization work for complete candidate enumeration, uniqueness, topological
ordering, batch partitioning, and completed-cursor publication.

## Completion criteria

- Every reported visibility bit equals the independent policy for its captured
  view; every authorized commit is included in the participation set.
- History and clock prerequisites follow from public construction paths, rather
  than application assumptions. Explicit standard-library contracts are allowed
  only through the [enumerated trusted boundary](TRUSTED_STDLIB.md).
- Production code is shared with the harness; model-only functions are labeled.
- The original suite passes `--no-cheating`; the full suite passes with only
  enumerated standard-library contracts, enforced by the trust audit. Semantic
  mutation checks and applicable runtime tests pass.
- Documentation names any remaining trusted library specifications and excludes
  cryptographic security and application crash recovery from this milestone.

## Progress

- [x] Stage 1: policy laws, nonempty examples, public API contract tests and mutations.
- [x] Stage 2: production AVL operations and coordinate-model refinement.
- [ ] Stage 3: history and cached-walk refinement.
- [ ] Stage 4: production permission compilation.
- [ ] Stage 5: public API authorization theorem.

Stage 1 evidence:

- [`authorization.rs`](authorization.rs) isolates the policy and proves denial,
  exact unbounded behavior, inclusive frontiers, author isolation, available
  frontier equivalence/monotonicity, and capture stability under graph append.
- [`authorization_tests.rs`](authorization_tests.rs) supplies nonempty witnesses
  with multiple heads, cross-author frontiers, unavailable heads and append.
- [`authorized_visibility.rs`](../stockfoil/src/history/authorized_visibility.rs)
  tests the public API across batch sizes, permission transitions, an unknown
  frontier delivered after capture, invalid heads and wrong-history rejection.
- The Stage 1 baseline reports 58 verified obligations; all 12 semantic mutations are
  rejected, including four new authorization-policy mutations.

Stage 2 evidence (conditional operation contracts; caller premises remain in later stages):

- The shared production tree now has a coordinate-map view and a strict BST
  ordering predicate. Its actual iterative lookup and membership functions are
  proved exact under that predicate; empty construction and shallow cloning are
  also verified. See [`tree.rs`](../stockfoil/src/verified/tree.rs).
- Actual tree witnesses, a structural-sharing regression and four additional
  semantic mutations exercise these contracts. The clock-stage baseline reported
  139 verified obligations and rejected all 47 mutations.
- Nonempty production node construction now preserves ordering, the exact
  coordinate map, and structural height/size under explicit child ordering,
  metadata and capacity premises. `height`/`len` accessors are also verified.
  Asymmetric witnesses use this actual constructor; three new mutations test it.
- Production balancing now verifies single and double rotations in both
  directions and the no-rotation path. Balanced children with height difference
  at most two produce a balanced, ordered tree with the exact coordinate map,
  size and metadata. The proof discharges intermediate constructor bounds from
  explicit input capacity premises and bounds the returned height.
- Nonempty witnesses cover each rotation and equal-height grandchildren, with
  distinct tips and retained snapshots. Five additional mutations test rotation
  selection, double-rotation steps and key/tip association.
- Actual `set`/`include` now preserve the tree invariant and exact coordinate
  semantics, with terminating recursion, exact size changes and height growth
  at most one. A structural counting proof derives height below 65 from balance
  and valid `u32` size metadata; only new-key size capacity remains a premise.
- Update witnesses cover replacement, pointwise maximum, extreme keys and old
  snapshots; three new mutations test updates. Whole-clock callers still must
  establish validity and size capacity.
- The actual Rust iterator now verifies exact ordered enumeration, termination
  and stable exhaustion. A structural lemma connects its in-order sequence to
  every map entry exactly once. Three mutations and a nonempty exhaustion
  witness exercise the iterator.
- Union now composes verified iteration and inclusion to compute exactly the
  pointwise maximum and union of keys, preserving the tree invariant. A
  cardinality proof connects structural size to distinct keys and establishes
  intermediate capacity from the final union's key count. Witnesses cover
  overlap, idempotence, empty inputs and both size-selection branches; three
  mutations test union.
- Sorted reconstruction now establishes ordering, exact metadata and AVL balance
  while preserving the entire input sequence. Tip-table construction establishes
  precisely the supplied contiguous coordinates. Both prove indexing, capacity
  and termination from an input-length bound. Two witnesses and three mutations
  exercise reconstruction.
- Masking now composes verified iteration, cap and reconstruction and returns
  exactly the callback-bound intersection, preserving the tree invariant and
  never increasing size. Callback existence is established through actual calls;
  deterministic callback behavior is an explicit premise. A nonempty witness
  covers denial, unbounded, zero and expanding bounds; three mutations test it.
  The production permission callback still must establish the bound contract.
- `tree_refinement.rs` proves actual lookup matches the coordinate model and
  composes the production mask with the independent graph authorization theorem.
  A nonempty executable witness retains an inclusive frontier ancestor and
  excludes a later commit. Two mutations test the coordinate/bound projections.
  Concrete history summaries, author mapping and compiled bounds remain premises
  to discharge in stages 3 and 4; no public API proof is claimed.
- Production difference enumeration now appends the exact selected table nodes,
  preserving prior output and chain traversal order, with safe indexing and
  termination. Symmetric difference covers both directions. Materialization and
  fragment callers use this checked output path. Table completeness/uniqueness
  remain history-stage obligations; candidate assembly and normalization are now
  checked under the enumerated standard-library contracts. A nonempty witness and
  three mutations exercise whole-clock enumeration.
- The actual `ClockBuilder` now composes the checked include/union contracts and
  returns the exact accumulated result. A cached-input witness and three mutations
  test accumulation. The graph walk still must prove it includes precisely the
  intended nodes and satisfies the builder's capacity premises.

Review stack: foundations (#4), authorization plan/policy (#3), then AVL
lookup (#5), node construction (#6), rotation correctness (#7), insertion (#8), ordered iteration (#9), union (#10), reconstruction (#11), masking (#12), the clock-to-policy connection (#13), complete differences (#14), accumulation (#15), history identity (#16), the public API roadmap (#17), and
the explicit standard-library contracts (#18). Candidate assembly follows those
contracts. Stage 1 adds
specifications and integration tests. Stage 2 has begun production refinement,
but no complete public API authorization theorem exists. The next implementation
step is proving concrete history and bound compilation establish the remaining
hypotheses, alongside captured batch assembly.
The remaining stages continue through concrete history/view construction and
`History::changes`/`History::next_changes`; this stack is not complete until
those public entry points establish their own prerequisites.

The [public API proof boundary](PUBLIC_API_PROOF_BOUNDARY.md) records the remaining
stack, the standard-library decision, and the history-ID exhaustion fix found
while checking identity assumptions. The public API stages remain unchecked.

Candidate assembly now lives in shared `verified/candidates.rs`. It composes the
actual difference operations with normalization and proves exact membership and
strict numeric ordering. `candidate_refinement.rs` connects its result to the
existing materialization pending contract, conditional on complete/correct chain
tables and a delivered clock covering every history node. A nonempty witness
covers overlapping delivery/visibility reasons, a new invisible node, and an
unchanged traversal. No concrete history invariant or public API theorem is
claimed. The full suite now reports 147 checks with two explicit stdlib contracts;
the original suite reports 140 checks with `--no-cheating`; 52 mutation checks
exercise the combined suite.

The next construction layer moves actual chain-tip and row updates into
`verified/chain_tables.rs`, originally called by both branches of `assign_chain`
and now called by `verified/storage.rs::append`. It proves
exact writes and preservation of nonempty strictly increasing rows with their
last node as tip. A refinement proves that appending a fresh node preserves the
candidate model's complete table layout, and that `Clock::from_tips` over valid
tables covers every recorded node. A witness starts from empty tables, creates
two interleaved chains, and extends the first. These are checked application
proofs; no additional trusted contracts were introduced.

Stage 3 remains open: History must connect the ghost chain-label sequence to its
stored nodes, verify chain selection and author mappings, establish fresh indices
and capacity, and verify the actual cached walk. The latest baseline is 143 strict
checks and 154 full-suite checks with the same two stdlib contracts. Three table
mutations bring the mutation suite to 55 cases.

The subsequent storage layer verifies the actual generic node representation,
empty construction, packed parent writes, node append and parent-slice access.
Chain labels are projected from stored nodes, rather than supplied as an unrelated
ghost sequence. Appending preserves complete sorted chain tables, the exact new
metadata, earlier nodes and parent slices, and parents-before-child order. The
stored graph is a DAG, and append preserves every old reachability relation.
History uses these checked functions directly; hashes, payload IDs and cache
values are opaque fields preserved by the storage proof.

Stage 3 still needs hash-map resolution, chain selection and author invariants,
capacity discharge at callers, and the concrete cached walk. The latest baseline
is 153 strict checks and 163 full-suite checks with the same two stdlib contracts.
Six new mutations bring the suite to 61 cases; the nine storage/table mutations
were rerun for this layer, not the entire mutation sweep.

The chain-selection layer verifies both production searches. The direct path
checks actual author identity and current-tip equality and selects the smallest
eligible chain. The fallback chooses the first clock-covered tip in the supplied
chain list. Author registration and topology append now compose in checked code.
A refinement derives maximal tips and fresh chain IDs from stored tables and
proves preservation of the graph's single-author ancestry invariant. The graph
chain theorem is generic over actual Author values; no identity or cryptographic
axiom was introduced. Witnesses exercise direct reuse, indirect reuse through a
different author, stale parents, fresh chains and merge ordering.

Stage 3 still needs the actual author-to-chain map and cached ancestry computation
to discharge the fallback's inputs, plus hash resolution and caller capacity.
The baseline is 170 strict and 180 full-suite checks with the same two stdlib
contracts. Eight new mutations bring the suite to 69; the eight new checks and
three table mutations were rerun, not the full sweep.


### Maintained topology validity, including unwinding

The production Topology now has a checked Verus type invariant. Allocation
preparation preserves all logical contents, and the complete storage/index/head/
lineage publication is checked not to unwind. Metadata mutation validates its
index before writing. Core capture, wrong-history checking and both captured
membership queries therefore have no caller preconditions. A proof witness uses
arbitrary borrowed handles and topologies to check this boundary.

This uses 12 additional collection capacity/unwind contracts (27 total), with
application preservation proofs remaining checked. The runtime keeps its existing
representation: no graph copies, poison flag or runtime lineage registry. See
[TRUSTED_STDLIB.md](TRUSTED_STDLIB.md) for allocation-failure behavior and the
standard abort-on-OOM assumption for infallible B-tree allocations.

### Exported ancestry API completed

The actual public History and Ancestry definitions, constructors, heads, clone,
membership and errors now live in shared checked production code. Arbitrary-value
consumers compose the public contracts without representation premises. A
nonempty fixture checks duplicate/redundant heads, denied/unknown membership,
unknown-head errors and frozen answers after policy changes and append; fixture
insertion is private Topology insertion, not a public publication proof.

The full suite passes 352 checks; the strict subset passes 208. Two additional
library contracts cover hash sort/dedup and Vec-to-Arc-slice conversion, bringing
the boundary to 29. No application assumption was added. Fifteen new public-layer
mutations are rejected with proof diagnostics; all 200 mutation anchors match.
Production export wiring is separately pinned and covered by audit regressions.

Next verify production lazy traversal and Materialization. Admission/evidence,
compaction, sync minimality and attestation completion need further invariants;
they are not proved by the ancestry milestone.


### Production per-chain interval cursor

The lazy traversal now uses checked production interval normalization, exact
cardinality, least-position advancement and maximal selected dependency queries.
Its VecDeque representation remains lazy. A composed consumer proves exact,
duplicate-free increasing enumeration and a theorem frames selected nodes under
table extension. The full baseline is 375 checks (208 in the strict subset), with
30 explicit library contracts; the additional contract covers range sorting only.
Fourteen new mutations target union/count/progress/dependency behavior, and runtime
regressions cover every triple of small ranges and maximum arithmetic endpoints.

This layer still has explicit nonempty-input and bounded/sorted-table premises.
The scheduler must derive those facts from clock-difference collection, maintain
its ready/blocked partition and aggregate count, and prove canonical multi-chain
ordering before `Ancestry::difference` or materialization enumeration is claimed
verified. Public captured ancestry membership remains the completed API boundary.


### Production ready and blocked queues

The lazy scheduler now uses shared checked ready insertion, blocked-waiter append,
minimum-ready removal and exact waiter-list detachment. They preserve the explicit
queue invariant under freshness and dependency-order preconditions; no whole-
scheduler type invariant or panic-recovery guarantee is claimed. Concrete consumers
cover hash/index order disagreement, ties, shared dependencies and reblocking.
A conditional minimum-frontier theorem proves progress from pending coverage,
and a checked consumer composes exact causal readiness with canonical queue pop.

The production pending-chain scheduler must still derive and preserve those
premises from actual ancestry and interval state, including the aggregate count.
The full baseline is 391 checks (208 strict), with 33 library declarations.
Three new declarations cover fixed-key comparison/hash laws and BTreeSet minimum
removal only. Fourteen mutations exercise queue transitions and the conditional
progress/readiness boundary. Captured ancestry membership remains the completed
public API; difference iteration and materialization remain open.


### Production causal scheduling decision

The actual Traversal scheduling call now delegates to checked `pending.rs`, using
real stored topology, ancestry clocks, pending intervals and queues. Under a
bounded, canonical, one-entry-per-chain pending map with exact frontier keys, the
scan either finds a selected strict ancestor or proves there is none anywhere in
the remaining selection. Scheduling adds exactly the candidate, uses its stored
hash when ready, or blocks on an earlier selected dependency covered by a frontier.
PendingChain queries now derive their node-table ordering and causal facts from
Topology. The generic lazy map-range adapter adds three collection contracts;
application selection and readiness remain proved, not trusted.

The full baseline is 399 checks (208 strict), with 36 library declarations, 195
runtime tests and 19 audit regressions. Twelve new semantic mutations cover this
step; the configured sweep now has 240 cases. This does not yet verify the public
difference iterator: construction and `next` must establish/preserve the pending
invariant, queue partition, exact aggregate count and captured selection through
advancement and wakeup. Materialization composition follows that boundary.


### Production frontier advancement and waiter wakeup

The actual pending-map advance now preserves one valid entry per selected chain,
replaces its frontier without colliding with another chain, retires exhausted
chains and removes exactly the emitted node from a finite selection derived from
stored topology. That selection's cardinality decreases by exactly one. Actual
waiter wakeup detaches and reschedules the entire row once each, preserves all
other queue memberships, and cannot recreate a dependency on an emitted node.
A waiter can become ready or reblock on another selected ancestor.

The baseline is 412 full checks (208 strict), 37 library declarations, 196 runtime
tests and 21 audit regressions. The sole added library declaration is NodeIdx's
derived comparison law for standard BTreeMap operations. A concrete nonempty
proof constructs and retires a real frontier. Fourteen new mutations and the
previous twelve scheduling mutations are checked locally; the full configured
sweep now has 254 cases.

The next obligation is the joint Traversal invariant: construction must produce
valid pending intervals and the exact count; pop, advance, successor scheduling
and wakeup must jointly preserve the ready/blocked partition and exact readiness.
Then prove capture stability and public difference iteration, followed by
Materialization. The aggregate executable counter and public iterator are not
claimed verified by these local transition proofs.


### Joint production scheduler invariant

Queue initialization and the complete production `next` body now share a checked
invariant: exact pending-frontier queue coverage, valid ready/blocked bookkeeping,
correct ready hashes and causal eligibility, selected ancestors for dependencies,
and exact remaining cardinality. Initialization establishes it from a valid pending
map and count; `next` preserves it, emits the minimum eligible hash, removes exactly
one selected record and decrements the actual counter once. None implies true
exhaustion. The temporary stale dependency after advancement is explicitly confined
to the emitted node and eliminated by wakeup.

A generic consumer proves termination, exact complete enumeration, uniqueness and
ancestor-first order at a fixed topology. A nonempty constructed proof exercises
initialization, emission and stable repeated exhaustion. The baseline is 427 full
checks (208 strict), 197 runtime tests and 21 audit regressions. Nineteen new semantic
mutations target the joint transition and invariant; 273 cases are configured in
total. No trusted declarations were added; the library count remains 37.

Next prove the actual clock-difference collector's output-map validity and initial
count, then preserve capture meaning and the joint invariant across history growth.
That discharges the remaining state premises before composing public difference
iteration. Materialization remains a subsequent public protocol boundary.


### Complete construction from clock differences

The actual `Traversal::new` now delegates all work to checked shared code:
collect each nonempty clock-difference interval, union/normalize each chain's
ranges, install unique frontier keys, sum the exact selected-record count and
initialize the joint queues. No pending-state or count premise is supplied by
its caller. Input premises are ordered clocks, bounded `after` chain IDs and
ordered stored chain rows. Normalization preserves the exact requested union;
injective interval-position/node mappings and distinct chain labels establish
cardinality and exclude overflow and frontier collisions.

A generic consumer constructs and drains the actual traversal at a fixed topology,
proving exact union enumeration, uniqueness, causal order and exhaustion. The
existing step theorem gives minimum eligible hash selection. A nonempty witness
proves duplicate differences count and emit once; the runtime regression includes
overlap, gaps, an independent chain, reversed/empty differences and exact counts.
The baseline is 449 full checks (208 strict), 198 workspace tests and 21 audit
regressions. All 22 new semantic mutations are checked locally; 295 are configured
in total. No trusted declarations were added; the count remains 37.

Next compose the public borrowed difference iterator: derive the clock premises
from checked captures, establish its invariant, and verify hash mapping, error
variants, exact size hints and exhaustion. Its History borrow fixes the topology.
Separately prove stability under growth for owned materialization traversals,
then compose materialization visibility and progress publication.


### Public causal difference API

The exported `Ancestry::difference` and `Commits` methods are now checked without
caller validity premises. The constructor rejects either wrong capture identity
and returns the exact difference of the frozen causal hash sets. Captured causal
coordinates retain their chain labels, allowing the guard/lineage proof to derive
current table bounds. The public iterator maintains the actual joint traversal
invariant and projects each selected index to its unique stored hash.

`next` removes exactly one eligible hash and chooses the minimum ready hash;
`len` and both size-hint bounds are exact, and exhaustion is stable. A checked,
erased canonical sequence model supplies the standard Iterator contract. An
arbitrary public consumer using `collect::<Vec<_>>()` proves exact membership,
uniqueness and ancestor-first order. A nonempty proof emits a real captured hash;
runtime regressions include later history growth, dropped capture handles, denied
participation, both wrong-history guards and exact counts through exhaustion.
`Commits` takes its optional cursor into a local before mutation, so an unwind
leaves a valid exhausted object rather than invalid pending/queue state.

The baseline is 482 full checks (208 strict), 200 workspace tests and 21 audit
regressions. All 28 new semantic mutations are checked locally; 323 are configured
in total. There are still 37 trusted library declarations and no application
assumptions. The separate `History::commits_since` wrapper is not yet verified.

This API is causal difference and intentionally ignores permissions. Next prove
owned traversal stability across history extension, then materialization's
policy-filtered visibility, payload mapping, batching and completion publication.
The borrowed `Commits` iterator fixes History for its lifetime; it does not settle
those owned protocol obligations.


### Owned traversal stability (completed component)

`Changes` now uses the checked `OwnedTraversal`. Maintained lineage receipts
record graph and per-chain row prefixes in addition to label/hash coordinates.
Actual Topology initialization, append and metadata bodies preserve that stronger
state. Checked framing lifts the retained pending map, ready queue, blocked
waiters and count across any number of appends without rebuilding them.

The owned cursor's type invariant binds its numeric identity, erased origin,
receipt and scheduler. Arbitrary `next`/`remaining` calls need no validity
premises. Matching histories preserve the captured hash set across growth;
successful steps remove exactly one minimum eligible hash and decrease the exact
count. Exhaustion is stable. Wrong histories leave the cursor unchanged and
return `WrongHistory`. A take/restore transition keeps an exhausted invariant
valid on unwind. All origin snapshots and receipts are erased at runtime.

Generic consumers exercise resumption and real insertion before resumption.
A concrete capture survives a same-chain append, independent root and merge.
Runtime tests compare paused change batches to uninterrupted traversal, including
metadata and completion cursor, and check zero-budget/exhausted identity guards.
The full baseline is 500 checks (208 strict), with 201 workspace tests and 22 audit
regressions. All 21 new semantic mutations are rejected locally; all 344 configured
anchors match, but the full sweep is not claimed rerun. The boundary stays at 37
library declarations, with no application assumptions.

Remaining: discharge `plan_changes_at`'s clock premises; verify actual payload and
visibility mapping, batching and completion publication; then compose
`Materialization::plan`/`complete`/`step`/`update` and Ledger/Document. This component
does not claim those public materialization APIs or the other protocol goals.


### Captured materialization planning (completed component)

The actual `plan_changes_at` entry point now shares its production body with the
harness. It accepts arbitrary typed cursor/view values and captured delivery
heads, resolves all four head lists, compiles actual before/after permissions,
checks required-head coverage and derives the owned traversal's clock-input
premises. Success selects exactly the delivery difference union the symmetric
difference of authorized visibility. The selected set stays below the supplied
captured frontier, even when History already knows later commits. Unknown heads
and insufficient captured coverage have checked error variants.

Actual view/cursor types and their constructors, getters and copies are included.
Permission copies preserve listed rules, fallback and bounded-frontier sequences;
a semantic lemma proves that copied views authorize the same set. The planner
retains exact delivery heads and the target policy, and `Changes::remaining` is
checked. vstd's existing Vec/HashMap copy contracts suffice; there are no new
library declarations or application assumptions. Codec implementations remain
outside this milestone.

Generic consumers have no validity premises. Concrete proofs cover denied new
delivery, a nonempty retraction, unchanged views, unknown heads and missing
coverage. Runtime regressions cover all head sources and coverage guards, plus
missing and out-of-view permission bounds. Validation is 209 strict/530 full
checks, 202 workspace tests, 23 audit regressions, all-features compilation and
28 new mutation rejections. All 372 anchors match; the complete sweep was not
rerun locally. The trusted declaration count remains 37.

Remaining: prove actual emitted payload/visibility mapping and batch/completion
behavior, then compose the public Materialization and Ledger/Document protocols.


### Actual record emission and batch completion (completed component)

The production record step and `emit_next_changes` batch loop are now checked
without caller validity premises. A maintained plan invariant connects captured
delivery and before/after membership sets to their real clocks and History
lineage. After growth, each emitted hash still has the captured new-delivery and
visibility flags, and its payload ID is the current retained node's payload ID.
The proof does not independently snapshot payload bytes or claim secrecy for
denied deliveries.

Every successful batch appends exactly the smaller of the budget and remaining
count, preserves existing output, removes precisely the emitted hashes, and
maintains minimum-ready-hash/ancestor-first order without duplicates. Wrong
History leaves traversal and output unchanged even with zero budget or exhausted
work. Zero budget preserves state. Complete means exhaustion and carries the
saved delivery-head sequence and target policy; repeated completion appends
nothing. Generic consumers compose actual planning and complete draining.
Concrete consumers emit a denied delivery and retraction after actual growth.

The plan adds a retained numeric identity and optional cursor; receipt, snapshots
and membership sets are erased. Cursor ownership transitions preserve an exhausted
invariant on unwind, without a new per-record allocation. These are normal-return
guarantees, not a transaction/recovery proof for partially applied batches.

Validation: 209 strict/542 full checks, 203 workspace tests (two existing ignored
doctests), 24 audit regressions and all-features compilation. All 31 new targeted
semantic mutations are rejected with proof diagnostics; all 403 anchors match,
but the complete sweep was not rerun. A redundant cursor-lineage invariant clause
was removed after its omission still verified; it is not counted as a rejection.
There are still 37 library declarations and no application assumptions.

Next verify the public `Materialization::plan`/`complete`/`step`/`update` lifecycle,
pending/stale/wrong-batch handling and matching application-state/checkpoint
publication through Ledger/Document. Checked internal emission and completion
cursor construction do not yet establish those public protocol guarantees.


### Immutable public batches and completion tokens (completed component)

The actual public Batch representation and accessors/clone are checked. The
logical event sequence preserves every kind, hash, author, ordered parent list,
payload ID and activity flag. The runtime rebuild flag exactly reflects Reset
membership. An erased persistent receipt binds these events to the planner and
ticket. The retained Rust token's event sequence remains private.

The actual Materialization pending slot now lives in the checked module.
Installation rejects overwrite or ticket exhaustion without changing the slot;
success stores the exact supplied pending value and issues the exact events.
Checking/consuming a stale or foreign token leaves the slot unchanged. Successful
consumption returns its pending value exactly once. All clones become stale, and
follow-up work receives a strictly newer ticket. The maximum ticket remains
completable, but it cannot wrap. Planner construction now uses the existing
checked identity allocator, eliminating the former wrapping planner counter.
No issued-event log or proof receipt survives erasure.

Generic consumers prove clone and follow-up invalidation, occupied-slot rejection,
foreign-planner rejection and public observations. Nonempty and saturated-ticket
consumers execute the real implementations. Runtime tests cover the last ticket
and existing public completion scenarios. Validation: 209 strict/565 full checks,
204 workspace tests (two existing ignored doctests), 26 audit regressions,
all-features compilation, formatting and rustdoc links. All 33 new semantic
mutations plus four existing identity mutations reject; all 436 anchors match.
The full sweep was not rerun. The boundary remains 37 library declarations, with
no application assumptions.

Remaining: prove that Materialization constructs the right events and proposed
checkpoint, validates application reports, handles rejection/correction and
reset/replay rounds, and publishes matching progress through Ledger/Document.
The generic pending slot preserves its supplied value without assuming that the
value is a correct checkpoint. The outer plan/complete/step/update lifecycle and
Batch::resolve remain outside the harness.


### Durable checkpoint construction and transitions (completed component)

Actual Checkpoint/Follow/InProgress representations and their copies are now
checked, together with public construction, follow, heads and partial-status
accessors. Logical copies preserve every follow/cursor/rejection/progress field.
Partial heads name the base cursor rather than the uncompleted target.

Materialization delegates checkpoint preparation, replay reset and rejection
insertion to checked production bodies. Preparation changes no published state:
it either records exact target/frontier/count alongside the supplied base cursor,
or installs the supplied completed cursor and clears partial progress. Follow and
rejection state survive both branches. Reset clears cursor/count but retains the
captured target/frontier and permanent rejection state. Rejection insertion only
changes the named hash, never weakens its cascade, and reports exact newness.

An arbitrary consumer composes actual planning and emission with staging and
proves completion metadata and partial/base correspondence. A second composes
proposed checkpoints with the checked token handshake, including changed follow
rules and stale clones. Nonempty examples distinguish old versus target heads
and retain cascades across reset. These witnesses do not verify the entire outer
Materialization lifecycle. A public regression changes follow rules during a
payload round, verifies delayed rejection publication and non-weakening cascades,
and round-trips the resulting checkpoint through the unchanged codec.

Validation: 209 strict/583 full checks, 205 passing workspace tests (workspace
baseline plus updated materialization suite), two existing ignored doctests,
28 audit regressions, all-features compilation, formatting and rustdoc links.
All 30 new semantic mutations reject; all 466 anchors match. The full sweep was
not rerun. There are still 37 library declarations and no application assumptions.

Next prove the source lifecycle's resumed/skipped count and captured completion
correspondence, then connect event translation, report validation, cascading
corrections, follow-up/reset control flow and matching checkpoint publication.
Checkpoint codecs and rejection iteration remain outside the harness.


### Captured source resumption and progress (completed component)

The production Source now maintains processed plus remaining equal to the
captured total. Construction validates arbitrary saved counts and skips exactly
the canonical prefix in chunks of at most 1,024 records. Checked schedule
concatenation and prefix uniqueness show that chunk boundaries cannot affect
the selected suffix. An executable consumer proves exact resumed/uninterrupted
remaining-set and count equality for the same History snapshot.

Advancement checks History identity before mutation, reserves a record-budget
slot for Reset, advances by the exact emitted length without overflow and stores
the matching completed cursor exactly at exhaustion. Captured total, target,
delivery frontier and visibility remain fixed across growth. Source checkpoint
preparation now derives progress and completion metadata from this maintained
state; replay-reset checkpoints keep the captured target/frontier and rejections.
An unwind during owned advancement leaves a poisoned source that rejects reuse.
The total and skipped-prefix witnesses are erased; runtime traversal stays lazy.

Generic consumers cover advancement/checkpoints, replay equivalence, growth and
reset budgeting. Nonempty and malformed-count proofs check actual constructors
and later insertion. Runtime tests cross the 1,024-record replay boundary,
check suffix payloads and counts after growth, repeat exhausted/wrong-history
calls and exercise the poisoned representation. Validation is 209 strict/601 full
checks, 207 workspace tests (two existing ignored doctests), 29 audit regressions,
default/all-features compilation, formatting and rustdoc links. All 34 new
mutations reject; all 500 anchors match. The complete sweep was not rerun. The
boundary remains 37 library declarations and no application assumptions.

Remaining: outer Ledger/checkpoint input selection, record-to-event translation,
application-report validation, rejection/correction/reset control flow and final
matching publication. Same-snapshot replay equivalence does not yet establish
equivalence across reconstructed graphs or captures recompiled after growth;
continuation of an already captured source across growth is checked separately.


## Completed: actual event translation and rejection filtering

The production Materialization planner now uses a checked translator for the
captured batch. Known hashes resolve to the actual author and complete ordered
parent sequence, including repeats. A checkpoint rejection applies directly or
through a saved cascade root's strict ancestry, using the verified History query.
New deliveries retain their metadata and payload IDs, with activity exactly when
after-visible and not rejected. Old records yield the exact non-rejected
Activate/Retract transitions; unchanged or rejected records are suppressed. Reset
precedes the sequence. A checked pass derives counts, delivered hashes and the
ordered active-new-delivery list passed to pending work.

A checked consumer composes real Source advancement, translation, staging and
Protocol issuance. Every activation in the resulting Batch belongs to the
source's frozen authorized set and passes rejection filtering. This derives the
translator's known-hash premises from actual emission, without application axioms.
A concrete checked witness covers all visibility labels and rejection decisions.
Runtime regressions cover cascades across a merge, an unaffected branch, repeated
parent order, pinned-view transitions and completion after suppressed events.

Validation: 209 strict/619 full checks, 209 workspace tests (two existing ignored
doctests), 31 audit regressions, default/all-features compilation, formatting and
rustdoc links. All 33 new mutations and 10 existing collector cases reject; all 533 configured
anchors match. The complete sweep was not rerun. The trusted boundary stays at 37 contracts.

Remaining: prove outer Ledger/checkpoint input selection and compose the public
Materialization lifecycle, including validation of application reports, correction
rounds and matching application-state/checkpoint publication. Cross-reconstruction
replay equivalence and the separate compaction/convergence/attestation goals remain
open. These normal-return event guarantees do not establish panic recovery or
application-state correctness.


## Completed: application report admission and rejection recording

The actual Pending, Applied and Rejection types now live in checked code. Pending
construction preserves the exact staged checkpoint and translated summaries.
Public Applied default/want are checked without restricting arbitrary report values.
The actual report gate checks the batch token, distinct rejection membership in the
pending source delivery set and existence of wanted hashes before consuming work.
Errors preserve the full Protocol state and have explicit precedence. Success
preserves report details, returns the exact deduplicated wanted set and consumes the
matching token once. Original source deliveries remain eligible for rejection in
immediate follow-up rounds; payload requests may name any hash known to History.

The production rejection-recording helper now proves exact map union, non-weakening
cascade OR, exact counts of distinct newly rejected hashes, and unchanged follow,
cursor and partial progress. Checked consumers compose actual Source/emission,
translation/summary, Pending construction, token issuance, report acceptance and
recording, and prove malformed-feedback retry and one-time consumption. Nonempty
proof/runtime witnesses cover duplicate wanted hashes, repeated rejection records,
follow-up cascade upgrades and delayed checkpoint publication.

Validation: 209 strict/636 full checks, 212 workspace tests (two existing ignored
doctests), 33 audit regressions, default/all-features compilation, formatting and
rustdoc links. All 34 new mutations reject; all 567 configured anchors match. The
complete sweep was not rerun. The trusted boundary remains 37 contracts.

Remaining: outer Ledger/checkpoint input and identity validation, cumulative
counter updates, correction rounds and matching application-state/checkpoint
publication through the public Materialization lifecycle. These helper guarantees
are for normal return; they do not prove recovery after token consumption, the
generic reason-conversion builder, report clone implementations, cross-reconstruction
replay equivalence or the separate compaction/convergence/attestation goals.


## Completed: correction generation and captured restart

The production correction generator now proves the exact ordered sequence of
retractions followed by requested payloads. It retains active source order, skips
directly rejected hashes and remembered corrections, and selects every remaining
active delivery affected by a cascade. Actual source emission and translation
establish that the initial active-delivery list is unique; the composed consumer
therefore derives unique retractions. With unchanged History and checkpoint,
remembering the first round's corrections makes the next round's retraction list
empty. Later feedback may legitimately add new cascades.

Payload requests produce exactly the deduplicated wanted set, in the existing
vstd BTree/Ord model's order, with each actual stored payload ID. The report gate
supplies the known-hash premise. This does not add a proof of the derived bytewise
Ord implementation, nor restrict requests to the captured authorized view: any
known hash remains requestable, so this is not a payload-confidentiality guarantee.

If affected deliveries exist and visibility changes are unsupported, the generator
returns only Reset, suppresses payload requests, leaves the corrected set unchanged
and reports zero retractions. The actual Pending restart transition preserves
rejections, delivered/corrected sets, follow rule and numeric counts; it clears
active deliveries, sets restart/rebuilt and stages the original captured target and
delivery frontier at progress zero. A poisoned source returns an error with Pending
unchanged. This local error guarantee does not restore a token already consumed by
the outer completion method.

Checked consumers compose real source emission, event translation, report admission,
rejection recording, correction planning and nonempty follow-up token issuance. The
old token becomes stale and the new token carries the exact events and pending work.
A concrete two-node witness covers a nonempty cascade, stored payload and Reset.
Runtime tests cover direct rejection exclusion, deduplicated payload ordering,
repeated rounds, delayed checkpoint publication and captured replay after growth.

The outer source/History input and identity guards, cumulative counter additions,
and final application-state/checkpoint publication remain unverified. These are
normal-return guarantees; allocation failure, unwind recovery and application
behavior are not proved. Compaction, sync minimality, attestation completion and
replay equivalence across reconstructed graphs remain separate open goals.

Validation: 209 strict/655 full checks, 214 workspace tests (two existing ignored
doctests), 35 audit regressions, default/all-features compilation, formatting and
rustdoc links. All 30 new mutations reject; all 597 configured anchors match. The
complete sweep was not rerun. The trusted boundary remains 37 contracts.


## Completed: public Materialization values and history binding

The harness now contains the actual exported `Materialization` type. Its public
`new`, `resume`, `checkpoint`, `follow`, `set_follow`, `heads` and `is_dirty` bodies
are checked without caller validity premises. Resume retains the exact supplied
checkpoint and visibility capability, while creating an unbound, clean, unpoisoned
driver with no source or pending work and zero counts. A fresh protocol cannot
accept an old batch token. Changing follow preserves the current capture, pending
work, completed cursor, rejections and partial progress; `heads` still names the
last completed view.

The production `check_history` guard used by planning, completion and the in-memory
driver is checked against the actual History identity accessor. Poisoning wins
over foreign-history errors; foreign-history errors win over missing references.
On initial binding, all completed delivery heads, completed view heads, pinned
heads and rejected hashes must exist. An unknown-reference error carries an actual
missing hash from those sources. Every returned guard error preserves the entire
Materialization value, including its owner and pending protocol; success changes
only an absent owner to the supplied History ID. Repeated checks on that bound
History preserve all state and do not revalidate changed checkpoint references.

Binding deliberately defers saved partial-traversal heads/counts to Source
construction. Successful binding can precede an error in the outer method: for
example, `plan` with zero budget retains its newly established owner. The guard's
unchanged-on-error guarantee must not be read as a transaction guarantee for the
whole public operation. The Ledger argument extraction, source-input selection,
visibility-reset decision, cumulative counters, complete lifecycle composition and
final application-state/checkpoint publication remain unverified.

Consumers prove arbitrary guard outcomes, retry after rejected binding, pending
token preservation under follow changes, independent missing-reference roles,
deferred partial validation, fresh-token isolation and a nonempty binding. Public
regressions cover retry, identical-but-foreign graphs, foreign completion without
token loss and error precedence. No application or library assumptions were added.

Validation: 209 strict/680 full checks, 217 workspace tests (two existing ignored
doctests), 37 audit regressions, default/all-features builds, formatting and rustdoc
links. All 30 new mutations reject; all 627 anchors match. The complete sweep was
not rerun. The trusted boundary remains 37 contracts.


## Completed: source selection, visibility reset and installation

The production follow-target helper now selects the exact stored delivered heads
for Working, stored admitted heads for Signed, or the exact caller pin (including
order and duplicates), and copies the supplied policy. Collected head sets are
unique and increasing under the existing vstd BTree/Ord model. This does not add an
authentication invariant for admitted heads or a proof of the derived bytewise Ord
implementation. `Ledger::view` and source startup use this same checked helper.

The actual visibility-change decision proves `before != after ∩ delivered`, where
all three sets are interpreted against the supplied History. Thus new deliveries
alone do not force reset, while changes to previously delivered participation do.
The formula deliberately leaves `before` unmasked: arbitrary stored cursors may
name a prior view outside their prior delivery. Unknown heads return an actual
missing requested hash. Permission proofs now establish that each clock endpoint
is a real node on its coordinate's chain. The production clock equality iterator
is checked, including its length comparison; for valid clocks it compares exact
maps independently of tree shape. These facts make map equality equivalent to
membership equality without adding a trusted application fact or changing the
linear comparison algorithm.

For interrupted traversals, input selection retains the saved base, target policy,
delivery frontier and processed count, with no second reset. Current follow rules,
policy and later deliveries do not replace that capture. Fresh traversals use the
current follow target and delivery frontier, start at zero, and use an empty base
exactly when visibility changes are unsupported and the comparison requires reset.

The actual source-installation helper composes these inputs with `Source::new`.
Success installs precisely that validated capture and its canonical remaining
selection, changing no other Materialization field. Errors are either an input
validation failure or a constructor error for those selected inputs; the entire
previous Materialization, including any previous source, remains unchanged. A
checked consumer connects startup to source emission, authorized event translation,
checkpoint staging and fresh token issuance. Nonempty witnesses cover reset,
new-delivery-only changes, equal clock maps with different insertion orders, and
successful partial startup; invalid saved counts preserve the previous source.

The remaining boundary is Ledger argument extraction, the outer `is_current` and
`plan`/`complete`/`step`/`update` control flow, cumulative counters and matching
application-state/checkpoint publication. Helper error guarantees do not imply
rollback of earlier outer-method transitions. These are normal-return guarantees;
application behavior, panic recovery and replay equivalence across independently
reconstructed graphs or recompiled captures remain open.

Validation: 209 strict/701 full checks, 220 workspace tests (two existing ignored
doctests), 40 audit regressions, default/all-features builds, formatting and rustdoc
links. All 34 new mutations reject; all 661 anchors match. The complete sweep was
not rerun. The trusted boundary remains 37 contracts.


## Completed: public planning and current-state detection

The actual public `Materialization::plan` and `is_current` bodies are in the
harness, with **no caller validity preconditions**. The proof includes the actual
`Ledger::history`, `permissions` and `view` accessors. Ledger's storage is generic
in its untouched receive-options and queue fields, with the existing concrete
production types as defaults. The same executable methods are checked for all
values of those fields; there is no replacement Ledger or assumed accessor.
Ledger construction, admission and queue behavior are not thereby verified.

`is_current` returns true exactly when the driver is unpoisoned, has no pending
batch or partial checkpoint, has no foreign binding, and its completed delivery
and view match the ledger and follow rule. Equality is structural: explicit
policy entries, fallback rules, bounded-frontier order/multiplicity and pinned
head order/multiplicity all matter. Working/signed heads retain the existing
BTree/Ord model. Actual permission, view and cursor equality bodies are checked.

`plan` preserves the published checkpoint, dirty flag, last completed counts and
visibility capability on every returned outcome. A successful batch has a fresh
accepted token and at most the supplied budget of events, including Reset. Its
records are the exact canonical prefix of the retained or newly selected source;
progress counts and remaining selection agree. The event sequence is the exact
translation of those records. Every activation is in the source's captured
authorized set and passes the actual direct/cascade rejection query. Pending
checkpoint metadata, event counts, delivered hashes and active-delivery order
match that batch. `Ok(None)` means the driver is current and has no source.
An empty `Some(batch)` remains meaningful: it can stage a view/policy change
without application events, and still needs acknowledgment.

The proof checks error precedence and origins: poisoning/history/reference checks
precede zero budget, which precedes pending-work rejection. Guard failures leave
the whole driver unchanged. Budget/pending errors retain any successful initial
history binding. Startup errors can leave an old finished source discarded; ticket
exhaustion can occur after source advancement. Every returned error preserves the
Protocol and its pending token, and never publishes a checkpoint. This is not a
whole-method rollback guarantee.

[plan_examples.rs](plan_examples.rs) calls the actual public methods with arbitrary
Ledger field types, checks the returned authorization/budget/frame contracts, and
constructs a nonempty History through checked insertion to prove an actual active
delivery. This fixture does not prove public commit admission. Runtime regressions
cover structural policy changes, reordered/repeated pins, empty checkpoint batches
and concrete-Ledger function-pointer compatibility.

`complete`, `step`, `update`, cumulative feedback counters and publication of
matching application state/checkpoint remain open. This milestone proves planning
and event authorization, not the effects of executing those events. Compaction,
sync minimality and attestation completion remain separate goals. The trusted
boundary remains 37 library/Rust contracts, with no new application assumptions.

Validation: 212 strict/727 full checks, 224 workspace tests (two existing ignored
doctests), 40 audit regressions, all-features build, formatting and rustdoc links.
All 33 new semantic mutations reject with proof diagnostics; all 694 anchors match.
The complete mutation sweep was not rerun. The trusted boundary remains 37 contracts.


## Completed: public feedback completion and checkpoint publication

The actual public `Materialization::complete` body now runs in the harness with
**no caller validity preconditions**. Together with `plan` and `is_current`, this
checks the state-independent batch/feedback protocol through checkpoint publication.
It does not prove application effects or persistence of that checkpoint.

After history binding and report validation, completion copies the latest follow
rule, merges rejection hashes/cascade flags, and adds exactly the number of newly
recorded hashes. Correction selection uses that updated rejection map. Retracted
counts increase by exactly the generated correction count. Repeating an already
recorded rejection adds zero, including across immediate follow-up rounds.
Delivered/activated counts remain those of the original source batch.

Follow-up events are precisely the selected retractions and requested stored
payload IDs, or Reset alone when visibility changes are unsupported. They contain
no activation events. The fresh token holds the updated pending checkpoint and
counts; the published checkpoint and last completed counts remain unchanged.
An unsupported cascade stages the original source target/frontier at progress zero,
clears active deliveries, and retains rejections and the restart flag. When no
follow-up events remain, completion publishes that pending checkpoint and its exact
counts together, clearing the source precisely when restart was staged. Follow
changes are preserved without replacing an interrupted traversal's captured target.
Payload requests still permit any known hash; this is not payload confidentiality.

The error contract separates retryable validation failures from failures after
acceptance. Poisoning/history/reference checks precede token validation, which
precedes rejection validation and unknown payload requests. Guard failures preserve
the entire driver; invalid token/report errors preserve pending work, apart from
any newly established history binding. After report acceptance, a failure consumes
the token while preserving the published checkpoint, last counts and source.
Restoring application state and checkpoint together is required before continuing
from those later failures. Successful completion also makes the submitted token
and its clones stale; follow-ups receive a strictly later ticket.

Two production failure paths are now explicit: overflowing rejection/retraction
counts return `Error::Exhausted`, and a requested reset with no stored source returns
`Error::Poisoned` instead of an unchecked unwrap. Existing poisoned-source and ticket
exhaustion errors retain their ordering. The contracts identify the exact arithmetic,
source or ticket condition for those failures; they do not assume those conditions
are unreachable. Proving their absence for every reachable API interleaving remains
an additional lifecycle invariant obligation. There are no new application or
library assumptions.

[complete_examples.rs](complete_examples.rs) checks arbitrary public outcomes,
invalid-report token preservation, poisoning precedence, repeated-rejection counts,
and successful acknowledgement when no rejections need correction. The latter
requires no unused ticket or source: even maximum counters accept zero increments.
The nonempty [plan_examples.rs](plan_examples.rs) witness now calls `plan`, proves
an actual active delivery, calls `complete`, verifies the published heads/counts,
and proves `is_current` returns true. Its History comes from checked insertion;
this still does not prove public commit admission. Runtime regressions exercise
multiple feedback rounds and deliberately injected extreme internal counter/source
states without enormous allocations.

Remaining: `step`/`update`, aggregate progress counters across source batches,
application callbacks and matching application-state/checkpoint persistence,
`mark_local`/`change`, and general lifecycle reachability/termination properties.
Compaction, sync minimality, attestation completion, and replay equivalence across
independently reconstructed graphs remain separate open goals.

Validation: 212 strict/737 full checks, 229 workspace tests (two existing ignored
doctests), 40 audit regressions, all-features build, formatting and rustdoc links.
All 32 new semantic mutations reject with proof diagnostics; all 726 anchors match.
The complete mutation sweep was not rerun. The trusted boundary remains 37 contracts.

## Payload resolution now preserves the verified event envelope

The actual public `Event::map_payload`, `Event::try_map_payload` and
`Batch::resolve` bodies now run in the harness. Mapping preserves each variant,
hash, author, parent sequence and `active` flag. Batch resolution preserves event
length and order without consuming or changing the completion token. Payload
results satisfy the supplied callback contract; fallible event mapping returns
its original error value. The proof requires callback preconditions only for
payloads present in the input. It does not verify arbitrary callback side effects.

The harness now contains the actual `Document` representation and its public
`ledger`/`history` accessors. Codec, receive options, queue, indexes and staged
bundles retain their production representation; byte reads assume nothing about
those untouched fields. Like Ledger, Document uses default type parameters for
its real receive options and queue, letting checked reads be generic over them.
Construction and mutation still require the production Codec implementation and
remain outside the proof.

The checked internal `payload_by_id` returns exactly the stored slot, or
`Error::Storage` exactly when the full ID is out of range. It checks before casting
to `usize`, preventing large IDs from wrapping to another slot on narrower
platforms. No premise claims that raw Ledger payload IDs exist in Document.
`resolve_batch` succeeds exactly when all referenced slots exist, preserving every
event and resolving both deliveries and requested payloads to their stored bytes.
A failure returns an error, never a partially resolved event vector. The actual
`step` now calls this shared checked resolver before invoking the application.

[resolution_examples.rs](resolution_examples.rs) composes public `plan` with
byte resolution and proves that resulting activations remain in the captured
authorized set and outside direct/cascade rejection. Concrete nonempty and
missing-storage witnesses check exact fields, a hidden delivery, repeated payload
references, token preservation, and successful token consumption after resolution.
Runtime regressions cover stateful resolvers, non-Clone callback captures/errors,
and a missing payload after a valid delivery: the application receives no batch,
the saved checkpoint remains published, and the driver becomes poisoned.

This closes the payload-conversion boundary; **`step`/`update` as complete methods
are still outside the proof**. Callback sequencing, checkpoint publication and
failure recovery around those callbacks, cumulative progress, and general
lifecycle invariants remain next. There are no new application axioms or explicit
library assumptions. Slot identity is not cryptographic payload authentication or
payload secrecy; inactive deliveries may still carry bytes.

Validation: 212 strict/753 full checks, 232 workspace tests, 44 audit regressions,
all-features build, rustdoc links and formatting pass. The existing large streaming
regression and one doctest remain ignored. All 26 new semantic mutations reject
with proof diagnostics, including truncation before bounds checking; all 752
configured anchors match. The complete mutation sweep was not rerun. The explicit
trusted boundary remains 37 library contracts.

## Public `step` now checks the in-memory driving protocol

The actual exported `Materialization::step` body is checked for arbitrary typed
Document, driver and Materializer values, without caller validity preconditions.
The proof uses the real Materializer trait, Step and DriveError types. Existing
named `step::<Codec, State>` calls and concrete function-pointer uses remain valid;
reads are parametric in Document's untouched codec/receive/queue fields.

Preflight checks history binding, positive budget and outstanding work in that
order. Errors here make no application calls and preserve the driver apart from
successful initial history binding. A successful preflight resets the last-step
counts, then invokes the checked public planner with the supplied budget.

Each source/follow-up round resolves the whole batch before calling the actual
`Materializer::apply` method with that Document's History and those exact byte
events. Its actual report is passed to public `complete`. An erased finite round
relation connects the application state and planner state across every call and
returned report. All callback activations belong to the first batch's activation
set; that set lies inside the captured authorization and outside direct/cascade
rejection. Completion follow-ups cannot add activations. Inactive deliveries may
still carry metadata and bytes: this remains visibility control, not secrecy.

Only after all follow-ups finish does the driver call the actual checkpoint hook,
with the completed checkpoint, and only if it planned work or was already dirty.
Successful checkpointing clears dirty; a clean current driver makes neither
callback. On success no token remains pending, and Step::Current is returned
exactly when the checked `is_current` predicate holds. Otherwise it returns More.

A returned error after preflight restores the original checkpoint's logical value
and poisons the driver. Apply and checkpoint errors retain their original generic
value and distinct phase. The rest of the planner remains at its failed transition:
there is no promise to restore consumed tokens or source progress. The application
can remain mutated, including after a failed checkpoint hook. Preflight errors do
not newly poison it. The feedback loop has a checked decreasing measure: every
follow-up consumes a fresh ticket, and the last round removes the pending batch.
It finishes or returns an exhaustion error when callbacks return. Termination of
application callbacks and panic/unwind recovery are not proved.

Application behavior is not an axiom. The callback relation uses Verus's contract
for the actual invoked method, including a checked implementation's stronger
postconditions. The trait adds no general application-effect or durability law;
its optional default checkpoint body is checked separately. The production Error
Display bound is unused by driving and therefore contributes no proof assumption.
The explicit trusted boundary remains 37 library contracts.

[step_examples.rs](step_examples.rs) checks arbitrary public outcomes, preflight
framing, exact errors from a checked materializer that mutates before failing,
absence of apply calls when no batch exists, and checkpoint success/failure on a
current driver. The nonempty public plan/complete witness now also invokes public
step to checkpoint its completed view. Runtime regressions exercise repeated
payload feedback before a single checkpoint, clean-current no-ops, function-pointer
compatibility, and an apply failure after an earlier successful round.

Remaining: the public `update` loop and cumulative progress counters, `mark_local`
and `change`, general lifecycle reachability, and proofs tying a concrete
application's effects and durable storage to the checkpoint. Compaction, sync
minimality and attestation completion remain separate open goals.

Validation: 212 strict/776 full checks, 234 workspace tests, 48 audit regressions,
all-features compilation, rustdoc links and formatting pass. The two existing ignored
tests are unchanged. All 36 new semantic mutations reject with proof diagnostics,
including skipped callbacks, wrong arguments, discarded feedback, premature
checkpointing and missing rollback/poisoning; all 788 configured anchors match.
The final mutation run includes the checked decreasing ticket measure. The complete
mutation sweep was not rerun. The explicit trusted boundary remains 37 contracts.

## Public `update` now composes verified steps

The actual exported `Materialization::update` body is checked for arbitrary typed
Document, driver and Materializer values, with no caller validity preconditions.
It calls the checked public `step` with budget 1024 until Current or an error.
An erased finite trace connects the exact driver and application state between
every successful More step and the final call. Each edge uses the actual step
contract, including its callback arguments, feedback, captured activation bound,
checkpoint hook and error handling. No application behavior is assumed.

On success the driver is current, clean, unpoisoned and has no pending batch.
`Progress::heads_before` is exactly the input checkpoint's completed-head sequence;
`heads_after` is exactly the final checkpoint's sequence. All four counters are
the mathematical sums of the completed steps' counters, including the final
Current step. `rebuilt` is their logical OR. Counts describe event activity,
including replay, rather than unique commits or a difference between head sets.
Public Progress Default, Clone and `changed` are checked too; a checkpoint-only
update can report no activity.

The previous unchecked counter additions now use checked arithmetic. Aggregation
computes every sum before changing the report. If any total exceeds `usize::MAX`,
update returns `DriveError::Stockfoil(Error::Exhausted("materialization progress"))`.
This happens **after a successful step**: its checkpoint and application effects
remain, its driver is clean and unpoisoned, and no total is silently wrapped or
saturated. The method returns no partial Progress on an error.

If a step itself fails, update returns that exact error and preserves that step's
recovery boundary. Earlier successful steps and checkpoints remain completed.
The failing step restores its entry checkpoint and poisons after successful
preflight; preflight failures make no application calls. Application mutations
from a failing callback are not rolled back. Restoring application state and the
matching checkpoint remains the application's responsibility.

The proof strengthens `step` to establish that More strictly advances its ticket
sequence. Update's loop decreases the remaining ticket range; the checked caller
also bounds the number of preceding More steps by that range. This proves driver
loop progress when callbacks return, not callback termination or panic recovery.
The explicit boundary is still 37 library/Rust contracts, with no application axioms.

[update_examples.rs](update_examples.rs) checks arbitrary public outcomes, exact
aggregate totals and error boundaries, ticket progress, and concrete arithmetic,
clone, no-op and overflow witnesses. Runtime regressions cross the 1024-record
budget with 1025 commits, include an immediate payload follow-up, check both
partial and final checkpoint hooks, and confirm a clean-current no-op. A later
apply failure leaves the earlier partial checkpoint usable after application
restoration. Separate tests check atomic overflow in each counter and rebuild
accumulation. Named `update::<Codec, State>` and function-pointer calls remain valid.

Next are `mark_local`/`change`, general lifecycle reachability and a checked
application's state/checkpoint correspondence. Durable storage, compaction, sync
minimality and attestation completion remain open goals.

Validation: 212 strict/791 full checks, 238 workspace tests, 52 audit regressions,
all-features compilation, rustdoc links and formatting pass. The existing large
streaming test and one doctest remain ignored. All 34 new semantic mutations
reject with proof diagnostics; all 822 configured anchors match. These mutations
cover dropped/misadded counts, lost rebuilds, partial mutation on overflow,
incorrect loop stopping/budget, missing or duplicated aggregation, heads, error
handling and contradictory callers. The complete mutation sweep was not rerun.
The explicit trusted boundary remains 37 contracts.

## Public `mark_local` now checks local acknowledgement

The actual exported `Materialization::mark_local` body is checked for arbitrary
typed driver, Ledger and Draft values, without caller validity preconditions.
The actual Draft representation is shared with the harness; it carries no assumed
publication or authenticity invariant. The method acknowledges application work
that the caller says has already happened. It neither applies events nor proves
that the application actually reflects the draft.

The acceptance rule is precise: history binding must succeed, no batch or partial
checkpoint may be outstanding, and the follow rule must not be Signed. The draft's
hash must name a stored record with the exact author and ordered parent sequence,
including repetitions. Separately, its parent **set** must equal the checkpoint's
completed-head set, current permissions must equal the saved policy, and the draft
must be the only delivered commit outside the causal past of saved delivery heads.
Those heads must exist. Finally, the draft must participate in the ancestry rooted
at itself under current permissions and pass both direct and cascade rejection.

Success advances delivery progress to the current delivered heads, installs the
view selected by the follow rule and current policy, clears the old source, and
marks the checkpoint dirty. Working remains Working; a pin moves to exactly the
new draft. The driver is current and unpoisoned, has no pending batch or partial
checkpoint, and preserves rejection records, protocol identity/tickets, support
flags and last-step counts. No materializer callback occurs in acknowledgement.
A subsequent ordinary step can checkpoint that already-applied state without
redelivering the draft.

Errors follow the production guard order: binding, outstanding/partial work,
signed-only view, metadata mismatch, unknown delivery heads, stale parent/delivery/
policy state, then nonparticipation or rejection. Every error preserves the entire
driver apart from successful initial history binding. It does not newly poison
the driver or consume a pending token. The successful-caller proof establishes
that satisfying all acceptance conditions returns Ok, not merely that Ok implies
them. These are normal-return guarantees; panic recovery is not claimed.

The supporting public `History::commits_since` wrapper is now checked too. It
returns all stored commits outside the supplied heads' causal past, irrespective
of permissions, or an actual unknown-head error. Its real Commits iterator retains
exact size, no duplicates and canonical ancestor-first order. Public Ledger
`commits_since`, `ancestry`, `author_of` and `parents` now have checked bodies and
contracts. Parent queries preserve order and repetitions; missing metadata returns
None. These query proofs establish stored metadata, not cryptographic authorship.

[local_examples.rs](local_examples.rs) checks arbitrary outcomes, acceptance,
poisoning precedence and collection through the standard iterator. A nonempty
fixture uses checked topology insertion, acknowledges an actual draft, proves the
pin current and invokes the real checkpoint hook without an apply callback.
Runtime regressions cover exact/repeated metadata, changed policy, additional
deliveries, denied and signed views, pending tokens, partial progress, retry after
errors, and `mark_local` followed by an update with no repeated delivery. Existing
concrete-Ledger function-pointer calls remain valid.

Next is `change`, especially its edit callback and commit-publication dependency.
`Document::commit`/`Ledger::commit` and admission are not assumed correct and remain
outside this proof. General lifecycle reachability, concrete application-state/
checkpoint correspondence and durable storage remain open, as do compaction,
sync minimality and attestation completion. The trusted boundary stays at 37
library/Rust contracts, with no new application assumptions.

Validation: 212 strict/807 full checks, 244 workspace tests, 57 audit regressions,
all-features compilation, rustdoc links and formatting pass. The existing large
streaming test and one doctest remain ignored. All 36 new semantic mutations
reject with proof diagnostics; all 858 configured anchors match. Mutations cover
causal clocks, exact metadata, guards, policy and rejection, extra deliveries,
pin/checkpoint publication, dirty/source state and contradictory callers.
The complete mutation sweep was not rerun. The former wrapper-only rejection
audit now checks for an unchecked rejection helper; semantic bypasses in the
checked local method are caught by the mutation suite. No new trust was added.

## Public prepared-commit publication is now checked

The harness imports the actual `Prepared` and `Delivered` representations and the
exported `History::publish_commit`, `Ledger::commit`, `History::epoch` and
`Ledger::author` bodies. There is no new application or library assumption.
`Prepared` has no assumed hash/authenticity invariant; its hash remains publicly
mutable. The contract concerns publication of the supplied metadata.

For a new hash, History checks parents in their supplied order, returns the first
missing parent, then checks epoch exhaustion and topology capacity. Success stores
exact author, ordered parents including repetitions, payload ID and leading-zero
byte depth; adds the node to drafts; advances epoch once; and preserves admitted
heads, signatures, signed drafts and inventory. Existing nodes and parent storage
remain intact, the causal state extends the old state, and history identity and
capture lineage are preserved. The existing topology proof supplies exact head
updates and validity. The shared executable depth calculation is now checked too.

A duplicate succeeds before parent or capacity checks, changes nothing and returns
the original payload ID. Submitted metadata need not match the existing record.
Ledger first rejects a different local author, then delegates publication while
preserving options, queue and policy. Its successful Draft describes the **submitted**
metadata, even for a conflicting duplicate; it does not assert that this Draft
matches storage. The separately checked `mark_local` validates that correspondence.
All returned History/Ledger publication errors preserve their entire prior value.

Epoch and topology-capacity exhaustion now return `Error::Exhausted` instead of
panicking. In particular, epoch exhaustion is detected before inserting a node.
This is a normal-return guarantee; allocation success and unwind recovery are not
proved, and the unchanged admission/signing paths retain their own counter logic.

The actual internal `Document::publish_prepared` helper is also checked. A new hash
registers the exact bytes at the previous vector length and records that payload
ID; an existing hash reuses the original slot and bytes. Registration happens
before Ledger publication, so a later error retains registered bytes and the index
entry while preserving Ledger. This matches the previous public path. Its contract
does not assume index completeness, payload availability or a hash/bytes identity
relation. A defensive payload-ID exhaustion check preserves the whole Document.

[publication_examples.rs](publication_examples.rs) checks arbitrary callers,
wrong-author rejection and successful root/child publication with repeated parents
and duplicate payload retention. [local_examples.rs](local_examples.rs) now obtains
its nonempty Draft from public `Ledger::commit`, then proves `mark_local` makes the
pin current and calls the real checkpoint hook without applying the draft again.
Runtime tests cover epoch boundaries, error precedence, duplicate metadata and
bytes, retries after failed publication, and every leading-zero depth position.

`CommitBuilder`, BLAKE3 identity construction and the complete public
`Document::commit` wrapper remain outside the harness. Next compose `change`'s
actual edit callback with preparation, storage/publication and acknowledgement,
then connect a checked application's state to its checkpoint. General lifecycle
reachability, durable storage, compaction, sync minimality and attestation
completion remain separate open obligations.

Validation: 212 strict/825 full checks, 250 workspace tests, 59 audit regressions,
all-features compilation, rustdoc links and formatting pass. The two existing
ignored tests remain unchanged. All 32 new semantic mutations reject with proof
diagnostics, as does the existing contradiction check for the updated nonempty
commit/acknowledgement/checkpoint composition. All 890 configured anchors match;
the complete mutation sweep was not rerun. The public wiring manifest now also
pins Prepared's re-export and the shared depth dispatch. No trust was added.

## Local edit phases now check callback effects and poisoning

The actual public `Materialization::change` now delegates to two checked production
helpers, `begin_edit` and `finish_edit`, around its existing Document commit call.
The wrapper itself remains outside the harness. This milestone does not claim an
end-to-end proof of `change` or of BLAKE3 identity construction.

`begin_edit` checks history binding first, then currentness, then Signed follow,
then the local author's effective write permission. Only Unbounded permits an
edit: an explicit rule overrides the unlisted-author fallback as usual. Each
failure preserves application state and all driver fields except successful
initial history binding, without newly poisoning the driver. The contract captures
the exact error precedence, including stale state preceding Signed rejection.

After those gates it invokes the actual supplied `FnOnce(&mut M)` callback. The
contract records the initial/final application state and original callback result
through Verus's native `call_ensures`; it does not introduce a separate assumed
application law. A verified caller must establish the callback's own preconditions
only when the gates admit the call. A rejected-caller proof even uses a callback
with an impossible precondition, proving it cannot be invoked on that path.
Normal-return guarantees exclude callback termination, panic recovery and rollback.

Returned bytes are preserved exactly. `Ok(None)` and `Ok(Some(bytes))` preserve the
driver apart from history binding, including its existing dirty flag, source,
checkpoint, protocol and counters. `None` does **not** assert that the application
state is unchanged: the caller's callback may mutate it without producing a commit.
An edit error is returned unchanged as `DriveError::Edit`, preserves those driver
fields and sets poisoned. No materializer apply/checkpoint callback is invoked.

`finish_edit` propagates a Document publication error unchanged and poisons the
driver. On a published Draft it calls the checked public `mark_local`; success
returns exactly that Draft and inherits its current/dirty, metadata, authorization
and checkpoint guarantees. A failed acknowledgement preserves its exact error and
then poisons the driver. This phase does not undo the edit or document publication.
The failure may follow a duplicate publication that succeeds but provides no sole
new delivery for acknowledgement.

The checked parent-validation loop from public publication is now also used by
`History::begin_commit`, preserving first-missing-parent precedence and parent
order/repetitions. The builder and hash calculation remain unchecked; sharing this
loop does not by itself prove the complete public preparation API.

[edit_examples.rs](edit_examples.rs) checks arbitrary rejection, success with no
bytes, exact bytes, exact edit errors, publication failures and acknowledgement.
A ready fixture is constructed from actual empty History, captured delivery heads,
policy and checkpoint values, rather than assuming readiness. Checked callback
implementations prove observable application effects. Runtime public `change`
tests cover one invocation, mutation followed by None/error, poison blocking retry,
stale-before-Signed precedence, and failed acknowledgement of a duplicate after
editing. The existing successful change test still checks no echoed application.

The next gap is preparation/hash construction and its composition with these
phases and verified payload publication in the public wrapper. The trusted boundary
remains 37 library/Rust contracts. General lifecycle reachability, concrete
application-state/checkpoint correspondence and durable storage remain open;
compaction, sync minimality and attestation completion remain separate goals.

Validation: 212 strict/839 full checks, 254 workspace tests, 61 audit regressions,
all-features compilation, rustdoc links and formatting pass. The two existing
ignored tests remain unchanged. All 26 new semantic mutations reject with proof
diagnostics; all 916 configured anchors match. The complete mutation sweep was
not rerun. Mutations cover skipped/reordered gates, permission lookup, omitted
callback calls, discarded bytes/errors, premature dirty/source/rejection changes,
poisoning, skipped acknowledgement and contradictory consumers. The old wrapper
history-guard audit now rejects an unchecked binding helper; the checked edit
phase's semantic guard bypass is covered by a mutation. No trust was added.

## Commit preparation now preserves checked metadata

Both public `History::begin_commit` and `Ledger::begin_commit` now use checked
metadata preparation before initializing the existing BLAKE3 builder. History
preserves the requested author and exact parent sequence, rejecting the first
missing parent. Ledger uses its actual local author. `Some(parents)` retains every
parent in order, including repetitions; `Some([])` starts a root. `None` captures
all delivered heads, including drafts, in the existing BTree order without
duplicates. A checked topology lemma derives that every delivered head is known,
so default preparation always succeeds on normal return.

The actual `CommitBuilder` stores the new private `CommitMetadata` value instead
of separate author/parent fields. Its checked accessors and clone preserve those
fields exactly. After the existing hash calculation, a checked consuming method
attaches the supplied digest while preserving author and ordered parents in the
actual `Prepared`. This contract accepts any digest and does not assume that it
matches the metadata or payload. Later history growth does not change the owned
metadata. No public signatures or commit encoding were changed.

The actual public `Prepared::signing_bytes` body is now checked too: it returns
exactly the current hash's 32 bytes. Since `Prepared.hash` remains publicly mutable,
this is deliberately a statement about the current value, not authentication of
the original preparation. A checked caller changes that hash and proves the
returned signing bytes follow it.

[metadata_examples.rs](metadata_examples.rs) checks arbitrary preparation results,
clone/finish preservation, default-parent success and explicit empty roots. A
constructed example prepares and publishes a root through actual checked methods,
then selects default, repeated and empty parent lists. Runtime public API tests
cover captured parents across later history growth, clone and chunking identity,
exact parent order/repetitions, and first-missing-parent errors without history
changes. The existing wire and commit identity fixtures remain unchanged.

The builder's BLAKE3 calls and commit-identity assembly remain outside the harness,
as do complete public `begin_commit`, builder `update`/`finish`, Document commit
and `change` wrapper proofs. The next work is to close that hashing boundary and
compose the existing metadata, storage/publication and edit-phase proofs. The
question of trusting BLAKE3's API versus verifying its implementation is pending;
this milestone adds no hashing contract or application assumption.

If the API boundary is accepted, its intended scope is new/clone/update/finalize
preserving the streamed byte sequence and returning a deterministic BLAKE3 digest.
It would not assume collision resistance, injectivity, authentic authorship or
application correctness. Parent-count encoding and exact digest-input assembly
would still require checked executable code. The active boundary remains the
existing 37 library/Rust contracts until that choice is made.

Validation: 212 strict/854 full checks, 257 workspace tests, 63 audit regressions,
all-features compilation, rustdoc links and formatting pass. The two existing
ignored tests remain unchanged. All 18 new semantic mutations reject with proof
diagnostics; all 934 configured anchors match. The complete mutation sweep was
not rerun. Mutations cover changed author/parents/digest, cloned metadata, current
signing bytes, missing validation, default/explicit parent selection, exact errors
and contradictory successful callers. Wiring checks pin the actual builder's
metadata storage, hashing inputs and final conversion. No trust was added.

## Public commit construction and local change now verify

The user corrected the hashing choice to **trust BLAKE3's implementation**. That
approval is now embodied in a small adapter with five explicit declarations for
opaque state, initialization, clone, append and finalization. The verifier links
the same pinned real BLAKE3 crate as production. See
[TRUSTED_BLAKE3.md](TRUSTED_BLAKE3.md) for the exact scope and dependency pins.
These are assumptions, not proofs of BLAKE3 or cryptographic security.

The actual public `History::begin_commit` and `Ledger::begin_commit` bodies now
verify parent validation/selection and construction of a builder with no payload
bytes. Public builder cloning preserves metadata and streamed bytes. Each update
appends exactly its supplied slice. Finish returns the exact selected author and
ordered parent sequence with identity
`BLAKE3(J(parent_count) || parents || author || BLAKE3(payload))`. The actual
parent-count encoder is checked against bijou64's tiers, offsets, tag and
big-endian payload. The outer streaming loop checks each parent, including
repetitions, followed by author and payload digest. No `bijoux` assumption was
added, and the existing wire encoding and identity fixtures stay unchanged.

The actual public `Document::commit` body now invokes the supplied native Into
conversion, prepares metadata, hashes the converted bytes, registers that exact
shared payload and publishes through the checked Ledger. Its contract relates the
result to the **actual conversion's** postcondition, rather than assuming arbitrary
user conversions preserve a separate byte interpretation. Preparation errors leave
the document unchanged. Later publication errors may retain registered bytes, and
duplicates retain the original slot and stored metadata as before.

The actual public `Materialization::change` body now connects every phase: checked
preflight and the native edit callback; exact Vec-to-Arc byte preservation; the
completed checkpoint's exact parent sequence; public Document commit; and checked
local acknowledgement. Its closed public outcome records the complete phase trace.
The only callback precondition is the actual closure's own precondition when
preflight admits it; there is no assumed application-state law or blanket caller
validity premise. `Ok(Some(draft))` additionally ensures the driver is current and
dirty. The draft is not echoed through apply, and change does not checkpoint it.

Preflight failures do not run the edit or change application state. `Ok(None)` may
retain application mutation without publishing. Edit errors retain their exact
value and poison the driver. Publication or acknowledgement errors also poison
it after editing; neither document nor application rollback is promised. Hash
collisions/freshness are not assumed away: duplicate publication can succeed while
acknowledgement fails. A successful result therefore does not assert that every
possible edit succeeds, or that arbitrary application state matches the draft.

The codec and Display bounds that the new bodies do not use were relaxed. Existing
concrete calls remain valid; the algorithms are checked for arbitrary untouched
codec/options/queue fields and preserve the public error types.

[builder_examples.rs](builder_examples.rs) proves public streaming/clone identity,
default-builder success and a nonempty public Document commit constructed from
actual empty values. [change_examples.rs](change_examples.rs) proves actual public
change rejection, no-commit success and exact edit errors through checked native
callback contracts. Runtime regressions cover independent hash clones, encoding
across every tier and the u64 range, custom Into conversion, unchanged wire
identities, and successful local changes without echoed application.

The active boundary is **38 Rust/standard-library contracts plus 5 BLAKE3
declarations**. The added standard-library contract is only Vec<u8>-to-Arc<[u8]>
byte preservation. No application correctness, collision resistance or signature
assumption was added. These proofs cover the API protocol on normal return;
concrete application-state/checkpoint correspondence, durable persistence, general
lifecycle reachability, replay across reconstructed histories, compaction, sync
minimality and attestation completion remain separate open goals.

Validation for this milestone: 880 full-suite and 212 overlapping strict Verus
checks pass, as do 260 workspace tests, 65 trust-audit regressions, all-features
compilation, rustdoc links and formatting. The two existing ignored tests remain
unchanged. All 33 new semantic mutations reject with proof diagnostics; all 967
configured anchors match. The complete mutation sweep was not rerun locally.
The production lockfile was normalized by Cargo; package versions, dependencies
and checksums are unchanged.

## Source lifecycle invariant now spans the public driver API

`Materialization::new` and `resume` establish a checked source invariant:
any retained source is ready and belongs to the driver's bound History, and any
outstanding batch has a retained source. Every public driver mutation preserves
it on normal return: `set_follow`, `plan`, `complete`, `mark_local`, `step`,
`update` and `change`. The proof includes errors, callback failures, follow-up
loops, checkpoint restoration and local publication; no caller-validity
precondition was added to those APIs.

Consequently, for states reached through these APIs, `plan` and `complete` return
`Error::Poisoned` only when the driver was already poisoned. Their defensive
missing/unready-source paths are unreachable on normal-return call sequences.
A planning `WrongHistory` error requires an actual mismatch with the driver's
bound history; a retained source cannot introduce a different owner. Genuine
callback poisoning, stale tokens, invalid checkpoints/reports and finite-counter
exhaustion remain possible and retain their checked behavior.

[lifecycle_examples.rs](lifecycle_examples.rs) checks arbitrary finite sequences
of actual public calls, including supplied tokens/reports, retained tokens from
planning and follow-ups, local acknowledgement, step/update/edit, and resume from
arbitrary checkpoints. Separate callers cover correction chains and constructor
establishment. This is an inductive source invariant, not an assertion that all
error paths or all cross-field inconsistencies are impossible. Stronger
checkpoint/pending/count relationships, checkpoint codecs and replay equivalence
across reconstructed histories remain library-side work.

Application payload interpretation, agreement between local state edits and their
encoded bytes, and atomic persistence of application state with its checkpoint
are **caller obligations**. They are not unfinished proofs of Stockfoil. A concrete
verified application may demonstrate the interface, but is not required to close
the library contract. Stockfoil's own checkpoint encoding/decoding and resume
behavior remain its responsibility.

No executable behavior or trust contract changed. The boundary remains 38
Rust/stdlib contracts plus five approved BLAKE3 declarations. These are guarantees
on normal return; this invariant does not cover resuming an unwound driver.

Validation: 887 full-suite checks and the overlapping 212-check strict subset
pass, along with 260 workspace tests, 66 trust-audit regressions, all-features
compilation, rustdoc links and formatting. All 19 new semantic mutations reject
with proof diagnostics; all 986 configured anchors match. The complete mutation
sweep was not rerun locally. The two existing ignored tests are unchanged.

## Pending checkpoints and rejection counts now compose across calls

A new inductive `checkpoint_consistent` contract extends source lifecycle
consistency through every public driver mutation. Retained sources conserve
`processed + remaining = captured total`. Each pending checkpoint records the
source's exact captured target and delivery frontier: normal partial work saves
the processed-record count, finished work saves the completed cursor, and a reset
saves an empty cursor with zero processed records for replay. Pending progress
cannot exceed the captured total. A changed follow rule does not retarget an
already captured traversal.

Across arbitrary feedback rounds, pending rejection sets retain all published
rejection hashes, and the pending rejection count equals the number of distinct
hashes added since that published checkpoint. Repeating a rejection does not
inflate the count. Successful `complete` now explicitly guarantees that its
published checkpoint matches the retained source (or its reset) and that its
reported rejection count equals the newly published set difference. Follow-ups
and errors do not publish that staged checkpoint.

The proof covers callback failures and checkpoint restoration: while pending work
remains, the driving helpers retain the original published checkpoint, so restoring
it does not invalidate the pending rejection-count relationship. Constructors
establish the invariant, and actual `set_follow`, `plan`, `complete`, `mark_local`,
`step`, `update` and `change` bodies preserve it without new caller preconditions.
[lifecycle_examples.rs](lifecycle_examples.rs) composes those public calls;
[pending_checkpoint_examples.rs](pending_checkpoint_examples.rs) checks exact
progress, reset handling, bounded progress, completion counts and repeat reports.
The set-cardinality and correction-round lemmas are checked production proof code
in [materialization_invariants.rs](../stockfoil/src/verified/materialization_invariants.rs).

This invariant concerns retained sources and pending work. It does not claim that
an arbitrary checkpoint supplied to `resume` has valid references or a valid skip
count: those still receive the existing checked validation errors when used.
Remaining library work includes stronger cumulative delivery/activation/retraction
relationships, checkpoint codecs and replay equivalence after history
reconstruction. Application interpretation, local edit/byte agreement and atomic
external persistence remain caller obligations. No executable behavior or trust
contract changed; the boundary remains 38 Rust/stdlib plus five BLAKE3 declarations.

Validation: 894 full-suite checks and the overlapping 212-check strict subset
pass, along with 260 workspace tests, 67 trust-audit regressions, all-features
compilation, rustdoc links and formatting. All 21 new semantic mutations and nine
updated source-lifecycle mutations reject with proof diagnostics; all 1,007
configured anchors match. The complete mutation sweep was not rerun locally.
The two existing ignored tests are unchanged.

## Batch counters now remain exact through correction rounds

The actual pending-batch constructor retains the initial counts and active-delivery
sequence as erased proof data. Checked translation establishes distinct delivered
hashes, unique active deliveries contained in that set, and at most one counted
delivery/activation/retraction event per captured source record. The proof data
adds no runtime storage or work.

The new `counters_consistent` invariant is established by both driver constructors
and preserved by every public driver mutation, including errors and callback
rollback. Within one initial batch and its follow-up rounds:

- Delivery and activation counts stay equal to the initial source-batch counts.
- Retractions equal the initial retraction count plus the cardinality of the
  remembered correction set. That set grows monotonically within the original
  active-delivery set; each round adds exactly its previously uncounted hashes.
- A reset preserves the original accounting, clears active correction work and
  sets the sticky rebuild flag. Repeated payload requests do not inflate counts.
- The prior distinct-rejection-count and checkpoint-progress invariants continue
  to hold.

Successful public `complete` now explicitly preserves the initial delivery and
activation totals and bounds additional retractions by the original active-delivery
set. The combined delivery/activation/retraction count is bounded by captured
processed records plus that correction budget. The existing `step`/`update`
trace and summation proofs remain in force; totals describe emitted events,
not globally distinct commits across resets or later materialization batches.

[counter_examples.rs](counter_examples.rs) checks actual public completion,
exact correction deltas, correction bounds and unchanged retraction counts when
no new correction is remembered. [lifecycle_examples.rs](lifecycle_examples.rs)
now preserves the stronger invariant across arbitrary public call sequences.
No executable behavior, caller precondition or trust assumption changed.

The pending-batch counter relationship gap is closed for normal-return API
sequences. Finite counter exhaustion remains a documented error. The next library
work is checkpoint codecs and replay equivalence after history reconstruction or
capture recompilation; admission, compaction, sync minimality and attestation
completion remain separate goals. Application semantics and external atomic
persistence remain caller obligations.

Validation: 902 full-suite checks and the overlapping 212-check strict subset
pass, along with 260 workspace tests, 68 trust-audit regressions, all-features
compilation, rustdoc links and formatting. All 21 new semantic mutations and one
updated constructor mutation reject with proof diagnostics; all 1,028 configured
anchors match. The complete mutation sweep was not rerun locally. The two
existing ignored tests are unchanged.

## Checked checkpoint codec primitives

The production byte reader now lives in `verified/reader.rs`; all existing wire,
cursor, export and checkpoint callers use that same implementation. Its contracts
establish exact prefix consumption, unchanged input on short fixed-width reads,
collection counts bounded by the remaining bytes before iteration, and rejection
of trailing data. Hash-list decoding preserves order and duplicates and consumes
exactly the length prefix and the declared members.

`verified/metadata_encoding.rs` checks little-endian integer and ordered hash-list
encoding, including preservation of an existing output prefix. The production
permission encoder/decoder and checkpoint boolean decoder now live in
`verified/permission_codec.rs`. Permission parsing preserves the exact frontier
and distinguishes denied, unbounded and bounded rules; unknown tags and truncated
frontiers fail. Boolean parsing accepts only zero and one. These are structural
checks: named hashes need not exist in a history.

[codec_examples.rs](codec_examples.rs) calls those actual bodies for arbitrary
integer, hash-list and permission round trips, preserving an arbitrary unread
suffix, and for malformed-length/tag/trailing-byte rejection. Integer conversion
uses checked copies and byte shifts so this adds no project trust declarations.
The wire format and public API are unchanged. Native little-endian compatibility
and truncated-input regressions supplement the proofs.

**Remaining codec work:** the complete `Checkpoint::to_bytes`/`from_bytes` and
`MaterializationCursor` methods are not yet proved. They still need composition
through magic/version checks, sorted permission/rejection maps, duplicate-key
rejection, nested cursor lengths, follow rules and optional captured progress.
The reader and leaf codecs are checked prerequisites, not a full checkpoint
round-trip theorem. Reconstructed-history replay remains the following step.

Validation: **934 full-suite checks** and the overlapping **212 strict checks**
pass, alongside **262 workspace tests**, **70 trust-audit regressions**, all-features
compilation, rustdoc links and formatting. All **22 new semantic mutations** reject
with proof diagnostics; all **1,050 configured anchors** match. The full mutation
sweep was not rerun. The 43 project library contracts and two existing ignored
tests are unchanged.

## Complete cursor and permission-map codecs are checked

The actual `MaterializationCursor::to_bytes` and `from_bytes` bodies now run in
the verification harness, along with the complete view and permission-map codecs.
Encoding followed by decoding preserves delivery heads, target heads, every
listed author's rule and the rule for unlisted authors. This holds for arbitrary
typed cursors, without a history-validity premise. Head/frontier order and repeated
hashes are retained exactly.

The decoder agrees with an explicit structural parsing specification on every
input. It rejects unknown versions and tags, truncated collections, duplicate
authors and trailing bytes. Unique author entries may arrive in any order, as in
the prior decoder. Encoding is deterministic for semantically equal cursors:
author keys are emitted in byte order, using the existing approved standard
sorting contract. There are no new project trust declarations or format changes.

[The checked cursor callers](cursor_codec_examples.rs) exercise the actual methods
for arbitrary round trips, deterministic output and input classification. The map
caller also proves that decoding leaves an arbitrary following suffix untouched.
Compatibility tests compare against the previous format, including all permission
variants, repeated frontiers and noncanonical author order.

**API scope:** `MaterializationCursor` is internal in normal builds and exported
under `fuzzing`. `Checkpoint` uses this codec internally. The public
`Checkpoint::to_bytes`/`from_bytes` composition proof followed this milestone and
is now complete (see the public checkpoint section). Reconstructed-history replay
remains subsequent work.

Validation: **982 full-suite checks** and the overlapping **212 strict checks**
pass, along with **264 workspace tests**, **72 trust-audit regressions**, all-features
compilation, rustdoc links and formatting. All **20 new semantic mutations** reject
with proof diagnostics; all **1,070 configured anchors** match. The complete
mutation sweep was not rerun. The 43 project library contracts and two existing
ignored tests are unchanged.

## Public checkpoint serialization is verified

The actual public `Checkpoint::to_bytes` and `Checkpoint::from_bytes` bodies are
now checked in `verified/checkpoint_codec.rs`. For every typed checkpoint,
encoding and then decoding preserves the complete metadata: follow rule and
ordered pin, completed cursor and permissions, rejected hashes and cascade flags,
and the presence and contents of partial progress (target, captured delivery heads
and exact `u64` processed count). There is no history-validity precondition.

The decoder agrees with an explicit structural parsing model on every input. It
rejects unknown versions/tags, truncated or oversized declared collections,
duplicate rejected hashes/authors, invalid booleans, malformed nested cursors and
trailing bytes. Unique map entries may arrive unsorted, preserving the existing
format's acceptance rules. Encoding is deterministic for equal checkpoint values,
including maps built in different insertion orders. The rejected map still uses
its existing BTreeMap iteration; no additional sorting or project trust contracts
were introduced.

[checkpoint_codec_examples.rs](checkpoint_codec_examples.rs) calls the actual
public methods to establish round trips, deterministic output and exact input
classification. Its serialize/decode/resume caller proves that
`Materialization::resume` receives the same complete checkpoint and establishes
the existing source/checkpoint/counter invariants. This is metadata preservation,
not a theorem about reconstructed-history replay or application state persistence.
Compatibility tests compare with the previous outer format and exercise all
follow/progress variants, maximum processed counts, unsorted rejected entries,
duplicate hashes, invalid flags, truncation and trailing bytes.

**The checkpoint/cursor codec gap is closed.** The next library obligation is
replay equivalence after history reconstruction or capture recompilation,
including canonical remaining records and their metadata. Semantic head/progress
validation remains in the planner. Application payload semantics and external
atomic persistence remain caller obligations; compaction, sync minimality,
admission and attestation completion remain separate verification goals.

Validation: **1,019 full-suite checks** and the overlapping **212 strict checks**
pass, along with **266 workspace tests**, **74 trust-audit regressions**, all-features
compilation, rustdoc links and formatting. All **24 new semantic mutations** reject
with proof diagnostics; all **1,094 configured anchors** match. The complete
mutation sweep was not rerun. The 43 project library contracts and two existing
ignored tests are unchanged.

## Replay across reconstructed histories

The actual source constructor, advancement and checkpoint methods now have checked
relational consumers for independently rebuilt histories. The premise describes
the data being reconstructed: the same commit hashes, direct parent-edge sets and
commit authors. It does **not** require matching node indices, chain assignments,
History identities, caches or caller-assigned payload handles. Both values use the
ordinary maintained Topology invariant; no new trusted declaration is introduced.

[history_equivalence.rs](history_equivalence.rs) derives reachability from this
explicit hash-based graph isomorphism by induction over parent paths. It then
proves equal causal selections and authorization sets for the saved heads and
permissions, including bounded permissions with unknown or out-of-view frontier
members. The canonical minimum-ready-hash order is consequently the same. These
conclusions are proved from the parent edges and authors, not supplied as replay
assumptions.

[replay_examples.rs](replay_examples.rs) calls the production methods. It proves
that reconstructing a source skips the same saved prefix and leaves the same
remaining selection, and that excessive counts fail as `Malformed`. A repeatable
paired advancement theorem preserves corresponding progress under any common
budget, including zero, reset-slot reservation and exhaustion. Records retain the
same hash, new-delivery flag and visibility; each payload handle resolves to that
hash in its own History. Raw handles may differ. Corresponding sources produce
semantically equal checkpoints and identical serialized bytes.

The persisted-partial consumer advances a live source, prepares and serializes
its checkpoint through the public codec, decodes it, reconstructs the source in
the other History, and proves that its next records and checkpoint agree with
uninterrupted execution. A concrete fork/join consumer constructs both topologies
through actual insertions, reverses root arrival and join-parent order, changes
payload handles, and establishes the equivalence premise without assuming it.
Existing runtime regressions already exercise reordered rebuilds and remapped
payload handles, including later deliveries.

**Scope:** this closes the source-level reconstruction and graph-isomorphism gap
for histories containing the same logical commits. The next step is composition
through public `Materialization::resume`/`plan`, event translation and completion,
and extending the relation to later additions outside the saved capture. Those
end-to-end and growth claims are not established here. The reconstruction premise
is a comparison of stored graph data; this theorem does not prove admission or
that a persistence implementation rebuilt those data faithfully. Application
semantics and atomic external persistence remain caller obligations. Production
runtime behavior and the checkpoint wire format are unchanged.

Validation: **1,036 full-suite checks** and the overlapping **212 strict checks**
pass, along with **266 workspace tests**, **75 trust-audit regressions**, all-features
compilation, rustdoc links and formatting. All **12 new semantic mutations** reject
with proof diagnostics; all **1,106 configured anchors** match. The complete
mutation sweep was not rerun. The 43 project library contracts and two existing
ignored tests are unchanged.

## Public partial resume, planning and acknowledgement replay

The reconstruction relation now reaches actual public API calls. A checked caller
serializes a partial checkpoint, decodes it, resumes two `Materialization`s against
independently rebuilt `Ledger`s, calls `plan` with the same positive budget, and
acknowledges both batches through `complete`. With no new rejection or payload
request in that acknowledgement, the two runs publish equivalent checkpoints,
equal counts and identical checkpoint bytes. Existing direct and cascading
rejections are allowed and remain effective.

The premise is explicit: the histories contain the same commit hashes, parent-edge
sets and authors; the saved checkpoint has known binding references and a valid
captured frontier/count. These are data-validity conditions, not an assumption
that planning succeeds. The caller proves success from the actual planner's
error contracts. Current ledger policies and visibility-change support may differ:
the partial traversal uses its saved target and permissions before any new view.
Node indices, chain assignments, History identities and payload handles may differ.

Batch comparison preserves event order and kind, hashes, authors, parent
**membership**, and activation/retraction decisions. Each delivered payload handle
is separately proved to belong to that hash in its own ledger. Parent-vector order
and multiplicity are not equated by the graph relation; nor are process-local
payload handles or batch tokens. The library still returns each history's stored
parent vector. A concrete checked fork/join consumer reverses arrivals and parent
order, changes payload handles and current ledger policy, and exercises the public
codec/resume/plan/complete path. Separate comparison witnesses ensure author,
parent membership, visibility and event order have not been discarded by the spec.

[replay_plan.rs](replay_plan.rs) composes source selection, skipped-prefix uniqueness,
record mapping, rejection filtering, event translation, counts and pending
checkpoints. [replay_ack.rs](replay_ack.rs) proves unchanged acknowledgement cannot
introduce a correction for an already-filtered active delivery, including when
other commits have permanent rejections. The planner contract now also retains
its captured before/delivered sets; a checked topology lemma exposes parent edges
and their hash/index mapping. These are proof changes with no runtime or wire-format
change and no new trusted declarations.

**Remaining reconstruction work:** compare feedback that adds rejections or asks
for payloads and the resulting correction rounds; compose repeated live driving
through completion of the saved capture; allow later history additions outside that
capture; and handle the subsequent fresh view. This milestone does not equate
application state or establish external atomic persistence, admission, compaction,
sync minimality or attestation completion.

Validation: **1,053 full-suite checks** and the overlapping **212 strict checks**
pass, along with **266 workspace tests**, **76 trust-audit regressions**, all-features
compilation, rustdoc links and formatting. All **18 new semantic mutations** reject
with proof diagnostics; all **1,124 configured anchors** match. The complete
mutation sweep was not rerun. The 43 project library contracts and two existing
ignored tests are unchanged.
