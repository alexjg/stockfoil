# Implementation progress

The rebuild around one payload-independent `History`, a request-driven
`Admission`, and thin adapters is implemented. This file separates what is built
and tested from what remains. Contracts are in
[design/history-operations-api.md](design/history-operations-api.md).

## Implemented

| Area | Where | Evidence |
| --- | --- | --- |
| History: append-only topology, chains, clocks, drafts, admission, heads, ancestry | `src/history/mod.rs`, `clock.rs` | unit tests; randomized model comparison (`tests/reference.rs`) |
| Local commits, synchronous signing, standalone and fragment certificate assembly, co-signing | `src/history/local.rs`, `admit.rs` | `tests/graph.rs`, `src/history/tests.rs` |
| Incremental verification: identities, boundaries, signatures, per-author coverage | `src/verify.rs` | `tests/admission.rs`, `tests/placement.rs` |
| Fragment placement against admitted history | `src/history/admit.rs` | `tests/placement.rs` (hand-built invalid fragments) |
| Request-driven admission: Read/Decode/Bind, tickets, stale/wrong-kind rejection, epoch-based waiting, exact counts, cancellation by drop | `src/admission.rs` | fake-host suite `tests/admission.rs`; JS driver tests |
| Atomic publication with first-delivery payload IDs | `src/history/admit.rs` | `tests/admission.rs` (concurrent admissions of one item) |
| Export inventory with fragment retirement, captured plans, acknowledged cursors, durable cursor encoding | `src/history/export.rs` | `tests/placement.rs`, `tests/reference.rs` (3-replica convergence) |
| Views, permissions, diff, metadata-only change batches, durable completed cursor | `src/history/view.rs` | `tests/reference.rs` (against a naive whole-history model) |
| Wire v2 header (v1 columns, length-prefixed header, streaming body) | `src/wire.rs`, `design/wire-v2.md` | unit tests, golden vectors `tests/vectors.rs` (identities equal v1's) |
| Native adapter, `MemoryStorage`, payload reads, single-pass item decoding | `src/graph.rs` | all integration tests |
| Opaque storage keys (no content-hash assumption, so encrypted stores work); payload reads checked against commits (`check_payload`, `CorruptPayload`) | `src/graph.rs`, `src/history/local.rs` | `tests/storage.rs` (counter-keyed, masked storage; corrupted blobs); JS `storage keys are opaque…` test; `admission_bytes` fuzzing under unrelated keys |
| Bounded payload buffers for items over 4 GiB | `tests/large.rs` | 4 GiB + 12 KiB admitted with unchanged peak RSS (release, ignored by default) |
| JavaScript/Wasm binding and async host driver | `javascript/` | `npm test` (Rust interop fixture, delayed hosts, BigInt IDs) |
| Examples | `examples/` | mv-register (Rust and JS) test suites; file-backed storage example |
| Fuzzing | `fuzz/` | wire, admission bytes, hostile replies, history operation sequences; found and fixed a draft liveness bug and a size overflow (regression inputs in `fuzz/regressions/`) |
| Persistent, structurally shared ancestry clocks | `src/history/clock.rs` | randomized persistence test; fork-and-merge memory regression test |

Benchmark (`cargo bench --bench history`, one run, release, this machine):
building 1,000,000 local commits 1.0 s; cold materialization of all 1,000,000
records 30 ms; a 10-commit incremental update 0.01 ms; revoking one author
(16,666 records) 0.2 ms; receiving 417 items covering 20,000 signed commits 26 ms.

## Remaining

1. **Mid-traversal materialization checkpoints** (deferred by decision). An
   interrupted change traversal restarts from the last completed cursor.
2. **Python, Kotlin, and Swift bindings**, expected as one UniFFI crate over
   `History` and `Admission`.
3. **Fragment size versus receiver limits.** Retirement folds history into ever
   larger fragments. Depth-3 fragments average about 16 million members, beyond
   the default limit of 2^20, so very long histories need larger limits or an
   export policy that keeps smaller items when a covering fragment is too large.
4. **Resource policy beyond per-item limits**: aggregate quotas across concurrent
   admissions, fallible allocation, and budgets for history steps. Placement and
   publication are unbudgeted, and a fragment naming a shallow external parent can
   make placement walk a large part of the history. Default byte limits are
   unbounded so large payloads stream; a peer can therefore demand a lot of
   hashing per item unless the application sets them.
5. **Export cost at scale**: planning is linear in the inventory size; retirement
   covers standalone items and nested fragments, not general cover minimization.
6. **Signed-draft scanning**: each signature or admission re-checks every signed
   draft that is still blocked (because a parent is an unsigned draft). Cheap
   while few are blocked; quadratic if many are.
7. **Storage durability and recovery** remain application responsibilities; the
   file-backed example shows one approach.
8. **Format policy**: v2 is the only wire format. There is no negotiation and no
   v1 reader.
