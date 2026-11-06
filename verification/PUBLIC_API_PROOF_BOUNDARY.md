# Remaining public API verification

The stack verifies production clock construction, lookup, balancing, updates,
iteration, union, masking, accumulation and difference enumeration. A checked
refinement connects actual mask output to the independent parent-edge policy,
conditional on history summaries and compiled permission bounds.

**The user-facing guarantees below are not yet proved end to end.** A proof of a
helper under explicit premises does not establish that public construction paths
satisfy those premises. The existing public API tests are supporting evidence,
not formal proofs of their wrappers.

## Intended public guarantees

| Goal | Public boundary and intended property | Remaining work |
| --- | --- | --- |
| Authorized visibility | `History::changes` / `next_changes`: each before/after visibility bit equals the permission policy for its captured view, including inclusive and unavailable frontiers. | Concrete history/author mapping, ancestry summaries, permission compilation and emitter refinement. |
| Consistent materialization | Every required change appears exactly once in topological order; batches preserve captured state, wrong-history traversals fail, and completion publishes the matching cursor. | Table completeness, candidate assembly/sort/dedup, record mapping, history identity and cursor publication. |
| Compaction | Certificate retirement preserves delivered/admitted history and the evidence required to export it. | `history/admit.rs::add_certificate`, canonical fragment containment, inventory coverage and evidence retention. |
| Sync convergence to minimal trees | Equivalent logical histories converge to the same irredundant canonical inventory under sufficient delivery/acknowledgment. | Specify minimality precisely over fragment coverage, prove canonical nesting/retirement and export/admission transitions, and state fairness assumptions. Successful delivery alone must not be treated as an acknowledgment. |
| Attestation completion | Complete evidence and admitted dependencies cause eligible signed drafts to be admitted; incomplete evidence never does. | `History::sign` / admission advancement, `coverage`, `assemble`, fixed-point progress and signature assumptions. |

The first two goals remain the current priority. Hashing/signature assumptions,
host persistence and fairness for distributed progress must be stated explicitly
when their stages begin.

## Next reviewable layers

1. Establish the concrete `History` invariant: parent indices precede children,
   hash lookup matches nodes, authors and chains agree, chain tables are complete
   and unique, and all counts satisfy the checked clock capacity contracts.
2. Verify chain selection and append preserve that invariant. Prove the actual
   cached walk and multi-head summary builder summarize precisely parent-edge
   ancestry, including reuse after append.
3. Verify view-head resolution and the production permission callback. Filter
   unavailable frontier heads before ancestry, select the stored author and
   discharge `compiled_bounds` from real maps and arrays.
4. Verify candidate assembly, sorting/deduplication and captured traversal
   construction. Complete difference enumeration is already checked against the
   supplied tables; their concrete completeness still comes from step 1.
5. Verify the actual emitter's hash/payload mapping, visibility, progress,
   wrong-history rejection and completed-cursor publication. Compose these with
   the public `History::changes` / `next_changes` wrappers.
6. Extend the invariant and proofs through certificate coverage, compaction and
   export/admission transitions for the remaining three goals.

## Accepted standard-library boundary

The selected route is explicit, narrowly scoped standard-library contracts.
See [TRUSTED_STDLIB.md](TRUSTED_STDLIB.md) for the enabled contracts and audit.
The runner retains `--no-cheating` for the original suite and separately checks
the full suite with those contracts. Application logic may not add assumptions.

The pinned Verus standard-library specifications do not cover the production
sorting calls or `std::sync::OnceLock`. Those operations occur on the path to the
public guarantees. This is a proof-tool boundary, not evidence that those Rust
operations are incorrect. Existing vstd specifications already form part of the
trusted base for operations such as `Arc::clone` and vector manipulation.

The alternatives considered were:

| Route | Concrete effect |
| --- | --- |
| Explicit standard-library contracts | Preserve production algorithms and synchronization. Add a separately reviewed, enumerated trusted-library boundary and update the verification plan/CI to make that boundary visible. Application logic must remain checked; a bare success count must not conceal trusted wrappers. |
| Preserve the current no-new-axioms boundary | Replace unsupported operations with implementations or wrappers whose behavior can be checked using existing Verus primitives. Prove sorting and cache invariants, retain thread-safety/structural sharing, and check performance and API compatibility. |

Sorting and deduplication are now enabled through two thin adapters. The other
contracts below remain future obligations, not declarations enabled in the suite:

- **Sorting:** return a permutation of the original nodes in nondecreasing
  `NodeIdx` order. It must neither invent nor lose an element. The relation
  between `NodeIdx` ordering and its index must also be justified.
- **Deduplication, if retained as a library call:** remove consecutive equal
  elements, preserve the represented set, and produce strict order when the
  input is sorted. A checked linear compaction loop is another option.
- **OnceLock:** initialization publishes only a successfully returned value;
  reads return that published value; subsequent successful reads are stable;
  racing initialization cannot publish two different values; panic does not
  publish a partial result. The application must separately prove that the
  initializer computes the correct ancestry clock and that append preserves it.
- **Identity allocation:** atomic read-modify-write has a single linearization
  point; a successful checked increment returns the prior value and advances
  exactly once; exhaustion leaves the counter unchanged. The application must
  establish which counter supplies each history and where identity is checked.

These contracts must not assume the desired Stockfoil result. In particular,
cache correctness, permission correctness, candidate completeness, certificate
coverage and cursor durability are application obligations, not library axioms.

## Concrete identity issue found during this work

`History::new` previously used wrapping `fetch_add` for history IDs. Repeated
allocation could eventually reuse an ID, undermining wrong-history checks for
captured traversals and export plans. The identity fix uses checked atomic
allocation: exhaustion panics without changing the counter. A local-counter
regression exercises the boundary and repeated failure without allocating an
astronomical number of histories.

That regression and fix do not constitute a formal proof of the global atomic
allocation protocol. That proof remains part of the public identity boundary
above. Resource exhaustion and the policy for finite node/chain capacities also
need explicit treatment at public construction paths.
