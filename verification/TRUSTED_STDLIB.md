# Explicit standard-library assumptions

Standard-library assumptions are an accepted part of this verification effort.
Stockfoil's application logic must still be verified. This document enumerates
**two assumed contracts, not two proved functions**. Neither assumes ancestry,
permission correctness, candidate completeness, or any public API guarantee.

## Enabled contracts

Both adapters live in
[`stdlib.rs`](../stockfoil/src/verified/stdlib.rs). Their complete source is
SHA-256 pinned in [`trusted_stdlib.json`](trusted_stdlib.json). Changing a body,
contract, or dependency import requires a visible manifest change and review.

| ID | Actual body | Assumed postcondition |
| --- | --- | --- |
| SORT-1 | `nodes.sort_unstable()` | Nondecreasing numeric node indices; output is a permutation of the input, preserving multiplicities and membership. |
| DEDUP-1 | `nodes.dedup()` | For nondecreasing input, output has strictly increasing numeric indices, preserves membership, and cannot grow. |

The contracts include the bridge from `NodeIdx`'s derived `Ord` and `PartialEq`
to its one private `u32` field. The field's projection is checked in `clock.rs`;
the Rust-derived trait implementations are not verified by this harness. There
is no custom comparator, callback or Stockfoil policy logic in these adapters.
The wrappers preserve the existing production sorting/deduplication calls.

Rust documents these behaviors for
[`slice::sort_unstable`](https://doc.rust-lang.org/std/primitive.slice.html#method.sort_unstable)
and [`Vec::dedup`](https://doc.rust-lang.org/std/vec/struct.Vec.html#method.dedup).
The contracts describe normal completion. Allocation failure and process abort
are outside the existing milestone. No sorting performance bound is proved.

## Checked application layer

`candidates::normalize` composes these contracts and proves exact preservation of
the input set and strict numeric ordering. The production `history_clock` uses
this function to normalize head indices. The rewritten lazy Traversal does not
call the eager assembler. `candidates::assemble` and its batch-model refinement
are retained only under `cfg(verus_keep_ghost)` as a reference set model, without
claiming the new canonical traversal's ordering or memory behavior.
The runtime regression checks the actual adapters on duplicate and extreme node
indices against an independent set of numeric indices. Tests support review;
they do not prove the trusted standard-library implementations.

## Enforcement and reporting

`verify.sh` first runs `check_trusted.py`, then runs:

1. The original proof suite with Verus's `--no-cheating` flag. The adapters and
   their dependent application modules are excluded from this pass.
2. The full suite with the explicit contracts enabled. This pass cannot use
   `--no-cheating`, because that flag rejects the two external bodies.

The runner prints the contract count separately from Verus's verified count.
The full count includes the original suite; the two pass counts must not be
added. Existing shared helpers are rechecked in the full suite as well.

The source audit pins the entire trusted file, rejects additional known proof
escape declarations in the audited Rust source directories, and rejects explicit
path imports outside those directories. Regression checks demonstrate rejection
of a modified adapter, application assumptions, unchecked application bodies,
and out-of-scope imports. It is a review guardrail, not a Rust parser or proof of
the build system. Review remains necessary for changes to the manifest, audit,
harness, compiler configuration, or dependencies.

Semantic mutation checks first run both baseline passes. Each mutation then runs
the audit and complete expanded suite using `verify.sh --mutation-only`; it does
not repeat the subset pass. This retains all application checks while avoiding
redundant subset verification for each mutation.

Semantic mutation checks run against checked application logic. Corrupting an
external body cannot be detected by its assumed contract; the source audit and
runtime checks address accidental changes there. The other trusted components
remain Rust, Verus and erasure, vstd, Z3, and the existing toolchain/build chain.

## Future contracts

No `OnceLock`, atomic identity, hashing or signature contracts are enabled yet.
Introduce each only alongside an actual consumer and an explicit contract review.
A cache contract may describe initialization/publication/stability, but must not
assume that the stored clock is correct. An atomic contract may describe the
linearized checked increment, but must not assume Stockfoil assigns or checks
identities correctly. Those remain application proofs.
