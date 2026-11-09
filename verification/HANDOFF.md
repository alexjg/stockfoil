# Verification handoff — 2026-11-09

Start here when continuing in another thread. This file describes the current
state; older milestone sections in the other documents are historical and may
still describe obligations that subsequent work has closed.

## Repository and user intent

Repository: `alexjg/stockfoil`; working branch: `verus/authorized-api`.
The branch is consolidated into **one commit on top of main** at
`f2d470eeae4d11fe7bd7d7f65d9d88bcd8f0a65d`, including the latest public replay work.
The old 47-commit tip was `499ef8a7c34ff52533dbf8a1ae2edff692f041a7`; use the
current branch, not that historical tip, to continue.

The user wants verification of user-facing behavior: authorized visibility,
consistent materialization, compaction, convergence to minimal trees during sync,
and attestation completion. Continue the actual implementation proofs rather than
only proving a disconnected model. They no longer want a PR stack; keep the work
on this branch, push finished changes, and do not merge without an instruction.
GitHub Actions minutes are exhausted; local validation is the current evidence.

The user explicitly accepts standard-library and BLAKE3 implementation assumptions.
They corrected an earlier request to verify BLAKE3: **trust its implementation**.
There is no collision-resistance assumption. Application payload semantics and
atomic external persistence are caller obligations; do not claim either is proved.

## What is established

Read [PUBLIC_API_PROOF_BOUNDARY.md](PUBLIC_API_PROOF_BOUNDARY.md) for the precise
five-goal boundary and [API_TARGET.md](API_TARGET.md) for API details.

- Actual public captured ancestry/participation and causal-difference APIs,
  including construction, membership, heads, clone, canonical iteration and errors.
- Actual materialization planning, completion, `step`/`update`, local edit guards,
  metadata preservation, authorization/activation bounds, tokens/feedback,
  rejection corrections, staged checkpoints and cumulative counters. These are
  library control-flow and metadata guarantees, not application-state correctness.
- Actual public `Checkpoint::to_bytes`/`from_bytes`: arbitrary typed checkpoint
  round trips, deterministic encoding and exact structural input classification.
  Cursor, view, permission, rejected-map and partial-progress codecs are included.
- Hash-based graph isomorphism derives ancestry, authorization and canonical source
  replay for independently rebuilt histories with equal hashes, direct parent-edge
  sets and authors. Internal indices, chains, History identities and caller payload
  handles may differ.
- **Latest milestone:** an actual public codec → `Materialization::resume` → `plan`
  → `complete` consumer compares a valid partial checkpoint across equivalent
  Ledgers. The same positive budget produces corresponding ordered event kinds,
  hashes, authors, parent membership and visibility, correct per-ledger payload
  handles, equal counts and identical published checkpoint bytes. Completion here
  uses `Applied::default()` (no new rejection or payload request); existing direct
  and cascading rejections are supported. Current ledger policies and visibility
  change support may differ, because partial replay uses the saved capture.

The relational event specification compares **parent sets**, not parent-vector
order/multiplicity. The actual event still contains its History's stored parent
vector. Raw payload handles and batch tokens are process-local and are not equated.
The comparison has concrete witnesses checking that author, parent membership,
visibility and event order are not accidentally omitted.

A saved partial checkpoint must name existing binding/capture references and have
an applied count within its captured selection. The public replay caller derives
successful planning from those data-validity conditions and the actual error
contracts; it does not assume successful calls. It also does not establish that
external persistence reconstructed the graph or application effects faithfully.

## Where to continue

The next focused work is **relational replay with application feedback**:

1. Extend `verification/replay_plan.rs` / `replay_ack.rs` to corresponding reports
   that add direct/cascading rejections or request payloads. Prove corresponding
   correction rounds, counts and checkpoint publication across rebuilt Ledgers.
   Relevant contracts are `materialization_complete::{reported,round,body_error}`,
   `materialization_feedback` and `materialization_corrections`. Reports should
   correspond by hash and cascade flags; returned payload IDs bind separately to
   their respective histories. Reasons are not persisted.
2. Carry the relation through repeated live `plan`/`complete` or `step`/`update`
   execution until the saved capture is exhausted. The source relation and count /
   pending-checkpoint invariants already exist; avoid restarting proofs from models.
3. Relax the whole-history equivalence premise to allow later additions outside
   the saved capture, including newly known permission-frontier hashes outside the
   captured view. Then address switching from the completed capture to a fresh
   working/signed/pinned view. Existing runtime regressions cover examples, but
   those relational theorems are still open.
4. Compaction, sync minimality and attestation completion still lack end-to-end
   public API proofs. They require admission/export/certificate invariants and
   explicit fairness/cryptographic boundaries. Supporting graph lemmas alone do
   not establish these guarantees.

## Code map

- `stockfoil/src/verified/`: actual shared production bodies inside `verus!`.
  Cargo erases proof material; the harness imports these same files.
- `verification/main.rs`: harness and fixtures; production export wiring is pinned
  by `public_api_wiring.json` and checked by `check_trusted.py`.
- `verification/history_equivalence.rs`: graph isomorphism, ancestry, causal and
  authorization transfer, canonical schedule and record equivalence.
- `verification/replay_examples.rs`: actual source reconstruction consumers and
  concrete reordered fork/join witness; that witness now invokes public replay.
- `verification/replay_plan.rs`: parent metadata, event comparison, payload binding,
  saved-checkpoint validity, corresponding source origins, public resume/plan,
  counts, pending checkpoints and the full public acknowledgement consumer.
- `verification/replay_ack.rs`: active-delivery safety and unchanged acknowledgement
  through actual `complete`, including existing rejection maps.
- `stockfoil/src/verified/materialization_plan.rs`: the `issued_from` contract now
  retains captured before/delivered membership as well as after membership.
- `stockfoil/src/verified/topology.rs`: `parent_graph_facts` exposes checked parent
  storage and hash/index correspondence without unrelated chain-traversal facts.
- `verification/README.md`, `AUTHORIZED_VISIBILITY_PLAN.md`: milestone narrative;
  newest section first in README, newest milestone last in the plan.

## Trust and validation

There are **43 explicit project library contracts/declarations: 38 Rust/stdlib and
5 BLAKE3**. See `TRUSTED_STDLIB.md`, `TRUSTED_BLAKE3.md` and their JSON manifests.
No project application assumptions, extra unchecked bodies or cryptographic
collision assumption were introduced. Verus, vstd, erasure, Z3 and Rust remain
part of the trusted toolchain. Resource exhaustion, allocation failure and panic
recovery are not general guarantees of this work.

Final validation for this consolidation is recorded in the newest README milestone.
Validated baseline: **1,053 full checks and 212 overlapping strict checks**,
**266 workspace tests**, **76 audit tests**, two existing ignored tests, and
**1,124 configured semantic mutation cases**. The latest **18** cases are named
`public replay ...`; they target the new composition and its non-vacuity witnesses.
The complete mutation sweep has not been rerun. Do not describe configured cases
as all executed successfully, or local results as successful GitHub CI.

From the repository root:

```sh
export VERUS_BIN=/absolute/path/to/verus-x86-linux/verus
bash verification/verify.sh
cargo +1.99.1 test --workspace --locked
python3 verification/test_trusted_audit.py
cargo +1.99.1 check --workspace --all-features --locked
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo +1.99.1 doc --workspace --no-deps --locked
cargo +1.99.1 fmt --all -- --check
git diff --check
```

The pinned Verus release is **0.2026.11.02.8bf68ee**, Rust **1.99.1**.
See the README installation section. `verification/blake3_args.py` builds the
locked real BLAKE3 dependency and obtains the artifact/lookup arguments; do not
replace it with a stub for tests. `verification/check_mutations.py` runs the full
(expensive) sweep. Individual mutations can use its `copy_sources` helper, retaining
all trust/wiring/dependency files; require proof diagnostics, not merely a compiler
error or timeout, as evidence of rejection.

For a focused module check, without depending on temporary scripts from this chat:

```sh
python3 - <<'PY'
import os, subprocess, sys
sys.path.insert(0, 'verification')
from blake3_args import arguments
subprocess.run([os.environ['VERUS_BIN'], '--crate-type', 'lib', '--edition', '2024',
    '--cfg', 'stockfoil_stdlib', '--verify-module', 'replay_plan',
    '--triggers-mode', 'silent', *arguments(), 'verification/main.rs'], check=True)
PY
```

In the current managed workspace only, the checkout is
`/workspace/stockfoil-authorized-api` and tools already exist under `/workspace/tools`:

```sh
export PATH=/workspace/tools/cargo/bin:$PATH
export CARGO_HOME=/workspace/tools/cargo
export RUSTUP_HOME=/workspace/tools/rustup
export CARGO_TARGET_DIR=/workspace/target
export VERUS_BIN=/workspace/tools/verus-x86-linux/verus
```

A new environment may use different locations. All required source, contracts,
mutation definitions and continuation context are committed; no `/tmp` artifact
is required to continue. Two proof functions use isolated provers with a bounded
resource limit to avoid unrelated SMT instantiations. Sequence-map/drop proofs
need explicit indexed equalities; fresh driver `checkpoint_view` must be related
to its fields with `representation()` after mutation.
