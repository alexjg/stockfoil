# Authorized visibility: public API verification plan

## What we want to verify

A commit participates in a view exactly when it belongs to the selected history
and its author's permission allows it. The selected history includes the view's
heads and every commit reachable by following their parents.

- Missing and denied permissions allow nothing.
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
  than unchecked assumptions or project `external_body` declarations.
- Production code is shared with the harness; model-only functions are labeled.
- `--no-cheating`, semantic mutation checks and applicable runtime tests pass.
- Documentation names any remaining trusted library specifications and excludes
  cryptographic security and application crash recovery from this milestone.

## Progress

- [x] Stage 1: policy laws, nonempty examples, public API contract tests and mutations.
- [ ] Stage 2: production AVL refinement.
- [ ] Stage 3: history and cached-walk refinement.
- [ ] Stage 4: production permission compilation.
- [ ] Stage 5: public API authorization theorem.

Stage 1 evidence:

- [`authorization.rs`](authorization.rs) isolates the policy and proves denial,
  exact unbounded behavior, inclusive frontiers, author isolation, available
  frontier equivalence/monotonicity, and capture stability under graph append.
- [`authorization_tests.rs`](authorization_tests.rs) supplies nonempty witnesses
  with multiple heads, cross-author frontiers, unavailable heads and append.
- [`authorized_visibility.rs`](../stockfoil-core/tests/authorized_visibility.rs)
  tests the public API across batch sizes, permission transitions, an unknown
  frontier delivered after capture, invalid heads and wrong-history rejection.
- The Stage 1 baseline reports 58 verified obligations; all 12 semantic mutations are
  rejected, including four new authorization-policy mutations.

Stage 2 progress (still incomplete):

- The shared production tree now has a coordinate-map view and a strict BST
  ordering predicate. Its actual iterative lookup and membership functions are
  proved exact under that predicate; empty construction and shallow cloning are
  also verified. See [`tree.rs`](../stockfoil-core/src/verified/tree.rs).
- Actual tree witnesses, a structural-sharing regression and four additional
  semantic mutations exercise these contracts. The combined suite reports
  104 verified obligations and rejects all 33 mutations.
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
  mutations test union. Masked reconstruction remains unproved.

Review stack: foundations (#4), authorization plan/policy (#3), then AVL
lookup (#5), node construction (#6), rotation correctness (#7), insertion (#8), ordered iteration (#9), and union. Stage 1 adds
specifications and integration tests. Stage 2 has begun production refinement,
but no complete public API authorization theorem exists. The next implementation
step is proving masked reconstruction using the checked iterator and sorted
construction.
The remaining stages continue through concrete history/view construction and
`History::changes`/`History::next_changes`; this stack is not complete until
those public entry points establish their own prerequisites.
