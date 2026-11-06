# Stockfoil fuzzing

A standalone cargo-fuzz crate with its own `[workspace]`. It depends on the
working copy of `stockfoil` by path; it is not part of `cargo test --workspace`.

```text
fuzz/
  src/codec.rs           keys, identity rule, ChunkCodec and document helpers
  src/host.rs            foreign-host model: blobs, decoder, payload IDs, driver
  src/model.rs           naive whole-history DAG and participation checks
  src/planner.rs         materialization sequences checked against that model
  src/templates.rs       valid items exported from real documents
  fuzz_targets/          the five targets below
  examples/gen_seeds.rs  writes byte/history seed corpora
  regressions/<target>/  minimized crashing inputs
```

## Targets

| Target | Input and exercised API | Main invariants |
| --- | --- | --- |
| `wire_header` | Arbitrary header bytes with small `Limits`, or structured `BundleMeta`; raw-mode bytes also enter `Checkpoint`, `MaterializationCursor` and `ExportCursor` parsers | No panic; stable prefix verdicts; decoded limits respected; header encoding/decoding stable; successful durable metadata parses round-trip. |
| `admission_bytes` | Nine configuration bytes followed by a raw item or a crafted signed bundle; varied buffers, quantum, host limits and pre-admitted templates | Termination; failed/waiting operations do not mutate history; admitted members and placement match the model; re-export preserves commits and bytes; crafted inputs match an independent validity predicate. |
| `admission_replies` | Valid templates with adversarial host replies: stale/wrong-kind tickets, short or garbage buffers, failures, EOF/status violations, duplicates, deferral and wrong-history advance | Rejected stale/wrong-kind replies preserve outstanding requests and counts; protocol failures do not publish; successful operations admit exactly the supplied commits. |
| `history_ops` | Up to 64 operations on two `Document<ChunkCodec>` replicas: local-author commits, repeated parents/duplicates/depth grinding, signatures, reordered partial exports/acknowledgements, raw changes with random heads/permissions, full sync | Parent-closed admission, heads/drafts/ancestry and raw change records match the DAG model; own-author-signed drafts with admitted parents make progress; cursor encodings round-trip; full sync converges and payload bytes are exact. |
| `materialization` | Up to 64 four-byte operations: edits, signing, remote-author delivery, document policy, working/signed/pinned follow rules, bounded steps, checkpoint resume and full updates | Known metadata and active sets match the whole-history oracle; permanent/cascading rejection, payload requests and reset/replay remain consistent; state and checkpoint are restored together. |

`history_ops` intentionally tests lower-level `History::changes` as well as
document admission. `materialization` tests the planner/memory driver instead;
it does not maintain a second manual traversal beside that planner.

The planner target round-trips checkpoints produced by valid sequences.
Malformed checkpoint encodings and additional token-misuse cases are covered
by the normal Rust tests, not claimed as arbitrary-byte coverage of this target.

## Compile and run deterministic checks

```sh
cargo check --manifest-path fuzz/Cargo.toml --bins
cargo test --manifest-path fuzz/Cargo.toml --lib
```

The library tests include deterministic randomized planner/model sequences.
Compiling targets or running uninstrumented smoke checks is **not** a long
coverage-guided/sanitizer campaign. Longer instrumented campaigns and highly
concurrent planner profiling remain hardening work.

## Run coverage-guided fuzzing

Install a nightly toolchain, then run from the repository root:

```sh
cargo install cargo-fuzz
cd fuzz
cargo +nightly run --example gen_seeds
# gen_seeds populates all targets, including valid checkpoints and deep
# planner sequences. Add a minimal planner input too:
mkdir -p corpus/materialization
printf '\000\000\000\001' > corpus/materialization/commit

# -a retains debug assertions and overflow checks; default sanitizer is ASan.
cargo +nightly fuzz run -a wire_header corpus/wire_header -- -max_total_time=600
cargo +nightly fuzz run -a admission_bytes corpus/admission_bytes -- -max_total_time=600 -max_len=4096
cargo +nightly fuzz run -a admission_replies corpus/admission_replies -- -max_total_time=600
cargo +nightly fuzz run -a history_ops corpus/history_ops -- -max_total_time=600 -max_len=4096
cargo +nightly fuzz run -a materialization corpus/materialization -- -max_total_time=600 -max_len=256
```

For faster coverage-guided exploration without ASan, explicitly add `-s none`.
That is still instrumented fuzzing, but does not provide sanitizer coverage.

Parallel exploration, retaining artifacts and continuing after crashes:

```sh
cargo +nightly fuzz run -a materialization corpus/materialization -- \
  -fork=8 -ignore_crashes=1 -max_total_time=600 -max_len=256
```

Reproduce or minimize an input with its matching target:

```sh
cargo +nightly fuzz run -a history_ops regressions/history_ops/liveness_min
cargo +nightly fuzz tmin -a history_ops artifacts/history_ops/<crash>
```

`STOCKFOIL_FUZZ_TRACE=1` prints `history_ops` operations and histories.
For `admission_bytes` it prints a `TRACE <mode> <kind> <outcome> <validity>`
line per input, useful for corpus summaries.

## Regressions

Minimized inputs for two previously found and fixed bugs:

- `regressions/history_ops/liveness_min`: an own-author-signed draft with admitted
  parents stalled because its fragment contained an admitted member whose
  covering signature lay outside it. Standalone certification provides progress.
- `regressions/admission_bytes/read_size_overflow`: a read size near
  `usize::MAX` overflowed a header-buffer bound. Sizes are clamped and arithmetic
  saturates.
- `regressions/admission_bytes/stranger_key_alias`: caught a **fuzz oracle** bug,
  not a library verification bug. The generator assumed that signing with the
  wrong key made evidence invalid, but a random author could alias a known
  signing key. The oracle now checks the actual public key/signature pairs; the
  input passes both the original verifier and the corrected oracle.

`corpus/`, `artifacts/` and build outputs are ignored. Regenerate original
seeds with `gen_seeds`; keep minimized useful failures under `regressions/`.
